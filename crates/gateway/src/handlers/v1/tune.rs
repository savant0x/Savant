//! Tune endpoints — 3 endpoints (parameters, tuning, settings).
//!
//! Mapped to the Tauri IPC commands `get_parameter_descriptors`,
//! `get_tuning_descriptors`, `save_settings`. Stubs for the FID-031
//! impl pass.

use axum::{
    extract::State,
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_get_parameter_descriptors(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    // See the existing /api/models endpoint for the real impl
    Err(V1ApiError::NotImplemented("v1_get_parameter_descriptors".into()))
}

pub async fn v1_get_tuning_descriptors(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_get_tuning_descriptors".into()))
}

pub async fn v1_save_settings(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_save_settings".into()))
}
