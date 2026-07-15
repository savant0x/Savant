//! Vault endpoints — 4 endpoints for vault profile management.
//!
//! Mapped to the Tauri IPC commands `setup_master_key`, `vault_list_profiles`,
//! `get_master_key_info`, `remove_master_key`. Stubs for the FID-031 impl
//! pass; the real impl wires into `crates/vault/`'s profile store.

use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::Value;
use std::sync::Arc;

use crate::handlers::v1::error::{V1ApiError, V1Result};
use crate::server::GatewayState;

pub async fn v1_setup_master_key(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented(
        "v1_setup_master_key (use savant vault CLI subcommand or web dashboard; full impl is FID-031 incremental)".into(),
    ))
}

pub async fn v1_list_profiles(
    _state: State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_list_profiles".into()))
}

pub async fn v1_get_master_key_info(
    _state: State<Arc<GatewayState>>,
    Path(_provider): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_get_master_key_info".into()))
}

pub async fn v1_remove_master_key(
    _state: State<Arc<GatewayState>>,
    Json(_body): Json<Value>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented("v1_remove_master_key".into()))
}

pub async fn v1_remove_master_key_by_path(
    _state: State<Arc<GatewayState>>,
    Path(_provider): Path<String>,
) -> V1Result<Json<Value>> {
    Err(V1ApiError::NotImplemented(
        "v1_remove_master_key_by_path (DELETE /v1/vault/profile/:provider)".into(),
    ))
}
