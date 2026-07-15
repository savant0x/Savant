//! FID-029 §Step 8 / §Step 9 — Tauri Runtime Async Memory Init + Production
//! Chat-Persistence IPC Commands.
//!
//! Layer 1 (FID-029) of the Strangler-Fig migration (FID-035 §Layered
//! Build Order): the Tauri commands here are the SOLE live interface
//! between the renderer chat page and `savant_memory`. The gateway REST
//! stubs at `crates/gateway/src/handlers/v1/chat.rs` remain
//! `NotImplemented` until Layer 3 (FID-032) swaps the renderer bridge
//! from `invoke<T>()` to `fetch(...)`.
//!
//! ## Design
//!
//! The 5 commands compose ONLY existing `AsyncMemoryBackend` primitives
//! (per ECHO Law 7): `list_chat_sessions()` + `search_chat_history()`
//! added in `crates/memory/src/async_backend.rs` (FID-029 §Step 9
//! extensions), plus `hydrate_session` / `delete_session` / `store` that
//! already shipped.
//!
//! ## Late init
//!
//! `AppMemory` is managed as `Option<Arc<AsyncMemoryBackend>>` so the
//! (potentially slow) `MemoryEngine::new` + directory-create cost is
//! deferred out of `setup()` — preventing white-screens on cold start
//! (FID-029 §Step 8).

use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

use savant_memory::{
    AsyncMemoryBackend, EngineConfig, LsmConfig, MemoryConfig, MemoryEngine,
    NullEmbeddingProvider, VectorConfig,
};
use savant_memory::models::MessageRole;
use savant_core::traits::MemoryBackend;
use savant_core::types::{AgentOutputChannel, ChatMessage, ChatRole, SessionId};

/// FID-029 §Step 8 — late-binding slot for the AsyncMemoryBackend handle.
///
/// Held in `AppState` via `app.manage(AppMemory::default())` so commands
/// reach it through Tauri's `State<'_, AppMemory>` extractor. `Option`
/// defers the engine + storage init out of `setup()`.
pub struct AppMemory {
    pub backend: Arc<RwLock<Option<Arc<AsyncMemoryBackend>>>>,
}

impl Default for AppMemory {
    fn default() -> Self {
        Self {
            backend: Arc::new(RwLock::new(None)),
        }
    }
}

/// Path under which chat memory persists for a given dev / packaged run.
/// Production wiring (workspace-aware path) lands in FID-029 §Step 1's
/// follow-on cycle. For now, deterministic per-pid temp dir keeps dev
/// runs isolated.
fn chat_memory_dir() -> PathBuf {
    std::env::temp_dir().join(format!("savant_chat_{}", std::process::id()))
}

/// Lazy-init the AsyncMemoryBackend on the first command call.
///
/// Double-checked locking: read first (cheap), upgrade to write only
/// if absent. Once populated the handle is reused for the process
/// lifetime.
pub async fn ensure_backend(
    app_memory: &State<'_, AppMemory>,
) -> Result<Arc<AsyncMemoryBackend>, String> {
    {
        let guard = app_memory.backend.read().await;
        if let Some(b) = guard.as_ref() {
            return Ok(b.clone());
        }
    }
    let mut guard = app_memory.backend.write().await;
    if let Some(b) = guard.as_ref() {
        return Ok(b.clone());
    }

    let dir = chat_memory_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create_dir_all({}): {e}", dir.display()))?;

    let config = EngineConfig {
        lsm_config: LsmConfig {
            vector_dimension: 768,
            ..LsmConfig::default()
        },
        vector_config: VectorConfig {
            dimensions: 768,
            ..VectorConfig::default()
        },
        distill_llm_provider: None,
        distill_params: None,
        embedding_service: Arc::new(NullEmbeddingProvider),
        memory_config: MemoryConfig::default(),
        personality: None,
    };

    // MemoryEngine::new returns Arc<MemoryEngine>; AsyncMemoryBackend::new
    // takes Arc<MemoryEngine>; so we wrap the AsyncMemoryBackend itself in
    // Arc (NOT the engine) for shared mutable access.
    let engine_arc = MemoryEngine::new(&dir, config)
        .map_err(|e| format!("MemoryEngine::new: {e}"))?;
    let backend = Arc::new(AsyncMemoryBackend::new(engine_arc));
    *guard = Some(backend.clone());
    Ok(backend)
}

#[tauri::command]
pub async fn list_chat_sessions(
    app_memory: State<'_, AppMemory>,
) -> Result<Vec<String>, String> {
    let backend = ensure_backend(&app_memory).await?;
    backend
        .list_chat_sessions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn load_chat_history(
    session_id: String,
    limit: usize,
    app_memory: State<'_, AppMemory>,
) -> Result<Vec<ChatMessage>, String> {
    let backend = ensure_backend(&app_memory).await?;
    let msgs = backend
        .hydrate_session(&session_id, limit)
        .map_err(|e| e.to_string())?;
    Ok(msgs
        .into_iter()
        .map(|m| ChatMessage {
            role: match m.role {
                MessageRole::User => ChatRole::User,
                MessageRole::Assistant => ChatRole::Assistant,
                _ => ChatRole::System,
            },
            content: m.content,
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: Some(SessionId(m.session_id)),
            channel: AgentOutputChannel::Chat,
            is_telemetry: false,
            images: Vec::new(),
            ..Default::default()
        })
        .collect())
}

#[tauri::command]
pub async fn persist_chat_turn(
    session_id: String,
    user_msg: String,
    assistant_msg: String,
    app_memory: State<'_, AppMemory>,
) -> Result<(), String> {
    let backend = ensure_backend(&app_memory).await?;
    let user = ChatMessage {
        role: ChatRole::User,
        content: user_msg,
        sender: None,
        recipient: None,
        agent_id: None,
        session_id: Some(SessionId(session_id.clone())),
        channel: AgentOutputChannel::Chat,
        is_telemetry: false,
        images: Vec::new(),
        ..Default::default()
    };
    let assistant = ChatMessage {
        role: ChatRole::Assistant,
        content: assistant_msg,
        sender: None,
        recipient: None,
        agent_id: None,
        session_id: Some(SessionId(session_id)),
        channel: AgentOutputChannel::Chat,
        is_telemetry: false,
        images: Vec::new(),
        ..Default::default()
    };
    let agent_id = "savant_chat_renderer";
    backend
        .store(agent_id, &user)
        .await
        .map_err(|e| e.to_string())?;
    backend
        .store(agent_id, &assistant)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_chat_session(
    session_id: String,
    app_memory: State<'_, AppMemory>,
) -> Result<(), String> {
    let backend = ensure_backend(&app_memory).await?;
    backend
        .delete_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_chat_history(
    query: String,
    limit: usize,
    app_memory: State<'_, AppMemory>,
) -> Result<Vec<ChatMessage>, String> {
    let backend = ensure_backend(&app_memory).await?;
    backend
        .search_chat_history(&query, limit)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_memory_default_holds_none_slot() {
        let m = AppMemory::default();
        let guard = m.backend.read().await;
        assert!(guard.is_none());
    }

    #[test]
    fn test_chat_memory_dir_is_pid_scoped() {
        let dir = chat_memory_dir();
        let name = dir.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("savant_chat_"),
            "expected pid-scoped dir, got {name}"
        );
    }
}
