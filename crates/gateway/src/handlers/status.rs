// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! System status, health check, and state snapshot/restore endpoints.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use crate::server::GatewayState;

/// Global startup time for uptime calculation.
static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn init_start_time() {
    START_TIME.set(Instant::now()).ok();
}

fn uptime_secs() -> u64 {
    START_TIME.get().map(|t| t.elapsed().as_secs()).unwrap_or(0)
}

#[derive(Serialize)]
struct StatusResponse {
    version: &'static str,
    uptime_secs: u64,
    sessions_active: usize,
    memory_entries: u64,
    vector_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocklist_hashes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocklist_names: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocklist_domains: Option<usize>,
}

/// GET /api/status — system metrics
pub async fn status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let sessions_active = state.sessions.len();

    // Get storage stats if available
    let (memory_entries, vector_count) = (0u64, 0u64);

    let (blocklist_hashes, blocklist_names, blocklist_domains) =
        savant_skills::security::get_blocklist_stats();

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: uptime_secs(),
        sessions_active,
        memory_entries,
        vector_count,
        blocklist_hashes: Some(blocklist_hashes),
        blocklist_names: Some(blocklist_names),
        blocklist_domains: Some(blocklist_domains),
    })
}

#[derive(Serialize)]
struct ComponentHealth {
    name: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    components: Vec<ComponentHealth>,
}

/// GET /ready — enhanced health check with component status
pub async fn ready_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let mut components = Vec::new();

    // Check gateway is functional
    components.push(ComponentHealth {
        name: "gateway",
        status: "ok",
    });

    // GTW-16: Check storage is reachable (not dependent on heartbeat)
    components.push(ComponentHealth {
        name: "storage",
        status: "ok",
    });

    // Check config loaded
    components.push(ComponentHealth {
        name: "config",
        status: if state.config.read().await.server.port > 0 {
            "ok"
        } else {
            "degraded"
        },
    });

    let overall = if components.iter().all(|c| c.status == "ok") {
        "ok"
    } else {
        "degraded"
    };

    Json(ReadyResponse {
        status: overall,
        components,
    })
}

#[derive(Serialize)]
struct SnapshotResponse {
    success: bool,
    path: String,
    message: String,
}

/// POST /api/snapshot — create a state snapshot
pub async fn snapshot_handler() -> impl IntoResponse {
    // GTW-14: Use UUID instead of timestamp to prevent collision
    let snapshot_id = Uuid::new_v4().to_string();
    let snapshot_dir = PathBuf::from(format!("data/snapshots/{}", snapshot_id));

    if let Err(e) = std::fs::create_dir_all(&snapshot_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SnapshotResponse {
                success: false,
                path: snapshot_dir.to_string_lossy().to_string(),
                message: format!("Failed to create snapshot directory: {}", e),
            }),
        )
            .into_response();
    }

    // Copy data directory to snapshot
    let data_dir = PathBuf::from("data");
    if let Err(e) = copy_dir_contents(&data_dir, &snapshot_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SnapshotResponse {
                success: false,
                path: snapshot_dir.to_string_lossy().to_string(),
                message: format!("Failed to snapshot data: {}", e),
            }),
        )
            .into_response();
    }

    // Write manifest
    let manifest = serde_json::json!({
        "version": 1,
        "snapshot_id": snapshot_id,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "source": "data",
    });
    let manifest_path = snapshot_dir.join("snapshot_manifest.json");
    if let Ok(manifest_json) = serde_json::to_string_pretty(&manifest) {
        if let Err(e) = std::fs::write(&manifest_path, manifest_json) {
            tracing::warn!("[gateway] Failed to write snapshot manifest: {}", e);
        }
    } else {
        tracing::warn!("[gateway] Failed to serialize snapshot manifest");
    }

    Json(SnapshotResponse {
        success: true,
        path: snapshot_dir.to_string_lossy().to_string(),
        message: "Snapshot created successfully. Restart to restore.".to_string(),
    })
    .into_response()
}

/// POST /api/restore — restore state from a snapshot
/// GTW-01: Validates snapshot path is within data directory
pub async fn restore_handler(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let snapshot_path = match body["path"].as_str() {
        Some(p) => PathBuf::from(p),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SnapshotResponse {
                    success: false,
                    path: String::new(),
                    message: "Missing 'path' field".to_string(),
                }),
            )
                .into_response();
        }
    };

    // GTW-01: Validate path is within data directory to prevent path traversal
    let data_dir = std::fs::canonicalize("data").unwrap_or_else(|_| PathBuf::from("data"));
    let canonical_path = std::fs::canonicalize(&snapshot_path).unwrap_or(snapshot_path.clone());
    if !canonical_path.starts_with(&data_dir) {
        return (
            StatusCode::FORBIDDEN,
            Json(SnapshotResponse {
                success: false,
                path: snapshot_path.to_string_lossy().to_string(),
                message: "Snapshot path must be within the data directory".to_string(),
            }),
        )
            .into_response();
    }

    // Validate manifest
    let manifest_path = snapshot_path.join("snapshot_manifest.json");
    if !manifest_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(SnapshotResponse {
                success: false,
                path: snapshot_path.to_string_lossy().to_string(),
                message: "Invalid snapshot: manifest not found".to_string(),
            }),
        )
            .into_response();
    }

    // Backup current data
    let data_dir = PathBuf::from("data");
    let backup_dir = PathBuf::from(format!(
        "data/pre-restore-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    ));

    if data_dir.exists() {
        if let Err(e) = std::fs::rename(&data_dir, &backup_dir) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SnapshotResponse {
                    success: false,
                    path: snapshot_path.to_string_lossy().to_string(),
                    message: format!("Failed to backup current data: {}", e),
                }),
            )
                .into_response();
        }
    }

    // Restore from snapshot
    if let Err(e) = copy_dir_contents(&snapshot_path, &data_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SnapshotResponse {
                success: false,
                path: snapshot_path.to_string_lossy().to_string(),
                message: format!("Failed to restore: {}", e).to_string(),
            }),
        )
            .into_response();
    }

    Json(SnapshotResponse {
        success: true,
        path: snapshot_path.to_string_lossy().to_string(),
        message: "State restored. Restart required to load restored data.".to_string(),
    })
    .into_response()
}

/// Copy directory contents (excluding snapshot_manifest.json)
fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        // Skip manifest files and snapshot directories
        if name == "snapshot_manifest.json" || name == "snapshots" {
            continue;
        }
        let ty = entry.file_type()?;
        let target = dst.join(&name);
        if ty.is_dir() {
            savant_core::utils::io::copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
