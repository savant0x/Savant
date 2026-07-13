//! Integration tests for the delegation pipeline.
//!
//! Tests the full flow: route -> delegate -> execute -> complete -> score.

#![allow(clippy::disallowed_methods)]

use savant_agent::delegation::{DelegationEngine, DelegationHooks};
use savant_agent::governor::SwarmGovernor;
use savant_agent::subagent_registry::SubAgentRegistry;
use savant_core::config::ResourceGovernorConfig;
use savant_core::types::SubAgentProfile;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn test_governor() -> std::sync::Arc<SwarmGovernor> {
    SwarmGovernor::new(ResourceGovernorConfig::default(), CancellationToken::new())
}

#[tokio::test]
async fn test_full_delegation_pipeline() {
    let governor = test_governor();
    let registry = SubAgentRegistry::new();
    let engine = DelegationEngine::new(governor, registry, vec![]);

    // Load profile
    let coding = SubAgentProfile {
        name: "coding".to_string(),
        max_iterations: 3,
        ..Default::default()
    };
    engine.add_profile(coding).await;

    // Route a coding task
    let routed = engine
        .route("fix the cargo build error in main.rs", "")
        .await;
    assert_eq!(routed, "coding");

    // Delegate with no hooks
    let hooks = DelegationHooks {
        on_start: None,
        on_complete: None,
    };

    let handle = engine
        .delegate(
            "coding",
            "Fix the bug".to_string(),
            "Context here".to_string(),
            hooks,
        )
        .await
        .expect("Delegation should succeed");

    assert!(!handle.id.is_empty());
    assert_eq!(handle.profile_name, "coding");

    // Await result
    let result = tokio::time::timeout(Duration::from_secs(5), handle.result_receiver)
        .await
        .expect("Timeout waiting for result")
        .expect("Result channel closed");

    assert!(result.success);
    assert!(result.iterations_used > 0);
    assert!(result.iterations_used <= 3);
    assert!(result.output.contains("Fix the bug"));
}

#[tokio::test]
async fn test_delegation_with_rejecting_hook() {
    let governor = test_governor();
    let registry = SubAgentRegistry::new();
    let engine = DelegationEngine::new(governor, registry, vec![]);

    let general = SubAgentProfile {
        name: "general".to_string(),
        max_iterations: 1,
        ..Default::default()
    };
    engine.add_profile(general).await;

    // on_start hook that rejects
    let hooks = DelegationHooks {
        on_start: Some(Box::new(|_req| false)),
        on_complete: None,
    };

    let result = engine
        .delegate("general", "task".to_string(), "ctx".to_string(), hooks)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("rejected by on_start hook"),
        "Error was: {}",
        err
    );
}

#[tokio::test]
async fn test_delegation_result_caching() {
    let governor = test_governor();
    let registry = SubAgentRegistry::new();
    let engine = DelegationEngine::new(governor, registry, vec![]);

    let coding = SubAgentProfile {
        name: "coding".to_string(),
        max_iterations: 1,
        ..Default::default()
    };
    engine.add_profile(coding).await;

    let hooks = DelegationHooks {
        on_start: None,
        on_complete: None,
    };

    // First delegation
    let handle1 = engine
        .delegate("coding", "Same task".to_string(), "ctx".to_string(), hooks)
        .await
        .expect("First delegation should succeed");

    let _ = tokio::time::timeout(Duration::from_secs(5), handle1.result_receiver)
        .await
        .expect("Timeout")
        .expect("Channel closed");

    // Second delegation with same task should return cached result
    let hooks2 = DelegationHooks {
        on_start: None,
        on_complete: None,
    };

    let handle2 = engine
        .delegate("coding", "Same task".to_string(), "ctx".to_string(), hooks2)
        .await
        .expect("Second delegation should succeed (cached)");

    assert!(handle2.id.starts_with("cached-"));
}

#[tokio::test]
async fn test_delegation_nonexistent_profile() {
    let governor = test_governor();
    let registry = SubAgentRegistry::new();
    let engine = DelegationEngine::new(governor, registry, vec![]);

    let hooks = DelegationHooks {
        on_start: None,
        on_complete: None,
    };

    let result = engine
        .delegate("nonexistent", "task".to_string(), "ctx".to_string(), hooks)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("not found"), "Error was: {}", err);
}

#[tokio::test]
async fn test_route_fallback_to_general() {
    let governor = test_governor();
    let registry = SubAgentRegistry::new();
    let engine = DelegationEngine::new(governor, registry, vec![]);

    // No profiles loaded - should fall back to "general"
    let routed = engine.route("do something random", "").await;
    assert_eq!(routed, "general");
}
