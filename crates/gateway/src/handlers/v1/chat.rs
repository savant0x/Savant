//! Chat persistence endpoints — 6 endpoints (FID-029 mapping).
//!
//! Mapped to the Tauri IPC commands `list_chat_sessions`,
//! `load_chat_history`, `persist_chat_turn`, `delete_chat_session`,
//! `search_chat_history`, `toggle_chat_session_pin`.
//! Stubs for the FID-031 impl pass; the real impl wires into
//! `crates/core/src/db/Storage::append_chat` + `get_history` etc.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_list_chat_sessions(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented(
        "v1_list_chat_sessions (FID-029 chat persistence — use savant memory list CLI subcommand or web dashboard)".into(),
    ))
}

pub async fn v1_load_chat_history(
    _state: State<Arc<GatewayState>>,
    Path(_session_id): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_load_chat_history".into()))
}

pub async fn v1_persist_chat_turn(
    _state: State<Arc<GatewayState>>,
    Path(_session_id): Path<String>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_persist_chat_turn".into()))
}

pub async fn v1_delete_chat_session(
    _state: State<Arc<GatewayState>>,
    Path(_session_id): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_delete_chat_session".into()))
}

pub async fn v1_search_chat_history(
    _state: State<Arc<GatewayState>>,
    Query(_params): Query<std::collections::HashMap<String, String>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_search_chat_history".into()))
}

pub async fn v1_toggle_chat_session_pin(
    _state: State<Arc<GatewayState>>,
    Path(_session_id): Path<String>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_toggle_chat_session_pin".into()))
}
