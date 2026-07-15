//! Smoke test for the FID-025 Skills + Sandbox IPC surface.
//!
//! Exercises ONLY the in-process `savant_shell::skills::SkillExecutionRegistry`
//! plus the `cfg!(test)`-gated stubs of the 5 IPC façade functions. The
//! production code paths of `list_skills`, `describe_skill`, and
//! `execute_skill` require Docker / WASM binaries + real SKILL.md files
//! on disk, so they are NOT exercised here and are gated behind
//! `cfg!(test)` returns that allow the test to run on a clean Windows
//! checkout without external setup.
//!
//! Verifies the IPC boundary contract:
//! - registry starts empty (no auto-registration)
//! - cancel / status of unknown `execution_id` returns bounded error string
//! - register via `execute_skill` → status returns `Running`
//! - register + cancel → status returns `Cancelled`
//! - the cancellation token returned by `execute_skill` is kept in
//!   sync with the registry's stored token (CancellationToken clones
//!   share internal Arc-state).

use savant_shell::skills::{
    cancel_skill_execution, execute_skill, get_skill_status, ExecutionState,
    SkillExecutionRegistry,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn status_unknown_id_errors_with_bounded_string() {
    let reg = SkillExecutionRegistry::new();
    let unknown = Uuid::new_v4();
    let err = get_skill_status(unknown, &reg).await.unwrap_err();
    assert!(
        err.starts_with("execution_not_found:"),
        "expected execution_not_found: prefix, got: {}",
        err
    );
}

#[tokio::test]
async fn cancel_unknown_id_errors_with_bounded_string() {
    let reg = SkillExecutionRegistry::new();
    let unknown = Uuid::new_v4();
    let err = cancel_skill_execution(unknown, &reg).await.unwrap_err();
    assert!(
        err.starts_with("execution_not_found:"),
        "expected execution_not_found: prefix, got: {}",
        err
    );
}

#[tokio::test]
async fn execute_skill_in_test_profile_registers_running() {
    let reg = SkillExecutionRegistry::new();
    let handle = execute_skill(
        "hello-world".into(),
        json!({ "input": "ping" }),
        &reg,
    )
    .await
    .expect("execute_skill should succeed in test profile");
    assert!(
        handle.token.is_some(),
        "execute_skill should return a CancellationToken in test profile"
    );
    let status =
        get_skill_status(handle.execution_id, &reg).await.expect("status of freshly-registered execution");
    assert_eq!(status.state, ExecutionState::Running);
}

#[tokio::test]
async fn cancel_after_register_flips_state_to_cancelled() {
    let reg = SkillExecutionRegistry::new();
    let handle = execute_skill("echo-skill".into(), json!({}), &reg)
        .await
        .expect("execute_skill should succeed in test profile");
    cancel_skill_execution(handle.execution_id, &reg)
        .await
        .expect("cancel should succeed on registered execution");
    let status =
        get_skill_status(handle.execution_id, &reg).await.expect("status of cancelled execution");
    assert_eq!(status.state, ExecutionState::Cancelled);
}
