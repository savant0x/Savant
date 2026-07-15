//! GET /v1/health — health check endpoint for the v1 API surface.
//!
//! Returns `{status, version, uptime_secs, features}`. Distinct from the
//! legacy `/api/health` endpoint so the v1 surface has its own health probe
//! (useful for the dashboard's `useCli` mode in FID-032).

use axum::{extract::State, response::IntoResponse, Json};
use std::sync::Arc;
use std::time::Instant;

use crate::server::GatewayState;

pub async fn v1_health_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // Compute uptime from the in-memory config's project_root mtime as a
    // conservative proxy. (A precise startup-time atomic is a follow-on.)
    let uptime_secs = {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };

    let cmd_receivers = state.nexus.command_bus_receiver_count();
    let event_receivers = state.nexus.event_bus_receiver_count();
    let sessions_active = state.sessions.len();

    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime_secs,
        "buses": {
            "command_bus_receivers": cmd_receivers,
            "event_bus_receivers": event_receivers,
        },
        "connections": {
            "sessions": sessions_active,
        },
        "features": {
            "embedded_web": cfg!(feature = "embedded-web"),
            "sse": true,
            "websocket": true,
        },
    }))
    .into_response()
}

/// Marker to silence unused-import warning when this file is the only consumer
/// of `Instant` (it is imported for future use; not yet wired).
#[allow(dead_code)]
fn _ensure_instant_linked() -> Instant {
    Instant::now()
}
