//! Savant — Phase 1 daemon + Phase 2 inner monologue wiring (FID-017).
//!
//! Tauri 2 surface that exposes 8 IPC commands:
//! - Phase 1: `setup_master_key`, `infer_openrouter`, `vault_list_profiles`
//! - Phase 2 (FID-017): `initialize_app_state`, `start_consciousness`,
//!   `stop_consciousness`, `get_consciousness_state`, `trigger_reflection`
//!
//! The crate is `savant_shell` (renamed from `savant_core` in FID-016r2
//! to disambiguate from `crates/core` which also exports `savant_core`).
//! The `src-tauri/src/main.rs` calls `savant_shell::run()` to bootstrap.

pub mod inference;

use crate::inference::openrouter;
use savant_vault::master_key::{self, ProfileSummary};
use savant_agent::consciousness::ConsciousnessState;
use savant_agent::pulse::prompts::LENSES;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// ─── Phase 1 commands (unchanged) ──────────────────────────────────

#[tauri::command]
async fn setup_master_key(provider: String, api_key: String) -> Result<(), String> {
    master_key::save_profile(&provider, &api_key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn infer_openrouter(prompt: String) -> Result<String, String> {
    openrouter::chat_completion(&prompt)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn vault_list_profiles() -> Result<Vec<ProfileSummary>, String> {
    master_key::list_profiles()
        .await
        .map_err(|e| e.to_string())
}

// ─── Phase 2 commands (FID-017) ────────────────────────────────────

/// AppState — the inner monologue bridge. Holds the workspace path,
/// the consciousness state handle, the lens rotation index, and the
/// daemon lifecycle (JoinHandle + CancellationToken).
pub struct AppState {
    pub workspace_path: PathBuf,
    pub state_handle: Arc<AtomicU8>,
    pub lens_index: Mutex<usize>,
    pub daemon_handle: Mutex<Option<JoinHandle<()>>>,
    pub shutdown_token: Mutex<Option<CancellationToken>>,
    pub daemon_state: Mutex<ConsciousnessState>,
}

impl AppState {
    fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            state_handle: Arc::new(AtomicU8::new(0)),
            lens_index: Mutex::new(0),
            daemon_handle: Mutex::new(None),
            shutdown_token: Mutex::new(None),
            daemon_state: Mutex::new(ConsciousnessState::Idle),
        }
    }
}

#[tauri::command]
async fn initialize_app_state(_workspace_path: String) -> Result<(), String> {
    // The actual AppState is initialized in the Tauri setup() callback
    // below (single source of truth at startup). This IPC command is a
    // no-op that the renderer can call to confirm the bridge is reachable.
    Ok(())
}

#[tauri::command]
async fn start_consciousness(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let mut handle_guard = state.daemon_handle.lock().await;
    if handle_guard.is_some() {
        return Err("daemon already running".into());
    }
    let shutdown = CancellationToken::new();
    let state_handle = state.state_handle.clone();
    let shutdown_for_task = shutdown.clone();
    let handle = tokio::spawn(async move {
        // Simplified daemon loop — cycles THINKING -> IDLE -> DORMANT -> WONDERING
        // every 5s. Full ConsciousnessDaemon integration with LlmProvider
        // comes in FID-018+ (this MVP proves the lifecycle works).
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        let mut current: u8 = 0;
        loop {
            tokio::select! {
                _ = shutdown_for_task.cancelled() => break,
                _ = interval.tick() => {
                    current = (current + 1) % 4;
                    state_handle.store(current, Ordering::Relaxed);
                }
            }
        }
    });
    *handle_guard = Some(handle);
    *state.shutdown_token.lock().await = Some(shutdown);
    *state.daemon_state.lock().await = ConsciousnessState::Thinking;
    state.state_handle.store(0, Ordering::Relaxed);
    Ok("THINKING".into())
}

#[tauri::command]
async fn stop_consciousness(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if let Some(token) = state.shutdown_token.lock().await.take() {
        token.cancel();
    }
    if let Some(handle) = state.daemon_handle.lock().await.take() {
        let _ = handle.await;
    }
    *state.daemon_state.lock().await = ConsciousnessState::Idle;
    state.state_handle.store(1, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
async fn get_consciousness_state(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let raw = state.state_handle.load(Ordering::Relaxed);
    Ok(match raw {
        0 => "THINKING".to_string(),
        1 => "IDLE".to_string(),
        2 => "DORMANT".to_string(),
        3 => "WONDERING".to_string(),
        _ => "UNKNOWN".to_string(),
    })
}

#[tauri::command]
async fn trigger_reflection(
    state: tauri::State<'_, AppState>,
    lens_override: Option<String>,
) -> Result<String, String> {
    let mut idx_guard = state.lens_index.lock().await;
    let (name, prompt) = if let Some(override_name) = lens_override {
        LENSES
            .iter()
            .find(|(n, _)| *n == override_name)
            .map(|(n, p)| (n.to_string(), p.to_string()))
            .ok_or_else(|| format!("unknown lens: {override_name}"))?
    } else {
        let (name, prompt) = LENSES[*idx_guard % LENSES.len()];
        *idx_guard = idx_guard.wrapping_add(1);
        (name.to_string(), prompt.to_string())
    };
    drop(idx_guard);

    // Build the full reflection prompt.
    let full_prompt = format!(
        "{}\n\nYou are Savant. Reflect on whatever comes to mind using this lens. Write freely in Markdown.",
        prompt
    );

    // Call OpenRouter via the existing inference module.
    let narrative = openrouter::chat_completion(&full_prompt)
        .await
        .map_err(|e| e.to_string())?;

    // Append to workspace-savant/REFLECTIONS.md with the parser-compatible
    // header format. Matches the LearningsParser::parse_entries() pattern at
    // crates/agent/src/learning/parser.rs:108 (splits on `### Learning (`).
    let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.6f UTC");
    let entry = format!("\n### Learning ({}) [{}]\n{}\n", ts, name, narrative);
    let reflections_path = state.workspace_path.join("REFLECTIONS.md");
    let mut current = tokio::fs::read_to_string(&reflections_path)
        .await
        .unwrap_or_default();
    current.push_str(&entry);
    tokio::fs::write(&reflections_path, current)
        .await
        .map_err(|e| format!("failed to write REFLECTIONS.md: {e}"))?;

    Ok(narrative)
}

/// FID-020r2: Load `<exe_dir>/.env` to wire the vault's strategy 3
/// (`crates/vault/src/master_key.rs:19`). Called from `run()` with
/// `std::env::current_exe()`; `pub` so integration tests can pass a
/// fake `exe_path` without booting Tauri.
///
/// Returns `Ok(())` on success (regardless of whether `.env` was
/// found — the caller wraps in `.ok()` to match strategy 2's
/// silent-no-fail behavior). Propagates the original dotenvy error
/// otherwise. Missing `.env` (the common dev / packaged-prod case
/// — the OS env or the vault file covers it) returns
/// `Err(dotenvy::Error::Io(NotFound))`.
///
/// **Note:** `run()` ALSO guards `current_exe()` with
/// `if let Ok(exe)` — if `current_exe()` itself fails, strategy 3
/// is silently skipped and the cwd fallback (strategy 2) covers the
/// common case. **The cwd-FIRST `.env` precedence rationale lives
/// in the canonical docstring block at [`savant_vault::master_key`]
/// — see the "Precedence & `.env` loading" paragraph below the
/// 5-strategy enumeration.**
pub fn load_env_from_exe_dir(exe_path: &Path) -> Result<(), dotenvy::Error> {
    let parent = exe_path.parent().ok_or_else(|| {
        dotenvy::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "exe_path has no parent directory (root path?)",
        ))
    })?;
    dotenvy::from_path(&parent.join(".env")).map(|_| ())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // FID-020 + FID-020r2: Wire the vault's strategies 2 (cwd `.env`)
    // + 3 (exe-dir `.env`) by loading `.env` files at startup. Without
    // these calls the vault's 5-strategy cascade lists both strategies
    // in its docstring but the env vars are never actually populated
    // from disk, so the cascade silently falls through to the vault
    // file on every `.env` reference. The `.ok()` ignores the common
    // case where no `.env` file exists (dev / packaged prod rely on
    // the OS env or the vault file instead). **The cwd-FIRST `.env`
    // precedence rationale lives in the canonical docstring block at
    // [`savant_vault::master_key`] — see the "Precedence & `.env` loading" paragraph below the 5-strategy enumeration.** Placed
    // BEFORE `tracing_subscriber::fmt().init()` so that `RUST_LOG` set
    // in `.env` is respected by the subscriber.
    dotenvy::dotenv().ok();
    if let Ok(exe) = std::env::current_exe() {
        load_env_from_exe_dir(&exe).ok();
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            setup_master_key,
            infer_openrouter,
            vault_list_profiles,
            initialize_app_state,
            start_consciousness,
            stop_consciousness,
            get_consciousness_state,
            trigger_reflection,
        ])
        .setup(|app| {
            tracing::info!(
                "Savant daemon starting; vault dir = {:?}",
                master_key::vault_file_path()
            );
            // Initialize AppState with the default workspace path.
            // workspace-savant/ is the savant-orig convention (skills/,
            // SOUL.md, LEARNINGS.md, REFLECTIONS.md all live here).
            let workspace_path = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("workspace-savant");
            std::fs::create_dir_all(&workspace_path).ok();
            app.manage(AppState::new(workspace_path));
            let _ = app.handle();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Savant daemon");
}
