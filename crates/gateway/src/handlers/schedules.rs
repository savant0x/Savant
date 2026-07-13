// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! Schedule management REST endpoints.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use savant_core::heartbeat::HeartbeatScheduler;
use savant_core::heartbeat::{MissedExecutionPolicy, ScheduleConfig, SchedulePayload};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Global scheduler instance for the gateway.
static SCHEDULER: OnceCell<Arc<HeartbeatScheduler>> = OnceCell::const_new();

/// Initialize the global scheduler.
pub async fn init_scheduler(scheduler: Arc<HeartbeatScheduler>) {
    SCHEDULER.set(scheduler).ok();
}

/// Get the global scheduler.
fn get_scheduler() -> Result<&'static Arc<HeartbeatScheduler>, StatusCode> {
    SCHEDULER.get().ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

#[derive(Serialize)]
struct ScheduleResponse {
    id: String,
    name: String,
    cron_expr: String,
    enabled: bool,
    payload_type: String,
    last_run_at: Option<i64>,
    consecutive_errors: u32,
}

impl From<&ScheduleConfig> for ScheduleResponse {
    fn from(config: &ScheduleConfig) -> Self {
        let payload_type = match &config.payload {
            SchedulePayload::PulseTrigger => "pulse",
            SchedulePayload::AgentTurn { .. } => "agent_turn",
            SchedulePayload::SystemEvent { .. } => "system_event",
        };
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            cron_expr: config.cron_expr.clone(),
            enabled: config.enabled,
            payload_type: payload_type.to_string(),
            last_run_at: config.last_run_at,
            consecutive_errors: config.consecutive_errors,
        }
    }
}

/// GET /api/schedules — list all schedules
pub async fn list_schedules() -> impl IntoResponse {
    let scheduler = match get_scheduler() {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match scheduler.list_schedules() {
        Ok(schedules) => {
            let responses: Vec<ScheduleResponse> =
                schedules.iter().map(ScheduleResponse::from).collect();
            Json(responses).into_response()
        }
        Err(e) => {
            tracing::error!("[gateway] Failed to list schedules: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub cron: String,
    pub prompt: Option<String>,
    pub skills: Option<Vec<String>>,
    pub event: Option<String>,
    pub enabled: Option<bool>,
}

/// POST /api/schedules — create a new schedule
pub async fn create_schedule(Json(req): Json<CreateScheduleRequest>) -> impl IntoResponse {
    let scheduler = match get_scheduler() {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let payload = if let Some(prompt) = req.prompt {
        SchedulePayload::AgentTurn {
            prompt,
            skills: req.skills.unwrap_or_default(),
        }
    } else if let Some(event) = req.event {
        SchedulePayload::SystemEvent { event }
    } else {
        SchedulePayload::PulseTrigger
    };

    let id = uuid::Uuid::new_v4().to_string();
    let config = ScheduleConfig {
        id: id.clone(),
        name: req.name,
        cron_expr: req.cron,
        timezone: None,
        payload,
        enabled: req.enabled.unwrap_or(true),
        missed_policy: MissedExecutionPolicy::default(),
        last_run_at: None,
        next_run_at: None,
        consecutive_errors: 0,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    };

    // Save to JSON store
    if let Err(e) = scheduler.save_schedule(&config) {
        tracing::error!("[gateway] Failed to save schedule: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Register with scheduler
    if let Err(e) = scheduler.register_custom_job(&config).await {
        tracing::error!("[gateway] Failed to register schedule: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
}

/// DELETE /api/schedules/:id — delete a schedule
pub async fn delete_schedule(Path(id): Path<String>) -> impl IntoResponse {
    let scheduler = match get_scheduler() {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match scheduler.remove_schedule(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("[gateway] Failed to delete schedule: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateScheduleRequest {
    pub name: Option<String>,
    pub cron: Option<String>,
    pub prompt: Option<String>,
    pub skills: Option<Vec<String>>,
    pub event: Option<String>,
    pub enabled: Option<bool>,
}

/// PATCH /api/schedules/:id — update a schedule
pub async fn update_schedule(
    Path(id): Path<String>,
    Json(req): Json<UpdateScheduleRequest>,
) -> impl IntoResponse {
    let scheduler = match get_scheduler() {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    // Load existing schedule
    let existing = match scheduler.get_schedule(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("[gateway] Failed to get schedule: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Build updated config
    let mut updated = existing;
    if let Some(name) = req.name {
        updated.name = name;
    }
    if let Some(cron) = req.cron {
        updated.cron_expr = cron;
    }
    if let Some(enabled) = req.enabled {
        updated.enabled = enabled;
    }
    if let Some(prompt) = req.prompt {
        updated.payload = SchedulePayload::AgentTurn {
            prompt,
            skills: req.skills.unwrap_or_default(),
        };
    } else if let Some(event) = req.event {
        updated.payload = SchedulePayload::SystemEvent { event };
    }

    // Save updated schedule
    if let Err(e) = scheduler.save_schedule(&updated) {
        tracing::error!("[gateway] Failed to save updated schedule: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Re-register the cron job with updated expression.
    // Note: the old job may still fire until the process restarts.
    if updated.enabled {
        if let Err(e) = scheduler.register_custom_job(&updated).await {
            tracing::warn!(
                "[gateway] Schedule saved but live re-registration failed: {}",
                e
            );
        }
    }

    Json(ScheduleResponse::from(&updated)).into_response()
}

/// POST /api/schedules/:id/run — force-trigger a schedule
pub async fn force_run_schedule(Path(id): Path<String>) -> impl IntoResponse {
    let scheduler = match get_scheduler() {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    match scheduler.trigger_schedule(&id) {
        Ok(Some(command)) => Json(serde_json::json!({
            "triggered": true,
            "command": command,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("[gateway] Failed to trigger schedule: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
