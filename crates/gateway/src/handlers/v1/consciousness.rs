//! Consciousness endpoints — 5 endpoints for the consciousness daemon.
//!
//! Mapped to the Tauri IPC commands `initialize_app_state`,
//! `start_consciousness`, `stop_consciousness`, `get_consciousness_state`,
//! `trigger_reflection`. Stubs for the FID-031 impl pass.

use axum::{
    extract::State,
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_initialize_app_state(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_initialize_app_state".into()))
}

pub async fn v1_start_consciousness(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_start_consciousness".into()))
}

pub async fn v1_stop_consciousness(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_stop_consciousness".into()))
}

pub async fn v1_get_consciousness_state(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    // Real impl: read the in-memory AtomicU8 from GatewayState
    Err(V1ApiError::NotImplemented(
        "v1_get_consciousness_state (see /api/consciousness/status for the legacy impl)".into(),
    ))
}

pub async fn v1_trigger_reflection(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_trigger_reflection".into()))
}
