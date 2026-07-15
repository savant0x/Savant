//! V1 API surface — the dashboard-facing REST + SSE endpoints.
//!
//! Mounted at `/v1/*` by `crates/gateway/src/server.rs`. The existing
//! `/api/*` endpoints stay for backward compatibility; the v1 surface is
//! the canonical Tauri-mapped (FID-031) + dashboard-`useCli` (FID-032)
//! surface.
//!
//! Pattern: each route group is its own file (vault.rs, consciousness.rs,
//! etc.). The handler functions take `State<Arc<GatewayState>>` and
//! return `V1Result<Json<Value>>` — the `IntoResponse` impl in error.rs
//! converts both the happy path (Json) and the error path
//! (V1ApiError → structured JSON error) into axum responses.

use axum::{
    routing::{delete, get, post, put},
    Json, Router,
};
use std::sync::Arc;

use crate::server::GatewayState;

pub mod changelog;
pub mod chat;
pub mod consciousness;
pub mod error;
pub mod faq;
pub mod health;
pub mod inference;
pub mod manifest;
pub mod session;
pub mod skills;
pub mod stream;
pub mod tune;
pub mod vault;

/// Build the v1 sub-router. Caller merges via `Router::new().merge(v1_routes())`
/// and applies state once at the top level (matches the existing
/// `ws_routes` / `api_routes` pattern in `crates/gateway/src/server.rs`).
pub fn v1_routes() -> Router<Arc<GatewayState>> {
    Router::new()
        // Health (real impl) + changelog + faq (real impls)
        .route("/health", get(health::v1_health_handler))
        .route("/changelog", get(changelog::v1_changelog_handler))
        .route("/faq", get(faq::v1_faq_handler))
        // Vault (4 endpoints)
        .route(
            "/vault/profile",
            post(vault::v1_setup_master_key)
                .delete(vault::v1_remove_master_key_by_path),
        )
        .route("/vault/profiles", get(vault::v1_list_profiles))
        .route("/vault/profile/:provider", get(vault::v1_get_master_key_info))
        // Consciousness (5 endpoints)
        .route(
            "/consciousness/initialize",
            post(consciousness::v1_initialize_app_state),
        )
        .route(
            "/consciousness/start",
            post(consciousness::v1_start_consciousness),
        )
        .route(
            "/consciousness/stop",
            post(consciousness::v1_stop_consciousness),
        )
        .route(
            "/consciousness/state",
            get(consciousness::v1_get_consciousness_state),
        )
        .route(
            "/consciousness/reflect",
            post(consciousness::v1_trigger_reflection),
        )
        // Inference (1 endpoint)
        .route("/inference/openrouter", post(inference::v1_infer_openrouter))
        // Manifest (3 endpoints + 1 SSE)
        .route("/manifest/soul", post(manifest::v1_manifest_soul))
        .route("/manifest/swarm", post(manifest::v1_bulk_manifest))
        .route(
            "/manifest/swarm/baseline",
            get(manifest::v1_get_swarm_baseline),
        )
        .route(
            "/manifest/soul/stream",
            get(stream::v1_manifest_soul_stream_sse),
        )
        // Session (2 endpoints)
        .route("/session/provision", post(session::v1_provision_session_key))
        .route("/session/clear", post(session::v1_clear_session_key))
        // Skills (5 endpoints)
        .route("/skills", get(skills::v1_list_skills))
        .route("/skills/:skill_id", get(skills::v1_describe_skill))
        .route(
            "/skills/:skill_id/execute",
            post(skills::v1_execute_skill),
        )
        .route(
            "/skills/executions/:execution_id/cancel",
            post(skills::v1_cancel_skill_execution),
        )
        .route(
            "/skills/executions/:execution_id",
            get(skills::v1_get_skill_status),
        )
        // Chat (6 endpoints — FID-029 chat persistence)
        .route("/chat/sessions", get(chat::v1_list_chat_sessions))
        .route(
            "/chat/sessions/:session_id/messages",
            get(chat::v1_load_chat_history).post(chat::v1_persist_chat_turn),
        )
        .route(
            "/chat/sessions/:session_id",
            delete(chat::v1_delete_chat_session),
        )
        .route("/chat/search", get(chat::v1_search_chat_history))
        .route(
            "/chat/sessions/:session_id/pin",
            put(chat::v1_toggle_chat_session_pin),
        )
        // Tune (3 endpoints)
        .route(
            "/tune/parameters",
            get(tune::v1_get_parameter_descriptors),
        )
        .route("/tune/tuning", get(tune::v1_get_tuning_descriptors))
        .route("/tune/settings", post(tune::v1_save_settings))
}

/// Mount the v1 sub-router into a parent router at `/v1`. Convenience
/// function for `server.rs` (keeps the route list in one place).
pub fn mount(parent: Router<Arc<GatewayState>>) -> Router<Arc<GatewayState>> {
    parent.nest("/v1", v1_routes())
}

// Re-export the error type so stub handlers in this module's submodules
// can use `crate::handlers::v1::error::V1ApiError` and `V1Result`.
pub use error::{V1ApiError, V1Result};

// Marker to ensure Json is referenced (it is used by the re-exports
// + the IntoResponse impl on V1Result).
#[allow(dead_code)]
fn _ensure_json_linked() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}
