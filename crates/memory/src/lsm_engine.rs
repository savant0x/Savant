//! CortexaDB Storage Engine
//!
//! This module provides a transactional storage backend using CortexaDB,
//! a vector + graph embedded database designed for AI agent memory.
//! It replaces the previous Fjall LSM-tree implementation.
//!
//! Key features:
//! - Collection-based partitioning (one per session)
//! - Vector search capabilities for semantic retrieval
//! - Graph relationships for DAG compaction
//! - WAL-backed hard durability
//! - Zero-copy reads using rkyv where applicable

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

use cortexadb_core::{BatchRecord, CortexaDB};
use rkyv::rancor::Error as RkyvError;

use crate::error::MemoryError;
use crate::models::{verify_tool_pair_integrity, AgentMessage};

/// Vector dimension for CortexaDB embeddings (default fallback only).
/// Actual dimension should come from LsmConfig.vector_dimension.
/// Must match OllamaEmbeddingService::dimensions() (768 for nomic-embed-text).
const DEFAULT_VECTOR_DIM: usize = 768;

/// Maximum entries to retrieve per collection query.
const MAX_BATCH_SIZE: usize = 100_000;

/// Creates metadata HashMap with a key field.
fn make_meta(key: &str, timestamp: i64) -> Option<HashMap<String, String>> {
    let mut m = HashMap::new();
    m.insert("key".to_string(), key.to_string());
    m.insert("timestamp".to_string(), timestamp.to_string());
    Some(m)
}

/// Creates metadata HashMap with just a key field.
fn make_key_meta(key: &str) -> Option<HashMap<String, String>> {
    let mut m = HashMap::new();
    m.insert("key".to_string(), key.to_string());
    Some(m)
}

/// Statistics about the storage engine (for monitoring)
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub total_messages: u64,
    pub total_sessions: u64,
    pub disk_usage_bytes: u64,
    pub cache_hit_rate: f32,
}

/// The core storage engine backed by CortexaDB.
///
/// This engine uses CortexaDB collections for partitioning:
/// - `transcript.{session_id}` — conversation transcripts
/// - `metadata` — semantic metadata entries
/// - `temporal` — bi-temporal metadata
/// - `dag` — DAG compaction nodes
/// - `distillation` — distillation state flags
/// - `facts` — semantic facts (SPO triples)
pub struct LsmStorageEngine {
    db: CortexaDB,
    /// Known session IDs (maintained in memory for iteration).
    sessions: dashmap::DashSet<String>,
    /// Configured vector dimension for zero-embedding construction.
    vector_dimension: usize,
    /// Message ID -> session ID index for O(1) lookups.
    /// Updated on append_message and atomic_compact.
    message_session_index: dashmap::DashMap<String, String>,
}

/// Configuration for the CortexaDB storage engine.
#[derive(Debug, Clone)]
pub struct LsmConfig {
    /// Vector dimension for embeddings (default: 768)
    pub vector_dimension: usize,
    /// Sync policy: true = sync after every write, false = async (default: true)
    pub strict_sync: bool,
    /// Checkpoint interval in operations (default: 1000)
    pub checkpoint_every_ops: usize,
}

impl Default for LsmConfig {
    fn default() -> Self {
        Self {
            vector_dimension: DEFAULT_VECTOR_DIM,
            strict_sync: true,
            checkpoint_every_ops: 1000,
        }
    }
}

/// Lazy iterator over all messages across sessions.
///
/// Messages are collected and sorted by timestamp on construction,
/// then yielded one at a time via `next()`.
pub struct AllMessagesIterator {
    messages: Vec<AgentMessage>,
    index: usize,
}

impl Iterator for AllMessagesIterator {
    type Item = AgentMessage;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.messages.len() {
            let msg = self.messages[self.index].clone();
            self.index += 1;
            Some(msg)
        } else {
            None
        }
    }
}

impl LsmStorageEngine {
    /// Initializes the CortexaDB storage engine.
    pub fn new(storage_path: &Path, config: LsmConfig) -> Result<Arc<Self>, MemoryError> {
        info!(
            "Initializing CortexaDB at {:?} (dim={}, sync={})",
            storage_path, config.vector_dimension, config.strict_sync
        );

        let path_str = storage_path.to_str().ok_or_else(|| {
            MemoryError::InitFailed("Database path is not valid UTF-8".to_string())
        })?;

        let db = CortexaDB::open(path_str, config.vector_dimension)
            .map_err(|e| MemoryError::InitFailed(format!("CortexaDB open failed: {}", e)))?;

        // Rebuild sessions set from the session registry
        let sessions = dashmap::DashSet::new();
        let vector_dimension = config.vector_dimension;
        if let Ok(hits) = db.search_in_collection(
            "_registry",
            vec![0.0; vector_dimension],
            MAX_BATCH_SIZE,
            None,
        ) {
            for hit in hits {
                if let Ok(memory) = db.get_memory(hit.id) {
                    if let Some(session_id) = memory.metadata.get("session_id") {
                        sessions.insert(session_id.clone());
                    }
                }
            }
        }

        info!(
            "CortexaDB Engine initialized with {} known sessions",
            sessions.len()
        );

        Ok(Arc::new(Self {
            db,
            sessions,
            vector_dimension,
            message_session_index: dashmap::DashMap::new(),
        }))
    }

    /// Convenience: Create with default configuration.
    pub fn with_defaults(storage_path: &Path) -> Result<Arc<Self>, MemoryError> {
        Self::new(storage_path, LsmConfig::default())
    }

    /// Creates a minimal-value placeholder embedding at the configured dimension.
    /// Uses 0.001 per dimension to avoid `VectorError::ZeroVector` in the search
    /// layer while remaining a distinguishable placeholder for missing embeddings.
    pub fn zero_embedding(&self) -> Vec<f32> {
        vec![0.001; self.vector_dimension]
    }

    /// Returns the collection name for a session transcript.
    fn transcript_collection(session_id: &str) -> String {
        format!("transcript.{}", session_id)
    }

    /// Appends a single message to the session transcript.
    #[instrument(skip(self, message), fields(session = %session_id, msg_id = %message.id))]
    pub fn append_message(
        &self,
        session_id: &str,
        message: &AgentMessage,
    ) -> Result<(), MemoryError> {
        let bytes = rkyv::to_bytes::<RkyvError>(message)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

        let collection = Self::transcript_collection(session_id);
        let timestamp: i64 = message.timestamp.into();
        let key = crate::models::message_key(session_id, timestamp, &message.id);

        self.db
            .add_with_content(
                &collection,
                bytes.to_vec(),
                self.zero_embedding(),
                make_meta(&key, timestamp),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        // Register session in the registry for persistence across restarts
        if self.sessions.insert(session_id.to_string()) {
            let mut reg_meta = HashMap::new();
            reg_meta.insert("session_id".to_string(), session_id.to_string());
            if let Err(e) = self.db.add_with_content(
                "_registry",
                session_id.as_bytes().to_vec(),
                self.zero_embedding(),
                Some(reg_meta),
            ) {
                warn!(
                    "[memory::lsm] Failed to register session {}: {}",
                    session_id, e
                );
            }
        }

        // Update message ID -> session ID index for O(1) lookup
        self.message_session_index
            .insert(message.id.clone(), session_id.to_string());

        debug!("Appended message {} to session {}", message.id, session_id);
        Ok(())
    }

    /// Iterates over messages across all sessions, with optional limit.
    /// Uses MAX_BATCH_SIZE as the cap per session scoped query to bound memory usage.
    ///
    /// Returns a lazy iterator that yields messages one at a time from a pre-sorted
    /// internal buffer. The full message set is collected and sorted on first call,
    /// then yielded incrementally to avoid holding all messages in scope simultaneously.
    pub fn iter_all_messages(&self, limit: usize) -> AllMessagesIterator {
        let mut all_msgs: Vec<AgentMessage> = Vec::new();
        let batch_limit = if limit > 0 { limit } else { MAX_BATCH_SIZE };

        for session_ref in self.sessions.iter() {
            let session_id = session_ref.key().clone();
            let collection = Self::transcript_collection(&session_id);

            if let Ok(hits) =
                self.db
                    .search_in_collection(&collection, self.zero_embedding(), batch_limit, None)
            {
                for hit in hits {
                    if all_msgs.len() >= batch_limit {
                        break;
                    }
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        if memory.content.len() <= 10 * 1024 * 1024 {
                            if let Ok(archived) = rkyv::access::<
                                rkyv::Archived<AgentMessage>,
                                rkyv::rancor::Error,
                            >(&memory.content)
                            {
                                if let Ok(msg) =
                                    rkyv::deserialize::<AgentMessage, rkyv::rancor::Error>(archived)
                                {
                                    all_msgs.push(msg);
                                }
                            }
                        }
                    }
                }
            }
            if all_msgs.len() >= batch_limit {
                break;
            }
        }

        all_msgs.sort_by_key(|m| i64::from(m.timestamp));
        AllMessagesIterator {
            messages: all_msgs,
            index: 0,
        }
    }

    /// Iterates over messages from the last N hours across all sessions.
    /// Used by the NREM dream phase for structured memory replay.
    pub fn iter_recent_messages(&self, hours: u64) -> Vec<AgentMessage> {
        let cutoff = chrono::Utc::now().timestamp() - (hours as i64 * 3600);
        self.iter_all_messages(MAX_BATCH_SIZE)
            .filter(|msg| i64::from(msg.timestamp) >= cutoff)
            .collect()
    }

    /// Inserts a MemoryEntry into the metadata collection.
    pub fn insert_metadata(
        &self,
        id: u64,
        entry: &crate::models::MemoryEntry,
    ) -> Result<(), MemoryError> {
        let bytes = rkyv::to_bytes::<RkyvError>(entry)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        let key = format!("meta:{}", id);

        self.db
            .add_with_content(
                "metadata",
                bytes.to_vec(),
                self.zero_embedding(),
                make_key_meta(&key),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Removes a MemoryEntry from the metadata collection.
    pub fn remove_metadata(&self, id: u64) -> Result<(), MemoryError> {
        let key = format!("meta:{}", id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        match self
            .db
            .search_in_collection("metadata", self.zero_embedding(), 1, Some(filter))
        {
            Ok(hits) => {
                for hit in hits {
                    self.db
                        .delete(hit.id)
                        .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] remove_metadata failed: {}", e);
            }
        }
        Ok(())
    }

    /// Fetches the tail of a session's conversation history.
    #[instrument(skip(self), fields(session = %session_id, limit))]
    pub fn fetch_session_tail(&self, session_id: &str, limit: usize) -> Vec<AgentMessage> {
        let collection = Self::transcript_collection(session_id);
        let mut messages = Vec::new();
        let mut validation_failures = 0;

        let hits = match self.db.search_in_collection(
            &collection,
            self.zero_embedding(),
            MAX_BATCH_SIZE,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query messages, returning partial results");
                return messages;
            }
        };

        for hit in hits {
            let memory = match self.db.get_memory(hit.id) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(id = %hit.id, error = %e, "Failed to get memory, skipping");
                    continue;
                }
            };

            if memory.content.len() > 10 * 1024 * 1024 {
                warn!("Oversized message detected: {} bytes", memory.content.len());
                continue;
            }

            let archived = match rkyv::access::<rkyv::Archived<AgentMessage>, rkyv::rancor::Error>(
                &memory.content,
            ) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        session = %memory.metadata.get("session_id").map(|s| s.as_str()).unwrap_or("unknown"),
                        error = %e,
                        "Skipping corrupt message (rkyv access failed)"
                    );
                    validation_failures += 1;
                    continue;
                }
            };

            match rkyv::deserialize::<AgentMessage, rkyv::rancor::Error>(archived) {
                Ok(msg) => {
                    if msg.channel != "Archive" {
                        messages.push(msg);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        session = %memory.metadata.get("session_id").map(|s| s.as_str()).unwrap_or("unknown"),
                        error = %e,
                        "Skipping corrupt message (rkyv deserialize failed)"
                    );
                    validation_failures += 1;
                }
            }
        }

        // Sort by timestamp ascending, then take last N and reverse for newest-first
        messages.sort_by_key(|m| i64::from(m.timestamp));
        if messages.len() > limit {
            messages = messages.split_off(messages.len() - limit);
        }
        messages.reverse();

        if validation_failures > 0 {
            debug!(
                "Skipped {} invalid entries for session '{}'",
                validation_failures, session_id
            );
        }

        debug!(
            "Fetched {} messages for session {}",
            messages.len(),
            session_id
        );
        messages
    }

    /// Atomically compacts a batch of messages into the database.
    #[instrument(skip(self, batch), fields(session = %session_id, batch_size = batch.len()))]
    pub fn atomic_compact(
        &self,
        session_id: &str,
        batch: Vec<AgentMessage>,
    ) -> Result<(), MemoryError> {
        if batch.is_empty() {
            return Ok(());
        }

        verify_tool_pair_integrity(&batch)?;

        let collection = Self::transcript_collection(session_id);

        // Phase 0: Capture all existing entry IDs BEFORE inserting the new batch.
        // We will only delete these old entries in Phase 2, preserving the newly
        // inserted batch from being deleted by its own compaction.
        let mut old_hit_ids: Vec<u64> = Vec::new();
        if let Ok(hits) =
            self.db
                .search_in_collection(&collection, self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            for hit in hits {
                old_hit_ids.push(hit.id);
            }
        }

        // Phase 1: Insert compacted batch FIRST (write-before-delete ensures data safety)
        // If this fails, old entries remain intact — no data loss.
        let mut records: Vec<BatchRecord> = Vec::with_capacity(batch.len());
        for msg in &batch {
            let bytes = rkyv::to_bytes::<RkyvError>(msg)
                .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
            let timestamp: i64 = msg.timestamp.into();
            let key = crate::models::message_key(session_id, timestamp, &msg.id);
            let mut meta = HashMap::new();
            meta.insert("key".to_string(), key);
            meta.insert("timestamp".to_string(), timestamp.to_string());
            records.push(BatchRecord {
                collection: collection.clone(),
                content: bytes.to_vec(),
                embedding: Some(self.zero_embedding()),
                metadata: Some(meta),
            });
        }

        self.db
            .add_batch(records)
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        // Phase 2: Delete ONLY the old entries captured in Phase 0.
        // This avoids deleting the newly-inserted batch entries.
        // If this fails, duplicates exist temporarily but next compaction cleans them up.
        for old_id in old_hit_ids {
            self.db
                .delete(old_id)
                .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        }

        // Update message ID -> session ID index for the newly inserted batch
        for msg in &batch {
            self.message_session_index
                .insert(msg.id.clone(), session_id.to_string());
        }

        info!(
            session = %session_id,
            inserted = batch.len(),
            "Atomic compaction succeeded"
        );
        Ok(())
    }

    /// Counts the number of messages in a session.
    pub fn count_session_messages(&self, session_id: &str) -> Result<u64, MemoryError> {
        let collection = Self::transcript_collection(session_id);
        let hits = self
            .db
            .search_in_collection(&collection, self.zero_embedding(), MAX_BATCH_SIZE, None)
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(hits.len() as u64)
    }

    /// Fetches all message IDs for a session.
    pub fn fetch_all_message_ids_for_session(&self, session_id: &str) -> Vec<String> {
        let collection = Self::transcript_collection(session_id);
        let mut ids = Vec::new();

        match self
            .db
            .search_in_collection(&collection, self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            Ok(hits) => {
                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        if let Some(key) = memory.metadata.get("key") {
                            ids.push(key.clone());
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] fetch_all_message_ids_for_session failed: {}", e);
            }
        }
        ids
    }

    /// Deletes a session entirely.
    pub fn delete_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let collection = Self::transcript_collection(session_id);
        let mut deleted = 0;

        match self
            .db
            .search_in_collection(&collection, self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            Ok(hits) => {
                for hit in hits {
                    self.db
                        .delete(hit.id)
                        .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
                    deleted += 1;
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] delete_session failed: {}", e);
            }
        }

        self.sessions.remove(session_id);
        info!("Deleted session {} ({} messages)", session_id, deleted);
        Ok(())
    }

    /// Returns a snapshot of all known session IDs at the time of the call.
    /// This is a read-only view of the session registry, used by the vault
    /// projection worker to enumerate sessions for markdown export.
    pub fn session_keys(&self) -> Vec<String> {
        self.sessions.iter().map(|s| s.clone()).collect()
    }

    /// Returns the number of known sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Retrieves engine statistics.
    pub fn stats(&self) -> Result<StorageStats, MemoryError> {
        let db_stats = self
            .db
            .stats()
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        // Approximate disk usage from WAL length (each entry ~1KB average)
        let disk_usage_bytes = db_stats.wal_length * 1024;

        // Index rate: fraction of entries that have been indexed
        let cache_hit_rate = if db_stats.entries > 0 {
            db_stats.indexed_embeddings as f32 / db_stats.entries as f32
        } else {
            0.0
        };

        Ok(StorageStats {
            total_messages: db_stats.entries as u64,
            total_sessions: self.sessions.len() as u64,
            disk_usage_bytes,
            cache_hit_rate,
        })
    }

    /// Returns an iterator over all metadata entries.
    pub fn iter_metadata(&self) -> Result<Vec<crate::models::MemoryEntry>, MemoryError> {
        let mut entries = Vec::new();
        let mut stale_count = 0;

        match self
            .db
            .search_in_collection("metadata", self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            Ok(hits) => {
                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        if memory.content.len() > 1024 * 1024 {
                            stale_count += 1;
                            continue;
                        }
                        let archived = match rkyv::access::<
                            <crate::models::MemoryEntry as rkyv::Archive>::Archived,
                            RkyvError,
                        >(&memory.content)
                        {
                            Ok(a) => a,
                            Err(e) => {
                                tracing::warn!(
                                    session = %memory.metadata.get("session_id").map(|s| s.as_str()).unwrap_or("unknown"),
                                    error = %e,
                                    "Skipping stale memory entry (rkyv access failed)"
                                );
                                stale_count += 1;
                                continue;
                            }
                        };
                        if let Ok(entry) =
                            rkyv::deserialize::<crate::models::MemoryEntry, RkyvError>(archived)
                        {
                            entries.push(entry);
                        } else {
                            stale_count += 1;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] iter_metadata failed: {}", e);
            }
        }

        if stale_count > 0 {
            debug!("Skipped {} stale metadata entries", stale_count);
        }
        Ok(entries)
    }

    /// Stores temporal metadata for a memory entry.
    pub fn store_temporal_metadata(
        &self,
        temporal: &crate::models::TemporalMetadata,
    ) -> Result<(), MemoryError> {
        let key = crate::models::temporal_key(temporal.memory_id);
        let bytes = serde_json::to_vec(temporal)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

        self.db
            .add_with_content(
                "temporal",
                bytes,
                self.zero_embedding(),
                make_key_meta(&key),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Looks up temporal metadata by memory ID.
    pub fn get_temporal_metadata(
        &self,
        memory_id: u64,
    ) -> Result<Option<crate::models::TemporalMetadata>, MemoryError> {
        let key = crate::models::temporal_key(memory_id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        match self
            .db
            .search_in_collection("temporal", self.zero_embedding(), 1, Some(filter))
        {
            Ok(hits) => {
                if let Some(hit) = hits.first() {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        let temporal: crate::models::TemporalMetadata =
                            serde_json::from_slice(&memory.content)
                                .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
                        return Ok(Some(temporal));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] get_temporal_metadata failed: {}", e);
            }
        }
        Ok(None)
    }

    /// Finds active temporal entries for an entity name.
    pub fn find_active_temporal_by_entity(
        &self,
        entity_name: &str,
    ) -> Result<Vec<crate::models::TemporalMetadata>, MemoryError> {
        let mut results = Vec::new();

        match self
            .db
            .search_in_collection("temporal", self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            Ok(hits) => {
                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        if let Ok(temporal) = serde_json::from_slice::<
                            crate::models::TemporalMetadata,
                        >(&memory.content)
                        {
                            if temporal.is_active() && temporal.entity_name == entity_name {
                                results.push(temporal);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] find_active_temporal_by_entity failed: {}", e);
            }
        }
        Ok(results)
    }

    /// Stores a DAG node for reversible compaction.
    pub fn store_dag_node(&self, node: &crate::models::DagNode) -> Result<(), MemoryError> {
        let key = crate::models::dag_node_key(&node.node_id);
        let bytes = serde_json::to_vec(node)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

        self.db
            .add_with_content("dag", bytes, self.zero_embedding(), make_key_meta(&key))
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        // Graph edges require u64 IDs, but DAG nodes use UUIDs.
        // Child relationships are stored in the DagNode.child_nodes field directly.
        // No graph edges are created since CortexaDB's connect() requires numeric IDs.
        if !node.child_nodes.is_empty() {
            debug!(
                "DAG node {} has {} child nodes (stored in node data)",
                node.node_id,
                node.child_nodes.len()
            );
        }

        Ok(())
    }

    /// Loads a DAG node by ID.
    pub fn load_dag_node(
        &self,
        node_id: &str,
    ) -> Result<Option<crate::models::DagNode>, MemoryError> {
        let key = crate::models::dag_node_key(node_id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        match self
            .db
            .search_in_collection("dag", self.zero_embedding(), 1, Some(filter))
        {
            Ok(hits) => {
                if let Some(hit) = hits.first() {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        let node: crate::models::DagNode = serde_json::from_slice(&memory.content)
                            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
                        return Ok(Some(node));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] load_dag_node failed: {}", e);
            }
        }
        Ok(None)
    }

    /// Inserts a semantic fact into the SPO index.
    pub fn insert_fact(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        entry_id: u64,
    ) -> Result<(), MemoryError> {
        let key = format!("{}:{}:{}", subject, predicate, entry_id);
        let mut meta = HashMap::new();
        meta.insert("key".to_string(), key);
        meta.insert("subject".to_string(), subject.to_string());
        meta.insert("predicate".to_string(), predicate.to_string());
        meta.insert("entry_id".to_string(), entry_id.to_string());

        self.db
            .add_with_content(
                "facts",
                object.as_bytes().to_vec(),
                self.zero_embedding(),
                Some(meta),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Iterates over all recorded facts (SPO triples) in the system.
    pub fn iter_facts(&self) -> Vec<(String, String, String, u64)> {
        let mut results = Vec::new();

        match self
            .db
            .search_in_collection("facts", self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            Ok(hits) => {
                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        let object = String::from_utf8_lossy(&memory.content).to_string();
                        let subject = memory.metadata.get("subject").cloned().unwrap_or_default();
                        let predicate = memory
                            .metadata
                            .get("predicate")
                            .cloned()
                            .unwrap_or_default();
                        let entry_id = memory
                            .metadata
                            .get("entry_id")
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        results.push((subject, predicate, object, entry_id));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] iter_facts failed: {}", e);
            }
        }
        results
    }

    /// Retrieves all facts for a given subject.
    pub fn get_facts_by_subject(&self, subject: &str) -> Vec<(String, String, u64)> {
        let mut results = Vec::new();
        let filter = {
            let mut m = HashMap::new();
            m.insert("subject".to_string(), subject.to_string());
            m
        };

        match self.db.search_in_collection(
            "facts",
            self.zero_embedding(),
            MAX_BATCH_SIZE,
            Some(filter),
        ) {
            Ok(hits) => {
                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        let predicate = memory
                            .metadata
                            .get("predicate")
                            .cloned()
                            .unwrap_or_default();
                        let entry_id = memory
                            .metadata
                            .get("entry_id")
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);
                        results.push((
                            predicate,
                            String::from_utf8_lossy(&memory.content).to_string(),
                            entry_id,
                        ));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] get_facts_by_subject failed: {}", e);
            }
        }
        results
    }

    /// Deletes a specific fact from the SPO index.
    pub fn delete_fact(
        &self,
        subject: &str,
        predicate: &str,
        entry_id: u64,
    ) -> Result<(), MemoryError> {
        let key = format!("{}:{}:{}", subject, predicate, entry_id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        match self
            .db
            .search_in_collection("facts", self.zero_embedding(), 1, Some(filter))
        {
            Ok(hits) => {
                for hit in hits {
                    self.db
                        .delete(hit.id)
                        .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] delete_fact failed: {}", e);
            }
        }
        Ok(())
    }

    /// Removes a MemoryEntry from the metadata collection by ID.
    pub fn delete_metadata(&self, id: u64) -> Result<(), MemoryError> {
        self.remove_metadata(id)
    }

    /// Fetches a MemoryEntry from the metadata collection by ID.
    pub fn get_metadata(&self, id: u64) -> Result<Option<crate::models::MemoryEntry>, MemoryError> {
        let key = format!("meta:{}", id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        if let Ok(hits) =
            self.db
                .search_in_collection("metadata", self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    let archived = rkyv::access::<
                        <crate::models::MemoryEntry as rkyv::Archive>::Archived,
                        rkyv::rancor::Error,
                    >(&memory.content)
                    .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

                    let entry =
                        rkyv::deserialize::<crate::models::MemoryEntry, rkyv::rancor::Error>(
                            archived,
                        )
                        .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
                    return Ok(Some(entry));
                }
            }
        }
        Ok(None)
    }

    /// Fetches a single message by ID across all sessions.
    ///
    /// Uses the message_session_index for O(1) session lookup when available,
    /// falling back to full scan for messages not yet indexed.
    pub fn fetch_message_by_id(&self, msg_id: &str) -> Result<Option<AgentMessage>, MemoryError> {
        // Fast path: look up session ID from the index
        let sessions_to_search: Vec<String> =
            if let Some(session_ref) = self.message_session_index.get(msg_id) {
                vec![session_ref.value().clone()]
            } else {
                // Fallback: scan all sessions
                self.sessions.iter().map(|s| s.key().clone()).collect()
            };

        for session_id in sessions_to_search {
            let collection = Self::transcript_collection(&session_id);

            match self.db.search_in_collection(
                &collection,
                self.zero_embedding(),
                MAX_BATCH_SIZE,
                None,
            ) {
                Ok(hits) => {
                    for hit in hits {
                        if let Ok(memory) = self.db.get_memory(hit.id) {
                            if memory.content.len() > 10 * 1024 * 1024 {
                                continue;
                            }
                            if let Ok(archived) = rkyv::access::<
                                <AgentMessage as rkyv::Archive>::Archived,
                                rkyv::rancor::Error,
                            >(&memory.content)
                            {
                                if let Ok(msg) =
                                    rkyv::deserialize::<AgentMessage, rkyv::rancor::Error>(archived)
                                {
                                    if msg.id == msg_id {
                                        return Ok(Some(msg));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[lsm] fetch_message_by_id failed: {}", e);
                }
            }
        }
        Ok(None)
    }

    /// Checks if a message has already been distilled.
    pub fn is_distilled(&self, msg_id: &str) -> bool {
        let filter = {
            let mut m = HashMap::new();
            m.insert("msg_id".to_string(), msg_id.to_string());
            m
        };

        self.db
            .search_in_collection("distillation", self.zero_embedding(), 1, Some(filter))
            .map(|h| !h.is_empty())
            .unwrap_or(false)
    }

    /// Marks a message as successfully distilled.
    pub fn mark_distilled(&self, msg_id: &str) -> Result<(), MemoryError> {
        let mut meta = HashMap::new();
        meta.insert("msg_id".to_string(), msg_id.to_string());

        self.db
            .add_with_content("distillation", vec![1], self.zero_embedding(), Some(meta))
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    // ========================================================================
    // Session / Turn State Management
    // ========================================================================
    // BM25 State Persistence (CortexaDB-backed)
    // ========================================================================

    /// Saves BM25 index state to the "bm25_state" collection.
    pub fn save_bm25_state(&self, bm25: &crate::bm25_index::Bm25Index) -> Result<(), MemoryError> {
        let bytes = bm25
            .save_snapshot()
            .map_err(MemoryError::SerializationFailed)?;

        self.db
            .add_with_content(
                "bm25_state",
                bytes,
                self.zero_embedding(),
                make_key_meta("bm25_snapshot"),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        debug!("Saved BM25 index state ({} docs)", bm25.doc_count());
        Ok(())
    }

    /// Loads BM25 index state from the "bm25_state" collection.
    /// Returns None if no BM25 state has been persisted.
    pub fn load_bm25_state(&self) -> Result<Option<crate::bm25_index::Bm25Index>, MemoryError> {
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), "bm25_snapshot".to_string());
            m
        };

        if let Ok(hits) =
            self.db
                .search_in_collection("bm25_state", self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    if !memory.content.is_empty() {
                        match crate::bm25_index::Bm25Index::load_snapshot(&memory.content) {
                            Ok(bm25) => {
                                info!("Loaded BM25 index ({} docs)", bm25.doc_count());
                                return Ok(Some(bm25));
                            }
                            Err(e) => {
                                warn!("Failed to deserialize BM25 state: {}", e);
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    // ========================================================================
    // Procedures/Lessons/Insights Persistence (B6)
    // ========================================================================

    /// Saves procedures to the "procedures" collection.
    pub fn save_procedures(
        &self,
        procedures: &[crate::procedural::ProceduralMemory],
    ) -> Result<(), MemoryError> {
        let bytes = serde_json::to_vec(procedures)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        self.db
            .add_with_content(
                "procedures",
                bytes,
                self.zero_embedding(),
                make_key_meta("procedures_snapshot"),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Loads procedures from the "procedures" collection.
    pub fn load_procedures(&self) -> Result<Vec<crate::procedural::ProceduralMemory>, MemoryError> {
        self.load_json_collection("procedures", "procedures_snapshot")
    }

    /// Saves lessons to the "lessons" collection.
    pub fn save_lessons(&self, lessons: &[crate::lessons::Lesson]) -> Result<(), MemoryError> {
        let bytes = serde_json::to_vec(lessons)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        self.db
            .add_with_content(
                "lessons",
                bytes,
                self.zero_embedding(),
                make_key_meta("lessons_snapshot"),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Loads lessons from the "lessons" collection.
    pub fn load_lessons(&self) -> Result<Vec<crate::lessons::Lesson>, MemoryError> {
        self.load_json_collection("lessons", "lessons_snapshot")
    }

    /// Saves insights to the "insights" collection.
    pub fn save_insights(&self, insights: &[crate::lessons::Insight]) -> Result<(), MemoryError> {
        let bytes = serde_json::to_vec(insights)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        self.db
            .add_with_content(
                "insights",
                bytes,
                self.zero_embedding(),
                make_key_meta("insights_snapshot"),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Loads insights from the "insights" collection.
    pub fn load_insights(&self) -> Result<Vec<crate::lessons::Insight>, MemoryError> {
        self.load_json_collection("insights", "insights_snapshot")
    }

    /// Generic loader for JSON-serialized collections from CortexaDB.
    fn load_json_collection<T: serde::de::DeserializeOwned>(
        &self,
        collection: &str,
        key: &str,
    ) -> Result<T, MemoryError> {
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key.to_string());
            m
        };
        if let Ok(hits) =
            self.db
                .search_in_collection(collection, self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    if !memory.content.is_empty() {
                        return serde_json::from_slice(&memory.content)
                            .map_err(|e| MemoryError::SerializationFailed(e.to_string()));
                    }
                }
            }
        }
        // Return empty default if not found
        serde_json::from_str("[]").map_err(|e| MemoryError::SerializationFailed(e.to_string()))
    }

    // ========================================================================
    // Session State
    // ========================================================================

    /// Saves or updates a session state in the "sessions" collection.
    pub fn save_session_state(
        &self,
        state: &crate::models::SessionState,
    ) -> Result<(), MemoryError> {
        let bytes = rkyv::to_bytes::<RkyvError>(state)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        let key = crate::models::session_state_key(&state.session_id);

        self.db
            .add_with_content(
                "sessions",
                bytes.to_vec(),
                self.zero_embedding(),
                make_key_meta(&key),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        debug!("Saved session state for {}", state.session_id);
        Ok(())
    }

    /// Loads a session state from the "sessions" collection.
    /// Returns None if the session has no stored state.
    pub fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::models::SessionState>, MemoryError> {
        let key = crate::models::session_state_key(session_id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };

        if let Ok(hits) =
            self.db
                .search_in_collection("sessions", self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    let archived = rkyv::access::<
                        <crate::models::SessionState as rkyv::Archive>::Archived,
                        rkyv::rancor::Error,
                    >(&memory.content)
                    .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

                    let state =
                        rkyv::deserialize::<crate::models::SessionState, rkyv::rancor::Error>(
                            archived,
                        )
                        .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
                    return Ok(Some(state));
                }
            }
        }
        Ok(None)
    }

    /// Gets or creates a session state. If no state exists, creates a new one and persists it.
    pub fn get_or_create_session_state(
        &self,
        session_id: &str,
    ) -> Result<crate::models::SessionState, MemoryError> {
        if let Some(state) = self.get_session_state(session_id)? {
            Ok(state)
        } else {
            let state = crate::models::SessionState::new(session_id);
            self.save_session_state(&state)?;
            Ok(state)
        }
    }

    /// Returns the turn collection name for a session.
    fn turn_collection(session_id: &str) -> String {
        format!("turns.{}", session_id)
    }

    /// S1: Iterates all session states in the "sessions" collection.
    pub fn iter_session_states(&self) -> Result<Vec<crate::models::SessionState>, MemoryError> {
        let hits = self
            .db
            .search_in_collection("sessions", self.zero_embedding(), 10_000, None)
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        let mut states = Vec::new();
        for hit in hits {
            if let Ok(memory) = self.db.get_memory(hit.id) {
                if !memory.content.is_empty() {
                    let archived = rkyv::access::<
                        <crate::models::SessionState as rkyv::Archive>::Archived,
                        rkyv::rancor::Error,
                    >(&memory.content);
                    if let Ok(archived) = archived {
                        if let Ok(state) = rkyv::deserialize::<
                            crate::models::SessionState,
                            rkyv::rancor::Error,
                        >(archived)
                        {
                            states.push(state);
                        }
                    }
                }
            }
        }
        Ok(states)
    }

    /// S1: Deletes a session state from the "sessions" collection.
    pub fn delete_session_state(&self, session_id: &str) -> Result<(), MemoryError> {
        let key = crate::models::session_state_key(session_id);
        // Search for the session entry to get its ID
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };
        if let Ok(hits) =
            self.db
                .search_in_collection("sessions", self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                self.db
                    .delete(hit.id)
                    .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Saves a turn state to the "turns.{session_id}" collection.
    pub fn save_turn_state(&self, turn: &crate::models::TurnState) -> Result<(), MemoryError> {
        let bytes = rkyv::to_bytes::<RkyvError>(turn)
            .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
        let key = crate::models::turn_state_key(&turn.session_id, &turn.turn_id);
        let collection = Self::turn_collection(&turn.session_id);

        self.db
            .add_with_content(
                &collection,
                bytes.to_vec(),
                self.zero_embedding(),
                make_key_meta(&key),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        debug!(
            "Saved turn state {} for session {}",
            turn.turn_id, turn.session_id
        );
        Ok(())
    }

    /// Loads a specific turn state by turn ID.
    pub fn get_turn_state(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::models::TurnState>, MemoryError> {
        let key = crate::models::turn_state_key(session_id, turn_id);
        let filter = {
            let mut m = HashMap::new();
            m.insert("key".to_string(), key);
            m
        };
        let collection = Self::turn_collection(session_id);

        if let Ok(hits) =
            self.db
                .search_in_collection(&collection, self.zero_embedding(), 1, Some(filter))
        {
            if let Some(hit) = hits.first() {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    let archived = rkyv::access::<
                        <crate::models::TurnState as rkyv::Archive>::Archived,
                        rkyv::rancor::Error,
                    >(&memory.content)
                    .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;

                    let turn = rkyv::deserialize::<crate::models::TurnState, rkyv::rancor::Error>(
                        archived,
                    )
                    .map_err(|e| MemoryError::SerializationFailed(e.to_string()))?;
                    return Ok(Some(turn));
                }
            }
        }
        Ok(None)
    }

    /// Fetches the most recent N turns for a session, ordered newest-first.
    pub fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::TurnState>, MemoryError> {
        let collection = Self::turn_collection(session_id);
        let mut turns = Vec::new();

        if let Ok(hits) =
            self.db
                .search_in_collection(&collection, self.zero_embedding(), MAX_BATCH_SIZE, None)
        {
            for hit in hits {
                if let Ok(memory) = self.db.get_memory(hit.id) {
                    if memory.content.len() > 1024 * 1024 {
                        continue;
                    }
                    if let Ok(archived) = rkyv::access::<
                        <crate::models::TurnState as rkyv::Archive>::Archived,
                        rkyv::rancor::Error,
                    >(&memory.content)
                    {
                        if let Ok(turn) = rkyv::deserialize::<
                            crate::models::TurnState,
                            rkyv::rancor::Error,
                        >(archived)
                        {
                            turns.push(turn);
                        }
                    }
                }
            }
        }

        turns.sort_by_key(|t| i64::from(t.started_at));
        turns.reverse();
        turns.truncate(limit);
        Ok(turns)
    }

    /// Flushes pending writes to disk.
    pub fn flush(&self) -> Result<(), MemoryError> {
        self.db
            .flush()
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))
    }

    // ========================================================================
    // TaskState WAL Journal
    // ========================================================================
    // TaskState transitions are journaled to a dedicated collection for crash
    // recovery. Each entry contains the task_id, the new state, a timestamp,
    // and an XXH3 checksum for integrity verification.
    // On orchestrator restart, recover_from_journal() can scan for
    // TaskState::Working entries and re-queue interrupted delegations.

    /// Journals a TaskState transition to the persistent WAL.
    ///
    /// Stores the transition in the `task_state_journal` collection keyed by
    /// `{task_id}:{timestamp_ms}`. Each entry includes an XXH3 checksum for
    /// integrity verification.
    ///
    /// # Arguments
    /// * `task_id` — The UUID of the delegated task (as hex string)
    /// * `new_state` — The TaskState being transitioned to
    pub fn journal_task_state(
        &self,
        task_id: &str,
        new_state: savant_ipc::a2a::protocol::TaskState,
    ) -> Result<(), MemoryError> {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Build the journal entry: a compact binary representation
        // [task_id_bytes][state_u8][timestamp_ms_u64]
        let mut entry = Vec::new();
        entry.extend_from_slice(task_id.as_bytes());
        entry.push(new_state as u8);
        entry.extend_from_slice(&timestamp_ms.to_le_bytes());

        // Compute XXH3 checksum for integrity verification
        let checksum = xxhash_rust::xxh3::xxh3_64(&entry);

        // Build metadata with checksum for verification on replay
        let key = format!("task_state:{}:{}", task_id, timestamp_ms);
        let mut meta = std::collections::HashMap::new();
        meta.insert("key".to_string(), key.clone());
        meta.insert("task_id".to_string(), task_id.to_string());
        meta.insert("state".to_string(), (new_state as u8).to_string());
        meta.insert("timestamp_ms".to_string(), timestamp_ms.to_string());
        meta.insert("checksum".to_string(), checksum.to_string());

        self.db
            .add_with_content(
                "task_state_journal",
                entry,
                self.zero_embedding(),
                Some(meta),
            )
            .map_err(|e| MemoryError::TransactionFailed(e.to_string()))?;

        debug!(
            task_id = %task_id,
            state = %new_state,
            timestamp_ms = %timestamp_ms,
            "TaskState transition journaled to WAL"
        );
        Ok(())
    }

    /// Scans the task state journal for interrupted delegations.
    ///
    /// Returns all task IDs that have a `Working` state entry but no
    /// corresponding `Completed`, `Failed`, or `Canceled` entry. These
    /// tasks were interrupted mid-execution and should be re-queued.
    pub fn recover_interrupted_delegations(
        &self,
    ) -> Result<Vec<(String, savant_ipc::a2a::protocol::TaskState)>, MemoryError> {
        let mut interrupted = Vec::new();

        match self.db.search_in_collection(
            "task_state_journal",
            self.zero_embedding(),
            MAX_BATCH_SIZE,
            None,
        ) {
            Ok(hits) => {
                // Collect all state entries per task_id
                let mut task_states: std::collections::HashMap<
                    String,
                    Vec<(savant_ipc::a2a::protocol::TaskState, u64)>,
                > = std::collections::HashMap::new();

                for hit in hits {
                    if let Ok(memory) = self.db.get_memory(hit.id) {
                        if let Some(task_id) = memory.metadata.get("task_id") {
                            if let Some(state_str) = memory.metadata.get("state") {
                                if let Ok(state_val) = state_str.parse::<u8>() {
                                    if let Some(ts_str) = memory.metadata.get("timestamp_ms") {
                                        if let Ok(ts) = ts_str.parse::<u64>() {
                                            let state = match state_val {
                                                0 => savant_ipc::a2a::protocol::TaskState::Submitted,
                                                1 => savant_ipc::a2a::protocol::TaskState::Working,
                                                2 => {
                                                    savant_ipc::a2a::protocol::TaskState::InputRequired
                                                }
                                                3 => savant_ipc::a2a::protocol::TaskState::Completed,
                                                4 => savant_ipc::a2a::protocol::TaskState::Failed,
                                                5 => savant_ipc::a2a::protocol::TaskState::Canceled,
                                                _ => continue,
                                            };
                                            task_states
                                                .entry(task_id.clone())
                                                .or_default()
                                                .push((state, ts));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Find tasks whose latest state is Working or InputRequired (interrupted)
                for (task_id, mut states) in task_states {
                    states.sort_by_key(|(_, ts)| *ts);
                    if let Some((latest_state, _)) = states.last() {
                        if matches!(
                            latest_state,
                            savant_ipc::a2a::protocol::TaskState::Working
                                | savant_ipc::a2a::protocol::TaskState::InputRequired
                        ) {
                            interrupted.push((task_id, *latest_state));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[lsm] recover_interrupted_delegations failed: {}", e);
            }
        }

        info!(
            interrupted_count = interrupted.len(),
            "Scanned task state journal for interrupted delegations"
        );
        Ok(interrupted)
    }
}

/// Flush pending writes to disk on engine drop.
/// Ensures data durability when the engine is dropped without explicit flush.
impl Drop for LsmStorageEngine {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            tracing::warn!("LsmStorageEngine: flush on drop failed: {}", e);
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::models::{AgentMessage, MessageRole, ToolResultRef};
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn test_lsm_engine_basic_operations() {
        let temp_dir =
            std::env::temp_dir().join(format!("savant_memory_test_cortexa_{}", Uuid::new_v4()));
        if let Err(e) = fs::create_dir_all(&temp_dir) {
            panic!("Failed to create temp dir: {}", e);
        }

        // Use small vector dimension for tests to avoid 332MB allocation on Windows
        let config = LsmConfig {
            vector_dimension: 64,
            ..LsmConfig::default()
        };
        let engine = LsmStorageEngine::new(&temp_dir, config).unwrap();

        let msg = AgentMessage::user("session123", "Hello, world!");
        engine.append_message("session123", &msg).unwrap();

        let tail = engine.fetch_session_tail("session123", 10);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].content, "Hello, world!");

        if let Err(e) = fs::remove_dir_all(&temp_dir) {
            warn!("[memory::lsm] Failed to clean up test temp dir: {}", e);
        }
    }

    #[test]
    fn test_orphan_detection() {
        let msg_with_orphan = AgentMessage {
            id: "msg1".to_string(),
            session_id: "sess".to_string(),
            role: MessageRole::Tool,
            content: "result".to_string(),
            tool_calls: Vec::new(),
            tool_results: vec![ToolResultRef {
                tool_use_id: "orphan".to_string(),
                result_content: "orphaned result".to_string(),
                is_error: false,
            }],
            timestamp: 1000.into(),
            parent_id: None,
            channel: "Telemetry".to_string(),
        };

        let batch = vec![msg_with_orphan];
        let result = verify_tool_pair_integrity(&batch);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MemoryError::OrphanedToolResult { .. }
        ));
    }
}
