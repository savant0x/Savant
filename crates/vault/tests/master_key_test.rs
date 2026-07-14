//! Integration tests for the master-key vault 5-strategy cascade.
//!
//! Moved from `src-tauri/tests/master_key_test.rs` in FID-019 as part of the
//! vault extraction to a workspace crate. Test logic is unchanged.

use savant_vault::master_key;

#[tokio::test]
async fn empty_vault_returns_default_when_no_file() {
    // When no vault file exists, load_vault returns Vault::default().
    // Use an isolated HOME (Unix) or APPDATA (Windows) so we don't touch the
    // user's real vault. Tests run with that env set in CI.
    let vault = master_key::load_vault().await.expect("default loads");
    assert_eq!(vault.version, 1);
    assert!(vault.profiles.is_empty());
}

#[tokio::test]
async fn env_secret_ref_resolves() {
    std::env::set_var("SAVANT_TEST_RESOLVE", "super-secret");
    let resolved =
        master_key::resolve_secret("env:SAVANT_TEST_RESOLVE").expect("resolves");
    assert_eq!(resolved, "super-secret");
    std::env::remove_var("SAVANT_TEST_RESOLVE");
}

#[tokio::test]
async fn missing_profile_returns_error() {
    std::env::remove_var("SAVANT_NONEXISTENT_PROFILE_API_KEY");
    let err = master_key::lookup_api_key("nonexistent-default")
        .await
        .expect_err("missing profile errors");
    assert!(matches!(err, master_key::VaultError::ProfileNotFound(_)));
}

#[tokio::test]
async fn non_env_secret_ref_rejected() {
    let err =
        master_key::resolve_secret("plain-string").expect_err("non-env rejected");
    assert!(matches!(err, master_key::VaultError::InvalidKeyFormat));
}

// ---------------------------------------------------------------------------
// Phase 5 — DPAPI at-rest encryption tests.
// ---------------------------------------------------------------------------

/// Unified keyring + AES-256-GCM protect → unprotect roundtrip. Cross-platform
/// (Phase 5 r4 unified the prior Windows DPAPI + Unix keyring paths into a single
/// AES-256-GCM + keyring path; the `keyring` crate's `windows-native` backend
/// wraps DPAPI for credential storage on Windows). Skips if the OS credential
/// service is unavailable (e.g., headless Linux CI without D-Bus session).
/// On Windows + macOS, the credential service is always available.
#[tokio::test]
async fn aes_gcm_roundtrip() {
    // Runtime probe: try to create + set + get + delete a test-only keyring entry.
    // If any step fails, the credential service is unavailable (no D-Bus session,
    // no Keychain, no DPAPI, etc.) — skip the test rather than fail.
    let probe = match keyring::Entry::new("savant-vault-test-probe", "availability") {
        Ok(e) => e,
        Err(e) => {
            eprintln!("keyring: Entry::new failed ({}); skipping test", e);
            return;
        }
    };
    if let Err(e) = probe.set_password("probe-value") {
        eprintln!(
            "keyring: set_password failed ({}); likely no D-Bus session / Keychain; skipping test",
            e
        );
        return;
    }
    match probe.get_password() {
        Ok(v) => assert_eq!(v, "probe-value", "probe roundtrip must match"),
        Err(e) => {
            eprintln!("keyring: get_password failed ({}); skipping test", e);
            let _ = probe.delete_credential();
            return;
        }
    }
    let _ = probe.delete_credential();

    // Keyring is available. Run the production keyring + AES-256-GCM roundtrip.
    let plaintext = b"super-secret-vault-payload-{unified-keyring-roundtrip}";
    let protected = master_key::platform_protect(plaintext).expect("protect succeeds");
    assert_ne!(
        protected.as_slice(),
        plaintext,
        "keyring + AES-256-GCM must mutate the plaintext bytes"
    );
    // Verify the on-disk payload shape: 12-byte nonce + ciphertext + 16-byte GCM tag.
    assert!(
        protected.len() >= 28,
        "protected payload must be at least 28 bytes (12 nonce + 16 tag); got {}",
        protected.len()
    );
    let recovered =
        master_key::platform_unprotect(&protected).expect("unprotect succeeds");
    assert_eq!(
        recovered.as_slice(),
        plaintext,
        "keyring + AES-256-GCM roundtrip must recover the exact original bytes"
    );
}

/// Write a vault with `write_vault_to_path` and verify the on-disk file
/// starts with the `SAVANT_VAULT_V2\n` magic header. Cross-platform:
/// Windows prepends magic + DPAPI blob; Unix prepends magic + plaintext
/// (passthrough, encryption is libsecret-deferred).
#[tokio::test]
async fn v2_file_is_written_with_magic_header() {
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("auth.json");
    let vault = master_key::Vault::default();

    master_key::write_vault_to_path(&vault, &path)
        .await
        .expect("write v2");

    let mut file = std::fs::File::open(&path).expect("open");
    let mut header = [0u8; 16];
    file.read_exact(&mut header).expect("read 16 header bytes");
    assert_eq!(
        &header,
        b"SAVANT_VAULT_V2\n",
        "v2 file must begin with the SAVANT_VAULT_V2 magic header"
    );

    let total_len = std::fs::metadata(&path).expect("metadata").len();
    assert!(
        total_len > 16,
        "v2 file must contain payload bytes after the 16-byte header (got {} total bytes)",
        total_len
    );
}

/// Roundtrip: write a v2 vault, load it back, verify contents are preserved.
#[tokio::test]
async fn v2_file_roundtrips_through_load() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("auth.json");

    // Seed with a non-default vault (version 1, empty profiles, fresh identity).
    let original = master_key::Vault::default();
    master_key::write_vault_to_path(&original, &path)
        .await
        .expect("write");

    let loaded = master_key::load_vault_from_path(&path)
        .await
        .expect("load v2");

    assert_eq!(loaded.version, original.version, "version preserved");
    assert_eq!(
        loaded.profiles.len(),
        original.profiles.len(),
        "profile count preserved"
    );
    assert_eq!(
        loaded.agent_identity.key_id, original.agent_identity.key_id,
        "agent identity key_id preserved through DPAPI roundtrip"
    );
}

/// v1 plain-JSON vault is auto-detected and lazily migrated to v2 on first load.
/// Cross-platform: on Windows, the v2 file is DPAPI-encrypted; on Unix, it's
/// magic-header + plaintext (libsecret is a future FID).
#[tokio::test]
async fn v1_vault_is_lazily_migrated_to_v2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("auth.json");

    // Seed a v1 plain-JSON vault (no magic header; starts with `{`).
    let v1_json = r#"{"version":1,"profiles":{}}"#;
    std::fs::write(&path, v1_json).expect("write v1");

    // Confirm the seeded file is detected as v1 (first byte == '{').
    let first_byte_before = std::fs::read(&path)
        .expect("read v1")
        .into_iter()
        .next();
    assert_eq!(
        first_byte_before,
        Some(b'{'),
        "seeded v1 file must start with '{{'"
    );

    // Loading triggers the lazy migration.
    let _loaded = master_key::load_vault_from_path(&path)
        .await
        .expect("load + migrate v1 → v2");

    // After migration, the file must start with the v2 magic header byte 'S'.
    let first_byte_after = std::fs::read(&path)
        .expect("read after migration")
        .into_iter()
        .next();
    assert_eq!(
        first_byte_after,
        Some(b'S'),
        "after migration, first byte must be 'S' (SAVANT_VAULT_V2 header)"
    );
}

/// Loading a v2 file a SECOND time must NOT re-trigger the migration (idempotent).
/// If it did, every read would do a write — wasteful + a potential race in concurrent use.
#[tokio::test]
async fn v2_load_is_idempotent_no_rewrite() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("auth.json");

    // Write v2.
    let vault = master_key::Vault::default();
    master_key::write_vault_to_path(&vault, &path)
        .await
        .expect("write v2");

    // Snapshot the on-disk mtime + size after the initial write.
    let metadata_before = std::fs::metadata(&path).expect("metadata before");
    let size_before = metadata_before.len();
    let mtime_before = metadata_before
        .modified()
        .expect("mtime before");

    // Sleep briefly so mtime would tick if a rewrite happened.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Load twice — neither should rewrite the file.
    let _ = master_key::load_vault_from_path(&path)
        .await
        .expect("load 1");
    let _ = master_key::load_vault_from_path(&path)
        .await
        .expect("load 2");

    let metadata_after = std::fs::metadata(&path).expect("metadata after");
    assert_eq!(
        metadata_after.len(),
        size_before,
        "v2 load must not rewrite the file (size unchanged)"
    );
    assert_eq!(
        metadata_after.modified().expect("mtime after"),
        mtime_before,
        "v2 load must not rewrite the file (mtime unchanged)"
    );
}

/// Corrupted v2 file (magic header but no payload) returns `CorruptedVault` rather
/// than silently producing an empty `Vault::default()`.
#[tokio::test]
async fn v2_with_header_but_no_payload_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("auth.json");

    // Write just the magic header, no payload.
    std::fs::write(&path, master_key::MAGIC_HEADER)
        .expect("write header-only file");

    let err = master_key::load_vault_from_path(&path)
        .await
        .expect_err("empty-payload v2 must error");
    assert!(
        matches!(err, master_key::VaultError::CorruptedVault),
        "expected CorruptedVault; got {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// Phase 5 r2 — atomic write tests.
// ---------------------------------------------------------------------------

/// `tmp_path_for` appends `.tmp` to the basename, keeping the same parent dir.
/// This guarantees the rename stays on one filesystem (required for Unix atomicity).
#[test]
fn tmp_path_for_appends_dot_tmp() {
    let path = std::path::Path::new("/some/dir/auth.json");
    let tmp = master_key::tmp_path_for(path);
    assert_eq!(tmp.to_str(), Some("/some/dir/auth.json.tmp"));

    // Windows-style path
    let path = std::path::Path::new(r"C:\Users\spenc\AppData\Roaming\savant\auth.json");
    let tmp = master_key::tmp_path_for(path);
    assert_eq!(
        tmp.to_str(),
        Some(r"C:\Users\spenc\AppData\Roaming\savant\auth.json.tmp")
    );
}

/// A successful write must NOT leave a `.tmp` file behind — the atomic rename
/// cleans it up. If a `.tmp` lingers, either the rename failed silently or the
/// implementation skipped the rename step.
#[tokio::test]
async fn successful_write_leaves_no_tempfile() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let path = tmpdir.path().join("auth.json");
    let tmp = master_key::tmp_path_for(&path);

    let vault = master_key::Vault::default();
    master_key::write_vault_to_path(&vault, &path)
        .await
        .expect("write");

    assert!(path.exists(), "vault must be written to the canonical path");
    assert!(
        !tmp.exists(),
        ".tmp file must be cleaned up after successful atomic rename (found leftover at {:?})",
        tmp
    );
}

/// A failed write must NOT corrupt the existing vault file. We force a failure
/// by pre-creating a DIRECTORY at the `.tmp` path — the `open()` call inside
/// `write_vault_file` will error, the original `auth.json` must be untouched
/// (size + mtime preserved), and the leftover `.tmp` directory is just our
/// test scaffolding (we clean it up).
#[tokio::test]
async fn failed_write_does_not_corrupt_existing_vault() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let path = tmpdir.path().join("auth.json");
    let tmp = master_key::tmp_path_for(&path);

    // Seed an existing vault + snapshot its on-disk state.
    let existing = master_key::Vault::default();
    master_key::write_vault_to_path(&existing, &path)
        .await
        .expect("seed");
    let size_before = std::fs::metadata(&path).expect("metadata before").len();
    let mtime_before = std::fs::metadata(&path)
        .expect("mtime before")
        .modified()
        .expect("mtime before");

    std::thread::sleep(std::time::Duration::from_millis(50));

    // Force the .tmp path to be a directory, so `open()` inside write_vault_file fails.
    std::fs::create_dir(&tmp).expect("create dir at .tmp path");

    // Attempt a write — it must error.
    let result = master_key::write_vault_to_path(&existing, &path).await;
    assert!(
        result.is_err(),
        "write must error when the .tmp path is occupied by a directory"
    );

    // The original vault must be byte-identical and timestamp-untouched.
    let size_after = std::fs::metadata(&path).expect("metadata after").len();
    let mtime_after = std::fs::metadata(&path)
        .expect("mtime after")
        .modified()
        .expect("mtime after");
    assert_eq!(
        size_after, size_before,
        "vault size must be unchanged after a failed write"
    );
    assert_eq!(
        mtime_after, mtime_before,
        "vault mtime must be unchanged after a failed write (no truncation, no rename)"
    );

    // Verify the vault is still loadable (no corruption).
    let reloaded = master_key::load_vault_from_path(&path)
        .await
        .expect("load after failed write");
    assert_eq!(
        reloaded.agent_identity.key_id, existing.agent_identity.key_id,
        "vault identity must survive a failed write attempt"
    );

    // Cleanup the .tmp dir we planted.
    let _ = std::fs::remove_dir(&tmp);
}
