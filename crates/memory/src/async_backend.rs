//! Async Backend Adapter
//!
//! This module provides an async implementation of the `MemoryBackend` trait
//! (from `savant_core::traits`) using the synchronous `MemoryEngine`.
//!
//! The adapter spawns blocking tasks on the Tokio runtime to ensure that
//! I/O operations don't block the async executor.
//!
//! When an `EmbeddingService` is provided, messages are automatically embedded
//! and indexed for semantic search. Retrieval uses hybrid search: semantic
//! similarity when embeddings are available, falling back to substring matching.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::engine::MemoryEngine;
use crate::models::{AgentMessage, AutoRecallConfig, ContextCacheBlock, MessageRole};
use crate::privacy;

use savant_core::error::SavantError;
use savant_core::traits::{EmbeddingProvider, MemoryBackend};
use savant_core::types::ChatMessage;

/// Dedup cache capacity — always non-zero.
const DEDUP_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(10_000) {
    Some(n) => n,
    None => unreachable!(),
};

/// Dedup entry: content hash + timestamp + entry ID for version chaining.
struct DedupEntry {
    _hash: u64,
    stored_at: std::time::Instant,
    /// The MemoryEntry ID that was stored — used for version chain linking.
    entry_id: u64,
}

/// Async wrapper around MemoryEngine that implements the MemoryBackend trait.
///
/// This type is cheap to clone (Arc) and can be shared across tasks.
/// When an `EmbeddingService` is provided, semantic search capabilities
/// are enabled for both storage and retrieval.
///
/// Features:
/// - **Privacy filter**: secrets are redacted before storage (MEM-02)
/// - **Dedup window**: duplicate content within 5 minutes is skipped (MEM-01)
pub struct AsyncMemoryBackend {
    engine: Arc<MemoryEngine>,
    embedding_service: Option<Arc<dyn EmbeddingProvider>>,
    /// SHA-256 dedup window: content hash -> DedupEntry.
    /// Prevents storing duplicate content within a 5-minute window.
    dedup_cache: tokio::sync::Mutex<lru::LruCache<u64, DedupEntry>>,
}

impl AsyncMemoryBackend {
    /// Creates a new async backend from a synchronous engine.
    pub fn new(engine: Arc<MemoryEngine>) -> Self {
        Self {
            engine,
            embedding_service: None,
            dedup_cache: tokio::sync::Mutex::new(lru::LruCache::new(DEDUP_CACHE_CAP)),
        }
    }

    /// Creates a new async backend with semantic search enabled.
    ///
    /// The embedding service is used to generate vector embeddings for
    /// stored messages and to perform semantic similarity search during
    /// retrieval.
    pub fn with_embeddings(
        engine: Arc<MemoryEngine>,
        embedding_service: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            engine,
            embedding_service: Some(embedding_service),
            dedup_cache: tokio::sync::Mutex::new(lru::LruCache::new(DEDUP_CACHE_CAP)),
        }
    }

    /// Gets a reference to the underlying engine.
    pub fn engine(&self) -> Arc<MemoryEngine> {
        Arc::clone(&self.engine)
    }

    /// Returns whether semantic search is enabled.
    pub fn has_embeddings(&self) -> bool {
        self.embedding_service.is_some()
    }
}

#[async_trait::async_trait]
impl MemoryBackend for AsyncMemoryBackend {
    async fn store(&self, agent_id: &str, message: &ChatMessage) -> Result<(), SavantError> {
        let agent_id_owned = agent_id.to_string();

        // Convert ChatMessage -> AgentMessage
        let agent_msg = AgentMessage::from_chat(message, &agent_id_owned)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let sid = agent_msg.session_id.clone();
        let content = agent_msg.content.clone();
        let msg_id = agent_msg.id.clone();

        // MEM-02: Privacy filter — redact secrets before storage
        let scan_result = privacy::scan_and_redact(&content);
        let content = if scan_result.redaction_count > 0 {
            warn!(
                session = %sid,
                redactions = scan_result.redaction_count,
                types = ?scan_result.redaction_types,
                "Privacy filter redacted secrets from message"
            );
            scan_result.content
        } else {
            content
        };

        // Compute deterministic entry ID early for dedup + version chaining
        let entry_id: u64 = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(sid.as_bytes());
            hasher.update(b"|");
            hasher.update(msg_id.as_bytes());
            let hash = hasher.finalize();
            let bytes = hash.as_bytes();
            u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]))
        };

        // MEM-01: Dedup window — skip duplicate content within 5 minutes
        // MEM-08: On dedup hit, update version chain on the previous entry
        // RC-04: Drop the cache lock before performing I/O operations.
        {
            let content_hash = {
                let mut hasher = DefaultHasher::new();
                sid.hash(&mut hasher);
                content.hash(&mut hasher);
                hasher.finish()
            };

            // Extract dedup hit info while holding the lock, then drop before I/O
            let dedup_hit: Option<(u64, std::time::Instant)> = {
                let mut cache = self.dedup_cache.lock().await;
                if let Some(prev) = cache.get(&content_hash) {
                    if prev.stored_at.elapsed() < std::time::Duration::from_secs(300) {
                        Some((prev.entry_id, prev.stored_at))
                    } else {
                        None
                    }
                } else {
                    None
                }
                // Lock dropped here at end of block
            };

            if let Some((prev_entry_id, _stored_at)) = dedup_hit {
                debug!(
                    session = %sid,
                    "Dedup: skipping duplicate content within 5-min window"
                );
                // CP-03: Update version chain on the previous entry
                // This I/O happens WITHOUT the cache lock held
                if let Ok(Some(mut prev_entry)) =
                    self.engine.enclave().lsm().get_metadata(prev_entry_id)
                {
                    let current_version: u32 = prev_entry.version.into();
                    prev_entry.is_latest = false;
                    prev_entry.updated_at = chrono::Utc::now().timestamp_millis().into();
                    if let Err(e) = self
                        .engine
                        .enclave()
                        .lsm()
                        .insert_metadata(prev_entry_id, &prev_entry)
                    {
                        warn!(
                            session = %sid,
                            prev_id = prev_entry_id,
                            error = %e,
                            "Dedup: failed to update version chain on previous entry"
                        );
                    }
                    debug!(
                        session = %sid,
                        prev_id = prev_entry_id,
                        version = current_version,
                        "Dedup: updated version chain on previous entry"
                    );
                }
                return Ok(());
            }

            // Store placeholder in cache; entry_id filled after indexing
            {
                let mut cache = self.dedup_cache.lock().await;
                cache.put(
                    content_hash,
                    DedupEntry {
                        _hash: content_hash,
                        stored_at: std::time::Instant::now(),
                        entry_id,
                    },
                );
            }
        }

        // Append to transcript
        self.engine
            .append_message(&sid, &agent_msg)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        // Generate embedding and index for semantic search
        if let Some(ref emb_service) = self.embedding_service {
            // Only embed meaningful content (skip very short or empty messages)
            if content.len() >= 3 {
                match emb_service.embed(&content).await {
                    Ok(embedding) => {
                        // Compute importance from message content
                        let mut importance: u8 = 5;
                        if content.len() > 500 {
                            importance = importance.saturating_add(1);
                        }
                        if content.contains('?') {
                            importance = importance.saturating_add(2);
                        }
                        // Commands: starts with / or contains imperative verbs
                        if content.starts_with('/')
                            || content.starts_with("please ")
                            || content.starts_with("Please ")
                            || content.starts_with("run ")
                            || content.starts_with("Run ")
                            || content.starts_with("do ")
                            || content.starts_with("Do ")
                            || content.starts_with("make ")
                            || content.starts_with("Make ")
                            || content.starts_with("create ")
                            || content.starts_with("Create ")
                            || content.starts_with("fix ")
                            || content.starts_with("Fix ")
                        {
                            importance = importance.saturating_add(2);
                        }
                        if content.len() < 50 {
                            importance = importance.saturating_sub(1);
                        }
                        // Clamp to valid range
                        let importance = importance.clamp(1, 10);

                        // Create a MemoryEntry for indexing
                        let entry = crate::models::MemoryEntry {
                            id: entry_id.into(),
                            session_id: sid.clone(),
                            created_at: chrono::Utc::now().timestamp_millis().into(),
                            updated_at: chrono::Utc::now().timestamp_millis().into(),
                            content: content.clone(),
                            category: "transcript".to_string(),
                            importance,
                            tags: vec![],
                            embedding,
                            shannon_entropy: crate::distillation::calculate_shannon_entropy(
                                &content,
                            )
                            .into(),
                            last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                            hit_count: 0.into(),
                            related_to: vec![],
                            // MEM-03: Access tracking
                            access_timestamps: vec![],
                            // MEM-08: Versioning
                            version: 1.into(),
                            parent_id: None,
                            supersedes: vec![],
                            is_latest: true,
                        };

                        if let Err(e) = self.engine.index_memory(entry).await {
                            warn!(
                                session = %sid,
                                error = %e,
                                "Failed to index message embedding"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            session = %sid,
                            error = %e,
                            "Failed to generate embedding for message"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn retrieve(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        let sid = savant_core::session::sanitize_session_id(agent_id)
            .unwrap_or_else(|| agent_id.to_string());
        let query_owned = query.to_string();
        let mut results: Vec<ChatMessage> = Vec::new();

        // CP-05: Semantic search is the PRIMARY retrieval path.
        // Look up matched MemoryEntry objects from LSM and convert to ChatMessage.
        // CP-01/CP-02: Update access tracking on every retrieved entry.
        if let Some(ref emb_service) = self.embedding_service {
            if !query_owned.is_empty() {
                // Try embedding with Ollama auto-start retry
                let embedding_result = match emb_service.embed(&query_owned).await {
                    Ok(emb) => Ok(emb),
                    Err(e) => {
                        warn!(
                            session = %sid,
                            error = %e,
                            "Embedding failed — attempting Ollama auto-start and retry"
                        );
                        if let Err(e) =
                            savant_core::utils::ollama_embeddings::auto_start_ollama().await
                        {
                            warn!(
                                session = %sid,
                                error = %e,
                                "Ollama auto-start failed during embedding retry"
                            );
                        }
                        emb_service.embed(&query_owned).await
                    }
                };

                if let Ok(query_embedding) = embedding_result {
                    // CP-08/09/10: Use hybrid search (BM25 + vector + RRF fusion)
                    if let Ok(search_results) = self
                        .engine
                        .enclave()
                        .hybrid_search(&query_owned, &query_embedding, limit)
                        .await
                    {
                        info!(
                            session = %sid,
                            results = search_results.len(),
                            "Hybrid search returned results"
                        );

                        // CP-05: Convert search results to ChatMessages via LSM lookup
                        for sr in &search_results {
                            if let Ok(memory_id) = sr.document_id.parse::<u64>() {
                                if let Ok(Some(mut entry)) =
                                    self.engine.enclave().lsm().get_metadata(memory_id)
                                {
                                    // CP-01/CP-02: Update access tracking
                                    let now_ts = chrono::Utc::now().timestamp();
                                    entry.last_accessed_at = (now_ts * 1000).into(); // millis to match field type
                                    let current_hits: u32 = entry.hit_count.into();
                                    entry.hit_count = (current_hits + 1).into();
                                    // Ring buffer, max 20 entries.
                                    // Use rotate_left(1) + replace last instead of remove(0)
                                    // to avoid O(n) shift on every access.
                                    if entry.access_timestamps.len() >= 20 {
                                        entry.access_timestamps.rotate_left(1);
                                        let last = entry.access_timestamps.len() - 1;
                                        entry.access_timestamps[last] = now_ts.into();
                                    } else {
                                        entry.access_timestamps.push(now_ts.into());
                                    }
                                    // Persist updated metadata (best effort)
                                    if let Err(e) = self
                                        .engine
                                        .enclave()
                                        .lsm()
                                        .insert_metadata(memory_id, &entry)
                                    {
                                        warn!(
                                            memory_id = memory_id,
                                            error = %e,
                                            "Failed to persist access tracking metadata"
                                        );
                                    }

                                    // Convert MemoryEntry to ChatMessage
                                    use savant_core::types::ChatRole;
                                    let role = match entry.category.as_str() {
                                        "user" => ChatRole::User,
                                        "assistant" => ChatRole::Assistant,
                                        "transcript" => ChatRole::Assistant,
                                        _ => ChatRole::System,
                                    };
                                    results.push(ChatMessage {
                                        role,
                                        content: entry.content.clone(),
                                        sender: None,
                                        recipient: None,
                                        agent_id: None,
                                        session_id: Some(savant_core::types::SessionId(
                                            entry.session_id.clone(),
                                        )),
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        is_telemetry: false,
                                        images: vec![],
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: if semantic search returned nothing, use transcript tail
        if results.is_empty() {
            let tail = self.engine.fetch_session_tail(&sid, limit * 2);
            let now = chrono::Utc::now().timestamp_millis();
            let lambda = self.engine.enclave().config.temporal_decay_lambda as f64;
            let min_relevance = 0.1_f64;
            results = tail
                .into_iter()
                .filter(|msg| {
                    if msg.timestamp <= 0 {
                        return true;
                    }
                    let age_hours = (now - msg.timestamp) as f64 / 3_600_000.0;
                    let decay = (-lambda * age_hours).exp();
                    decay >= min_relevance
                })
                .take(limit)
                .map(|msg| msg.to_chat())
                .collect();
        }

        // Substring filter when no embeddings available
        if !query_owned.is_empty() && self.embedding_service.is_none() {
            let query_lower = query_owned.to_lowercase();
            results.retain(|msg| msg.content.to_lowercase().contains(&query_lower));
        }

        Ok(results)
    }

    async fn consolidate(&self, agent_id: &str) -> Result<(), SavantError> {
        let sid = savant_core::session::sanitize_session_id(agent_id)
            .unwrap_or_else(|| agent_id.to_string());

        // Fetch session messages (up to 500 for consolidation)
        let messages = self.engine.fetch_session_tail(&sid, 500);

        if messages.len() < 50 {
            debug!(
                "Session {} has only {} messages, skipping consolidation",
                sid,
                messages.len()
            );
            return Ok(());
        }

        // Split into older (to consolidate) and recent (to keep as-is)
        let recent_count = self.engine.enclave().config.recent_message_count;
        let (to_consolidate, recent) = if messages.len() > recent_count {
            let split_idx = messages.len() - recent_count;
            let older = messages[..split_idx].to_vec();
            let newer = messages[split_idx..].to_vec();
            (older, newer)
        } else {
            return Ok(());
        };

        // Non-LLM consolidation: content-hash dedup
        // Normalize messages, hash content, remove duplicates (keep most recent)
        use std::collections::HashMap;
        let mut seen_hashes: HashMap<String, usize> = HashMap::new();
        let mut deduped: Vec<AgentMessage> = Vec::new();
        let mut duplicates_removed = 0;

        for msg in &to_consolidate {
            let normalized = msg
                .content
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let hash = {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(format!("{:?}", msg.role).as_bytes());
                hasher.update(b":");
                hasher.update(normalized.as_bytes());
                format!("{:x}", hasher.finalize())[..16].to_string()
            };

            if let Some(&idx) = seen_hashes.get(&hash) {
                // Duplicate found — keep the newer one
                deduped[idx] = msg.clone();
                duplicates_removed += 1;
            } else {
                seen_hashes.insert(hash, deduped.len());
                deduped.push(msg.clone());
            }
        }

        // Build summary from dedup results
        let summary = format!(
            "Conversation compacted: {} messages → {} unique ({} duplicates removed).",
            to_consolidate.len(),
            deduped.len(),
            duplicates_removed
        );
        let summary_id = uuid::Uuid::new_v4().to_string();
        let summary_msg = AgentMessage {
            id: summary_id.clone(),
            role: MessageRole::System,
            content: summary,
            session_id: sid.clone(),
            timestamp: chrono::Utc::now().timestamp_millis().into(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            parent_id: None,
            channel: "Chat".to_string(), // Summary stays in active context
        };

        // Link older messages to summary node (DAG architecture)
        let mut archived_older = Vec::new();
        for mut old_msg in deduped {
            old_msg.channel = "Archive".to_string();
            old_msg.parent_id = Some(summary_id.clone());
            archived_older.push(old_msg);
        }

        let mut updated_recent = recent;
        if let Some(first_recent) = updated_recent.first_mut() {
            // Link the active thread to the summary node
            first_recent.parent_id = Some(summary_id.clone());
        }

        // Combine archived data + new summary node + linked recent messages
        let mut compacted = Vec::new();
        compacted.extend(archived_older);
        compacted.push(summary_msg);
        compacted.extend(updated_recent);

        // Atomically compact the session
        self.engine
            .atomic_compact(&sid, compacted)
            .await
            .map_err(|e| SavantError::Unknown(format!("Compact failed: {}", e)))?;

        debug!("Consolidated session {}", sid);

        Ok(())
    }

    async fn get_or_create_session(
        &self,
        session_id: &str,
    ) -> Result<savant_core::types::SessionState, SavantError> {
        let sid = savant_core::session::sanitize_session_id(session_id)
            .unwrap_or_else(|| session_id.to_string());

        let state = self
            .engine
            .get_or_create_session_state(&sid)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        // FID-029 §Step 1: fetch title from the `session_titles` sibling collection.
        // Graceful degradation — title is metadata, not core state; failure
        // defaults to None rather than failing the entire session read
        // (LESSON-028 fix-forward 2026-07-15).
        let title = self
            .engine
            .load_session_title(&sid)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    session = %sid,
                    error = %e,
                    "load_session_title failed; defaulting title to None"
                );
                None
            });

        Ok(savant_core::types::SessionState {
            session_id: state.session_id,
            created_at: state.created_at.into(),
            last_active: state.last_active.into(),
            turn_count: state.turn_count.into(),
            active_turn_id: state.active_turn_id,
            auto_approved_tools: state.auto_approved_tools,
            denied_tools: state.denied_tools,
            parent_session_id: state.parent_session_id,
            fork_point_turn_id: state.fork_point_turn_id,
            title,
        })
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<savant_core::types::SessionState>, SavantError> {
        let sid = savant_core::session::sanitize_session_id(session_id)
            .unwrap_or_else(|| session_id.to_string());

        match self
            .engine
            .get_session_state(&sid)
            .map_err(|e| SavantError::Unknown(e.to_string()))?
        {
            Some(state) => {
                // FID-029 §Step 1: fetch title from the `session_titles` sibling collection.
                // Graceful degradation — title is metadata, not core state; failure
                // defaults to None rather than failing the entire session read
                // (LESSON-028 fix-forward 2026-07-15).
                let title = self
                    .engine
                    .load_session_title(&sid)
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            session = %sid,
                            error = %e,
                            "load_session_title failed; defaulting title to None"
                        );
                        None
                    });
                Ok(Some(savant_core::types::SessionState {
                    session_id: state.session_id,
                    created_at: state.created_at.into(),
                    last_active: state.last_active.into(),
                    turn_count: state.turn_count.into(),
                    active_turn_id: state.active_turn_id,
                    auto_approved_tools: state.auto_approved_tools,
                    denied_tools: state.denied_tools,
                    parent_session_id: state.parent_session_id,
                    fork_point_turn_id: state.fork_point_turn_id,
                    title,
                }))
            }
            None => Ok(None),
        }
    }

    async fn save_session(
        &self,
        state: &savant_core::types::SessionState,
    ) -> Result<(), SavantError> {
        let rkyv_state = crate::models::SessionState {
            session_id: state.session_id.clone(),
            created_at: state.created_at.into(),
            last_active: state.last_active.into(),
            turn_count: state.turn_count.into(),
            active_turn_id: state.active_turn_id.clone(),
            auto_approved_tools: state.auto_approved_tools.clone(),
            denied_tools: state.denied_tools.clone(),
            parent_session_id: state.parent_session_id.clone(),
            fork_point_turn_id: state.fork_point_turn_id.clone(),
        };

        self.engine
            .save_session_state(&rkyv_state)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        // FID-029 §Step 1: persist title to the `session_titles` sibling collection.
        // Graceful degradation — title-save failure does NOT block the session
        // save (LESSON-028 fix-forward 2026-07-15).
        if let Err(e) = self
            .engine
            .save_session_title(&state.session_id, state.title.as_deref())
            .await
        {
            tracing::warn!(
                session = %state.session_id,
                error = %e,
                "save_session_title failed; session saved without title"
            );
        }

        Ok(())
    }

    async fn save_turn(&self, turn: &savant_core::types::TurnState) -> Result<(), SavantError> {
        let phase = match turn.state {
            savant_core::types::TurnPhase::Processing => crate::models::TurnPhase::Processing,
            savant_core::types::TurnPhase::Completed => crate::models::TurnPhase::Completed,
            savant_core::types::TurnPhase::Failed => crate::models::TurnPhase::Failed,
            savant_core::types::TurnPhase::Interrupted => crate::models::TurnPhase::Interrupted,
            savant_core::types::TurnPhase::AwaitingApproval => {
                crate::models::TurnPhase::AwaitingApproval
            }
        };

        let rkyv_turn = crate::models::TurnState {
            turn_id: turn.turn_id.clone(),
            session_id: turn.session_id.clone(),
            state: phase,
            tool_calls_made: turn.tool_calls_made.clone(),
            started_at: turn.started_at.into(),
            completed_at: turn.completed_at.into(),
        };

        self.engine
            .save_turn_state(&rkyv_turn)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    async fn get_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<savant_core::types::TurnState>, SavantError> {
        let sid = savant_core::session::sanitize_session_id(session_id)
            .unwrap_or_else(|| session_id.to_string());

        match self
            .engine
            .get_turn_state(&sid, turn_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))?
        {
            Some(turn) => {
                let phase = match turn.state {
                    crate::models::TurnPhase::Processing => {
                        savant_core::types::TurnPhase::Processing
                    }
                    crate::models::TurnPhase::Completed => savant_core::types::TurnPhase::Completed,
                    crate::models::TurnPhase::Failed => savant_core::types::TurnPhase::Failed,
                    crate::models::TurnPhase::Interrupted => {
                        savant_core::types::TurnPhase::Interrupted
                    }
                    crate::models::TurnPhase::AwaitingApproval => {
                        savant_core::types::TurnPhase::AwaitingApproval
                    }
                };

                Ok(Some(savant_core::types::TurnState {
                    turn_id: turn.turn_id,
                    session_id: turn.session_id,
                    state: phase,
                    tool_calls_made: turn.tool_calls_made,
                    started_at: turn.started_at.into(),
                    completed_at: turn.completed_at.into(),
                }))
            }
            None => Ok(None),
        }
    }

    async fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<savant_core::types::TurnState>, SavantError> {
        let sid = savant_core::session::sanitize_session_id(session_id)
            .unwrap_or_else(|| session_id.to_string());

        let turns = self
            .engine
            .fetch_recent_turns(&sid, limit)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(turns
            .into_iter()
            .map(|t| {
                let phase = match t.state {
                    crate::models::TurnPhase::Processing => {
                        savant_core::types::TurnPhase::Processing
                    }
                    crate::models::TurnPhase::Completed => savant_core::types::TurnPhase::Completed,
                    crate::models::TurnPhase::Failed => savant_core::types::TurnPhase::Failed,
                    crate::models::TurnPhase::Interrupted => {
                        savant_core::types::TurnPhase::Interrupted
                    }
                    crate::models::TurnPhase::AwaitingApproval => {
                        savant_core::types::TurnPhase::AwaitingApproval
                    }
                };
                savant_core::types::TurnState {
                    turn_id: t.turn_id,
                    session_id: t.session_id,
                    state: phase,
                    tool_calls_made: t.tool_calls_made,
                    started_at: t.started_at.into(),
                    completed_at: t.completed_at.into(),
                }
            })
            .collect())
    }

    async fn run_promotion_cycle(&self, _agent_id: &str) -> Result<(), SavantError> {
        self.engine.enclave().run_promotion_cycle().await;
        Ok(())
    }

    async fn synthesize_lessons(&self, agent_id: &str) -> Result<(), SavantError> {
        self.engine.synthesize_lessons(agent_id).await;
        Ok(())
    }

    async fn synthesize_insights(&self, _agent_id: &str) -> Result<(), SavantError> {
        self.engine.synthesize_insights().await;
        Ok(())
    }

    async fn get_lessons_context(&self) -> String {
        let guard = self.engine.enclave().get_lessons_vec().await;
        if guard.is_empty() {
            return String::new();
        }
        let lines: Vec<String> = guard
            .iter()
            .map(|l| {
                format!(
                    "- [conf:{:.2}, reinforced:{}] {}",
                    l.confidence, l.reinforcements, l.content
                )
            })
            .collect();
        format!(
            "<SYNTHESIZED_LESSONS>\n{}\n</SYNTHESIZED_LESSONS>",
            lines.join("\n")
        )
    }

    async fn get_insights_context(&self) -> String {
        let guard = self.engine.enclave().get_insights_vec().await;
        if guard.is_empty() {
            return String::new();
        }
        let lines: Vec<String> = guard
            .iter()
            .map(|i| {
                format!(
                    "- [conf:{:.2}, cat:{}] {}",
                    i.confidence, i.category, i.content
                )
            })
            .collect();
        format!(
            "<SYNTHESIZED_INSIGHTS>\n{}\n</SYNTHESIZED_INSIGHTS>",
            lines.join("\n")
        )
    }

    async fn restore_state(&self, _agent_id: &str) -> Result<(), SavantError> {
        let snapshot_dir = std::path::PathBuf::from("data/snapshots");
        if snapshot_dir.exists() {
            if let Err(e) = self.engine.restore_state(&snapshot_dir) {
                tracing::warn!("[memory] Failed to restore state: {}", e);
            }
        }
        Ok(())
    }

    async fn auto_recall(
        &self,
        agent_id: &str,
        query: &str,
    ) -> Result<Vec<savant_core::types::ChatMessage>, SavantError> {
        let config = AutoRecallConfig::default();
        let block = self.auto_recall(agent_id, query, config).await?;
        Ok(block
            .retrieved_memories
            .into_iter()
            .map(|m| {
                savant_core::types::ChatMessage::new(
                    savant_core::types::ChatRole::System,
                    m.content,
                )
            })
            .collect())
    }
}

impl AsyncMemoryBackend {
    pub async fn delete_memory(&self, id: u64) -> Result<(), SavantError> {
        self.engine
            .delete_memory(id)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    pub async fn delete_session(&self, agent_id: &str) -> Result<(), SavantError> {
        let sid = savant_core::session::sanitize_session_id(agent_id)
            .unwrap_or_else(|| agent_id.to_string());

        self.engine
            .delete_session(&sid)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    // ─── FID-029 §Step 9 backend extensions ─────────────────────────────
    //
    // Renderer-side IPC commands (`src-tauri/src/chat_persistence.rs`)
    // call these methods. They compose ONLY existing engine primitives
    // per ECHO Law 7 (search-for-existing BEFORE creating new):
    //   - `iter_session_titles()` for cross-session enumeration
    //   - `hydrate_session()`     for per-session message reads
    //   - `delete_session()`      for surgical session removal

    /// Lists all chat session IDs by enumerating the `session_titles`
    /// sibling collection (FID-029 §Step 1 pattern, generalized for list).
    pub fn list_chat_sessions(&self) -> Result<Vec<String>, SavantError> {
        let titles = self
            .engine
            .iter_session_titles()
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        Ok(titles.into_keys().collect())
    }

    /// Cross-session substring search. Iterates all known sessions,
    /// hydrates each (bounded to `PER_SESSION_BUDGET` messages per
    /// session), and applies a case-insensitive substring filter on the
    /// message content. Stops once the global `limit` is reached.
    pub async fn search_chat_history(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<savant_core::types::ChatMessage>, SavantError> {
        use savant_core::types::{
            AgentOutputChannel, ChatMessage, ChatRole, SessionId,
        };

        const PER_SESSION_BUDGET: usize = 200;

        let query_lower = query.to_lowercase();
        if query_lower.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let titles = self
            .engine
            .iter_session_titles()
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        let mut hits: Vec<ChatMessage> = Vec::new();
        for sid in titles.keys() {
            let msgs = self
                .engine
                .hydrate_session(sid, PER_SESSION_BUDGET)
                .map_err(|e| SavantError::Unknown(e.to_string()))?;
            for m in msgs {
                if !m.content.to_lowercase().contains(&query_lower) {
                    continue;
                }
                if hits.len() >= limit {
                    return Ok(hits);
                }
                hits.push(ChatMessage {
                    role: match m.role {
                        crate::models::MessageRole::User => ChatRole::User,
                        crate::models::MessageRole::Assistant => ChatRole::Assistant,
                        _ => ChatRole::System,
                    },
                    content: m.content.clone(),
                    sender: None,
                    recipient: None,
                    agent_id: None,
                    session_id: Some(SessionId(m.session_id.clone())),
                    channel: AgentOutputChannel::Chat,
                    is_telemetry: false,
                    images: Vec::new(),
                    ..Default::default()
                });
            }
        }
        Ok(hits)
    }

    /// Counts the number of messages in a session.
    pub fn count_session_messages(&self, session_id: &str) -> Result<u64, SavantError> {
        self.engine
            .count_session_messages(session_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Fetches all message IDs for a session.
    pub fn fetch_all_message_ids_for_session(&self, session_id: &str) -> Vec<String> {
        self.engine.fetch_all_message_ids_for_session(session_id)
    }

    /// Fetches a message by its ID across all sessions.
    pub fn fetch_message_by_id(&self, msg_id: &str) -> Result<Option<AgentMessage>, SavantError> {
        self.engine
            .fetch_message_by_id(msg_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Subscribes to memory notifications for high-importance discoveries.
    pub fn subscribe_notifications(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::notifications::MemoryNotification> {
        self.engine.subscribe_notifications()
    }

    /// Returns the current notification subscriber count.
    pub fn notification_subscriber_count(&self) -> usize {
        self.engine.notification_subscriber_count()
    }

    /// Cull low-entropy memories below the given threshold.
    pub fn cull_low_entropy_memories(&self, threshold: f32) -> Result<usize, SavantError> {
        self.engine
            .cull_low_entropy_memories(threshold)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Hydrates a session from persistent storage.
    pub fn hydrate_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<AgentMessage>, SavantError> {
        self.engine
            .hydrate_session(session_id, limit)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Temporal semantic search.
    pub fn semantic_search_temporal(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, SavantError> {
        self.engine
            .semantic_search_temporal(query_embedding, top_k)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Temporal semantic search with decay.
    pub fn semantic_search_temporal_decay(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        lambda: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, SavantError> {
        self.engine
            .semantic_search_temporal_decay(query_embedding, top_k, lambda)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Returns all vectors within `max_distance` of the query embedding.
    pub fn recall_within_distance(
        &self,
        query_embedding: &[f32],
        max_distance: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, SavantError> {
        self.engine
            .recall_within_distance(query_embedding, max_distance)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Journals a task state transition to the persistent WAL.
    pub fn journal_task_state(
        &self,
        task_id: &str,
        new_state: savant_ipc::a2a::protocol::TaskState,
    ) -> Result<(), SavantError> {
        self.engine
            .enclave()
            .lsm()
            .journal_task_state(task_id, new_state)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Recovers interrupted delegations from the task state journal.
    pub fn recover_interrupted_delegations(
        &self,
    ) -> Result<Vec<(String, savant_ipc::a2a::protocol::TaskState)>, SavantError> {
        self.engine
            .enclave()
            .lsm()
            .recover_interrupted_delegations()
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Stores temporal metadata for bi-temporal tracking.
    pub fn store_temporal_metadata(
        &self,
        temporal: &crate::models::TemporalMetadata,
    ) -> Result<(), SavantError> {
        self.engine
            .enclave()
            .lsm()
            .store_temporal_metadata(temporal)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Finds active temporal metadata by entity.
    pub fn find_active_temporal_by_entity(
        &self,
        entity: &str,
    ) -> Result<Vec<crate::models::TemporalMetadata>, SavantError> {
        self.engine
            .enclave()
            .lsm()
            .find_active_temporal_by_entity(entity)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Stores a DAG node for reversible session compaction.
    pub fn store_dag_node(&self, node: &crate::models::DagNode) -> Result<(), SavantError> {
        self.engine
            .enclave()
            .lsm()
            .store_dag_node(node)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Loads a DAG node by ID.
    pub fn load_dag_node(
        &self,
        node_id: &str,
    ) -> Result<Option<crate::models::DagNode>, SavantError> {
        self.engine
            .enclave()
            .lsm()
            .load_dag_node(node_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Flushes pending writes to disk.
    pub fn flush(&self) -> Result<(), SavantError> {
        self.engine
            .enclave()
            .lsm()
            .flush()
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    /// Auto-recall: searches memory for relevant context and returns a cache block.
    ///
    /// This method:
    /// 1. Extracts the last 3 user messages as a query window
    /// 2. Embeds the query using the EmbeddingService
    /// 3. Performs semantic search against the vector index
    /// 4. Filters by similarity threshold and token budget
    /// 5. Returns a ContextCacheBlock for injection into the system prompt
    ///
    /// # Arguments
    /// * `agent_id` — The agent/session ID
    /// * `query_text` — The current user query
    /// * `config` — AutoRecallConfig with thresholds and limits
    pub async fn auto_recall(
        &self,
        agent_id: &str,
        query_text: &str,
        config: AutoRecallConfig,
    ) -> Result<ContextCacheBlock, SavantError> {
        let sid = agent_id.to_string();
        let query_owned = query_text.to_string();

        let mut block = ContextCacheBlock {
            query_intent: query_owned.clone(),
            retrieved_memories: Vec::new(),
            injected_at: savant_core::utils::time::now_millis().unwrap_or_else(|e| {
                tracing::warn!("Failed to get current time: {}, using 0", e);
                0
            }) as i64,
            estimated_tokens: 0,
        };

        // Skip if no embedding service
        let emb_service = match self.embedding_service {
            Some(ref s) => s,
            None => return Ok(block),
        };

        // Skip if query is empty
        if query_owned.is_empty() {
            return Ok(block);
        }

        // Extract last 3 user messages as query window for better context
        let tail = self.engine.fetch_session_tail(&sid, 10);
        let user_msgs: Vec<&str> = tail
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .take(3)
            .map(|m| m.content.as_str())
            .collect();

        let query_window = if user_msgs.is_empty() {
            query_owned.clone()
        } else {
            user_msgs.join(" | ")
        };

        // Embed the query window
        let embedding = match emb_service.embed(&query_window).await {
            Ok(e) => e,
            Err(e) => {
                warn!("Auto-recall: failed to embed query: {}", e);
                return Ok(block);
            }
        };

        // Hybrid search (BM25 + vector + graph fused via RRF)
        let search_results = match self
            .engine
            .enclave()
            .hybrid_search(&query_owned, &embedding, config.max_results)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                debug!("Auto-recall: hybrid search failed: {}", e);
                return Ok(block);
            }
        };

        // Filter by similarity threshold and build context block
        let mut token_estimate = 0usize;
        for result in search_results {
            if result.score < config.similarity_threshold {
                continue;
            }

            // Estimate tokens for this memory (4 chars ≈ 1 token)
            let memory_tokens =
                (result.document_id.len() + result.score.to_string().len() + 50) / 4;
            token_estimate += memory_tokens;

            if token_estimate > config.max_tokens {
                break;
            }

            // Create a lightweight MemoryEntry from the search result
            let entry = crate::models::MemoryEntry {
                id: 0.into(),
                session_id: sid.clone(),
                category: "semantic_recall".to_string(),
                content: format!(
                    "Recalled memory (similarity: {:.2}): {}",
                    result.score, result.document_id
                ),
                importance: 5,
                tags: vec!["auto_recall".to_string()],
                embedding: vec![],
                created_at: chrono::Utc::now().timestamp_millis().into(),
                updated_at: chrono::Utc::now().timestamp_millis().into(),
                shannon_entropy: 0.0.into(),
                last_accessed_at: chrono::Utc::now().timestamp_millis().into(),
                hit_count: 0.into(),
                related_to: vec![],
                access_timestamps: vec![],
                version: 1.into(),
                parent_id: None,
                supersedes: vec![],
                is_latest: true,
            };

            block.retrieved_memories.push(entry);

            if block.retrieved_memories.len() >= config.max_results {
                break;
            }
        }

        block.estimated_tokens = token_estimate;

        if !block.retrieved_memories.is_empty() {
            info!(
                session = %sid,
                memories = block.retrieved_memories.len(),
                tokens = token_estimate,
                "Auto-recall: injected context from memory"
            );
        }

        Ok(block)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::error::SavantError;
    use savant_core::traits::EmbeddingProvider;
    use savant_core::types::{AgentOutputChannel, ChatRole, SessionId};

    /// Mock embedding provider for tests — returns fixed 768-dim zero vectors.
    struct MockEmbeddingProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, SavantError> {
            Ok(vec![0.0; 768])
        }
        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError> {
            Ok(texts.iter().map(|_| vec![0.0; 64]).collect())
        }
        fn dimensions(&self) -> usize {
            64
        }
    }

    fn mock_engine(dir: &std::path::Path) -> Arc<MemoryEngine> {
        use crate::engine::EngineConfig;
        use crate::lsm_engine::LsmConfig;
        use crate::vector_engine::VectorConfig;
        MemoryEngine::new(
            dir,
            EngineConfig {
                lsm_config: LsmConfig {
                    vector_dimension: 64,
                    ..LsmConfig::default()
                },
                vector_config: VectorConfig {
                    dimensions: 64,
                    ..VectorConfig::default()
                },
                distill_llm_provider: None,
                distill_params: None,
                embedding_service: Arc::new(MockEmbeddingProvider),
                memory_config: crate::models::MemoryConfig::default(),
                personality: None,
            },
        )
        .expect("Failed to init engine")
    }

    #[tokio::test]
    async fn test_async_backend_store_and_retrieve() {
        let temp_dir = std::env::temp_dir().join(format!(
            "savant_async_backend_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let engine = mock_engine(&temp_dir);
        let backend = AsyncMemoryBackend::new(engine);

        let chat_msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: "Test message".to_string(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: Some(SessionId("test_session".to_string())),
            channel: AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        };

        // Store
        backend.store("test_session", &chat_msg).await.unwrap();

        // Retrieve
        let retrieved = backend.retrieve("test_session", "", 10).await.unwrap();
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].content, "Test message");

        // Cleanup
        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[tokio::test]
    async fn test_async_backend_retrieve_with_query() {
        let temp_dir =
            std::env::temp_dir().join(format!("savant_async_query_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let engine = mock_engine(&temp_dir);
        let backend = AsyncMemoryBackend::new(engine);

        // Store multiple messages
        for content in &["hello world", "foo bar", "hello there"] {
            let msg = ChatMessage {
                is_telemetry: false,
                role: ChatRole::User,
                content: content.to_string(),
                sender: None,
                recipient: None,
                agent_id: None,
                session_id: Some(SessionId("query_session".to_string())),
                channel: AgentOutputChannel::Chat,
                images: Vec::new(),
                ..Default::default()
            };
            backend.store("query_session", &msg).await.unwrap();
        }

        // Retrieve with query filter (substring match since no embeddings)
        let results = backend
            .retrieve("query_session", "hello", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 2); // "hello world" and "hello there"

        // Retrieve with no filter
        let all = backend.retrieve("query_session", "", 10).await.unwrap();
        assert_eq!(all.len(), 3);

        // Cleanup
        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[tokio::test]
    async fn test_async_backend_has_embeddings_flag() {
        let temp_dir =
            std::env::temp_dir().join(format!("savant_async_emb_flag_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let engine = mock_engine(&temp_dir);

        let backend_no_emb = AsyncMemoryBackend::new(engine.clone());
        assert!(!backend_no_emb.has_embeddings());

        // Cleanup
        std::fs::remove_dir_all(temp_dir).ok();
    }
}
