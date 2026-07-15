//! Manifest endpoints — 3 endpoints (POST /v1/manifest/soul, POST /v1/manifest/swarm,
//! GET /v1/manifest/swarm/baseline).
//!
//! Mapped to the Tauri IPC commands `manifest_soul`, `bulk_manifest`,
//! `get_swarm_baseline`. Stubs for the FID-031 impl pass.

use axum::{
    extract::State,
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_manifest_soul(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented(
        "v1_manifest_soul (see /ws WebSocket MANIFEST_DRAFT for the streaming variant; SSE is at /v1/manifest/soul/stream)".into(),
    ))
}

pub async fn v1_bulk_manifest(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_bulk_manifest".into()))
}

pub async fn v1_get_swarm_baseline(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_get_swarm_baseline".into()))
}
