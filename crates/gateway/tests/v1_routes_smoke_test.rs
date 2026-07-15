//! Integration smoke tests for the `/v1/*` route surface (FID-031).
//!
//! Verifies the contract that the dashboard's `api-client.ts` (FID-032) will
//! consume: 3 real impls return 200 + 6 stubs return 501 NotImplemented.
//! Uses `tower::util::ServiceExt::oneshot` for in-process router tests
//! (no real network, no port binding, <100ms total).
//!
//! Routes are mounted at `/v1/*` via `.nest("/v1", v1_routes())` in
//! `crates/gateway/src/server.rs`. The smoke tests use a no-op auth
//! middleware layer so they don't need the dashboard_api_key.

use axum::{body::{to_bytes, Body}, http::Request};
use dashmap::DashMap;
use ed25519_dalek::SigningKey;
use lru::LruCache;
use savant_core::bus::NexusBridge;
use savant_core::config::Config;
use savant_core::db::Storage;
use savant_gateway::handlers::v1;
use savant_gateway::server::GatewayState;
use savant_gateway::auth::oauth::OAuthManager;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tower_0_5::util::ServiceExt;

/// Build a test `GatewayState` for in-process router tests.
fn make_test_state() -> Arc<GatewayState> {
    let config = Config::default();
    let nexus = Arc::new(NexusBridge::new());
    let tmp = std::env::temp_dir().join(format!("savant_v1_test_{}", rand::random::<u64>()));
    // SAFETY: test-only construction with temp directory
    #[allow(clippy::disallowed_methods)]
    let storage = Arc::new(Storage::with_defaults(tmp).unwrap());
    let canvas_manager = Arc::new(savant_canvas::a2ui::CanvasManager::new(1000));
    let channel_pool = Arc::new(savant_channels::pool::InboxPool::new(nexus.clone()));
    let echo_metrics = Arc::new(savant_echo::ComponentMetrics::new(0.05, 100));

    Arc::new(GatewayState {
        config: Arc::new(tokio::sync::RwLock::new(config)),
        sessions: DashMap::new(),
        nexus,
        storage,
        #[allow(clippy::disallowed_methods)]
        avatar_cache: TokioMutex::new(LruCache::new(NonZeroUsize::new(100).unwrap())),
        oauth_manager: Arc::new(OAuthManager::new()),
        gateway_signing_key: SigningKey::generate(&mut rand::rngs::OsRng),
        canvas_manager,
        channel_pool,
        echo_metrics,
        consciousness_state: None,
        ws_connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        governor_pressure: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        governor_cpu_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_mem_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_permits: Arc::new(std::sync::atomic::AtomicUsize::new(16)),
    })
}

/// No-op auth middleware for smoke tests. Skips the dashboard_api_key
/// check (which would require setting `server.dashboard_api_key` in
/// the test config) so the tests focus on the route contract.
async fn no_op_auth_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    next.run(req).await
}

/// Build the v1 router with state applied + no-op auth. Routes are mounted
/// at `/v1/*` (the canonical Tauri-mapped + dashboard-`useCli` surface
/// per FID-031). This mirrors the production wiring in server.rs but
/// replaces the auth middleware with a no-op for testability.
fn build_v1_router() -> axum::Router {
    let state = make_test_state();
    v1::v1_routes()
        .with_state(state)
        .layer(axum::middleware::from_fn(no_op_auth_middleware))
}

#[tokio::test]
async fn test_v1_health_returns_200_with_status_ok() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
    assert!(body["features"]["sse"].as_bool().unwrap_or(false));
    assert!(body["features"]["websocket"].as_bool().unwrap_or(false));
    // embedded_web is feature-gated; the value reflects the current build
    assert!(body["features"]["embedded_web"].is_boolean());
}

#[tokio::test]
async fn test_v1_changelog_returns_200_with_content() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/changelog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(body["content"].is_string());
    assert!(
        !body["content"].as_str().unwrap().is_empty(),
        "changelog content should be non-empty"
    );
    assert!(body["version"].is_string());
    assert!(body["source"].is_string());
}

#[tokio::test]
async fn test_v1_faq_returns_200_with_items() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/faq")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let items = body["items"].as_array().expect("items should be an array");
    assert!(!items.is_empty(), "FAQ should have at least 1 item");
    // Verify the required 4 fields on each item
    for item in items {
        assert!(item["id"].is_string());
        assert!(item["category"].is_string());
        assert!(item["question"].is_string());
        assert!(item["answer"].is_string());
    }
}

#[tokio::test]
async fn test_v1_vault_profiles_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/vault/profiles")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        501,
        "stub should return 501 NotImplemented"
    );
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["code"], "NOT_IMPLEMENTED");
    assert!(body["error"].is_string());
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn test_v1_skills_list_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/skills")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn test_v1_chat_sessions_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/chat/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
}

#[tokio::test]
async fn test_v1_tune_parameters_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/tune/parameters")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
}

#[tokio::test]
async fn test_v1_inference_openrouter_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/inference/openrouter")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
}

#[tokio::test]
async fn test_v1_consciousness_state_stub_returns_501() {
    let app = build_v1_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/consciousness/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
}
