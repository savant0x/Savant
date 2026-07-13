//! Integration tests for the master-key vault 5-strategy cascade.

use savant_shell::security::master_key;

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
