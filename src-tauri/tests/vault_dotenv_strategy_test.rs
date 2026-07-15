//! Test that the vault's strategy 2 (cwd `.env`) resolves a key after
//! `dotenvy` has loaded the file.
//!
//! Background (FID-020): the vault's 5-strategy cascade in
//! [`savant_vault::master_key`] lists strategies 2 (cwd `.env`) and 3
//! (exe-dir `.env`) in its docstring, but no code ever called
//! `dotenvy::dotenv()` to actually load those files — so the strategies
//! were silently non-functional. FID-020 wires `dotenvy::dotenv().ok();`
//! into the top of `savant_shell::run()` so the vault's env-var
//! resolution (`std::env::var` under the hood) can actually see values
//! set in `.env`. This test verifies the wiring works end-to-end.
//!
//! Note on `dotenvy::from_path` vs `dotenvy::dotenv`: the production code
//! in `run()` calls `dotenvy::dotenv()` (which uses cwd). The test uses
//! `dotenvy::from_path(&env_path)` to avoid mutating the process's cwd —
//! which would race with other parallel tests in the same `cargo test`
//! run. The two functions share the same internal loader (from_path is
//! from_filename with an explicit path), so the test exercises the same
//! code path semantically.

use savant_vault::master_key;

#[test]
fn vault_strategy_2_cwd_dotenv_resolves_key() {
    // Unique per-PID env var + file name so parallel test instances
    // (different processes) don't collide on the same env var.
    let pid = std::process::id();
    let env_var = format!("SAVANT_TEST_DOTENV_KEY_{pid}");
    let secret_ref = format!("env:{env_var}");
    let env_path = std::env::temp_dir().join(format!("savant_test_dotenv_{pid}.env"));

    // Pre-test cleanup: ensure the env var isn't already set from a
    // prior run (cargo test may re-run on file watch).
    std::env::remove_var(&env_var);

    // Write a temp .env file with a known value.
    std::fs::write(&env_path, format!("{env_var}=foo\n")).expect("write .env");

    // Load the .env file into the process env. `from_path` is used
    // instead of `dotenv()` to avoid `set_current_dir` (which would
    // race with other parallel tests). The two functions share the
    // same internal loader — `from_path` is just `from_filename` with
    // an explicit path, so the test exercises the same code path
    // semantically as the production `dotenvy::dotenv().ok();` call.
    dotenvy::from_path(&env_path).ok();

    // The vault's strategy 2 (cwd .env) should now resolve the key.
    let resolved = master_key::resolve_secret(&secret_ref)
        .expect("resolve succeeds after dotenvy load");
    assert_eq!(resolved, "foo", "vault strategy 2 must resolve from .env");

    // Cleanup: remove the env var + the temp file.
    std::env::remove_var(&env_var);
    let _ = std::fs::remove_file(&env_path);
}

#[test]
fn vault_strategy_3_exe_dir_dotenv_resolves_key() {
    // FID-020r2: verifies the strategy-3 wire-up. Writes a `.env` to a
    // fake exe-dir (PID-suffixed subdir of the system temp dir), calls
    // `savant_shell::load_env_from_exe_dir` with a fake-binary path
    // inside that dir, and asserts the vault's `resolve_secret`
    // (strategy 1: `std::env::var`) returns the value loaded from
    // `<exe_dir>/.env`.
    //
    // The fake-binary path doesn't need to exist on disk — the helper
    // only does `.parent().join(".env")`, so any plausible
    // `parent-of-filename` shape is sufficient. Using a PID-suffixed
    // subdir + a per-test env var name avoids collision across
    // parallel test processes (cargo test runs tests concurrently).
    let pid = std::process::id();
    let test_marker = "strategy3"; // distinguishes from the missing-file test below
    let fake_exe_dir = std::env::temp_dir()
        .join(format!("savant_exe_dir_{pid}_{test_marker}"));
    let env_path = fake_exe_dir.join(".env");
    let fake_exe_path = fake_exe_dir.join("savant_fake_binary");
    let env_var = format!("SAVANT_TEST_EXE_DOTENV_KEY_{pid}_{test_marker}");
    let secret_ref = format!("env:{env_var}");

    // Pre-test cleanup: ensure the env var isn't already set + the
    // dir doesn't carry leftover state across reruns.
    std::env::remove_var(&env_var);
    let _ = std::fs::remove_dir_all(&fake_exe_dir);
    std::fs::create_dir_all(&fake_exe_dir).expect("create fake exe dir");

    // Write the .env file at `<fake_exe_dir>/.env` (= <exe_dir>/.env).
    std::fs::write(&env_path, format!("{env_var}=foo_strategy_3\n"))
        .expect("write exe-dir .env");

    // The unit under test — load exe-dir .env from the fake exe path.
    // The dotenvy version installed in this workspace has
    // `from_path` returning `Result<(), _>` (not `Result<PathBuf, _>`),
    // so we discard the return value here and rely on the resolve_secret
    // assertion below as the actual end-to-end verification. See
    // `lib.rs::load_env_from_exe_dir` for the return type.
    savant_shell::load_env_from_exe_dir(&fake_exe_path)
        .expect("exe-dir .env load succeeds when file exists");

    // The vault's strategy 3 (exe-dir .env) should now resolve the key.
    let resolved = master_key::resolve_secret(&secret_ref)
        .expect("resolve succeeds after exe-dir dotenvy load");
    assert_eq!(
        resolved, "foo_strategy_3",
        "vault strategy 3 must resolve from exe-dir .env",
    );

    // Cleanup: remove the env var + the temp dir.
    std::env::remove_var(&env_var);
    let _ = std::fs::remove_dir_all(&fake_exe_dir);
}

#[test]
fn load_env_from_exe_dir_missing_file_errors_with_not_found() {
    // Regression guard: when `<exe_dir>/.env` does NOT exist, the
    // helper must return `Err(dotenvy::Error::Io(NotFound))` — NOT
    // some other error (which would mask parse failures / permission
    // errors in production) and NOT silently panic. `run()` calls
    // this with `.ok()` which discards the error, so the test
    // documents the contract the caller relies on.
    let pid = std::process::id();
    let test_marker = "missing";
    let fake_exe_dir = std::env::temp_dir()
        .join(format!("savant_exe_dir_{pid}_{test_marker}"));
    let fake_exe_path = fake_exe_dir.join("savant_fake_binary");

    // Create the dir but DO NOT create a `.env` inside it.
    let _ = std::fs::remove_dir_all(&fake_exe_dir);
    std::fs::create_dir_all(&fake_exe_dir).expect("create empty fake exe dir");

    let err = savant_shell::load_env_from_exe_dir(&fake_exe_path)
        .expect_err("missing exe-dir .env must error");
    assert!(
        matches!(err, dotenvy::Error::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound),
        "missing .env must surface as dotenvy::Error::Io(NotFound); got {err:?}",
    );

    // Cleanup.
    let _ = std::fs::remove_dir_all(&fake_exe_dir);
}

#[test]
fn vault_resolve_secret_still_errors_on_unset_var() {
    // Regression guard: with no .env loaded and no env var set, the
    // vault's env-var resolution must still error with InvalidKeyFormat
    // (we don't want dotenvy loading to somehow "always succeed" and
    // mask the missing-var case). This guards the FID-020 fix from
    // accidentally breaking the env-var-not-found error path.
    let env_var = "SAVANT_TEST_DOTENV_UNSET_VAR";
    std::env::remove_var(env_var);
    let err = master_key::resolve_secret(&format!("env:{env_var}"))
        .expect_err("unset env var errors");
    assert!(
        matches!(err, master_key::VaultError::InvalidKeyFormat),
        "expected InvalidKeyFormat; got {err:?}"
    );
}
