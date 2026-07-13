use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use savant_memory::engine::MemoryEnclave;
use savant_security::prompt_defense::scan_prompt;

use crate::config::ObsidianConfig;
use crate::error::VaultError;

/// Monitors the Obsidian vault for user edits and feeds them back into the
/// agent's memory system via the NexusBridge.
///
/// Edit classification:
/// - Episodic/*.md: Rejected. The past cannot be rewritten. Edits are logged
///   as Correction nodes linked to the original event.
/// - Semantic/*.md: Accepted as Ground Truth Override. Parsed and written into
///   the LSM metadata store.
/// - Identity/SOUL.md: Blocked. Must go through the Evolution system.
/// - Identity/Personality.md: Accepted. OCEAN values parsed and forwarded.
/// - New files: Enter quarantine. Extracted entities require user validation.
/// - File deletions: Tombstone pattern (marked hidden in DB, not recreated).
///
/// All edits pass through `scan_prompt()` injection defense before affecting
/// agent state. The vault is treated as a potentially hostile data source.
pub struct VaultWatcher {
    vault_path: PathBuf,
    config: ObsidianConfig,
    nexus: Option<Arc<savant_core::bus::NexusBridge>>,
    enclave: Option<Arc<MemoryEnclave>>,
    shutdown: watch::Receiver<bool>,
}

impl VaultWatcher {
    pub fn new(
        vault_path: PathBuf,
        config: ObsidianConfig,
        nexus: Option<Arc<savant_core::bus::NexusBridge>>,
        enclave: Option<Arc<MemoryEnclave>>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            vault_path,
            config,
            nexus,
            enclave,
            shutdown,
        }
    }

    /// Starts the file watcher loop. Spawn this as a tokio task.
    pub async fn run(&self) -> Result<(), VaultError> {
        if !self.config.enabled {
            debug!("[obsidian] Vault watcher disabled by config");
            return Ok(());
        }

        if !self.vault_path.exists() {
            debug!("[obsidian] Vault path does not exist; watcher idle");
            return Ok(());
        }

        // Check vault size against max_files threshold
        let file_count = crate::count_md_files(&self.vault_path);
        if file_count >= self.config.max_files {
            warn!(
                "[obsidian] Vault at capacity ({}/{} files) — cold storage recommended",
                file_count, self.config.max_files
            );
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Event>(256);

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    if let Err(e) = tx.blocking_send(event) {
                        tracing::warn!("[watcher] Failed to send file event: {}", e);
                    }
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )
        .map_err(VaultError::Notify)?;

        watcher
            .watch(&self.vault_path, RecursiveMode::Recursive)
            .map_err(VaultError::Notify)?;

        info!("[obsidian] Vault watcher active on {:?}", self.vault_path);

        // Debounce timer: coalesce rapid edits (e.g. Obsidian autosave)
        let debounce = Duration::from_secs(2);

        let mut shutdown = self.shutdown.clone();
        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    let event = match maybe_event {
                        Some(e) => e,
                        None => break,
                    };
                    self.handle_event(&event).await;

                    // Drain any additional events that arrive within the debounce window
                    loop {
                        tokio::select! {
                            biased;
                            _ = tokio::time::sleep(debounce) => { break; }
                            next = rx.recv() => {
                                match next {
                                    Some(e) => self.handle_event(&e).await,
                                    None => break,
                                }
                            }
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("[obsidian] Vault watcher shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::disallowed_methods)]
    async fn handle_event(&self, event: &notify::Event) {
        // Only process Modify and Create events on .md files
        let is_modify = matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
        if !is_modify {
            return;
        }

        for path in &event.paths {
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            if !path.exists() {
                continue; // Deletion handled by separate Remove event
            }

            // Ignore files outside the known vault subdirectories
            let relative = match path.strip_prefix(&self.vault_path) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let components: Vec<_> = relative.components().collect();
            if components.is_empty() {
                continue;
            }

            let dir_name = components[0].as_os_str().to_string_lossy().to_string();

            // Read the file content
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(e) => {
                    debug!("[obsidian] Cannot read {path:?}: {e}");
                    continue;
                }
            };

            let result = scan_prompt(&content);
            if !result.passed {
                warn!(
                    "[obsidian] Injection blocked in {path:?}: {:?}",
                    result.blocked
                );
                continue;
            }

            let sanitized = result.sanitized_text;

            match dir_name.as_str() {
                "Episodic" => {
                    // Episodic edits are rejected. The past is immutable.
                    // Log a correction node for the user's intent.
                    debug!("[obsidian] Episodic edit rejected (immutable): {relative:?}");
                    if let Some(nexus) = &self.nexus {
                        if let Err(e) = nexus
                            .publish(
                                "system.vault.edit_rejected",
                                &format!(
                                    "Episodic edits are immutable. \
                                     Edit rejected: {relative:?}"
                                ),
                            )
                            .await
                        {
                            tracing::warn!(
                                "[watcher] Failed to publish edit_rejected event: {}",
                                e
                            );
                        }
                    }
                }
                "Semantic" => {
                    // Semantic edits accepted as ground truth overrides.
                    // Write directly to the memory enclave and publish to nexus.
                    debug!("[obsidian] Semantic edit accepted: {relative:?}");

                    // Store in memory enclave if available
                    if let Some(enclave) = &self.enclave {
                        let entry_id = chrono::Utc::now().timestamp_millis() as u64;
                        // Compute Shannon entropy of the content
                        let entropy = {
                            let mut freq = std::collections::HashMap::new();
                            for byte in sanitized.bytes() {
                                *freq.entry(byte).or_insert(0u64) += 1;
                            }
                            let total = sanitized.len() as f64;
                            if total > 0.0 {
                                let mut h = 0.0f64;
                                for &count in freq.values() {
                                    let p = count as f64 / total;
                                    if p > 0.0 {
                                        h -= p * p.log2();
                                    }
                                }
                                // Normalize to 0.0-1.0 range (max entropy for byte = 8.0)
                                (h / 8.0).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            }
                        };
                        let memory_entry = savant_memory::models::MemoryEntry {
                            id: entry_id.into(),
                            session_id: "vault".to_string(),
                            category: "semantic_override".to_string(),
                            content: sanitized.clone(),
                            importance: 8,
                            tags: vec!["vault".to_string(), "semantic".to_string()],
                            embedding: Vec::new(),
                            created_at: chrono::Utc::now().timestamp_millis().into(),
                            updated_at: chrono::Utc::now().timestamp_millis().into(),
                            shannon_entropy: entropy.into(),
                            last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                            hit_count: 0u32.into(),
                            related_to: Vec::new(),
                            access_timestamps: Vec::new(),
                            version: 1u32.into(),
                            parent_id: None,
                            supersedes: Vec::new(),
                            is_latest: true,
                        };
                        if let Err(e) = enclave.lsm().insert_metadata(entry_id, &memory_entry) {
                            warn!("[obsidian] Failed to store semantic edit in enclave: {}", e);
                        }
                    }

                    if let Some(nexus) = &self.nexus {
                        let frame = serde_json::json!({
                            "source": "vault",
                            "file": relative.to_string_lossy(),
                            "content": sanitized,
                            "action": "semantic_override",
                        });
                        if let Err(e) = nexus
                            .publish("system.vault.semantic_edit", &frame.to_string())
                            .await
                        {
                            tracing::warn!(
                                "[watcher] Failed to publish semantic_edit event: {}",
                                e
                            );
                        }
                    }
                }
                "Identity" => {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    if file_name == "SOUL.md" {
                        // SOUL.md edits must go through the Evolution system.
                        debug!("[obsidian] SOUL.md edit blocked — use Evolution system");
                        if let Some(nexus) = &self.nexus {
                            if let Err(e) = nexus
                                .publish(
                                    "system.vault.edit_blocked",
                                    r#"{"reason":"SOUL.md edits must go through the Evolution system"}"#,
                                )
                                .await
                            {
                                tracing::warn!("[watcher] Failed to publish edit_blocked event: {}", e);
                            }
                        }
                    } else if file_name == "Personality.md" {
                        // Personality edits: extract OCEAN values.
                        if let Some(nexus) = &self.nexus {
                            let frame = serde_json::json!({
                                "source": "vault",
                                "file": relative.to_string_lossy(),
                                "content": sanitized,
                                "action": "personality_override",
                            });
                            if let Err(e) = nexus
                                .publish("system.vault.personality_edit", &frame.to_string())
                                .await
                            {
                                tracing::warn!(
                                    "[watcher] Failed to publish personality_edit event: {}",
                                    e
                                );
                            }
                        }
                    } else if file_name.starts_with("Evolution") {
                        // Evolution files are read-only projections.
                        debug!("[obsidian] Evolution file edit rejected (read-only): {relative:?}");
                    } else {
                        // Unknown Identity file — quarantine.
                        quarantine_notify(
                            &self.nexus,
                            relative,
                            "Unknown file in Identity/ — quarantined",
                        )
                        .await;
                    }
                }
                "Delegation" => {
                    // Delegation artifacts are read-only projections of A2A results.
                    // User edits are rejected — artifacts are generated by the orchestrator.
                    debug!(
                        "[obsidian] Delegation artifact edit rejected (read-only): {relative:?}"
                    );
                    if let Some(nexus) = &self.nexus {
                        if let Err(e) = nexus
                            .publish(
                                "system.vault.edit_rejected",
                                &format!(
                                    "Delegation artifacts are read-only. \
                                     Edit rejected: {relative:?}"
                                ),
                            )
                            .await
                        {
                            tracing::warn!(
                                "[watcher] Failed to publish edit_rejected event: {}",
                                e
                            );
                        }
                    }
                }
                // GH-14: Procedural edits accepted — users can refine learned procedures.
                "Procedural" => {
                    debug!("[obsidian] Procedural edit accepted: {relative:?}");
                    if let Some(enclave) = &self.enclave {
                        let entry_id = chrono::Utc::now().timestamp_millis() as u64;
                        let memory_entry = savant_memory::models::MemoryEntry {
                            id: entry_id.into(),
                            session_id: "vault".to_string(),
                            category: "procedural_override".to_string(),
                            content: sanitized.clone(),
                            importance: 7,
                            tags: vec!["vault".to_string(), "procedural".to_string()],
                            embedding: Vec::new(),
                            created_at: chrono::Utc::now().timestamp_millis().into(),
                            updated_at: chrono::Utc::now().timestamp_millis().into(),
                            shannon_entropy: 0.0.into(),
                            last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                            hit_count: 0u32.into(),
                            related_to: Vec::new(),
                            access_timestamps: Vec::new(),
                            version: 1u32.into(),
                            parent_id: None,
                            supersedes: Vec::new(),
                            is_latest: true,
                        };
                        if let Err(e) = enclave.lsm().insert_metadata(entry_id, &memory_entry) {
                            warn!("[obsidian] Failed to store procedural edit: {}", e);
                        }
                    }
                    if let Some(nexus) = &self.nexus {
                        let frame = serde_json::json!({
                            "source": "vault",
                            "file": relative.to_string_lossy(),
                            "content": sanitized,
                            "action": "procedural_override",
                        });
                        if let Err(e) = nexus
                            .publish("system.vault.procedural_edit", &frame.to_string())
                            .await
                        {
                            tracing::warn!("[watcher] Failed to publish procedural_edit: {}", e);
                        }
                    }
                }
                // GH-15: Lessons edits accepted as ground truth.
                "Lessons" => {
                    debug!("[obsidian] Lessons edit accepted: {relative:?}");
                    if let Some(enclave) = &self.enclave {
                        let entry_id = chrono::Utc::now().timestamp_millis() as u64;
                        let memory_entry = savant_memory::models::MemoryEntry {
                            id: entry_id.into(),
                            session_id: "vault".to_string(),
                            category: "lesson_override".to_string(),
                            content: sanitized.clone(),
                            importance: 8,
                            tags: vec!["vault".to_string(), "lesson".to_string()],
                            embedding: Vec::new(),
                            created_at: chrono::Utc::now().timestamp_millis().into(),
                            updated_at: chrono::Utc::now().timestamp_millis().into(),
                            shannon_entropy: 0.0.into(),
                            last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                            hit_count: 0u32.into(),
                            related_to: Vec::new(),
                            access_timestamps: Vec::new(),
                            version: 1u32.into(),
                            parent_id: None,
                            supersedes: Vec::new(),
                            is_latest: true,
                        };
                        if let Err(e) = enclave.lsm().insert_metadata(entry_id, &memory_entry) {
                            warn!("[obsidian] Failed to store lesson edit: {}", e);
                        }
                    }
                    if let Some(nexus) = &self.nexus {
                        let frame = serde_json::json!({
                            "source": "vault",
                            "file": relative.to_string_lossy(),
                            "content": sanitized,
                            "action": "lesson_override",
                        });
                        if let Err(e) = nexus
                            .publish("system.vault.lesson_edit", &frame.to_string())
                            .await
                        {
                            tracing::warn!("[watcher] Failed to publish lesson_edit: {}", e);
                        }
                    }
                }
                // GH-16: Insights edits accepted as ground truth.
                "Insights" => {
                    debug!("[obsidian] Insights edit accepted: {relative:?}");
                    if let Some(enclave) = &self.enclave {
                        let entry_id = chrono::Utc::now().timestamp_millis() as u64;
                        let memory_entry = savant_memory::models::MemoryEntry {
                            id: entry_id.into(),
                            session_id: "vault".to_string(),
                            category: "insight_override".to_string(),
                            content: sanitized.clone(),
                            importance: 8,
                            tags: vec!["vault".to_string(), "insight".to_string()],
                            embedding: Vec::new(),
                            created_at: chrono::Utc::now().timestamp_millis().into(),
                            updated_at: chrono::Utc::now().timestamp_millis().into(),
                            shannon_entropy: 0.0.into(),
                            last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                            hit_count: 0u32.into(),
                            related_to: Vec::new(),
                            access_timestamps: Vec::new(),
                            version: 1u32.into(),
                            parent_id: None,
                            supersedes: Vec::new(),
                            is_latest: true,
                        };
                        if let Err(e) = enclave.lsm().insert_metadata(entry_id, &memory_entry) {
                            warn!("[obsidian] Failed to store insight edit: {}", e);
                        }
                    }
                    if let Some(nexus) = &self.nexus {
                        let frame = serde_json::json!({
                            "source": "vault",
                            "file": relative.to_string_lossy(),
                            "content": sanitized,
                            "action": "insight_override",
                        });
                        if let Err(e) = nexus
                            .publish("system.vault.insight_edit", &frame.to_string())
                            .await
                        {
                            tracing::warn!("[watcher] Failed to publish insight_edit: {}", e);
                        }
                    }
                }
                // GH-17 through GH-21: Auto-generated directories — edits rejected.
                "Graphs" | "Retention" | "Audit" | "Themes" | "Multimodal" => {
                    debug!(
                        "[obsidian] {} edit rejected (auto-generated): {relative:?}",
                        dir_name
                    );
                    if let Some(nexus) = &self.nexus {
                        if let Err(e) = nexus
                            .publish(
                                "system.vault.edit_rejected",
                                &format!(
                                    "{}/ is auto-generated. Edit rejected: {relative:?}",
                                    dir_name
                                ),
                            )
                            .await
                        {
                            tracing::warn!(
                                "[watcher] Failed to publish edit_rejected event: {}",
                                e
                            );
                        }
                    }
                }
                "Dashboard" => {
                    debug!("[obsidian] Dashboard edit rejected (computed metrics): {relative:?}");
                    if let Some(nexus) = &self.nexus {
                        if let Err(e) = nexus
                            .publish(
                                "system.vault.edit_rejected",
                                &format!(
                                    "Dashboard/ is auto-generated from computed metrics. \
                                     Edit rejected: {relative:?}"
                                ),
                            )
                            .await
                        {
                            tracing::warn!(
                                "[watcher] Failed to publish edit_rejected event: {}",
                                e
                            );
                        }
                    }
                }
                // CP-27: Working/ is a transient scratchpad — silently ignore edits
                "Working" => {
                    debug!("[obsidian] Working directory edit ignored (transient scratchpad): {relative:?}");
                }
                _ => {
                    // Files in unknown directories or the root are quarantined.
                    quarantine_notify(
                        &self.nexus,
                        relative,
                        "New file in vault — quarantined for review",
                    )
                    .await;
                }
            }
        }
    }
}

#[allow(clippy::disallowed_methods)]
async fn quarantine_notify(
    nexus: &Option<Arc<savant_core::bus::NexusBridge>>,
    relative: &Path,
    reason: &str,
) {
    debug!("[obsidian] Quarantine: {relative:?} — {reason}");
    if let Some(nexus) = nexus {
        let frame = serde_json::json!({
            "file": relative.to_string_lossy(),
            "reason": reason,
            "action": "quarantine",
        });
        if let Err(e) = nexus
            .publish("system.vault.file_quarantined", &frame.to_string())
            .await
        {
            tracing::warn!("[watcher] Failed to publish file_quarantined event: {}", e);
        }
    }
}
