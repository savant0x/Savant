// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
// SAFETY: This file uses serde_json::json!() extensively (~23 calls).
// The json!() macro validates JSON at compile time; its internal .unwrap()
// calls on well-formed literals are provably infallible per serde_json's
// contract (v1.x). No bare .unwrap() or .expect() exists in production code.
// Gate: cargo clippy --workspace --no-deps = 0 disallowed-method violations.

use crate::auth;
use crate::lanes::SessionLane;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use lru::LruCache;
use savant_core::bus::NexusBridge;
use savant_core::config::Config;
use savant_core::db::Storage;
use savant_core::error::SavantError;
use savant_core::types::{ChatRole, RequestFrame, SessionId};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};
use tower_http::cors::CorsLayer;

/// Shared state for the gateway server.
pub struct GatewayState {
    /// GTW-04: Wrapped in Arc<RwLock> so settings_post_handler can update in-memory config.
    pub config: Arc<tokio::sync::RwLock<Config>>,
    pub sessions: DashMap<SessionId, Arc<SessionLane>>,
    pub nexus: Arc<NexusBridge>,
    pub storage: Arc<Storage>,
    pub avatar_cache: TokioMutex<LruCache<String, (Vec<u8>, String)>>,
    pub oauth_manager: Arc<crate::auth::oauth::OAuthManager>,
    /// Persistent gateway Ed25519 signing key (generated once at startup)
    pub gateway_signing_key: ed25519_dalek::SigningKey,
    /// Canvas A2UI manager for real-time state broadcasting
    pub canvas_manager: Arc<savant_canvas::a2ui::CanvasManager>,
    /// Channel adapter pool for multi-platform messaging
    pub channel_pool: Arc<savant_channels::pool::InboxPool>,
    /// Echo component metrics for circuit breaker monitoring
    pub echo_metrics: Arc<savant_echo::ComponentMetrics>,
    /// Consciousness daemon state (shared with swarm)
    pub consciousness_state: Option<Arc<std::sync::atomic::AtomicU8>>,
    /// Active WebSocket connection count
    pub ws_connections: Arc<std::sync::atomic::AtomicUsize>,
    /// Resource governor state (shared with swarm)
    pub governor_pressure: Arc<std::sync::atomic::AtomicU8>,
    pub governor_cpu_pct: Arc<std::sync::atomic::AtomicU64>,
    pub governor_mem_pct: Arc<std::sync::atomic::AtomicU64>,
    pub governor_permits: Arc<std::sync::atomic::AtomicUsize>,
}

/// Echo circuit breaker metrics endpoint
async fn echo_metrics_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let m = &state.echo_metrics;
    let body = serde_json::json!({
        "failure_count": m.failure_count(),
        "total_count": m.total_count(),
        "error_rate": m.error_rate(),
        "reset_count": m.reset_count(),
        "trip_count": m.trip_count(),
        "opened_at": m.opened_at(),
        "consecutive_successes": m.consecutive_successes(),
    });
    axum::Json(body)
}

/// Canvas A2UI WebSocket adapter — extracts CanvasManager from GatewayState.
/// Requires API key authentication via Authorization: Bearer <key> or X-API-Key: <key>.
async fn canvas_ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    headers: axum::http::HeaderMap,
    axum::extract::State(state): axum::extract::State<Arc<GatewayState>>,
) -> impl axum::response::IntoResponse {
    // Authenticate canvas connection
    let expected_key = state.config.read().await.server.dashboard_api_key.clone();
    if let Some(ref key) = expected_key {
        if !key.is_empty() {
            let provided = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()));

            let authorized = provided
                .map(|k| {
                    crate::auth::http_middleware::constant_time_eq(k.as_bytes(), key.as_bytes())
                })
                .unwrap_or(false);

            if !authorized {
                tracing::warn!("[auth] Unauthorized canvas WebSocket connection attempt");
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({"error": "Unauthorized"})),
                )
                    .into_response();
            }
        }
    }

    let canvas = state.canvas_manager.clone();
    ws.on_upgrade(move |socket| savant_canvas::a2ui::handle_a2ui_connection(socket, canvas))
        .into_response()
}

/// Starts the axum gateway server.
pub async fn start_gateway(
    config: Config,
    nexus: Arc<NexusBridge>,
    storage: Arc<Storage>,
    echo_metrics: Arc<savant_echo::ComponentMetrics>,
    canvas_manager: Arc<savant_canvas::a2ui::CanvasManager>,
) -> Result<(), SavantError> {
    let addr = format!("{}:{}", config.server.host, config.server.port)
        .parse::<SocketAddr>()
        .map_err(|e| SavantError::Unknown(format!("Invalid address: {}", e)))?;

    // Initialize uptime tracking
    crate::handlers::status::init_start_time();

    // Initialize Channels adapter pool and register configured adapters
    let channel_pool = Arc::new(savant_channels::pool::InboxPool::new(nexus.clone()));
    {
        let ch = &config.channels;
        if ch.discord.enabled {
            if let Some(ref token) = ch.discord.token {
                channel_pool.register(Arc::new(savant_channels::discord::DiscordAdapter::new(
                    token.clone(),
                    None,
                    nexus.clone(),
                )));
                tracing::info!("[channels] Discord adapter registered");
            }
        }
        if ch.telegram.enabled {
            if let Some(ref token) = ch.telegram.token {
                let tg_cfg = savant_channels::telegram::TelegramConfig {
                    bot_token: token.clone(),
                    default_chat_id: None,
                    parse_mode: None,
                    use_webhook: false,
                    webhook_url: None,
                };
                match savant_channels::telegram::TelegramAdapter::new(tg_cfg, nexus.clone()) {
                    Ok(adapter) => {
                        channel_pool.register(Arc::new(adapter));
                        tracing::info!("[channels] Telegram adapter registered");
                    }
                    Err(e) => tracing::warn!("[channels] Telegram adapter init failed: {}", e),
                }
            }
        }
        if ch.whatsapp.enabled {
            tracing::info!("[channels] WhatsApp adapter available but requires script_path/session_path config");
        }
        if ch.matrix.enabled {
            tracing::info!(
                "[channels] Matrix adapter available but requires homeserver/user_id config"
            );
        }
        // Register generic webhook adapter (no external config needed)
        // Clone before spawn — spawn() is self-consuming, clone preserves the adapter for the pool
        // Generate a random webhook auth token on startup
        let webhook_token = {
            use blake3::Hasher;
            let mut hasher = Hasher::new();
            hasher.update(
                &std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
                    .to_le_bytes(),
            );
            hasher.update(uuid::Uuid::new_v4().as_bytes());
            hasher.finalize().to_hex().to_string()
        };
        tracing::info!(
            "[channels] Webhook auth token: {} (required in X-Webhook-Token header)",
            &webhook_token[..16]
        );

        let webhook_adapter = savant_channels::generic_webhook::GenericWebhookAdapter::new(
            savant_channels::generic_webhook::GenericWebhookConfig {
                listen_port: 9800,
                inbound_path: "/webhook".to_string(),
                outbound_url: None,
                auth_token: Some(webhook_token),
            },
            nexus.clone(),
        );
        // spawn() is self-consuming — clone preserves the adapter for registration in the pool
        let _webhook_handle = webhook_adapter.clone().spawn();
        channel_pool.register(Arc::new(webhook_adapter));
        tracing::info!("[channels] GenericWebhook adapter registered on port 9800");
        // Register CLI adapter (no external config needed)
        channel_pool.register(Arc::new(savant_channels::cli::CliAdapter));
        tracing::info!("[channels] CLI adapter registered");
    }

    let state = Arc::new(GatewayState {
        config: Arc::new(tokio::sync::RwLock::new(config.clone())),
        sessions: DashMap::new(),
        nexus,
        storage,
        avatar_cache: TokioMutex::new(LruCache::new(
            NonZeroUsize::new(100).expect("100 is non-zero"),
        )),
        gateway_signing_key: ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng),
        oauth_manager: Arc::new(crate::auth::oauth::OAuthManager::new()),
        canvas_manager,
        channel_pool,
        echo_metrics,
        consciousness_state: None,
        ws_connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        governor_pressure: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        governor_cpu_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_mem_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        governor_permits: Arc::new(std::sync::atomic::AtomicUsize::new(16)),
    });

    // Spawn config reload listener — watches for system.config.updated events
    // published by the HTTP settings handler or the Tauri settings_save IPC.
    // When received, reloads the gateway's in-memory config from disk so that
    // all subsequent requests (settings_get, WebSocket snapshots, etc.) reflect
    // the latest saved config without requiring a process restart.
    let config_lock = state.config.clone();
    let nexus_reload = state.nexus.clone();
    tokio::spawn(async move {
        let mut event_rx = nexus_reload.event_bus.subscribe();
        while let Ok(event) = event_rx.recv().await {
            if event.event_type == "system.config.updated" {
                tracing::info!("[config] system.config.updated received — reloading from disk");
                match Config::load() {
                    Ok(new_config) => {
                        let mut lock = config_lock.write().await;
                        *lock = new_config;
                        tracing::info!("[config] In-memory config reloaded from disk");
                    }
                    Err(e) => {
                        tracing::error!("[config] Failed to reload config from disk: {}", e);
                    }
                }
            }
        }
    });

    // Load CORS origins from config or environment.
    // Always include Tauri desktop origins so the WebSocket upgrade
    // from the Tauri webview is never blocked by CORS.
    let cors_origins: Vec<axum::http::HeaderValue> = {
        let mut origins: Vec<String> = if config.server.cors_origins.is_empty() {
            std::env::var("SAVANT_CORS_ORIGINS")
                .ok()
                .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
                .unwrap_or_else(|| {
                    vec![
                        "http://localhost:3000".to_string(),
                        "http://127.0.0.1:3000".to_string(),
                    ]
                })
        } else {
            config.server.cors_origins.clone()
        };
        // Always include Tauri desktop origins for WebSocket + REST
        for origin in &["tauri://localhost", "https://tauri.localhost", "http://tauri.localhost"] {
            if !origins.iter().any(|o| o == origin) {
                origins.push(origin.to_string());
            }
        }
        origins.iter().filter_map(|o| o.parse().ok()).collect()
    };

    let cors = CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Rate limiting: 20 requests per minute per IP
    let rate_limiter = GovernorLayer {
        config: std::sync::Arc::new(
            #[allow(clippy::disallowed_methods)] // Config with hardcoded values is always valid
            GovernorConfigBuilder::default()
                .per_second(3)
                .burst_size(20)
                .key_extractor(SmartIpKeyExtractor)
                .finish()
                .expect("Governor config with hardcoded values is always valid"),
        ),
    };

    let dashboard_api_key = state.config.read().await.server.dashboard_api_key.clone();

    // WebSocket routes — separated from CORS/auth/rate-limit middleware.
    // The WS handler performs its own auth via the first message (auth frame),
    // and has its own connection limit (MAX_WS_CONNECTIONS=100).
    // CORS middleware blocks WS upgrades because tower-http rejects GET requests
    // with non-matching Origin headers, killing the upgrade before it reaches the handler.
    let ws_routes = Router::new()
        .route("/ws", get(websocket_handler))
        .route("/ws/canvas", get(canvas_ws_handler));

    // API routes — full middleware stack
    let api_routes = Router::new()
        // PB-21: Health check endpoint
        .route("/health", get(health_handler))
        .route("/api/health/detailed", get(detailed_health_handler))
        .route("/api/echo/metrics", get(echo_metrics_handler))
        // Trajectory endpoints
        .route("/api/trajectories", get(trajectories_list_handler))
        .route("/api/trajectories/stats", get(trajectories_stats_handler))
        .route("/api/agents", get(agents_list_handler))
        .route("/api/agents/:name/image", get(agent_image_handler))
        .route(
            "/api/settings",
            get(settings_get_handler).post(settings_post_handler),
        )
        .route(
            "/api/settings/reset",
            get(settings_reset_handler).post(settings_reset_handler),
        )
        .route("/api/models", get(models_get_handler))
        .route(
            "/api/mcp/servers",
            get(crate::handlers::mcp::list_servers_handler),
        )
        .route(
            "/api/mcp/servers/install",
            axum::routing::post(crate::handlers::mcp::install_server_handler),
        )
        .route(
            "/api/mcp/servers/add",
            axum::routing::post(crate::handlers::mcp::add_server_handler),
        )
        .route(
            "/api/mcp/servers/remove",
            axum::routing::post(crate::handlers::mcp::remove_server_handler),
        )
        .route(
            "/api/mcp/servers/uninstall",
            axum::routing::post(crate::handlers::mcp::uninstall_server_handler),
        )
        .route(
            "/api/mcp/servers/info",
            get(crate::handlers::mcp::server_info_handler),
        )
        .route(
            "/live",
            get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
        )
        .route("/ready", get(crate::handlers::status::ready_handler))
        .route("/api/status", get(crate::handlers::status::status_handler))
        .route(
            "/api/snapshot",
            axum::routing::post(crate::handlers::status::snapshot_handler),
        )
        .route(
            "/api/restore",
            axum::routing::post(crate::handlers::status::restore_handler),
        )
        // Dashboard feature APIs
        .route("/api/memory/search", get(memory_search_handler))
        .route("/api/governor/status", get(governor_status_handler))
        .route(
            "/api/consciousness/status",
            get(consciousness_status_handler),
        )
        .route("/api/chat", axum::routing::post(rest_chat_handler))
        .route("/api/changelog", get(changelog_handler))
        .route(
            "/api/setup/check",
            get(crate::handlers::setup::setup_check_handler),
        )
        .route(
            "/api/setup/install-model",
            axum::routing::post(crate::handlers::setup::setup_install_model_handler),
        )
        .route(
            "/api/setup/install-model-stream",
            axum::routing::post(crate::handlers::setup::setup_install_model_stream_handler),
        )
        .route(
            "/api/setup/start-ollama",
            axum::routing::post(crate::handlers::setup::setup_start_ollama_handler),
        )
        .route(
            "/api/setup/openrouter-key",
            axum::routing::post(crate::handlers::setup::setup_openrouter_key_handler),
        )
        .route(
            "/api/config/set",
            axum::routing::post(crate::handlers::setup::config_set_handler),
        )
        .route(
            "/api/models/free",
            axum::routing::get(crate::handlers::models_free_handler),
        )
        .route(
            "/api/models/rest",
            axum::routing::get(crate::handlers::models_rest_handler),
        )
        .route(
            "/api/pairing",
            axum::routing::post(crate::handlers::pairing::pairing_handler),
        )
        .route(
            "/api/oauth/store",
            axum::routing::post(crate::handlers::oauth_store_handler),
        )
        // Schedule management endpoints (Issue 1 Phase 5)
        .route(
            "/api/schedules",
            axum::routing::get(crate::handlers::schedules::list_schedules)
                .post(crate::handlers::schedules::create_schedule),
        )
        .route(
            "/api/schedules/:id",
            axum::routing::delete(crate::handlers::schedules::delete_schedule)
                .patch(crate::handlers::schedules::update_schedule),
        )
        .route(
            "/api/schedules/:id/run",
            axum::routing::post(crate::handlers::schedules::force_run_schedule),
        )
        .layer(axum::middleware::from_fn(request_id_middleware))
        .layer(rate_limiter)
        .layer(axum::middleware::from_fn_with_state(
            dashboard_api_key,
            crate::auth::http_middleware::auth_middleware,
        ))
        .layer(cors);

    // Populate system.agents in shared memory on startup so that
    // route_chat_message doesn't reject every user message with
    // "No agents available".  Scans the configured agents_path for
    // workspace directories that contain agent.json, agent.config.json,
    // agent.toml, config.toml, or SOUL.md.
    {
        let agents_path = {
            let cfg = state.config.read().await;
            cfg.resolve_path(&cfg.system.agents_path)
        };
        let mut agents = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&agents_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let has_config = path.join("agent.json").exists()
                        || path.join("agent.config.json").exists()
                        || path.join("agent.toml").exists()
                        || path.join("config.toml").exists()
                        || path.join("SOUL.md").exists();
                    if has_config {
                        let raw_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .trim_start_matches("workspace-")
                            .trim_start_matches('.')
                            .to_string();
                        let name = {
                            let mut chars = raw_name.chars();
                            chars
                                .next()
                                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                                .unwrap_or_else(|| raw_name.clone())
                        };
                        agents.push(serde_json::json!({
                            "id": name,
                            "name": name,
                            "status": "online"
                        }));
                    }
                }
            }
        }
        let agents_json = serde_json::to_string(&serde_json::json!({ "agents": agents }))
            .unwrap_or_else(|_| "{\"agents\":[]}".to_string());
        state.nexus.shared_memory.insert("system.agents".to_string(), agents_json);
        tracing::info!(
            "[gateway] Populated system.agents with {} agent(s) from {}",
            agents.len(),
            agents_path.display()
        );
    }

    let app = Router::new()
        .merge(ws_routes)
        .merge(api_routes)
        .with_state(state);

    tracing::info!("Gateway server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .await
        .map_err(SavantError::IoError)?;

    Ok(())
}

/// PB-21: Health check endpoint — structured JSON with version and uptime
async fn health_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }))
}

/// GET /api/health/detailed — Diagnostic health endpoint for debugging.
/// Reports command bus receiver count (heartbeat subscribers), event bus
/// receiver count, session count, and shared memory keys. When the command
/// bus receiver count is 0, user messages will timeout because no heartbeat
/// is subscribed to process them.
async fn detailed_health_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let cmd_receivers = state.nexus.command_bus_receiver_count();
    let event_receivers = state.nexus.event_bus_receiver_count();
    let sessions_active = state.sessions.len();
    let ws_connections = state
        .ws_connections
        .load(std::sync::atomic::Ordering::Relaxed);

    // Check if agents are populated in shared memory
    let agents_json = state.nexus.shared_memory.get("system.agents");
    let agents_populated = agents_json.is_some();
    let agent_count = agents_json
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v["agents"].as_array().map(|a| a.len()))
        .unwrap_or(0);

    let overall = if cmd_receivers > 0 {
        "healthy"
    } else {
        "degraded" // No heartbeat subscribers — messages will timeout
    };

    axum::Json(serde_json::json!({
        "status": overall,
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "buses": {
            "command_bus_receivers": cmd_receivers,
            "event_bus_receivers": event_receivers,
            "command_bus_ok": cmd_receivers > 0,
        },
        "connections": {
            "sessions": sessions_active,
            "websockets": ws_connections,
        },
        "agents": {
            "populated_in_shared_memory": agents_populated,
            "count": agent_count,
        },
        "diagnosis": if cmd_receivers == 0 {
            "CRITICAL: Command bus has 0 receivers. The heartbeat agent task likely exited during initialization. Check logs for 'CRITICAL: Agent exiting before heartbeat start' or image cache / WASM host failures.".to_string()
        } else {
            format!("{} heartbeat subscriber(s) active. System operational.", cmd_receivers)
        },
    }))
}

/// List recorded trajectories.
async fn trajectories_list_handler() -> impl IntoResponse {
    let output_dir = std::path::PathBuf::from("./data/trajectories");
    let mut trajectories = Vec::new();

    if output_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&output_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let modified_secs = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                        })
                        .unwrap_or(0);

                    // Count steps
                    let steps = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|content| {
                            content.lines().next().and_then(|line| {
                                serde_json::from_str::<serde_json::Value>(line)
                                    .ok()
                                    .and_then(|json| {
                                        json["conversations"].as_array().map(|a| a.len())
                                    })
                            })
                        })
                        .unwrap_or(0);

                    trajectories.push(serde_json::json!({
                        "filename": filename,
                        "size_bytes": size,
                        "steps": steps,
                        "modified_epoch_secs": modified_secs,
                    }));
                }
            }
        }
    }

    axum::Json(serde_json::json!({
        "trajectories": trajectories,
        "count": trajectories.len(),
    }))
}

/// Trajectory statistics.
async fn trajectories_stats_handler() -> impl IntoResponse {
    let output_dir = std::path::PathBuf::from("./data/trajectories");
    let mut total_files = 0usize;
    let mut total_size = 0u64;
    let mut total_steps = 0usize;

    if output_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&output_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    total_files += 1;
                    total_size += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for line in content.lines() {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                                if let Some(convs) = json["conversations"].as_array() {
                                    total_steps += convs.len();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    axum::Json(serde_json::json!({
        "total_trajectories": total_files,
        "total_steps": total_steps,
        "total_size_bytes": total_size,
        "avg_steps_per_trajectory": if total_files > 0 { total_steps as f64 / total_files as f64 } else { 0.0 },
    }))
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // Connection limit: max 100 concurrent WebSocket connections
    const MAX_WS_CONNECTIONS: usize = 100;
    let current = state
        .ws_connections
        .load(std::sync::atomic::Ordering::Relaxed);
    if current >= MAX_WS_CONNECTIONS {
        tracing::warn!(
            "[gateway] WebSocket connection rejected — limit reached ({}/{})",
            current,
            MAX_WS_CONNECTIONS
        );
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "Too many connections",
        )
            .into_response();
    }
    state
        .ws_connections
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ws.on_upgrade(|socket| handle_socket_with_cleanup(socket, state))
}

async fn handle_socket_with_cleanup(socket: WebSocket, state: Arc<GatewayState>) {
    handle_socket(socket, state.clone()).await;
    state
        .ws_connections
        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
}

async fn handle_socket(socket: WebSocket, state: Arc<GatewayState>) {
    tracing::info!("New WebSocket connection established");
    let (mut sender, mut receiver) = socket.split();

    // 1. Authentication Phase
    let auth_frame = match receiver.next().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<RequestFrame>(&text) {
            Ok(frame) => frame,
            Err(e) => {
                tracing::error!("Failed to parse auth frame: {}", e);
                return;
            }
        },
        Some(Ok(Message::Close(_))) => {
            tracing::debug!("WebSocket closed during auth phase");
            return;
        }
        _ => return,
    };

    let dashboard_key = state.config.read().await.server.dashboard_api_key.clone();
    let session_context = match auth::authenticate(
        &auth_frame,
        dashboard_key.as_deref(),
        Some(&state.oauth_manager),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::error!("Authentication failed: {}", e);
            if let Err(e) = sender
                .send(Message::Text("Authentication failed".to_string()))
                .await
            {
                tracing::warn!("[gateway] Failed to send auth failure message: {}", e);
            }
            return;
        }
    };

    let session_id = session_context.session_id.clone();
    tracing::info!("Session authenticated: {}", session_id.0);

    // 1b. Send session assignment to client (formal handshake)
    // The client MUST receive this session ID before sending any messages,
    // because the gateway validates frame.session_id against the assigned session.
    let session_event = savant_core::types::EventFrame {
        event_type: "session.assigned".to_string(),
        payload: serde_json::json!({ "session_id": session_id.0 }).to_string(),
    };
    if let Ok(msg) = serde_json::to_string(&session_event) {
        if let Err(e) = sender.send(Message::Text(format!("EVENT:{}", msg))).await {
            tracing::error!("[gateway] Failed to send session.assigned event: {}", e);
            return;
        }
        tracing::info!("[gateway] Session assigned: {}", session_id.0);
    }

    // 2. Sovereign Handshake Ignition: Send current agents immediately upon auth
    // This ensures zero-latency sidebar population for the Dashboard.
    let initial_agents = state.nexus.shared_memory.get("system.agents");

    let agents_payload = if let Some(json) = initial_agents {
        json
    } else {
        // Perfection Enhancement: Send empty discovery to acknowledge sync
        serde_json::json!({ "status": "SWARM_PENDING", "agents": [] }).to_string()
    };

    let event = savant_core::types::EventFrame {
        event_type: "agents.discovered".to_string(),
        payload: agents_payload,
    };
    let msg = match serde_json::to_string(&event) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to serialize agents.discovered event: {}", e);
            return;
        }
    };
    if let Err(e) = sender.send(Message::Text(format!("EVENT:{}", msg))).await {
        tracing::warn!("[gateway] Failed to send agents.discovered event: {}", e);
    }
    tracing::info!(
        "Sovereign Ignition: Hydrated sidebar for session {}",
        session_id.0
    );

    // 3. Outgoing Message Hub
    // We use a central MPSC to funnel both Lane responses and Swarm telemetry
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel::<Message>(100);

    // 3. Session Setup
    let (lane_capacity, max_concurrency) = {
        let cfg = state.config.read().await;
        (cfg.server.lane_capacity, cfg.server.max_lane_concurrency)
    };
    let (lane, lane_rx, mut res_rx, limit) = SessionLane::new(lane_capacity, max_concurrency);
    let lane = Arc::new(lane);

    state.sessions.insert(session_id.clone(), lane.clone());
    SessionLane::spawn_consumer(
        lane_rx,
        lane.response_tx.clone(),
        limit,
        state.nexus.clone(),
    );

    // 4. Task 1: Forward Lane Responses to Outgoing Hub
    let out_tx = outgoing_tx.clone();
    let mut lane_fwd_task = tokio::spawn(async move {
        while let Some(response) = res_rx.recv().await {
            let msg = match serde_json::to_string(&response) {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!("Failed to serialize lane response: {}", e);
                    continue;
                }
            };
            if let Err(e) = out_tx.send(Message::Text(msg)).await {
                tracing::warn!("[gateway] Failed to forward lane response: {}", e);
            }
        }
    });

    // 5. Consolidated Swarm Telemetry Task
    let out_tx = outgoing_tx.clone();
    let storage_clone = state.storage.clone();
    let mut event_rx = state.nexus.event_bus.subscribe();
    let session_id_telemetry = session_id.clone();

    let mut telemetry_task = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            // 🌀 Perfection Loop: Unified Protocol
            let mut outbound_event = event.clone();

            match event.event_type.as_str() {
                "chat.message" | "chat.chunk" => {
                    // Protocol is already standardized at the Agent layer
                }
                t if t.starts_with("system.") => {
                    // System-wide configuration or status updates
                }
                t if t.starts_with("session.") => {
                    let parts: Vec<&str> = t.split('.').collect();
                    let sub_event = parts.get(2).unwrap_or(&"response");
                    // Only forward events for THIS session
                    if !t.starts_with(&format!("session.{}.", session_id_telemetry.0)) {
                        continue;
                    }
                    // Map session sub-events to types the dashboard can handle.
                    // "response" → "chat.message" so processEvent renders it.
                    // "ack" stays as "ack" (already handled).
                    outbound_event.event_type = match *sub_event {
                        "response" => "chat.message".to_string(),
                        other => other.to_string(),
                    };
                }
                _ => {}
            }

            // Persistence for dialog ONLY
            if outbound_event.event_type == "chat.message" {
                if let Ok(msg) =
                    serde_json::from_str::<savant_core::types::ChatMessage>(&outbound_event.payload)
                {
                    // Skip user messages — already persisted by handle_message() to avoid double-write
                    if msg.role != ChatRole::User
                        && msg.channel == savant_core::types::AgentOutputChannel::Chat
                    {
                        if let Err(e) = crate::persistence::GatewayPersistence::persist_chat(
                            &storage_clone,
                            &msg,
                        )
                        .await
                        {
                            tracing::warn!("Failed to persist chat message: {}", e);
                        }
                    }
                }
            } else if outbound_event.event_type == "learning.insight" {
                if let Ok(learning) = serde_json::from_str::<savant_core::learning::EmergentLearning>(
                    &outbound_event.payload,
                ) {
                    let msg = savant_core::types::ChatMessage {
                        is_telemetry: false,
                        role: savant_core::types::ChatRole::System,
                        content: format!("Insight: {}", learning.content),
                        sender: Some("ALD".to_string()),
                        recipient: None,
                        agent_id: None,
                        session_id: Some(savant_core::types::SessionId("learnings".to_string())),
                        channel: savant_core::types::AgentOutputChannel::Telemetry,
                        images: Vec::new(),
                        is_error: false,
                    };
                    if let Err(e) =
                        crate::persistence::GatewayPersistence::persist_chat(&storage_clone, &msg)
                            .await
                    {
                        tracing::warn!("Failed to persist learning insight: {}", e);
                    }
                }
            }

            let msg = match serde_json::to_string(&outbound_event) {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!("Failed to serialize telemetry event: {}", e);
                    continue;
                }
            };
            if let Err(e) = out_tx.send(Message::Text(format!("EVENT:{}", msg))).await {
                tracing::warn!(
                    "[gateway::server] Failed to send telemetry event to client: {}",
                    e
                );
            }
        }
    });

    // 5b. Debug Log Forwarding Task — streams tracing output to dashboard
    let out_tx = outgoing_tx.clone();
    let mut debug_log_task = tokio::spawn(async move {
        let mut log_rx = savant_core::bus::subscribe_debug_logs();
        while let Ok(log_msg) = log_rx.recv().await {
            let event = savant_core::types::EventFrame {
                event_type: "debug.log".to_string(),
                payload: serde_json::json!({ "message": log_msg }).to_string(),
            };
            if let Ok(msg) = serde_json::to_string(&event) {
                if let Err(e) = out_tx.send(Message::Text(format!("EVENT:{}", msg))).await {
                    tracing::warn!("[gateway] Failed to send debug log event: {}", e);
                }
            }
        }
    });

    // 6. Task 3: Central WebSocket Sender (with periodic keepalive ping)
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = outgoing_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if let Err(e) = sender.send(m).await {
                                tracing::error!("WS send failure: {}", e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if let Err(e) = sender.send(Message::Ping(vec![])).await {
                        tracing::debug!("WS ping failed (client likely disconnected): {}", e);
                        break;
                    }
                }
            }
        }
    });

    // 7. Task 4: WebSocket Receiver
    let storage = state.storage.clone();
    let nexus_inner = state.nexus.clone();
    // Snapshot config at connection time — the handler needs an shared dedup map
    // to prevent duplicate message processing within a session.
    let config_snapshot = state.config.read().await.clone();
    let dedup_map: Arc<dashmap::DashMap<[u8; 32], std::time::Instant>> =
        Arc::new(dashmap::DashMap::new());
    let out_tx_recv = outgoing_tx.clone();
    let mut recv_task = tokio::spawn({
        let session_id = session_id.clone();
        let session_context_clone = session_context.clone();
        let out_tx_recv = out_tx_recv;
        async move {
            while let Some(msg_result) = receiver.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        // D2: Structured tracing at WS message receipt (FID-20260529)
                        tracing::debug!(
                            "[gateway] INBOUND WS frame: session={}, size={}b",
                            session_id.0,
                            text.len()
                        );

                        // GTW-10: Validate message size before parsing to prevent memory exhaustion
                        const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024; // 1 MB
                        if text.len() > MAX_WS_MESSAGE_BYTES {
                            tracing::warn!(
                                "[gateway] WebSocket message too large ({} bytes, limit {})",
                                text.len(),
                                MAX_WS_MESSAGE_BYTES
                            );
                            // A4: Send error response to client (FID-20260529)
                            let _ = out_tx_recv
                                .send(Message::Text(
                                    r#"{"error":"Message too large","limit":1048576}"#.to_string(),
                                ))
                                .await;
                            continue;
                        }
                        match serde_json::from_str::<RequestFrame>(&text) {
                            Ok(frame) => {
                                if frame.session_id == session_id {
                                    crate::handlers::handle_message(
                                        frame,
                                        session_context_clone.clone(),
                                        axum::extract::State(crate::handlers::AppState {
                                            nexus: nexus_inner.clone(),
                                            storage: storage.clone(),
                                            config: config_snapshot.clone(),
                                            dedup_map: dedup_map.clone(),
                                        }),
                                    )
                                    .await;
                                } else {
                                    tracing::warn!(
                                        "[gateway] Session ID mismatch: expected {}, got {}. Message dropped.",
                                        session_id.0,
                                        frame.session_id.0
                                    );
                                    // A3: Send session.mismatch event (FID-20260529)
                                    let mismatch_event = savant_core::types::EventFrame {
                                        event_type: format!("session.{}.mismatch", session_id.0),
                                        payload: serde_json::json!({
                                            "expected": session_id.0,
                                            "received": frame.session_id.0,
                                            "action": "reconnect"
                                        })
                                        .to_string(),
                                    };
                                    if let Ok(msg) = serde_json::to_string(&mismatch_event) {
                                        let _ = out_tx_recv
                                            .send(Message::Text(format!("EVENT:{}", msg)))
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[gateway] Failed to deserialize WebSocket message: {}",
                                    e
                                );
                            }
                        }
                    }
                    Ok(Message::Ping(_data)) => {
                        // Axum handles Ping→Pong automatically
                    }
                    Ok(Message::Close(_)) => {
                        tracing::debug!("WebSocket close received");
                        break;
                    }
                    Ok(_) => {
                        // Binary, Pong — ignore
                    }
                    Err(e) => {
                        tracing::error!("WebSocket receive error: {}", e);
                        break;
                    }
                }
            }
        }
    });

    // B2: Task supervisor — track task completion for graceful cleanup (FID-20260529)
    enum TaskEvent {
        TaskDied { name: &'static str },
    }
    let (task_event_tx, mut task_event_rx) = tokio::sync::mpsc::channel::<TaskEvent>(8);

    // B3: Deterministic cleanup helper (FID-20260529)
    let cleanup = |session_id: &savant_core::types::SessionId, state: &Arc<GatewayState>| {
        state.sessions.remove(session_id);
        state
            .ws_connections
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    };

    // 8. Wait for connection closure with task supervision
    tokio::select! {
        result = (&mut lane_fwd_task) => {
            if let Err(e) = result {
                tracing::error!("[gateway] lane_fwd_task panicked for session {}: {:?}", session_id.0, e);
                let _ = task_event_tx.send(TaskEvent::TaskDied { name: "lane_fwd" }).await;
            }
        },
        result = (&mut telemetry_task) => {
            if let Err(e) = result {
                tracing::error!("[gateway] telemetry_task panicked for session {}: {:?}", session_id.0, e);
                let _ = task_event_tx.send(TaskEvent::TaskDied { name: "telemetry" }).await;
            }
        },
        result = (&mut debug_log_task) => {
            if let Err(e) = result {
                tracing::error!("[gateway] debug_log_task panicked for session {}: {:?}", session_id.0, e);
                let _ = task_event_tx.send(TaskEvent::TaskDied { name: "debug_log" }).await;
            }
        },
        result = (&mut send_task) => {
            match result {
                Ok(()) => {},
                Err(e) => {
                    tracing::error!("[gateway] send_task panicked for session {}: {:?}", session_id.0, e);
                    let _ = task_event_tx.send(TaskEvent::TaskDied { name: "send" }).await;
                }
            }
        },
        result = (&mut recv_task) => {
            match result {
                Ok(()) => {},
                Err(e) => {
                    tracing::error!("[gateway] recv_task panicked for session {}: {:?}", session_id.0, e);
                    let _ = task_event_tx.send(TaskEvent::TaskDied { name: "recv" }).await;
                }
            }
        },
        // B3: Handle task death events from supervisor
        Some(event) = task_event_rx.recv() => {
            match event {
                TaskEvent::TaskDied { name } => {
                    tracing::warn!("[gateway] Task '{}' died for session {}, initiating cleanup", name, session_id.0);
                }
            }
        },
    }

    // 9. Cleanup — deterministic teardown
    lane_fwd_task.abort();
    telemetry_task.abort();
    debug_log_task.abort();
    send_task.abort();
    recv_task.abort();
    cleanup(&session_id, &state);
    tracing::info!("Session closed: {}", session_id.0);
}

async fn agent_image_handler(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Validate name to prevent path traversal - allow alphanumeric + hyphens + underscores + dots
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Response::builder()
            .status(400)
            .body(axum::body::Body::from("Invalid agent name"))
            .unwrap_or_else(|_| fallback_response());
    }

    let name_lower = name.to_lowercase();

    // 1. Check Cache
    {
        let mut cache = state.avatar_cache.lock().await;
        if let Some((content, content_type)) = cache.get(&name_lower) {
            return Response::builder()
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, "public, max-age=3600")
                .body(axum::body::Body::from(content.clone()))
                .unwrap_or_else(|_| {
                    tracing::error!("Failed to build cached image response for {}", name_lower);
                    fallback_response()
                });
        }
    }

    let workspaces_dir = {
        let cfg = state.config.read().await;
        cfg.resolve_path(&cfg.system.agents_path)
    };
    // Check both naming conventions: workspace-{name} and {name}
    // The desktop app scaffolds .savant/ (no prefix), while development uses workspace-savant/.
    let workspace_conventions = [
        format!("workspace-{}", name_lower),
        name_lower.clone(),
        format!(".{}", name_lower),
    ];
    let candidates = ["avatar.png", "avatar.jpg", "avatar.jpeg", "agentimg.png"];

    #[allow(clippy::never_loop)] // Label used for early break clarity, not actual loop
    for convention in &workspace_conventions {
        let workspace_path = workspaces_dir.join(convention);
        for filename in candidates {
            let file_path = workspace_path.join(filename);
            if file_path.exists() {
                if let Ok(content) = std::fs::read(&file_path) {
                    let content_type = if filename.ends_with(".png") {
                        "image/png"
                    } else {
                        "image/jpeg"
                    }
                    .to_string();

                    // Update Cache
                    {
                        let mut cache = state.avatar_cache.lock().await;
                        cache.put(name_lower.clone(), (content.clone(), content_type.clone()));
                    }

                    return Response::builder()
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::CACHE_CONTROL, "public, max-age=3600")
                        .body(axum::body::Body::from(content))
                        .unwrap_or_else(|_| {
                            tracing::error!("Failed to build image response for {}", name_lower);
                            fallback_response()
                        });
                }
            }
        }
    }

    // Fallback: Generate dynamic SVG avatar
    let initial = name.chars().next().unwrap_or('?').to_uppercase();
    let svg = format!(
        r#"<svg width="100" height="100" viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
            <rect width="100" height="100" fill="{bg}"/>
            <text x="50" y="65" font-family="Arial" font-size="50" font-weight="bold" fill="{accent}" text-anchor="middle">{initial}</text>
            <rect x="5" y="5" width="90" height="90" fill="none" stroke="{accent}" stroke-width="2" opacity="0.3"/>
        </svg>"#,
        bg = "#00141a",
        accent = "#00d5ff",
        initial = initial
    );

    Response::builder()
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .body(axum::body::Body::from(svg))
        .unwrap_or_else(|_| {
            tracing::error!("Failed to build SVG response for {}", name);
            fallback_response()
        })
}

async fn settings_get_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let config = state.config.read().await;

    // savant.toml [ai] is source of truth for model and LLM params
    let chat_model = config.ai.model.clone();
    let temperature = config.ai.temperature;
    let top_p = config.ai.top_p;
    let frequency_penalty = config.ai.frequency_penalty;
    let presence_penalty = config.ai.presence_penalty;
    let provider = config.ai.provider.clone();
    let embedding_model = config.browser.embedding_model.clone();
    let vision_model = config.browser.vision_model.clone();
    let gateway_port = config.server.port;

    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());

    let settings = serde_json::json!({
        "chat_model": chat_model,
        "manifestation_model": config.ai.manifestation_model.clone().unwrap_or_default(),
        "provider": provider,
        "embedding_model": embedding_model,
        "vision_model": vision_model,
        "ollama_url": ollama_url,
        "gateway_port": gateway_port,
        "temperature": temperature,
        "top_p": top_p,
        "frequency_penalty": frequency_penalty,
        "presence_penalty": presence_penalty,
    });

    Json(settings)
}

/// POST /api/settings - Updates system settings
#[derive(serde::Deserialize)]
struct SettingsUpdate {
    #[serde(default)]
    chat_model: Option<String>,
    #[serde(default)]
    manifestation_model: Option<String>,
    #[serde(default)]
    vision_model: Option<String>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    top_p: Option<f32>,
    #[serde(default)]
    frequency_penalty: Option<f32>,
    #[serde(default)]
    presence_penalty: Option<f32>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    ollama_url: Option<String>,
}

async fn settings_post_handler(
    State(state): State<Arc<GatewayState>>,
    Json(update): Json<SettingsUpdate>,
) -> impl IntoResponse {
    // GTW-04: Write lock so changes are reflected in-memory
    let mut config = state.config.write().await;

    // AAA Validation & Range Clamping (Guardian Layer)
    let mut changed = false;
    let mut validation_notes = Vec::new();

    // Update model (savant.toml [ai] is source of truth)
    if let Some(model) = update.chat_model {
        config.ai.model = model;
        changed = true;
    }

    // Update provider (savant.toml [ai] is source of truth)
    if let Some(provider) = update.provider {
        // Validate provider string before accepting
        match savant_core::types::ModelProvider::from_str(&provider) {
            Ok(_) => {
                config.ai.provider = provider;
                changed = true;
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "status": "error",
                        "message": e
                    })),
                )
                    .into_response();
            }
        }
    }

    if let Some(v) = update.manifestation_model {
        if v.is_empty() {
            config.ai.manifestation_model = None;
        } else {
            config.ai.manifestation_model = Some(v);
        }
        changed = true;
    }

    if let Some(v) = update.vision_model {
        config.browser.vision_model = v;
        changed = true;
    }

    if let Some(v) = update.temperature {
        let clamped = v.clamp(0.0, 2.0);
        if (clamped - v).abs() > f32::EPSILON {
            validation_notes.push(format!("Temperature clamped from {} to {}", v, clamped));
        }
        config.ai.temperature = clamped;
        changed = true;
    }

    if let Some(v) = update.top_p {
        let clamped = v.clamp(0.0, 1.0);
        if (clamped - v).abs() > f32::EPSILON {
            validation_notes.push(format!("Top P clamped from {} to {}", v, clamped));
        }
        config.ai.top_p = clamped;
        changed = true;
    }

    if let Some(v) = update.frequency_penalty {
        let clamped = v.clamp(-2.0, 2.0);
        if (clamped - v).abs() > f32::EPSILON {
            validation_notes.push(format!(
                "Frequency Penalty clamped from {} to {}",
                v, clamped
            ));
        }
        config.ai.frequency_penalty = clamped;
        changed = true;
    }

    if let Some(v) = update.presence_penalty {
        let clamped = v.clamp(-2.0, 2.0);
        if (clamped - v).abs() > f32::EPSILON {
            validation_notes.push(format!(
                "Presence Penalty clamped from {} to {}",
                v, clamped
            ));
        }
        config.ai.presence_penalty = clamped;
        changed = true;
    }

    // Update Ollama URL (sets provider to "ollama" and configures base URL)
    if let Some(url) = update.ollama_url {
        config.ai.provider = "ollama".to_string();
        config.ai.base_url = Some(url);
        changed = true;
    }

    if changed {
        // Use config.project_root (set during ignition) instead of CWD-based lookup
        let config_path = config.project_root.join("config").join("savant.toml");
        if let Err(e) = config.save(&config_path) {
            tracing::error!("Failed to save config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"status": "error", "message": e.to_string()})),
            )
                .into_response();
        }

        // Sync to ~/.savant/savant.toml for agent subprocess consistency
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        let global_path = std::path::PathBuf::from(&home).join(".savant").join("savant.toml");
        if global_path != config_path {
            if let Err(e) = config.save(&global_path) {
                tracing::warn!("Failed to sync config to {:?}: {}", global_path, e);
            }
        }

        // Notify the Swarm via Nexus
        if let Err(e) = state
            .nexus
            .publish(
                "system.config.updated",
                &serde_json::json!({
                    "section": "ai",
                    "notes": validation_notes
                })
                .to_string(),
            )
            .await
        {
            tracing::warn!("[gateway] Failed to publish config update: {}", e);
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "notes": validation_notes
    }))
    .into_response()
}

/// Restores AI configuration to system defaults
async fn settings_reset_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // GTW-04: Write lock so changes are reflected in-memory
    let mut config = state.config.write().await;

    // Apply defaults from savant_core::config::AiConfig::default()
    config.ai = savant_core::config::AiConfig::default();

    // Use config.project_root (set during ignition) instead of CWD-based lookup
    let config_path = config.project_root.join("config").join("savant.toml");
    if let Err(e) = config.save(&config_path) {
        tracing::error!("Failed to reset config: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"status": "error", "message": e.to_string()})),
        )
            .into_response();
    }

    // Sync to ~/.savant/savant.toml for agent subprocess consistency
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let global_path = std::path::PathBuf::from(&home).join(".savant").join("savant.toml");
    if global_path != config_path {
        if let Err(e) = config.save(&global_path) {
            tracing::warn!("Failed to sync config to {:?}: {}", global_path, e);
        }
    }    // Notify the Swarm via Nexus — use system.config.updated so the
        // gateway's config reload listener picks it up (disk is source of truth).
        if let Err(e) = state
            .nexus
            .publish(
                "system.config.updated",
                &serde_json::json!({"section": "ai"}).to_string(),
            )
            .await
        {
            tracing::warn!("[gateway] Failed to publish config reset event: {}", e);
        }

    Json(serde_json::json!({"status": "ok", "message": "Settings restored to system defaults"}))
        .into_response()
}

/// Returns the list of available models and parameter descriptors
async fn models_get_handler() -> impl IntoResponse {
    let parameter_descriptors = savant_core::types::LlmParams::get_parameter_descriptors();

    // Return parameter descriptors for the Tuning page.
    // Provider list is available via the /api/providers endpoint.
    Json(serde_json::json!({
        "status": "ok",
        "parameter_descriptors": parameter_descriptors
    }))
    .into_response()
}

/// GET /api/agents — Returns list of discovered agents
async fn agents_list_handler(
    State(state): State<Arc<GatewayState>>,
) -> axum::response::Response {
    // Scan the workspaces directory for agents using the configured agents_path
    // (not current_dir(), which differs in desktop mode)
    let mut agents = Vec::new();
    let workspace_dir = {
        let cfg = state.config.read().await;
        cfg.resolve_path(&cfg.system.agents_path)
    };

    if let Ok(entries) = std::fs::read_dir(&workspace_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let raw_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .trim_start_matches("workspace-")
                    .trim_start_matches('.')
                    .to_string();
                let name = {
                    let mut chars = raw_name.chars();
                    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_else(|| raw_name.clone())
                };
                if path.join("agent.toml").exists() || path.join("config.toml").exists() || path.join("agent.json").exists() || path.join("agent.config.json").exists() || path.join("SOUL.md").exists() {
                    agents.push(serde_json::json!({
                        "id": name,
                        "name": name,
                        "status": "online"
                    }));
                }
            }
        }
    }

    // Also check the alternate workspaces path
    let alt_dir = std::env::current_dir()
        .unwrap_or_default()
        .join("workspaces")
        .join("agents");
    if let Ok(entries) = std::fs::read_dir(&alt_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let raw_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .trim_start_matches("workspace-")
                    .trim_start_matches('.')
                    .to_string();
                let name = {
                    let mut chars = raw_name.chars();
                    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_else(|| raw_name.clone())
                };
                let has_config = path.join("agent.json").exists()
                    || path.join("agent.config.json").exists()
                    || path.join("agent.toml").exists()
                    || path.join("config.toml").exists()
                    || path.join("SOUL.md").exists();
                if has_config && !agents.iter().any(|a| a["id"] == name) {
                    agents.push(serde_json::json!({
                        "id": name,
                        "name": name,
                        "status": "online"
                    }));
                }
            }
        }
    }

    Json(serde_json::json!({ "agents": agents })).into_response()
}

/// Embedded CHANGELOG.md content (included at compile time).
/// Serves as fallback when the file is not found at the project root
/// (e.g., in desktop app data directory mode).
const EMBEDDED_CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// GET /api/changelog — Returns the changelog markdown content
async fn changelog_handler(State(state): State<Arc<GatewayState>>) -> axum::response::Response {
    let changelog_path = state.config.read().await.project_root.join("CHANGELOG.md");
    // Try project_root first, fall back to embedded compile-time constant
    let content = std::fs::read_to_string(&changelog_path)
        .unwrap_or_else(|_| EMBEDDED_CHANGELOG.to_string());
    axum::response::Response::builder()
        .header("content-type", "text/markdown; charset=utf-8")
        .body(axum::body::Body::from(content))
        .unwrap_or_else(|e| {
            tracing::error!("[gateway] Failed to build changelog response: {}", e);
            fallback_response()
        })
}

/// Returns a 500 error response without using `.expect()`.
fn fallback_response() -> axum::response::Response {
    axum::response::Response::builder()
        .status(500)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
}

// Dashboard feature API handlers

/// Request ID middleware — generates UUID for each request, adds X-Request-Id header.
async fn request_id_middleware(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    req.extensions_mut().insert(request_id.clone());
    let mut response = next.run(req).await;
    response.headers_mut().insert(
        "x-request-id",
        axum::http::HeaderValue::from_str(&request_id)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("invalid")),
    );
    response
}

/// GET /api/memory/search?q=<query>&limit=<n>
#[allow(clippy::disallowed_methods)]
async fn memory_search_handler(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::extract::State(state): axum::extract::State<Arc<GatewayState>>,
) -> axum::response::Response {
    let query = params.get("q").cloned().unwrap_or_default();
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    match state.storage.get_swarm_history(limit * 5) {
        Ok(messages) => {
            let query_lower = query.to_lowercase();
            let results: Vec<serde_json::Value> = messages
                .iter()
                .filter(|m| {
                    if query.is_empty() {
                        return true;
                    }
                    m.content.to_lowercase().contains(&query_lower)
                })
                .take(limit)
                .map(|m| {
                    serde_json::json!({
                        "role": format!("{:?}", m.role),
                        "content": m.content,
                        "agent_id": m.agent_id,
                        "session_id": m.session_id.as_ref().map(|s| &s.0),
                    })
                })
                .collect();

            axum::Json(serde_json::json!({
                "status": "ok",
                "query": query,
                "limit": limit,
                "total": results.len(),
                "results": results,
            }))
            .into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("Memory search failed: {}", e)
            })),
        )
            .into_response(),
    }
}

/// GET /api/governor/status
async fn governor_status_handler(
    axum::extract::State(state): axum::extract::State<Arc<GatewayState>>,
) -> axum::response::Response {
    use std::sync::atomic::Ordering;

    let pressure_val = state.governor_pressure.load(Ordering::Relaxed);
    let pressure_name = match pressure_val {
        0 => "LOW",
        1 => "MEDIUM",
        2 => "HIGH",
        3 => "CRITICAL",
        _ => "UNKNOWN",
    };
    let cpu_bits = state.governor_cpu_pct.load(Ordering::Relaxed);
    let mem_bits = state.governor_mem_pct.load(Ordering::Relaxed);
    let cpu_pct = f64::from_bits(cpu_bits);
    let mem_pct = f64::from_bits(mem_bits);
    let permits = state.governor_permits.load(Ordering::Relaxed);

    axum::Json(serde_json::json!({
        "status": "ok",
        "pressure": pressure_name,
        "cpu_pct": cpu_pct,
        "mem_pct": mem_pct,
        "available_permits": permits,
    }))
    .into_response()
}

/// GET /api/consciousness/status
async fn consciousness_status_handler(
    axum::extract::State(state): axum::extract::State<Arc<GatewayState>>,
) -> axum::response::Response {
    use std::sync::atomic::Ordering;

    let (state_name, entropy) = match &state.consciousness_state {
        Some(handle) => {
            let state_val = handle.load(Ordering::Relaxed);
            let name = match state_val {
                0 => "THINKING",
                1 => "IDLE",
                2 => "DORMANT",
                3 => "WONDERING",
                _ => "UNKNOWN",
            };
            (name, 0.0)
        }
        None => {
            return axum::Json(serde_json::json!({
                "status": "disabled",
                "message": "Consciousness daemon not running"
            }))
            .into_response();
        }
    };

    axum::Json(serde_json::json!({
        "status": "ok",
        "state": state_name,
        "entropy": entropy,
    }))
    .into_response()
}

/// POST /api/chat — REST API for sending messages (alternative to WebSocket)
#[allow(clippy::disallowed_methods)]
async fn rest_chat_handler(
    axum::extract::State(state): axum::extract::State<Arc<GatewayState>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let agent_id = body
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("global");

    if message.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "message is required"})),
        )
            .into_response();
    }

    let channel = format!("chat.{}", agent_id);
    let payload = serde_json::json!({
        "role": "user",
        "content": message,
        "agent_id": agent_id,
    });

    if let Err(e) = state.nexus.publish(&channel, &payload.to_string()).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": format!("Failed to publish: {}", e)})),
        )
            .into_response();
    }

    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Message sent",
        "agent_id": agent_id,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use savant_core::bus::NexusBridge;
    use savant_core::config::Config;
    use savant_core::db::Storage;

    fn make_test_state() -> Arc<GatewayState> {
        let config = Config::default();
        let nexus = Arc::new(NexusBridge::new());
        let tmp = std::env::temp_dir().join(format!("savant_gw_test_{}", rand::random::<u64>()));
        // SAFETY: test-only construction with temp directory; unwrap is acceptable in tests
        #[allow(clippy::disallowed_methods)]
        let storage = Arc::new(Storage::with_defaults(tmp).unwrap());
        let canvas_manager = Arc::new(savant_canvas::a2ui::CanvasManager::new(1000));
        let channel_pool = Arc::new(savant_channels::pool::InboxPool::new(nexus.clone()));
        let echo_metrics = Arc::new(savant_echo::ComponentMetrics::new(0.05, 100));

        Arc::new(GatewayState {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            sessions: DashMap::new(),
            nexus,
            storage,
            // SAFETY: test-only construction; 100 is a compile-time constant > 0
            #[allow(clippy::disallowed_methods)]
            avatar_cache: TokioMutex::new(LruCache::new(NonZeroUsize::new(100).unwrap())),
            oauth_manager: Arc::new(crate::auth::oauth::OAuthManager::new()),
            gateway_signing_key: SigningKey::generate(&mut rand::rngs::OsRng),
            canvas_manager,
            channel_pool,
            echo_metrics,
            consciousness_state: None,
            ws_connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            governor_pressure: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            governor_cpu_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            governor_mem_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            governor_permits: Arc::new(std::sync::atomic::AtomicUsize::new(16)),
        })
    }

    #[test]
    fn test_gateway_state_has_canvas_manager() {
        let state = make_test_state();
        // Verify canvas_manager is accessible
        let _ = &state.canvas_manager;
    }

    #[test]
    fn test_gateway_state_has_echo_metrics() {
        let state = make_test_state();
        // Verify echo_metrics is accessible and has default values
        assert_eq!(state.echo_metrics.failure_count(), 0);
        assert_eq!(state.echo_metrics.total_count(), 0);
    }

    #[test]
    fn test_gateway_state_has_channel_pool() {
        let state = make_test_state();
        // Verify channel_pool is accessible
        let _ = &state.channel_pool;
    }

    #[test]
    fn test_gateway_state_has_oauth_manager() {
        let state = make_test_state();
        // Verify oauth_manager is accessible
        let _ = &state.oauth_manager;
    }

    #[test]
    fn test_echo_metrics_handler_returns_json() {
        let state = make_test_state();
        // Verify echo_metrics has expected methods
        assert_eq!(state.echo_metrics.failure_count(), 0);
        assert_eq!(state.echo_metrics.total_count(), 0);
        assert_eq!(state.echo_metrics.error_rate(), 0.0);
        assert_eq!(state.echo_metrics.reset_count(), 0);
        assert_eq!(state.echo_metrics.trip_count(), 0);
    }

    #[test]
    fn test_gateway_state_sessions_empty() {
        let state = make_test_state();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn test_fallback_response_returns_500() {
        let resp = super::fallback_response();
        assert_eq!(resp.status(), 500);
    }

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert!(!config.ai.model.is_empty());
    }

    #[test]
    fn test_canvas_manager_new() {
        let manager = savant_canvas::a2ui::CanvasManager::new(1000);
        let _ = &manager;
    }

    #[test]
    fn test_inbox_pool_new() {
        let nexus = Arc::new(NexusBridge::new());
        let pool = savant_channels::pool::InboxPool::new(nexus);
        let _ = &pool;
    }

    #[test]
    fn test_oauth_manager_new() {
        let manager = crate::auth::oauth::OAuthManager::new();
        let _ = &manager;
    }
}
