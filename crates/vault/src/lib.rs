//! Savant vault — application-layer secrets management.
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
//!      (Windows: `%APPDATA%/savant/auth.json` [AES-256-GCM, key in OS credential service] /
//!       Unix: `~/.config/savant/auth.json` [AES-256-GCM, key in OS credential service])
//!   5. UI prompt (`MasterKeySetup.tsx`) → persist to vault file
//!
//! The at-rest encryption model is the same on every desktop OS:
//!   - **Windows**: `keyring` `windows-native` backend wraps DPAPI (user scope).
//!   - **Linux**:   Secret Service D-Bus API (GNOME Keyring / KWallet) via `sync-secret-service`.
//!   - **macOS**:   Keychain via Security framework via `apple-native`.
//! The 256-bit AES key is held in the OS credential service; the file holds
//! `<12-byte nonce><AES-256-GCM ciphertext + 16-byte tag>` (NIST SP 800-38D §5.2.1.1).
//!
//! File format versioning:
//!   - v1: plain JSON `{ "version": 1, "profiles": {...}, "agent_identity": {...} }`
//!   - v2: `SAVANT_VAULT_V2\n` magic header (16 bytes) + AES-256-GCM ciphertext
//!   On `load_vault()`, v1 files are detected via the absence of the magic header
//!   and lazily migrated to v2 (immediate re-write on first load after upgrade).
//!
//! Moved from `src-tauri/src/security/master_key.rs` in FID-019 so the Tauri shell
//! can be a thin IPC layer. See [`master_key`] for the full implementation.

pub mod master_key;

// Re-export the public API at the crate root for ergonomic consumers
// (e.g. `savant_vault::Vault` instead of `savant_vault::master_key::Vault`).
pub use master_key::{
    AgentKeyPair, ProfileSummary, ProviderProfile, Vault, VaultError, MAGIC_HEADER,
    list_profiles, load_vault, load_vault_from_path, lookup_api_key, platform_protect,
    platform_unprotect, resolve_secret, save_profile, tmp_path_for, vault_file_path,
    write_vault_to_path,
};
