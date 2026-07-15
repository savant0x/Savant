//! GET /v1/changelog — returns CHANGELOG.md content as JSON.
//!
//! Real impl. Reads the file from `state.config.project_root` (the same
//! source the legacy `/api/changelog` uses). Returns `{content, version, source}`.

use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::handlers::v1::error::V1Result;
use crate::server::GatewayState;

/// Embedded fallback for offline / dev environments where CHANGELOG.md
/// isn't at the project root (e.g., desktop app data directory mode).
const EMBEDDED_CHANGELOG: &str = include_str!("../../../../../CHANGELOG.md");

pub async fn v1_changelog_handler(
    State(state): State<Arc<GatewayState>>,
) -> V1Result<Json<Value>> {
    let changelog_path = state.config.read().await.project_root.join("CHANGELOG.md");
    let content = std::fs::read_to_string(&changelog_path)
        .unwrap_or_else(|_| EMBEDDED_CHANGELOG.to_string());
    let version = env!("CARGO_PKG_VERSION").to_string();
    Ok(Json(json!({
        "content": content,
        "version": version,
        "source": "github_or_embedded",
    })))
}
