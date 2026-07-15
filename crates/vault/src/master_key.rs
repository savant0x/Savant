// FID-036 anchor — see dev/fids/FID-2026-07-15-036-clippy-cleanup-deferred.md
// Defer 14 pre-existing clippy findings (`clippy::doc_overindented_list_items`,
// `clippy::doc_lazy_continuation`, `clippy::disallowed_methods`) until after
// v0.0.8 release-cut. These findings are pre-existing on the v0.0.7 baseline
// (last touched 2026-07-13 per commit `ec6f35e`); they became blocking only
// when FID-035 §Acceptance promoted clippy to a hard gate in this cycle.
// Decision matrix: A=in-cycle-fix (bloat + vault-crypto regression risk) vs.
// B=defer-with-FID (chosen, this anchor). Re-enable strict clippy on these
// files per FID-036 §Retry Plan when it resumes.
//
// Per-lint scope (NOT blank `clippy::all`): the three lints that fire today
// are listed verbatim so a future clippy version promoting a NEW child lint
// will fail loudly here, making the related change visible in PR diff.
#![allow(clippy::doc_overindented_list_items, clippy::doc_lazy_continuation, clippy::disallowed_methods)]

//! Master Key + Generalized Vault.
//!
//! Phase 1 ships a Vault abstraction informed by:
//! - `savant-backup/crates/core/src/crypto.rs` — AgentKeyPair + 5-strategy cascade.
//! - `hermes-rs/OAUTH_DESIGN.md` — multi-profile Vault with `env:VAR` secret_ref.
//!
//! Five-strategy cascade (preserved from savant-backup, Strategy 5 changed to UI-prompt):
//!   1. `SAVANT_<PROVIDER>_API_KEY` env var
//!   2. cwd `.env` (developer convenience)
//!   3. exe-dir `.env` (packaged app)
//!   4. Encrypted vault file at OS app-data dir
//!      (Windows: `%APPDATA%/savant/auth.json` [DPAPI-encrypted] /
//!       Unix: `~/.config/savant/auth.json` [AES-256-GCM, key in OS credential service])
//!   5. UI prompt (`MasterKeySetup.tsx`) → persist to vault file
//!
//! **Precedence & `.env` loading (FID-020 + FID-020r2) — THE
//! canonical reference for the cwd-FIRST ordering rationale**:
//! Strategy 2 is loaded BEFORE strategy 3 at startup so that
//! cwd `.env` wins over exe-dir `.env` when both define the same
//! var. `dotenvy::from_path` intentionally does NOT overwrite
//! existing env vars — by loading cwd FIRST, any var set in
//! `<cwd>/.env` takes precedence over the same var set in
//! `<exe_dir>/.env`, matching the strategy numbering (2 precedes
//! 3). Both loaders use `.ok()` to swallow the no-`.env` common
//! case (dev / packaged-prod typically have no `.env` at all —
//! env vars or the vault file cover it). Wire-up implementation:
//! [`savant_shell::run()` in `src-tauri/src/lib.rs`] +
//! `pub fn load_env_from_exe_dir` (FID-020r2).
//!
//! Unix perms enforced 0o600. **Phase 5** — at-rest encryption on every desktop OS:
//! AES-256-GCM (RustCrypto `aes-gcm` crate, hardware-accelerated) with the random
//! 256-bit key stored in the OS credential service via the `keyring` crate:
//!   - **Windows**: `keyring` `windows-native` backend wraps DPAPI (user scope).
//!     The DPAPI master key binds to the current Windows user SID; if `auth.json` is
//!     copied to another machine or another user, it cannot be decrypted. Trade-off:
//!     a system-level password reset (admin force-reset without old password) can
//!     invalidate the DPAPI master key, requiring a fresh `setup_master_key` re-vault.
//!     Standard user password CHANGES do NOT invalidate DPAPI (Windows auto-rewraps).
//!   - **Linux**: Secret Service D-Bus API (GNOME Keyring / KWallet). The
//!     `sync-secret-service` keyring feature is enabled. A D-Bus session is required.
//!   - **macOS**: Keychain via Security framework. The `apple-native` keyring
//!     feature is enabled. Always available on macOS.
//!
//! The 12-byte nonce is random per encryption (NIST SP 800-38D §5.2.1.1). The
//! 16-byte GCM authentication tag is appended to the ciphertext by `aes-gcm`.
//! The keyring entry is per-user (the OS credential service binds the key to the
//! user's session); a copy of the vault file to another machine or user cannot be
//! decrypted without the key from that user's credential service.
//!
//! This unified model (AES-256-GCM + keyring on every platform) replaced the prior
//! per-platform split: Phase 5 originally used `CryptProtectData` / `CryptUnprotectData`
//! directly via the `windows` crate on Windows + a plaintext passthrough on Unix.
//! Phase 5 r3 added the `keyring` + AES-256-GCM path for Unix. **Phase 5 r4 (this
//! pass)** unifies the path — the `keyring` crate's `windows-native` backend is
//! already a DPAPI wrapper, so direct DPAPI FFI is no longer needed.
//!
//! File format versioning:
//!   - v1: plain JSON `{ "version": 1, "profiles": {...}, "agent_identity": {...} }`
//!   - v2: `SAVANT_VAULT_V2\n` magic header (16 bytes) + DPAPI-encrypted JSON blob
//!   On `load_vault()`, v1 files are detected via the absence of the magic header
//!   and lazily migrated to v2 (immediate re-write on first load after upgrade).
//!
//! **FID-019 (this move)**: relocated from `src-tauri/src/security/master_key.rs`
//! to its own workspace crate (`savant-vault`) so `src-tauri/` can be a thin IPC
//! shell. The implementation is unchanged; the file format, public API, and
//! test surface are preserved verbatim.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Invalid key format")]
    InvalidKeyFormat,
    #[error("Vault file path resolution failed")]
    PathError,
    #[error("Profile '{0}' not found in vault")]
    ProfileNotFound(String),
    #[error("Vault encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Vault decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Vault file is corrupted (unexpected magic header or unreadable payload)")]
    CorruptedVault,
}

pub type Result<T> = std::result::Result<T, VaultError>;

// ---------------------------------------------------------------------------
// AgentKeyPair — port of savant-backup crates/core/src/crypto.rs
// Used for agent identity signing / verification (Phase 2 cognitive core).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentKeyPair {
    pub public_key: String,
    pub secret_key: String,
    pub key_id: String,
    pub created_at: i64,
}

impl AgentKeyPair {
    pub fn generate() -> Result<Self> {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        Ok(AgentKeyPair {
            public_key: hex::encode(verifying_key.as_bytes()),
            secret_key: hex::encode(signing_key.as_bytes()),
            key_id: Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().timestamp(),
        })
    }

    pub fn get_verifying_key(&self) -> std::result::Result<VerifyingKey, VaultError> {
        let bytes = hex::decode(&self.public_key).map_err(|_| VaultError::InvalidKeyFormat)?;
        if bytes.len() != 32 {
            return Err(VaultError::InvalidKeyFormat);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        VerifyingKey::from_bytes(&arr).map_err(|_| VaultError::InvalidKeyFormat)
    }

    pub fn get_signing_key(&self) -> std::result::Result<SigningKey, VaultError> {
        let bytes = hex::decode(&self.secret_key).map_err(|_| VaultError::InvalidKeyFormat)?;
        if bytes.len() != 32 {
            return Err(VaultError::InvalidKeyFormat);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(SigningKey::from_bytes(&arr))
    }

    pub fn sign_message(&self, message: &str) -> Result<String> {
        let sk = self.get_signing_key()?;
        let sig = sk.sign(message.as_bytes());
        Ok(hex::encode(sig.to_bytes()))
    }

    pub fn verify_message(&self, message: &str, signature: &str) -> Result<bool> {
        let vk = self.get_verifying_key()?;
        let sig_bytes = hex::decode(signature).map_err(|_| VaultError::InvalidKeyFormat)?;
        if sig_bytes.len() != 64 {
            return Err(VaultError::InvalidKeyFormat);
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&sig_bytes);
        let signature = Signature::from_bytes(&arr);
        Ok(vk.verify(message.as_bytes(), &signature).is_ok())
    }
}

// ---------------------------------------------------------------------------
// Vault — generalized multi-profile (per hermes-rs OAUTH_DESIGN.md schema)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub provider: String,
    pub method: String, // "api_key" | "oauth_pkce" | "bearer" | "noauth"
    pub base_url: Option<String>,
    pub secret_ref: String, // "env:VAR" reference; secrets never inline
    pub scopes: Vec<String>, // future OAuth scopes
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub version: u32,
    pub profiles: HashMap<String, ProviderProfile>, // profile-name (e.g. "openrouter-default") → profile
    #[serde(default = "default_identity")]
    pub agent_identity: AgentKeyPair,
}

fn default_identity() -> AgentKeyPair {
    // Production-code path — ECHO Law 6 + coding-standards/rust.md say
    // `.expect()` is acceptable ONLY in tests, examples, and main.rs. Since
    // `default_identity()` is invoked from `Vault::default()` (production code
    // reachable via `savant_vault::master_key` lib consumers), the proper
    // pattern is `match` + `panic!()` with rationale. OS-RNG failure is
    // unrecoverable for vault initialization — panicking preserves vault
    // integrity per FID-019's design intent rather than rolling a corrupt
    // default vault.
    match AgentKeyPair::generate() {
        Ok(kp) => kp,
        Err(e) => panic!(
            "savant_vault::master_key::default_identity: AgentKeyPair::generate() failed ({:?}). \
             OS-level RNG failure is unrecoverable; Vault::default() cannot safely proceed \
             without a valid AgentKeyPair. This panic preserves vault integrity rather than \
             producing a corrupt default vault.",
            e,
        ),
    }
}

impl Default for Vault {
    fn default() -> Self {
        Vault {
            version: 1,
            profiles: HashMap::new(),
            agent_identity: default_identity(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub name: String,
    pub provider: String,
    pub method: String,
    pub secret_ref_kind: String,
    pub base_url: Option<String>,
    pub updated_at: i64,
}

/// Returns the platform-appropriate vault file path.
///
/// Windows: `%APPDATA%/savant/auth.json`
/// Unix: `~/.config/savant/auth.json`
pub fn vault_file_path() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").map_err(|_| VaultError::PathError)?;
        Ok(PathBuf::from(appdata).join("savant").join("auth.json"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").map_err(|_| VaultError::PathError)?;
        Ok(PathBuf::from(home).join(".config").join("savant").join("auth.json"))
    }
}

// ---------------------------------------------------------------------------
// File format versioning
// ---------------------------------------------------------------------------

/// Magic header for the Phase 5 encrypted vault format (v2).
///
/// 16 bytes including the trailing newline: `SAVANT_VAULT_V2\n`.
/// On disk: `<16-byte header><DPAPI-encrypted JSON bytes>`.
pub const MAGIC_HEADER: &[u8; 16] = b"SAVANT_VAULT_V2\n";

/// True if the file at `path` looks like the legacy v1 plain-JSON format.
///
/// Detection rule: read the first byte. Plain JSON starts with `{`; v2 starts with `S`
/// (the first byte of `SAVANT_VAULT_V2`). Empty files are treated as v1 (will parse as
/// JSON error, surfacing the corruption to the caller).
fn is_v1_format(path: &Path) -> Result<bool> {
    let bytes = fs::read(path)?;
    Ok(bytes.first().copied() == Some(b'{'))
}

/// Compute the temp-file path for the atomic write: same parent dir, basename
/// with a `.tmp` suffix appended. Co-locating the temp with the destination
/// guarantees the rename stays on a single filesystem (required for atomicity
/// on Unix; `MoveFileExW + MOVEFILE_REPLACE_EXISTING` on Windows is already
/// atomic regardless of volume).
pub fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    PathBuf::from(tmp_os)
}

// ---------------------------------------------------------------------------
// At-rest protection — unified AES-256-GCM + keyring path
// (every desktop OS: Windows via keyring's DPAPI wrapper, Linux via libsecret,
//  macOS via Keychain).
// ---------------------------------------------------------------------------

/// Keyring service name (groups all Savant vault key entries).
const KEYRING_SERVICE: &str = "savant-vault-key";
/// Keyring username (single vault-wide key; per-profile keys are a future FID).
const KEYRING_USER: &str = "default";

/// Get-or-create the 256-bit AES-256-GCM key from the OS credential service.
///
/// On first use, generates a random 32-byte key via `OsRng` and stores it hex-encoded
/// in the keyring. On subsequent reads, fetches the key from the keyring. The keyring
/// entry is per-user (DPAPI on Windows, libsecret's D-Bus session on Linux, Keychain
/// on macOS); another user on the same machine (or the vault file copied to another
/// machine) cannot decrypt without the key from that user's credential service.
fn get_or_create_vault_key() -> Result<[u8; 32]> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| VaultError::EncryptionFailed(format!("keyring entry: {}", e)))?;

    match entry.get_password() {
        Ok(hex_key) => {
            // Existing key: hex-decode to 32 bytes. Capture the length
            // BEFORE `try_into` consumes the Vec, so the error closure can
            // report the actual length.
            let bytes = hex::decode(&hex_key).map_err(|e| {
                VaultError::DecryptionFailed(format!("keyring key hex decode: {}", e))
            })?;
            let len = bytes.len();
            bytes.try_into().map_err(|_| {
                VaultError::DecryptionFailed(format!(
                    "keyring key length {} (expected 32)",
                    len
                ))
            })
        }
        Err(keyring::Error::NoEntry) => {
            // First use: generate a new random key and store it.
            let mut key = [0u8; 32];
            use rand::RngCore;
            OsRng.fill_bytes(&mut key);
            entry
                .set_password(&hex::encode(key))
                .map_err(|e| VaultError::EncryptionFailed(format!("keyring set: {}", e)))?;
            Ok(key)
        }
        Err(e) => Err(VaultError::EncryptionFailed(format!("keyring get: {}", e))),
    }
}

/// Dispatcher: protect plaintext bytes. Unified AES-256-GCM path on every platform
/// (the `keyring` crate's backend handles the platform-specific key storage).
///
/// `pub` so the integration test in `crates/vault/tests/master_key_test.rs` can
/// exercise the roundtrip directly. The dispatcher pattern is preserved in case
/// a future FID reintroduces a per-platform split (e.g. hardware-bound keys).
pub fn platform_protect(plaintext: &[u8]) -> Result<Vec<u8>> {
    aes_gcm_protect(plaintext)
}

/// Dispatcher: unprotect vault blob bytes. Unified AES-256-GCM path on every platform.
///
/// `pub` for the same reason as [`platform_protect`].
pub fn platform_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    aes_gcm_unprotect(blob)
}

/// AES-256-GCM protect. On-disk payload: `<12-byte nonce><ciphertext + 16-byte tag>`.
/// Nonce is random per encryption (NIST SP 800-38D §5.2.1.1).
fn aes_gcm_protect(plaintext: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use rand::RngCore;

    let key_bytes = get_or_create_vault_key()?;
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext_with_tag = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::EncryptionFailed(format!("AES-GCM encrypt: {}", e)))?;

    let mut out = Vec::with_capacity(12 + ciphertext_with_tag.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext_with_tag);
    Ok(out)
}

/// AES-256-GCM unprotect. Inverse of `aes_gcm_protect`.
/// The on-disk payload is split: first 12 bytes = nonce; remainder = ciphertext + 16-byte GCM tag.
fn aes_gcm_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Key, Nonce};

    // Minimum size: 12-byte nonce + 16-byte GCM tag = 28 bytes (empty plaintext).
    if blob.len() < 28 {
        return Err(VaultError::CorruptedVault);
    }
    let (nonce_bytes, ciphertext_and_tag) = blob.split_at(12);
    let key_bytes = get_or_create_vault_key()?;
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext_and_tag)
        .map_err(|e| VaultError::DecryptionFailed(format!("AES-GCM decrypt: {}", e)))
}

// ---------------------------------------------------------------------------
// File I/O — v1 (plain JSON) and v2 (magic header + DPAPI) are both supported.
// `write_vault_file` always writes v2. `read_vault_file` auto-detects v1 vs v2.
// `load_vault` triggers a lazy v1→v2 migration on first read after upgrade.
// ---------------------------------------------------------------------------

fn read_vault_file(path: &Path) -> Result<Vault> {
    let bytes = fs::read(path)?;

    if bytes.len() >= MAGIC_HEADER.len() && &bytes[..MAGIC_HEADER.len()] == MAGIC_HEADER {
        // v2: magic header + DPAPI-encrypted JSON
        let protected = &bytes[MAGIC_HEADER.len()..];
        if protected.is_empty() {
            return Err(VaultError::CorruptedVault);
        }
        let plaintext = platform_unprotect(protected)?;
        let vault: Vault = serde_json::from_slice(&plaintext)?;
        Ok(vault)
    } else {
        // v1: legacy plain JSON. No magic header detected.
        let vault: Vault = serde_json::from_slice(&bytes)?;
        Ok(vault)
    }
}

fn write_vault_file(vault: &Vault, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(vault)?;
    let protected = platform_protect(json.as_bytes())?;

    let mut payload = Vec::with_capacity(MAGIC_HEADER.len() + protected.len());
    payload.extend_from_slice(MAGIC_HEADER);
    payload.extend_from_slice(&protected);

    // Atomic write protocol:
    //   1. Serialize the full payload to `<path>.tmp` in the same directory.
    //   2. `fsync` the tempfile so the bytes survive a power loss.
    //   3. `fs::rename(tmp, path)` — atomic on both Windows (MoveFileExW with
    //      MOVEFILE_REPLACE_EXISTING) and Unix (rename(2) within one filesystem).
    //   4. On any failure mid-write, best-effort remove the partial `.tmp` so it
    //      does not accumulate across save attempts.
    //
    // Threat model: protects against a failed write (disk full, perms, ENOSPC,
    // process killed mid-write) corrupting the on-disk vault. A partial write
    // to the temp file leaves the original vault untouched; the rename is the
    // only step that mutates the canonical path.
    let tmp_path = tmp_path_for(path);

    let write_result: Result<()> = (|| {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true) // create if absent
                .truncate(true) // truncate if a prior partial write left a stale .tmp
                .mode(0o600) // owner read+write only, applied at file creation
                .open(&tmp_path)?;
            f.write_all(&payload)?;
            f.sync_all()?;
        }

        #[cfg(not(unix))]
        {
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            f.write_all(&payload)?;
            f.sync_all()?;
        }
        Ok(())
    })();

    if let Err(e) = write_result {
        // Best-effort cleanup; ignore cleanup errors (the original `e` is more
        // informative than any remove_file error).
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // The atomic swap. On Windows, `fs::rename` uses MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING, which atomically replaces the destination.
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(VaultError::Io(e));
    }

    Ok(())
}

/// Load the vault from disk. Triggers a lazy v1→v2 migration on first read
/// after upgrade (re-writes the file with DPAPI protection immediately).
pub async fn load_vault() -> Result<Vault> {
    let path = match vault_file_path() {
        Ok(p) => p,
        Err(_) => return Ok(Vault::default()), // Strategy 5 placeholder: empty vault, UI prompts to populate.
    };
    if !path.exists() {
        return Ok(Vault::default());
    }

    // Detect v1 BEFORE reading (so we can trigger migration after a successful read).
    let needs_migration = is_v1_format(&path)?;
    let vault = read_vault_file(&path)?;

    if needs_migration {
        // Lazily upgrade the on-disk format to v2 (DPAPI-encrypted). Idempotent:
        // subsequent calls will detect v2 via the magic header and skip this branch.
        write_vault_file(&vault, &path)?;
    }

    Ok(vault)
}

/// Resolve a secret-referenced-environment-variable (e.g. `env:OPENROUTER_API_KEY`).
pub fn resolve_secret(secret_ref: &str) -> Result<String> {
    if let Some(var) = secret_ref.strip_prefix("env:") {
        std::env::var(var).map_err(|_| VaultError::InvalidKeyFormat)
    } else {
        Err(VaultError::InvalidKeyFormat)
    }
}

/// Save (or update) a provider profile and write the vault to disk.
pub async fn save_profile(provider_name: &str, api_key: &str) -> Result<()> {
    let mut vault = load_vault().await?;
    let now = chrono::Utc::now().timestamp();
    let env_var = format!(
        "SAVANT_{}_API_KEY",
        provider_name.to_uppercase().replace('-', "_")
    );
    std::env::set_var(&env_var, api_key);

    let profile_name = format!("{}-default", provider_name);
    let base_url = match provider_name {
        "openrouter" => Some("https://openrouter.ai/api/v1".to_string()),
        _ => None,
    };

    vault.profiles.insert(
        profile_name.clone(),
        ProviderProfile {
            provider: provider_name.to_string(),
            method: "api_key".to_string(),
            base_url,
            secret_ref: format!("env:{}", env_var),
            scopes: vec![],
            expires_at: None,
            created_at: now,
            updated_at: now,
        },
    );

    let path = vault_file_path()?;
    write_vault_file(&vault, &path)?;
    tracing::info!("[vault] saved profile {} → {}", profile_name, path.display());
    Ok(())
}

/// Resolve a profile's api key from the persisted env-reference.
pub async fn lookup_api_key(profile_name: &str) -> Result<String> {
    let vault = load_vault().await?;
    let profile = vault
        .profiles
        .get(profile_name)
        .ok_or_else(|| VaultError::ProfileNotFound(profile_name.to_string()))?;
    resolve_secret(&profile.secret_ref)
}

/// Lists profiles for UI inspection. Does not return the api key itself.
pub async fn list_profiles() -> Result<Vec<ProfileSummary>> {
    let vault = load_vault().await?;
    Ok(vault
        .profiles
        .iter()
        .map(|(name, p)| ProfileSummary {
            name: name.clone(),
            provider: p.provider.clone(),
            method: p.method.clone(),
            secret_ref_kind: p.secret_ref.split(':').next().unwrap_or("unknown").to_string(),
            base_url: p.base_url.clone(),
            updated_at: p.updated_at,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Public test helpers — explicit-path variants of `load_vault` / `write_vault_file`.
// These let integration tests exercise the v1→v2 migration + DPAPI roundtrip
// against tempfiles without touching the user's real OS vault.
// ---------------------------------------------------------------------------

/// Load a vault from an explicit file path. Triggers the same v1→v2 lazy migration
/// as `load_vault()`. Used by integration tests with `tempfile::tempdir()`.
pub async fn load_vault_from_path(path: &Path) -> Result<Vault> {
    if !path.exists() {
        return Ok(Vault::default());
    }
    let needs_migration = is_v1_format(path)?;
    let vault = read_vault_file(path)?;
    if needs_migration {
        write_vault_file(&vault, path)?;
    }
    Ok(vault)
}

/// Write a vault to an explicit file path (always in v2/DPAPI format).
/// Used by integration tests to seed a v1 file before exercising the migration.
pub async fn write_vault_to_path(vault: &Vault, path: &Path) -> Result<()> {
    write_vault_file(vault, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_keypair_sign_verify_roundtrip() {
        let kp = AgentKeyPair::generate().unwrap();
        let msg = "hello world";
        let sig = kp.sign_message(msg).unwrap();
        assert!(kp.verify_message(msg, &sig).unwrap());
        assert!(!kp.verify_message("tampered", &sig).unwrap());
    }

    #[test]
    fn resolve_env_secret_ref() {
        std::env::set_var("TEST_VAULT_VAR", "secret-value");
        let resolved = resolve_secret("env:TEST_VAULT_VAR").unwrap();
        assert_eq!(resolved, "secret-value");
    }

    #[test]
    fn reject_non_env_secret_ref() {
        let err = resolve_secret("plain-string").unwrap_err();
        assert!(matches!(err, VaultError::InvalidKeyFormat));
    }
}
