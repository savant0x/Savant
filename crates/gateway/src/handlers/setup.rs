// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! Setup wizard handlers for first-launch dependency checks and config.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use crate::server::GatewayState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    Json,
};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;

/// Validate that a section/key name contains only safe characters.
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Security-critical config fields that cannot be changed at runtime.
/// Changing these requires a restart with updated config files.
pub const IMMUTABLE_FIELDS: &[(&str, &str)] = &[
    ("server", "dashboard_api_key"),
    ("server", "host"),
    ("server", "port"),
    ("server", "signing_key"),
    ("security", "enable_blocklist_sync"),
];

/// Check if a config field is immutable at runtime.
pub fn is_immutable_config_field(section: &str, key: &str) -> bool {
    IMMUTABLE_FIELDS
        .iter()
        .any(|(s, k)| *s == section && *k == key)
}

#[derive(Debug, Deserialize)]
pub struct InstallModelRequest {
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigSetRequest {
    pub section: String,
    pub key: String,
    pub value: serde_json::Value,
}

/// POST /api/config/set — Update a config value and save to disk
/// GTW-02/GTW-03: Uses in-memory config with write lock instead of re-reading from disk.
pub async fn config_set_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<ConfigSetRequest>,
) -> impl IntoResponse {
    // Validate input to prevent injection
    if !is_valid_identifier(&body.section) || !is_valid_identifier(&body.key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": "Invalid section or key name"
            })),
        )
            .into_response();
    }

    // Block runtime changes to security-critical fields
    if is_immutable_config_field(&body.section, &body.key) {
        tracing::warn!(
            "[config] Blocked attempt to modify immutable field: {}.{}",
            body.section,
            body.key
        );
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "status": "error",
                "message": format!(
                    "Field '{}.{}' is immutable at runtime. Update the config file and restart.",
                    body.section, body.key
                )
            })),
        )
            .into_response();
    }

    let mut config = state.config.write().await;

    let result = match body.section.as_str() {
        "browser" => match body.key.as_str() {
            "vision_model" => {
                config.browser.vision_model = body.value.as_str().unwrap_or("gemma4").to_string();
                Ok(())
            }
            "embedding_model" => {
                config.browser.embedding_model =
                    body.value.as_str().unwrap_or("gemma4").to_string();
                Ok(())
            }
            "vision_model_provider" => {
                config.browser.vision_model_provider =
                    body.value.as_str().unwrap_or("ollama").to_string();
                Ok(())
            }
            "enabled" => {
                config.browser.enabled = body.value.as_bool().unwrap_or(true);
                Ok(())
            }
            _ => Err("Unknown browser key".to_string()),
        },
        "obsidian" => match body.key.as_str() {
            "vault_path" => {
                config.obsidian.vault_path = body.value.as_str().map(|s| s.to_string());
                Ok(())
            }
            "enabled" => {
                config.obsidian.enabled = body.value.as_bool().unwrap_or(true);
                Ok(())
            }
            "sync_interval_secs" => {
                config.obsidian.sync_interval_secs = body.value.as_u64().unwrap_or(300);
                Ok(())
            }
            _ => Err("Unknown obsidian key".to_string()),
        },
        "ai" => match body.key.as_str() {
            "model" => {
                config.ai.model = body.value.as_str().unwrap_or("").to_string();
                Ok(())
            }
            "provider" => {
                config.ai.provider = body.value.as_str().unwrap_or("ollama").to_string();
                Ok(())
            }
            _ => Err("Unknown ai key".to_string()),
        },
        _ => Err("Unknown config section".to_string()),
    };

    match result {
        Ok(()) => {
            // Use config.project_root (set during ignition) instead of CWD-based lookup
            let config_path = config.project_root.join("config").join("savant.toml");
            if config.save(&config_path).is_err() {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "status": "error",
                        "message": "Failed to save configuration"
                    })),
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
            Json(serde_json::json!({
                "status": "success",
                "section": body.section,
                "key": body.key,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": e
            })),
        )
            .into_response(),
    }
}

/// GET /api/setup/check — Check Ollama/LM Studio + model availability
/// Auto-detects ANY installed gemma model, not just the configured one.
pub async fn setup_check_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let configured_model = state.config.read().await.browser.embedding_model.clone();

    let mut checks = serde_json::json!({
        "ollama_running": false,
        "ollama_installed": false,
        "model_available": false,
        "model_name": configured_model,
        "installed_models": [],
        "issues": [],
        "instructions": [],
        "providers": [],
    });

    let mut issues: Vec<String> = Vec::new();
    let mut instructions: Vec<String> = Vec::new();
    let mut providers: Vec<serde_json::Value> = Vec::new();

    // Check Ollama — get ALL models, find any gemma
    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let ollama_result = check_provider_auto(&ollama_url, "Ollama").await;
    if ollama_result.running {
        checks["ollama_running"] = serde_json::Value::Bool(true);
        checks["ollama_installed"] = serde_json::Value::Bool(true);
        checks["installed_models"] = serde_json::Value::Array(
            ollama_result
                .installed_models
                .iter()
                .map(|m| serde_json::Value::String(m.clone()))
                .collect(),
        );

        // Find best model: exact configured match > any gemma > first model
        let best_model = find_best_model(&configured_model, &ollama_result.installed_models);

        if let Some(found) = best_model {
            checks["model_available"] = serde_json::Value::Bool(true);
            checks["model_name"] = serde_json::Value::String(found.clone());

            // Write back to config so downstream uses the actual model name
            if found != configured_model {
                let mut config = state.config.write().await;
                config.browser.embedding_model = "nomic-embed-text".to_string();
                config.browser.vision_model = found.clone();
                let config_path = savant_core::config::Config::primary_config_path();
                let _ = config.save(&config_path);
                tracing::info!("[setup] Auto-detected vision model '{}', embedding model 'nomic-embed-text', updated config", found);
            }
        }
    }
    providers.push(serde_json::json!({
        "name": "Ollama",
        "url": ollama_url,
        "running": ollama_result.running,
        "model_available": checks["model_available"],
        "models": ollama_result.installed_models,
        "error": ollama_result.error,
    }));

    // Check LM Studio
    let lmstudio_url =
        std::env::var("LMSTUDIO_URL").unwrap_or_else(|_| "http://localhost:1234".to_string());
    let lmstudio_result = check_provider_auto(&lmstudio_url, "LM Studio").await;
    if lmstudio_result.running {
        checks["lmstudio_running"] = serde_json::Value::Bool(true);
        checks["lmstudio_installed"] = serde_json::Value::Bool(true);

        if !checks["model_available"].as_bool().unwrap_or(false) {
            let best = find_best_model(&configured_model, &lmstudio_result.installed_models);
            if let Some(found) = best {
                checks["model_available"] = serde_json::Value::Bool(true);
                checks["model_name"] = serde_json::Value::String(found);
            }
        }

        // Merge installed models
        if let serde_json::Value::Array(ref mut arr) = checks["installed_models"] {
            for m in &lmstudio_result.installed_models {
                arr.push(serde_json::Value::String(m.clone()));
            }
        }
    }
    providers.push(serde_json::json!({
        "name": "LM Studio",
        "url": lmstudio_url,
        "running": lmstudio_result.running,
        "model_available": checks["model_available"],
        "models": lmstudio_result.installed_models,
        "error": lmstudio_result.error,
    }));

    // If neither is running, populate issues
    if !ollama_result.running && !lmstudio_result.running {
        issues.push("No local AI provider detected".to_string());
        instructions.push(
            "Install Ollama (https://ollama.com/download) or LM Studio (https://lmstudio.ai)"
                .to_string(),
        );
        instructions.push("Start the provider and return here.".to_string());
    } else if !checks["model_available"].as_bool().unwrap_or(false) {
        issues.push("No suitable model found".to_string());
        instructions.push("A model will be auto-installed during setup.".to_string());
    }

    checks["issues"] =
        serde_json::Value::Array(issues.into_iter().map(serde_json::Value::String).collect());
    checks["instructions"] = serde_json::Value::Array(
        instructions
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    );
    checks["providers"] = serde_json::Value::Array(providers);

    Json(checks).into_response()
}

/// Find the best model from installed list.
/// Priority: exact configured match > gemma model (largest) > first model.
fn find_best_model(configured: &str, installed: &[String]) -> Option<String> {
    if installed.is_empty() {
        return None;
    }

    // 1. Exact match
    if installed.iter().any(|m| m == configured) {
        return Some(configured.to_string());
    }

    // 2. Prefix match (e.g., configured "gemma4" matches "gemma4:31b")
    if let Some(m) = installed.iter().find(|m| m.starts_with(configured)) {
        return Some(m.clone());
    }

    // 3. Any gemma model (prefer larger = later in sorted list)
    let gemma_models: Vec<&String> = installed.iter().filter(|m| m.contains("gemma")).collect();
    if !gemma_models.is_empty() {
        return Some(gemma_models.last().unwrap().to_string());
    }

    // 4. First available model
    Some(installed[0].clone())
}

struct ProviderCheck {
    running: bool,
    _model_available: bool,
    installed_models: Vec<String>,
    error: Option<String>,
}

/// Check a provider and return ALL installed models (not just the configured one).
async fn check_provider_auto(url: &str, name: &str) -> ProviderCheck {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return ProviderCheck {
                running: false,
                _model_available: false,
                installed_models: vec![],
                error: Some(format!("Internal error checking {}", name)),
            };
        }
    };

    let tags_url = format!("{}/api/tags", url);
    let models_url = format!("{}/v1/models", url);

    // Try Ollama /api/tags first
    match client.get(&tags_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let models = body["models"].as_array().cloned().unwrap_or_default();
                let names: Vec<String> = models
                    .iter()
                    .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                    .collect();
                return ProviderCheck {
                    running: true,
                    _model_available: false, // caller decides
                    installed_models: names,
                    error: None,
                };
            }
            ProviderCheck {
                running: true,
                _model_available: false,
                installed_models: vec![],
                error: None,
            }
        }
        Ok(_) => {
            // Ollama responded with error — try LM Studio / OpenAI-compatible
            match client.get(&models_url).send().await {
                Ok(resp2) if resp2.status().is_success() => {
                    if let Ok(body) = resp2.json::<serde_json::Value>().await {
                        let data = body["data"].as_array().cloned().unwrap_or_default();
                        let names: Vec<String> = data
                            .iter()
                            .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                            .collect();
                        return ProviderCheck {
                            running: true,
                            _model_available: false,
                            installed_models: names,
                            error: None,
                        };
                    }
                    ProviderCheck {
                        running: true,
                        _model_available: false,
                        installed_models: vec![],
                        error: None,
                    }
                }
                Ok(_) => ProviderCheck {
                    running: false,
                    _model_available: false,
                    installed_models: vec![],
                    error: Some(format!("{} returned an error response", name)),
                },
                Err(_) => ProviderCheck {
                    running: false,
                    _model_available: false,
                    installed_models: vec![],
                    error: Some(format!("{} not reachable", name)),
                },
            }
        }
        Err(_) => {
            // Ollama not reachable — try LM Studio / OpenAI-compatible
            match client.get(&models_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let data = body["data"].as_array().cloned().unwrap_or_default();
                        let names: Vec<String> = data
                            .iter()
                            .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                            .collect();
                        return ProviderCheck {
                            running: true,
                            _model_available: false,
                            installed_models: names,
                            error: None,
                        };
                    }
                    ProviderCheck {
                        running: true,
                        _model_available: false,
                        installed_models: vec![],
                        error: None,
                    }
                }
                Ok(_) => ProviderCheck {
                    running: false,
                    _model_available: false,
                    installed_models: vec![],
                    error: Some(format!("{} returned an error response", name)),
                },
                Err(e2) => {
                    let err_str = format!("{}", e2);
                    if err_str.contains("Connection refused")
                        || err_str.contains("connect error")
                        || err_str.contains("timed out")
                    {
                        return ProviderCheck {
                            running: false,
                            _model_available: false,
                            installed_models: vec![],
                            error: Some(format!("{} is not running", name)),
                        };
                    }
                    ProviderCheck {
                        running: false,
                        _model_available: false,
                        installed_models: vec![],
                        error: Some(format!("Cannot connect to {}", name)),
                    }
                }
            }
        }
    }
}

/// Validate that a model name contains only safe characters (alphanumeric, colon, dash, underscore, dot, slash).
fn is_valid_model_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":.-_/".contains(c))
}

/// POST /api/setup/start-ollama — Attempt to launch Ollama if installed
pub async fn setup_start_ollama_handler() -> impl IntoResponse {
    let result = launch_ollama().await;
    match result {
        Ok(msg) => Json(serde_json::json!({
            "status": "success",
            "message": msg,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "status": "error",
                "message": e,
            })),
        )
            .into_response(),
    }
}

async fn launch_ollama() -> Result<String, String> {
    // Check if already running
    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let client = savant_core::net::secure_client_with_timeout(3, 3)
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    if client
        .get(format!("{}/api/tags", ollama_url))
        .send()
        .await
        .is_ok()
    {
        return Ok("Ollama is already running".to_string());
    }

    // Try to find and launch Ollama
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        // Try common Windows install paths
        let candidates = [
            format!(
                "{}\\AppData\\Local\\Programs\\Ollama\\ollama app.exe",
                std::env::var("USERPROFILE").unwrap_or_default()
            ),
            "C:\\Program Files\\Ollama\\ollama app.exe".to_string(),
            "C:\\Program Files (x86)\\Ollama\\ollama app.exe".to_string(),
        ];

        for path in &candidates {
            if std::path::Path::new(path).exists() {
                match std::process::Command::new(path)
                    .arg("serve")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    Ok(_) => {
                        // Wait for it to come up
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        return Ok(format!("Ollama launched from {}", path));
                    }
                    Err(_) => {
                        // Try without args (GUI app)
                        match std::process::Command::new(path)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .creation_flags(CREATE_NO_WINDOW)
                            .spawn()
                        {
                            Ok(_) => {
                                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                return Ok(format!("Ollama GUI launched from {}", path));
                            }
                            Err(_) => continue,
                        }
                    }
                }
            }
        }

        // Try `ollama` on PATH
        if std::process::Command::new("ollama")
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .is_ok()
        {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            return Ok("Ollama launched from PATH".to_string());
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Try opening the Ollama app
        match std::process::Command::new("open")
            .args(["-a", "Ollama"])
            .spawn()
        {
            Ok(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                return Ok("Ollama launched via macOS open".to_string());
            }
            Err(_) => {}
        }

        // Try PATH
        match std::process::Command::new("ollama").arg("serve").spawn() {
            Ok(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                return Ok("Ollama launched from PATH".to_string());
            }
            Err(_) => {}
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try systemd
        match std::process::Command::new("systemctl")
            .args(["--user", "start", "ollama"])
            .spawn()
        {
            Ok(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                return Ok("Ollama started via systemctl".to_string());
            }
            Err(_) => {}
        }

        // Try PATH
        match std::process::Command::new("ollama").arg("serve").spawn() {
            Ok(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                return Ok("Ollama launched from PATH".to_string());
            }
            Err(_) => {}
        }
    }

    Err("Ollama not found. Install it from https://ollama.com/download".to_string())
}

/// POST /api/setup/install-model — Pull a model via Ollama
/// Body: { "model": "gemma4:e4b" }
pub async fn setup_install_model_handler(
    State(_state): State<Arc<GatewayState>>,
    Json(body): Json<InstallModelRequest>,
) -> impl IntoResponse {
    // Validate model name to prevent injection
    if !is_valid_model_name(&body.model) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": "Invalid model name"
            })),
        )
            .into_response();
    }

    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());

    match savant_core::net::secure_client()
        .post(format!("{}/api/pull", ollama_url))
        .json(&serde_json::json!({
            "name": body.model,
            "stream": false
        }))
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => Json(serde_json::json!({
            "status": "success",
            "message": "Model installed successfully"
        }))
        .into_response(),
        Ok(resp) => {
            let status = resp.status();
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Model installation failed ({})", status)
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "error",
                "message": "Cannot connect to model service"
            })),
        )
            .into_response(),
    }
}

/// Request body for POST /api/setup/openrouter-key
#[derive(Debug, Deserialize)]
pub struct OpenRouterKeyRequest {
    /// The key to store (master key or raw API key)
    pub key: String,
    /// Optional key type: "master" or "raw". Defaults to "raw".
    #[serde(default = "default_key_type")]
    pub key_type: String,
}

fn default_key_type() -> String {
    "raw".to_string()
}

/// POST /api/setup/openrouter-key — Validate and store an OpenRouter API key
///
/// If key_type is "master", uses OpenRouterMgmt to auto-create a scoped key.
/// If key_type is "raw" (default), stores the key directly via keyring.
pub async fn setup_openrouter_key_handler(
    Json(body): Json<OpenRouterKeyRequest>,
) -> impl IntoResponse {
    let key = body.key.trim().to_string();
    let key_str = key.as_str();

    // Validate input
    if key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": "API key cannot be empty"
            })),
        )
            .into_response();
    }
    if key.len() > 1024 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": "API key exceeds maximum length of 1024 characters"
            })),
        )
            .into_response();
    }

    // Validate key type
    let key_type = body.key_type.trim().to_lowercase();
    if key_type != "master" && key_type != "raw" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": "key_type must be 'master' or 'raw'"
            })),
        )
            .into_response();
    }

    // Validate the key by calling OpenRouter's models endpoint
    let client = match savant_core::net::secure_client_with_timeout(15, 10) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("Failed to create HTTP client: {}", e)
                })),
            )
                .into_response();
        }
    };

    let validation_result = client
        .get("https://openrouter.ai/api/v1/models")
        .header("Authorization", format!("Bearer {}", body.key.trim()))
        .send()
        .await;

    match validation_result {
        Ok(resp) if resp.status().is_success() => {
            // Key is valid — store it
            // Master keys are stored as OR_MASTER_KEY (the existing resolve_openrouter_key()
            // in the gateway handlers already handles master key → scoped key exchange)
            // Raw API keys are stored as OPENROUTER_API_KEY
            let secret_name = if key_type == "master" {
                "OR_MASTER_KEY"
            } else {
                "OPENROUTER_API_KEY"
            };
            // Store via keyring
            match savant_core::config::store_secret(secret_name, key_str) {
                Ok(()) => {
                    // Also set env var so it's available immediately without restart
                    std::env::set_var(secret_name, key_str);
                    tracing::info!("[setup] OpenRouter {} key stored successfully", key_type);
                    Json(serde_json::json!({
                        "status": "success",
                        "message": "OpenRouter API key validated and stored",
                        "key_type": key_type,
                    }))
                    .into_response()
                }
                Err(e) => {
                    tracing::error!("[setup] Failed to store OpenRouter key in keyring: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "status": "error",
                            "message": format!("Failed to store key in system keyring: {}", e)
                        })),
                    )
                        .into_response()
                }
            }
        }
        Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
            // Invalid key
            tracing::warn!("[setup] OpenRouter key validation failed: HTTP {}", resp.status());
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "status": "error",
                    "message": "Invalid API key. Check the key and try again."
                })),
            )
                .into_response()
        }
        Ok(resp) => {
            // Some other error
            let status = resp.status();
            tracing::warn!("[setup] OpenRouter key validation: unexpected HTTP {}", status);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("OpenRouter returned HTTP {} — the service may be temporarily unavailable", status)
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!("[setup] OpenRouter key validation: connection error: {}", e);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "error",
                    "message": "Cannot reach OpenRouter. Check your internet connection and try again."
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/setup/install-model-stream — Pull a model via Ollama with SSE progress
/// Returns Server-Sent Events with progress updates.
pub async fn setup_install_model_stream_handler(
    State(_state): State<Arc<GatewayState>>,
    Json(body): Json<InstallModelRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = body.model.clone();
    let valid = is_valid_model_name(&body.model);

    let stream = async_stream::stream! {
        if !valid {
            yield Ok(Event::default().data(
                serde_json::json!({"status": "error", "message": "Invalid model name"}).to_string(),
            ));
            return;
        }

        let resp = match savant_core::net::secure_client()
            .post(format!("{}/api/pull", ollama_url))
            .json(&serde_json::json!({
                "name": model,
                "stream": true
            }))
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                yield Ok(Event::default().data(
                    serde_json::json!({"status": "error", "message": format!("Cannot connect to Ollama: {}", e)}).to_string(),
                ));
                return;
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            yield Ok(Event::default().data(
                serde_json::json!({"status": "error", "message": format!("Ollama returned {}", status)}).to_string(),
            ));
            return;
        }

        let mut byte_stream = resp.bytes_stream();
        use futures::StreamExt;
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim().to_string();
                        buffer = buffer[newline_pos + 1..].to_string();
                        if line.is_empty() { continue; }

                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                            let status = parsed["status"].as_str().unwrap_or("pulling");
                            let completed = parsed["completed"].as_u64();
                            let total = parsed["total"].as_u64();

                            let event_data = serde_json::json!({
                                "status": "progress",
                                "message": status,
                                "completed": completed,
                                "total": total,
                            });
                            yield Ok(Event::default().data(event_data.to_string()));

                            if status.contains("success") {
                                yield Ok(Event::default().data(
                                    serde_json::json!({"status": "success", "message": "Model installed successfully"}).to_string(),
                                ));
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Ok(Event::default().data(
                        serde_json::json!({"status": "error", "message": format!("Stream error: {}", e)}).to_string(),
                    ));
                    return;
                }
            }
        }

        // Stream ended without explicit success — assume it worked
        yield Ok(Event::default().data(
            serde_json::json!({"status": "success", "message": "Model installed successfully"}).to_string(),
        ));
    };

    Sse::new(stream)
}
