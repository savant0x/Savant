use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use savant_memory::engine::MemoryEnclave;

use crate::cold_storage::ColdStorageManager;
use crate::config::ObsidianConfig;
use crate::writer::VaultWriter;

/// Drives periodic vault projection by polling the outbox cursor.
///
/// Uses a cursor-based approach: on each tick, the worker queries the LSM for current
/// state statistics and compares against the last-projected state stored in a cursor file
/// at `{vault_path}/.cursor.json`. If the state has changed, a full projection is triggered.
///
/// This avoids the dual-write problem entirely: the vault writer always reads from the
/// canonically consistent LSM, and all file writes are atomic via tempfile+rename.
pub struct OutboxWorker {
    vault_path: PathBuf,
    writer: VaultWriter,
    cold_storage: ColdStorageManager,
    config: ObsidianConfig,
    enclave: Option<Arc<MemoryEnclave>>,
    workspace_root: PathBuf,
    shutdown: watch::Receiver<bool>,
}

impl OutboxWorker {
    pub fn new(
        vault_path: PathBuf,
        writer: VaultWriter,
        cold_storage: ColdStorageManager,
        config: ObsidianConfig,
        enclave: Option<Arc<MemoryEnclave>>,
        workspace_root: PathBuf,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            vault_path,
            writer,
            cold_storage,
            config,
            enclave,
            workspace_root,
            shutdown,
        }
    }

    /// Runs the outbox drain loop. Spawn this as a tokio task.
    /// Polls on the configured interval and triggers projection when state changes.
    pub async fn run(&self) {
        let interval_secs = self.config.sync_interval_secs.max(10);
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.tick().await; // skip first immediate tick

        // Load the last-known cursor on startup
        let cursor = CursorState::load(&self.vault_path).await;

        info!(
            "[obsidian] Outbox worker started (interval={interval_secs}s, \
             vault={vault})",
            vault = self.vault_path.display(),
        );

        loop {
            let mut shutdown = self.shutdown.clone();
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown.changed() => {
                    info!("[obsidian] Outbox worker shutting down");
                    return;
                }
            }

            // Compare current state against cursor
            let current = self.snapshot_state().await;
            if !cursor.has_changed(&current) {
                debug!("[obsidian] No state change since last sync; skipping");
                continue;
            }

            debug!("[obsidian] State change detected; running projection");
            match self.writer.run_full_sync(&self.workspace_root).await {
                Ok(stats) => {
                    CursorState {
                        session_count: stats.session_count,
                        memory_count: stats.memory_count,
                        vector_count: stats.vector_count,
                        mutation_count: stats.mutation_count,
                        vault_file_count: stats.vault_file_count as u64,
                        timestamp: Utc::now().timestamp(),
                        procedure_count: 0,
                        lesson_count: 0,
                        insight_count: 0,
                        audit_count: 0,
                    }
                    .save(&self.vault_path)
                    .await;

                    // Run cold storage check after successful sync
                    if let Err(e) = self.cold_storage.run(&self.writer).await {
                        warn!("[obsidian] Cold storage check failed: {e}");
                    }

                    info!(
                        "[obsidian] Sync complete: {files} files, \
                         {sessions} sessions, {memories} memories, {vectors} vectors",
                        files = stats.vault_file_count,
                        sessions = stats.session_count,
                        memories = stats.memory_count,
                        vectors = stats.vector_count,
                    );
                }
                Err(e) => {
                    warn!("[obsidian] Vault sync failed: {e}");
                }
            }
        }
    }

    async fn snapshot_state(&self) -> StateSnapshot {
        let mut snapshot = StateSnapshot::default();
        if let Some(enclave) = &self.enclave {
            let lsm = enclave.lsm();
            if let Ok(s) = lsm.stats() {
                snapshot.session_count = s.total_sessions;
                snapshot.memory_count = s.total_messages;
            }
            snapshot.vector_count = enclave.vector_count() as u64;
            // CP-29: Track derived artifact counts
            snapshot.procedure_count = enclave.procedures().await.len() as u64;
            snapshot.lesson_count = enclave.lessons().await.len() as u64;
            snapshot.insight_count = enclave.insights().await.len() as u64;
            snapshot.audit_count = enclave.audit().await.entries().len() as u64;
        }
        snapshot
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    pub session_count: u64,
    pub memory_count: u64,
    pub vector_count: u64,
    /// CP-29: Derived artifact counts for change detection
    pub procedure_count: u64,
    pub lesson_count: u64,
    pub insight_count: u64,
    pub audit_count: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct CursorState {
    pub session_count: u64,
    pub memory_count: u64,
    pub vector_count: u64,
    pub mutation_count: u64,
    pub vault_file_count: u64,
    pub timestamp: i64,
    /// CP-29: Derived artifact counts for change detection
    #[serde(default)]
    pub procedure_count: u64,
    #[serde(default)]
    pub lesson_count: u64,
    #[serde(default)]
    pub insight_count: u64,
    #[serde(default)]
    pub audit_count: u64,
}

impl CursorState {
    pub async fn load(vault_path: &Path) -> Self {
        let path = vault_path.join(".cursor.json");
        if path.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                if let Ok(cursor) = serde_json::from_str::<CursorState>(&content) {
                    return cursor;
                }
            }
        }
        CursorState::default()
    }

    pub async fn save(&self, vault_path: &Path) {
        let path = vault_path.join(".cursor.json");
        if let Ok(content) = serde_json::to_string(self) {
            let tmp = path.with_extension("tmp");
            if let Err(e) = tokio::fs::write(&tmp, content.as_bytes()).await {
                tracing::warn!("[outbox] Failed to write cursor data: {}", e);
                return;
            }
            if let Err(e) = tokio::fs::rename(&tmp, &path).await {
                tracing::warn!("[outbox] Failed to rename cursor file: {}", e);
            }
        }
    }

    pub fn has_changed(&self, state: &StateSnapshot) -> bool {
        self.session_count != state.session_count
            || self.memory_count != state.memory_count
            || self.vector_count != state.vector_count
            || self.procedure_count != state.procedure_count
            || self.lesson_count != state.lesson_count
            || self.insight_count != state.insight_count
            || self.audit_count != state.audit_count
    }
}
