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
//!      (Windows: `%APPDATA%/savant/auth.json` [DPAPI-encrypted] / Unix: `~/.config/savant/auth.json` [plain JSON, 0o600])
//!   5. UI prompt (`MasterKeySetup.tsx`) → persist to vault file
//!
//! Unix perms enforced 0o600. Windows: **Phase 5** — vault is DPAPI-encrypted at rest
//! (user scope, `CRYPTPROTECT_LOCAL_MACHINE` flag = 0). The encrypted blob binds to
//! the current Windows user SID; if `auth.json` is copied to another machine or
//! another user, it cannot be decrypted. Trade-off: a system-level password reset
//! (admin force-reset without old password) can invalidate the DPAPI master key,
//! requiring a fresh `setup_master_key` re-vault. Standard user password CHANGES
//! do NOT invalidate DPAPI (Windows auto-rewraps the master key on logon).
//!
//! File format versioning:
//!   - v1: plain JSON `{ "version": 1, "profiles": {...}, "agent_identity": {...} }`
//!   - v2: `SAVANT_VAULT_V2\n` magic header (16 bytes) + DPAPI-encrypted JSON blob
//!   On `load_vault()`, v1 files are detected via the absence of the magic header
//!   and lazily migrated to v2 (immediate re-write on first load after upgrade).

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
    AgentKeyPair::generate().expect("OS RNG must produce keys")
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
// Platform-conditional at-rest protection (DPAPI on Windows, passthrough on Unix)
// ---------------------------------------------------------------------------

/// Dispatcher: protect plaintext bytes. Windows uses DPAPI; Unix is a passthrough
/// (the file's 0o600 perms are the only at-rest protection; libsecret parity is a future FID).
fn platform_protect(plaintext: &[u8]) -> Result<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        dpapi_protect(plaintext)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Unix passthrough: encryption is deferred to libsecret in a future FID.
        // For now, the magic header is still prepended by `write_vault_file`, so the
        // file format is consistent across platforms even though the "encrypted"
        // bytes on Unix are the same as the plaintext.
        Ok(plaintext.to_vec())
    }
}

/// Dispatcher: unprotect vault blob bytes. Windows uses DPAPI; Unix is a passthrough.
fn platform_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        dpapi_unprotect(blob)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(blob.to_vec())
    }
}

/// Windows DPAPI protect (user scope, `CRYPTPROTECT_LOCAL_MACHINE` flag = 0).
///
/// The encrypted blob is bound to the current Windows user SID. It can only be
/// decrypted by the same user on the same machine. Standard password changes do
/// not invalidate DPAPI (Windows re-wraps the master key on logon); a system-level
/// password reset (admin force-reset without old password) can.
#[cfg(target_os = "windows")]
fn dpapi_protect(plaintext: &[u8]) -> Result<Vec<u8>> {
    use std::ffi::c_void;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    unsafe {
        CryptProtectData(
            &mut input,
            PCWSTR::null(),
            None,                       // poptionalentropy: Option<*const CRYPT_INTEGER_BLOB>
            None,                       // pvreserved: Option<*const c_void>
            Some(std::ptr::null()),     // ppromptstruct: Option<*const CRYPT_PROMPTSTRUCT> (no UI prompt)
            0,                          // dwflags: user scope (no CRYPTPROTECT_LOCAL_MACHINE)
            &mut output,
        )
        .map_err(|e| {
            VaultError::EncryptionFailed(format!("CryptProtectData: {}", e.message()))
        })?;
    }

    let protected = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };

    // DPAPI allocates `output.pbData` on the local heap; we MUST LocalFree it
    // after copying the bytes into a Rust Vec. Ignoring the return value is safe:
    // LocalFree returns NULL on success and the original handle on failure (which
    // we can't do anything about at this point).
    unsafe {
        let _ = LocalFree(HLOCAL(output.pbData as *mut c_void));
    }

    Ok(protected)
}

/// Windows DPAPI unprotect. Inverse of `dpapi_protect`.
#[cfg(target_os = "windows")]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    unsafe {
        CryptUnprotectData(
            &mut input,
            Some(std::ptr::null_mut()),   // ppszdatadescr: Option<*mut PWSTR> (description output, not needed)
            None,                          // poptionalentropy: Option<*const CRYPT_INTEGER_BLOB>
            None,                          // pvreserved: Option<*const c_void>
            Some(std::ptr::null()),        // ppromptstruct: Option<*const CRYPT_PROMPTSTRUCT>
            0,
            &mut output,
        )
        .map_err(|e| {
            VaultError::DecryptionFailed(format!("CryptUnprotectData: {}", e.message()))
        })?;
    }

    let plaintext = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };

    unsafe {
        let _ = LocalFree(HLOCAL(output.pbData as *mut c_void));
    }

    Ok(plaintext)
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
