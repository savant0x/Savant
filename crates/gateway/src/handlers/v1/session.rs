//! Session key endpoints — 2 endpoints (provision, clear).
//!
//! Mapped to the Tauri IPC commands `provision_session_key`,
//! `clear_session_key`. Stubs for the FID-031 impl pass.

use axum::{
    extract::State,
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_provision_session_key(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_provision_session_key".into()))
}

pub async fn v1_clear_session_key(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_clear_session_key".into()))
}
