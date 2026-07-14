//! Smoke tests for the OpenRouter inference client.
//!
//! These guard the contract that **`chat_completion` errors out with
//! `VaultError::ProfileNotFound` BEFORE any HTTP traffic** when the vault
//! has no `openrouter-default` profile. We never want to fall back to
//! ambient `SAVANT_OPENROUTER_API_KEY` env vars (the spirit of the
//! per-hermes-rs OAUTH_DESIGN.md auth profile model).

use savant_shell::inference::openrouter::{self, InferenceError};
use savant_vault::master_key::{self, VaultError};
use std::env;

#[tokio::test]
async fn chat_completion_returns_vault_error_when_no_profile() {
    // Make sure ambient env vars do NOT silently authorize the call.
    env::remove_var("SAVANT_OPENROUTER_API_KEY");
    env::remove_var("SAVANT_OPENROUTERDEFAULT_API_KEY");

    let result = openrouter::chat_completion("hello").await;
    let err = result.expect_err("expected vault-error before HTTP");

    match err {
        InferenceError::Vault(VaultError::ProfileNotFound(name)) => {
            assert_eq!(name, "openrouter-default");
        }
        other => panic!("expected VaultError::ProfileNotFound; got {:?}", other),
    }
}

#[test]
fn openrouter_endpoint_url_pinned() {
    // Compile-time guard against accidental URL drift.
    assert_eq!(
        openrouter::OPENROUTER_CHAT_COMPLETIONS,
        "https://openrouter.ai/api/v1/chat/completions"
    );
}

#[tokio::test]
async fn incomplete_profile_path_also_errors() {
    // Even if the profile exists but its env var is unset, lookup must fail
    // (not fall through to network traffic with a stale key).
    env::remove_var("SAVANT_PHANTOM_API_KEY");
    // We do not call save_profile here because that would write to the
    // real OS vault; instead, simulate by calling resolve_secret directly.
    let err = master_key::resolve_secret("env:SAVANT_PHANTOM_API_KEY")
        .expect_err("unset env ref errors");
    assert!(matches!(err, master_key::VaultError::InvalidKeyFormat));
}
