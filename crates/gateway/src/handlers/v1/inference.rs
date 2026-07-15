//! Inference endpoint — 1 endpoint (POST /v1/inference/openrouter).
//!
//! Mapped to the Tauri IPC command `infer_openrouter`. Stub for the
//! FID-031 impl pass; the real impl proxies to the OpenRouter API.

use axum::{
    extract::State,
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_infer_openrouter(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_infer_openrouter".into()))
}
