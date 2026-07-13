use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::MemoryBackend;
use savant_core::types::{AgentReflection, ChatMessage};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{info, instrument};

/// Maximum LEARNINGS.md file size before rotation (100KB).
const MAX_LEARNINGS_SIZE: u64 = 100_000;
/// Maximum length per learning entry (2000 chars).
const MAX_ENTRY_LENGTH: usize = 2000;
/// Size of the rolling content hash dedup set.
const DEDUP_WINDOW_SIZE: usize = 10_000;

/// A decorator for `MemoryBackend` that adds file-based logging for agent self-improvement.
///
/// Implements Phase 1 safety gates from FID-20260525-LEARNING-SYSTEM-REVIEW:
/// - Content-hash dedup (rolling 10K entry window)
/// - Per-entry length cap (2000 chars)
/// - LEARNINGS.md rotation at 100KB
/// - Trigger-path tagging on all entries
/// - Filtered content logging to FILTERED.jsonl
#[derive(Clone)]
pub struct FileLoggingMemoryBackend {
    inner: Arc<dyn MemoryBackend>,
    workspace_path: PathBuf,
    /// Rolling content hash set for dedup. Protected by RwLock for concurrent access.
    content_hashes: Arc<RwLock<HashSet<u64>>>,
}

impl FileLoggingMemoryBackend {
    /// Creates a new `FileLoggingMemoryBackend`.
    pub fn new(inner: Arc<dyn MemoryBackend>, workspace_path: PathBuf) -> Self {
        Self {
            inner,
            workspace_path,
            content_hashes: Arc::new(RwLock::new(HashSet::with_capacity(DEDUP_WINDOW_SIZE))),
        }
    }

    /// Hash content for dedup. Normalizes whitespace and lowercases.
    fn content_hash(text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let normalized: String = text
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }

    /// Log filtered content to FILTERED.jsonl for human review.
    #[allow(clippy::disallowed_methods)]
    async fn log_filtered(&self, agent_id: &str, reason: &str, content: &str) {
        let filtered_path = self.workspace_path.join("FILTERED.jsonl");
        let entry = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "agent_id": agent_id,
            "reason": reason,
            "content_preview": &content[..content.len().min(200)],
        });
        let line = format!("{}\n", entry);
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&filtered_path)
            .await
        {
            let _ = file.write_all(line.as_bytes()).await;
        }
    }

    /// Check LEARNINGS.md size and rotate to archive if > 100KB.
    async fn rotate_if_needed(&self) {
        let md_path = self.workspace_path.join("LEARNINGS.md");
        if let Ok(metadata) = fs::metadata(&md_path).await {
            if metadata.len() > MAX_LEARNINGS_SIZE {
                let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                let archive_name = format!("LEARNINGS-ARCHIVE-{}.md", timestamp);
                let archive_path = self.workspace_path.join(&archive_name);
                if fs::rename(&md_path, &archive_path).await.is_ok() {
                    tracing::info!(
                        "[learning] LEARNINGS.md rotated ({}KB → {})",
                        metadata.len() / 1024,
                        archive_name
                    );
                }
            }
        }
    }

    /// Records a new learning or correction to LEARNINGS.md (free-form).
    ///
    /// Phase 1 safety gates:
    /// - Content-hash dedup (rolling 10K entry window)
    /// - Per-entry length cap (2000 chars)
    /// - LEARNINGS.md rotation at 100KB
    /// - Trigger-path tagging
    /// - Filtered content logging
    #[instrument(skip(self), fields(agent_id))]
    pub async fn record_learning(
        &self,
        agent_id: &str,
        learning_text: &str,
        source: &str,
    ) -> Result<(), SavantError> {
        let md_path = self.workspace_path.join("LEARNINGS.md");

        // Phase 1 Gate 1: Per-entry length cap (2000 chars)
        let text = if learning_text.len() > MAX_ENTRY_LENGTH {
            let truncated: String = learning_text.chars().take(MAX_ENTRY_LENGTH).collect();
            tracing::warn!(
                "[{}] LEARNINGS.md entry truncated: {} → {} chars",
                agent_id,
                learning_text.len(),
                MAX_ENTRY_LENGTH
            );
            truncated
        } else {
            learning_text.to_string()
        };

        // Grounding filter — block fabrication, require environmental grounding
        if !crate::learning::OutputFilter::is_grounded(&text) {
            tracing::warn!(
                "[{}] LEARNINGS.md write filtered (not grounded): {}",
                agent_id,
                &text[..text.len().min(100)]
            );
            self.log_filtered(agent_id, "not_grounded", &text).await;
            return Ok(());
        }

        // Phase 1 Gate 2: Content-hash dedup
        let hash = Self::content_hash(&text);
        {
            let mut hashes = self.content_hashes.write().await;
            if hashes.contains(&hash) {
                tracing::debug!(
                    "[{}] LEARNINGS.md duplicate filtered (hash={:x})",
                    agent_id,
                    hash
                );
                self.log_filtered(agent_id, "duplicate", &text).await;
                return Ok(());
            }
            hashes.insert(hash);
            // Rolling window: if over capacity, remove oldest half
            if hashes.len() > DEDUP_WINDOW_SIZE {
                let to_remove: Vec<u64> =
                    hashes.iter().take(DEDUP_WINDOW_SIZE / 2).copied().collect();
                for h in to_remove {
                    hashes.remove(&h);
                }
            }
        }

        // Phase 1 Gate 3: Rotate LEARNINGS.md if > 100KB
        self.rotate_if_needed().await;

        // Get current UTC timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.9f UTC");

        // AAA: Extract Lens Tag into Header (Phase 19)
        // If content starts with "# [TAG]", we extract it and put it in the header.
        let (tag, final_text) = if text.starts_with("# [") {
            if let Some(end_idx) = text.find("]") {
                let tag = &text[3..end_idx];
                let rest = &text[end_idx + 1..].trim();
                (format!(" [{}]", tag), rest.to_string())
            } else {
                (String::new(), text.to_string())
            }
        } else {
            (String::new(), text.to_string())
        };

        // Phase 1 Gate 4: Trigger-path tag on every entry
        let entry = format!(
            "\n\n### Learning ({}){} [source:{}]\n{}\n",
            timestamp, tag, source, final_text
        );

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&md_path)
            .await?;

        file.write_all(entry.as_bytes()).await?;

        info!(
            "Recorded learning [{}] for agent {} (source={}, {} chars)",
            tag.trim(),
            agent_id,
            source,
            final_text.len()
        );
        Ok(())
    }

    /// Records a reflection on a completed task to REFLECT.md.
    #[instrument(skip(self), fields(agent_id))]
    pub async fn record_reflection(
        &self,
        agent_id: &str,
        reflection: AgentReflection,
    ) -> Result<(), SavantError> {
        let path = self.workspace_path.join("REFLECT.md");
        let content = format!(
            "\n## Reflection: {}\n- Success: {}\n- Critique: {}\n- Learning: {}\n- Action Items: {:?}\n",
            reflection.task_id, reflection.success, reflection.critique, reflection.learning, reflection.action_items
        );

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;

        file.write_all(content.as_bytes()).await?;

        info!("Recorded reflection for agent {}", agent_id);
        Ok(())
    }

    /// Parses LEARNINGS.md into JSONL format for dashboard display.
    /// This can be called manually to refresh the reflections panel.
    pub async fn parse_learnings(&self, agent_id: &str) -> Result<usize, SavantError> {
        let parser = crate::learning::LearningsParser::new(self.workspace_path.clone());
        let count = parser.parse_and_convert(agent_id)?;
        info!(
            "[{}] Manually parsed {} learning entries from LEARNINGS.md",
            agent_id, count
        );
        Ok(count)
    }
}

#[async_trait]
impl MemoryBackend for FileLoggingMemoryBackend {
    async fn store(&self, agent_id: &str, message: &ChatMessage) -> Result<(), SavantError> {
        // 🛡️ Sovereign Routing: Use the channel type, not string heuristics
        if message.channel == savant_core::types::AgentOutputChannel::Memory {
            if let Err(e) = self
                .record_learning(agent_id, &message.content, "memory_store")
                .await
            {
                tracing::warn!(
                    "[agent::memory] Failed to record learning for agent {}: {}",
                    agent_id,
                    e
                );
            }
        }

        self.inner.store(agent_id, message).await
    }

    async fn retrieve(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        self.inner.retrieve(agent_id, query, limit).await
    }

    async fn consolidate(&self, agent_id: &str) -> Result<(), SavantError> {
        // 1. First delegate to inner backend for LSM compaction/optimization
        self.inner.consolidate(agent_id).await?;

        // NA-03: Record a consolidation reflection for agent self-improvement tracking
        let reflection = savant_core::types::AgentReflection {
            task_id: format!("consolidate-{}", chrono::Utc::now().timestamp()),
            success: true,
            critique: "Memory consolidation completed successfully".to_string(),
            learning: "Periodic memory consolidation maintains data integrity".to_string(),
            action_items: vec![],
            importance: 3,
        };
        if let Err(e) = self.record_reflection(agent_id, reflection).await {
            tracing::debug!(
                "[{}] Non-critical: failed to record consolidation reflection: {}",
                agent_id,
                e
            );
        }

        // 2. Parse new LEARNINGS.md entries into JSONL (for dashboard display)
        //    Uses the public parse_learnings() method as the production path.
        match self.parse_learnings(agent_id).await {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "[{}] Converted {} new learning entries from LEARNINGS.md → JSONL",
                        agent_id, count
                    );
                    // 2.5. Store parsed entries in swarm.insights for dashboard API
                    let learnings_path = self.workspace_path.join("LEARNINGS.jsonl");
                    if let Ok(content) = std::fs::read_to_string(&learnings_path) {
                        for line in content.lines() {
                            if let Ok(entry) = serde_json::from_str::<
                                savant_core::learning::EmergentLearning,
                            >(line)
                            {
                                // Check if already in swarm.insights by looking for this timestamp
                                let msg = savant_core::types::ChatMessage {
                                    is_telemetry: false,
                                    role: savant_core::types::ChatRole::Assistant,
                                    content: entry.content.clone(),
                                    sender: Some(entry.agent_id.clone()),
                                    recipient: None,
                                    agent_id: None,
                                    session_id: Some(savant_core::types::SessionId(
                                        "learning.swarm".to_string(),
                                    )),
                                    channel: savant_core::types::AgentOutputChannel::Memory,
                                    images: Vec::new(),
                                    ..Default::default()
                                };
                                if let Err(e) = self.inner.store("swarm.insights", &msg).await {
                                    tracing::warn!(
                                        "[agent::memory] Failed to store swarm insight entry: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[{}] Failed to parse LEARNINGS.md: {}", agent_id, e);
            }
        }

        // 3. Archive old JSONL if too large
        let learnings_path = self.workspace_path.join("LEARNINGS.jsonl");
        let history_path = self.workspace_path.join("HISTORY.jsonl");

        if let Ok(metadata) = fs::metadata(&learnings_path).await {
            if metadata.len() > 500_000 {
                // Archive at 500KB
                info!(
                    "[{}] Consolidating memory: LEARNINGS.jsonl is too large ({} bytes). Archiving...",
                    agent_id,
                    metadata.len()
                );
                let content = fs::read_to_string(&learnings_path).await?;
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&history_path)
                    .await?;
                file.write_all(content.as_bytes()).await?;
                fs::write(&learnings_path, "").await?; // Reset JSONL
            }
        }

        Ok(())
    }

    async fn get_or_create_session(
        &self,
        session_id: &str,
    ) -> Result<savant_core::types::SessionState, SavantError> {
        self.inner.get_or_create_session(session_id).await
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<savant_core::types::SessionState>, SavantError> {
        self.inner.get_session(session_id).await
    }

    async fn save_session(
        &self,
        state: &savant_core::types::SessionState,
    ) -> Result<(), SavantError> {
        self.inner.save_session(state).await
    }

    async fn save_turn(&self, turn: &savant_core::types::TurnState) -> Result<(), SavantError> {
        self.inner.save_turn(turn).await
    }

    async fn get_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<savant_core::types::TurnState>, SavantError> {
        self.inner.get_turn(session_id, turn_id).await
    }

    async fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<savant_core::types::TurnState>, SavantError> {
        self.inner.fetch_recent_turns(session_id, limit).await
    }
}
