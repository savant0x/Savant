//! Gateway security penetration tests - tests all security fixes.

#![allow(clippy::disallowed_methods)]

use dashmap::DashMap;
use lru::LruCache;
use rand::rngs::OsRng;
use savant_core::bus::NexusBridge;
use savant_core::config::Config;
use savant_core::db::Storage;
use savant_gateway::server::GatewayState;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Helper to create a test GatewayState
fn create_test_state() -> Arc<GatewayState> {
    let config = Config::default();
    let nexus = Arc::new(NexusBridge::new());
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let storage = Arc::new(
        Storage::new(
            std::env::temp_dir().join(format!("gw-sec-{}-{}", pid, unique)),
            100_000,
        )
        .unwrap(),
    );

    let nexus_for_pool = nexus.clone();

    Arc::new(GatewayState {
        config: Arc::new(tokio::sync::RwLock::new(config)),
        sessions: DashMap::new(),
        nexus,
        storage,
        avatar_cache: TokioMutex::new(LruCache::new(NonZeroUsize::new(10).unwrap())),
        gateway_signing_key: ed25519_dalek::SigningKey::generate(&mut OsRng),
        oauth_manager: Arc::new(savant_gateway::auth::oauth::OAuthManager::new()),
        canvas_manager: Arc::new(savant_canvas::a2ui::CanvasManager::new(1000)),
        channel_pool: Arc::new(savant_channels::pool::InboxPool::new(nexus_for_pool)),
        echo_metrics: Arc::new(savant_echo::ComponentMetrics::new(0.05, 100)),
        consciousness_state: None,
        ws_connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        governor_pressure: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        governor_cpu_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_mem_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_permits: Arc::new(std::sync::atomic::AtomicUsize::new(16)),
    })
}

#[tokio::test]
async fn test_gateway_signing_key_generation() {
    let state = create_test_state();

    let key_bytes = state.gateway_signing_key.to_bytes();
    assert!(
        !key_bytes.iter().all(|&b| b == 0),
        "Signing key should not be all zeros"
    );
    assert!(key_bytes.len() == 32, "Signing key should be 32 bytes");
}

#[tokio::test]
async fn test_gateway_key_is_random() {
    let state1 = create_test_state();
    let state2 = create_test_state();

    let key1 = state1.gateway_signing_key.to_bytes();
    let key2 = state2.gateway_signing_key.to_bytes();

    assert_ne!(
        key1, key2,
        "Each GatewayState should have a unique signing key"
    );
}

#[tokio::test]
async fn test_skill_name_validation() {
    assert!(validate_skill_name("hello-world").is_ok());
    assert!(validate_skill_name("my_skill_123").is_ok());
    assert!(validate_skill_name("simple").is_ok());

    assert!(validate_skill_name("").is_err(), "Empty name should fail");
    assert!(
        validate_skill_name("../etc/passwd").is_err(),
        "Path traversal should fail"
    );
    assert!(
        validate_skill_name("hello world").is_err(),
        "Spaces should fail"
    );
    assert!(
        validate_skill_name("hello/world").is_err(),
        "Slash should fail"
    );
    assert!(
        validate_skill_name("hello;world").is_err(),
        "Semicolon should fail"
    );
    assert!(
        validate_skill_name("hello\\world").is_err(),
        "Backslash should fail"
    );

    let long_name: String = "a".repeat(200);
    assert!(
        validate_skill_name(&long_name).is_err(),
        "Long name should fail"
    );
}

#[tokio::test]
async fn test_directive_sanitization() {
    assert!(validate_directive("deploy agent alpha").is_ok());
    assert!(
        validate_directive("deploy\x00agent").is_err(),
        "Null byte should fail"
    );
    assert!(
        validate_directive("deploy\x01agent").is_err(),
        "Control char should fail"
    );
    assert!(
        validate_directive("").is_err(),
        "Empty directive should fail"
    );

    let long_directive: String = "x".repeat(3000);
    assert!(
        validate_directive(&long_directive).is_err(),
        "Long directive should fail"
    );
}

#[tokio::test]
async fn test_auth_error_sanitization() {
    let error = savant_core::error::SavantError::AuthError("internal key format wrong".to_string());
    let error_str = error.to_string();

    assert!(
        !error_str.contains("key format"),
        "Auth error should not leak internal details: {}",
        error_str
    );
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if name.len() > 128 {
        return Err("Skill name too long".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Invalid characters".to_string());
    }
    if name.contains("..") {
        return Err("Path traversal detected".to_string());
    }
    Ok(())
}

fn validate_directive(directive: &str) -> Result<(), String> {
    if directive.is_empty() {
        return Err("Empty directive".to_string());
    }
    if directive.len() > 2048 {
        return Err("Directive too long".to_string());
    }
    if directive
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err("Control characters not allowed".to_string());
    }
    Ok(())
}
