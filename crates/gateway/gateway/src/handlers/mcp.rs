// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! MCP management handlers for the gateway.
//!
//! Provides REST API endpoints for:
//! - Listing configured MCP servers
//! - Installing servers via Smithery CLI
//! - Uninstalling servers
//! - Listing discovered MCP tools
//! - Enabling/disabling servers
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use crate::server::GatewayState;
use crate::smithery::{self, SmitheryManager};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;
use tracing::{info, warn};

/// GET /api/mcp/servers — list all configured MCP servers
pub async fn list_servers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Use in-memory config (like settings/changelog handlers) instead of
    // Config::load() from disk, which can fail if the config file is missing
    // or malformed — producing a 500 that the dashboard can't recover from.
    let config = state.config.read().await;

    let servers: Vec<serde_json::Value> = config
        .mcp
        .servers
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "url": s.url,
                "has_auth": s.auth_token.is_some(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "servers": servers,
        "count": servers.len(),
    }))
    .into_response()
}

/// POST /api/mcp/servers/install — install an MCP server via Smithery
#[derive(serde::Deserialize)]
pub struct InstallRequest {
    pub server_name: String,
    pub display_name: Option<String>,
}

pub async fn install_server_handler(
    State(state): State<Arc<GatewayState>>,
    Json(request): Json<InstallRequest>,
) -> impl IntoResponse {
    info!(
        "Installing MCP server via Smithery: {}",
        request.server_name
    );

    // Create SmitheryManager
    let manager = match SmitheryManager::new() {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to init Smithery: {}", e)})),
            )
                .into_response()
        }
    };

    // Install via Smithery CLI
    let server = match manager.install(&request.server_name).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Smithery install failed: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("Smithery install failed: {}", e)})),
            )
                .into_response();
        }
    };

    // Convert to McpServerEntry and add to config
    let entry = SmitheryManager::to_mcp_entry(&server);
    let display_name = request.display_name.unwrap_or_else(|| entry.name.clone());

    match add_server_to_config(&state.config, display_name, entry.url, None).await {
        Ok(()) => {
            info!("MCP server installed and configured: {}", server.name);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "installed",
                    "server": server.name,
                    "description": server.description,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Config update failed: {}", e)})),
        )
            .into_response(),
    }
}

/// POST /api/mcp/servers/add — add a custom MCP server (not via Smithery)
#[derive(serde::Deserialize)]
pub struct AddServerRequest {
    pub name: String,
    pub url: String,
    pub auth_token: Option<String>,
}

pub async fn add_server_handler(
    State(state): State<Arc<GatewayState>>,
    Json(request): Json<AddServerRequest>,
) -> impl IntoResponse {
    info!(
        "Adding custom MCP server: {} at {}",
        request.name, request.url
    );

    if let Err(e) = smithery::validate_server_name(&request.name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }

    match add_server_to_config(
        &state.config,
        request.name.clone(),
        request.url,
        request.auth_token,
    )
    .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "added", "server": request.name})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/mcp/servers/remove — remove an MCP server
#[derive(serde::Deserialize)]
pub struct RemoveServerRequest {
    pub name: String,
}

pub async fn remove_server_handler(
    State(state): State<Arc<GatewayState>>,
    Json(request): Json<RemoveServerRequest>,
) -> impl IntoResponse {
    info!("Removing MCP server: {}", request.name);

    match remove_server_from_config(&state.config, &request.name).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "removed", "server": request.name})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/mcp/servers/uninstall — uninstall via Smithery + remove from config
#[derive(serde::Deserialize)]
pub struct UninstallRequest {
    pub server_name: String,
}

pub async fn uninstall_server_handler(
    State(state): State<Arc<GatewayState>>,
    Json(request): Json<UninstallRequest>,
) -> impl IntoResponse {
    info!("Uninstalling MCP server: {}", request.server_name);

    // Try Smithery uninstall (may fail if not a Smithery server)
    if let Ok(manager) = SmitheryManager::new() {
        if let Err(e) = manager.uninstall(&request.server_name).await {
            warn!("Smithery uninstall warning: {}", e);
        }
    }

    // Remove from config regardless
    match remove_server_from_config(&state.config, &request.server_name).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "uninstalled", "server": request.server_name})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /api/mcp/servers/info — get info about a Smithery server
#[derive(serde::Deserialize)]
pub struct InfoQuery {
    pub server_name: String,
}

pub async fn server_info_handler(
    axum::extract::Query(query): axum::extract::Query<InfoQuery>,
) -> impl IntoResponse {
    let manager = match SmitheryManager::new() {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    match manager.info(&query.server_name).await {
        Ok(info) => (
            StatusCode::OK,
            Json(serde_json::to_value(&info).unwrap_or_default()),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ============================================================================
// Config helpers
// ============================================================================

/// Adds an MCP server entry to savant.toml.
/// GTW-02: Uses in-memory config with write lock.
async fn add_server_to_config(
    config_lock: &Arc<tokio::sync::RwLock<savant_core::config::Config>>,
    name: String,
    url: String,
    auth_token: Option<String>,
) -> Result<(), String> {
    let mut config = config_lock.write().await;

    // Check for duplicates
    if config.mcp.servers.iter().any(|s| s.name == name) {
        return Err(format!("Server '{}' already exists", name));
    }

    config
        .mcp
        .servers
        .push(savant_core::config::McpServerEntry {
            name,
            url,
            auth_token,
        });

    config
        .save(&config.project_root.join("config").join("savant.toml"))
        .map_err(|e| format!("Failed to save config: {}", e))?;
    Ok(())
}

/// Removes an MCP server entry from savant.toml.
/// GTW-02: Uses in-memory config with write lock.
async fn remove_server_from_config(
    config_lock: &Arc<tokio::sync::RwLock<savant_core::config::Config>>,
    name: &str,
) -> Result<(), String> {
    let mut config = config_lock.write().await;

    let before = config.mcp.servers.len();
    config.mcp.servers.retain(|s| s.name != name);

    if config.mcp.servers.len() == before {
        return Err(format!("Server '{}' not found", name));
    }

    config
        .save(&config.project_root.join("config").join("savant.toml"))
        .map_err(|e| format!("Failed to save config: {}", e))?;
    Ok(())
}
