//! Skills endpoints — 5 endpoints (list, describe, execute, cancel, status).
//!
//! Mapped to the Tauri IPC commands `list_skills`, `describe_skill`,
//! `execute_skill`, `cancel_skill_execution`, `get_skill_status`.
//! Stubs for the FID-031 impl pass; the real impl wires into
//! `crates/skills/` + `crates/savant_skills`.

use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_list_skills(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_list_skills".into()))
}

pub async fn v1_describe_skill(
    _state: State<Arc<GatewayState>>,
    Path(_skill_id): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_describe_skill".into()))
}

pub async fn v1_execute_skill(
    _state: State<Arc<GatewayState>>,
    Path(_skill_id): Path<String>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_execute_skill".into()))
}

pub async fn v1_cancel_skill_execution(
    _state: State<Arc<GatewayState>>,
    Path(_execution_id): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_cancel_skill_execution".into()))
}

pub async fn v1_get_skill_status(
    _state: State<Arc<GatewayState>>,
    Path(_execution_id): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_get_skill_status".into()))
}
