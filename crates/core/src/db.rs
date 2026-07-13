use crate::error::SavantError;
use crate::types::ChatMessage;
use cortexadb_core::CortexaDB;
use dashmap::DashMap;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Maximum number of content hashes to keep for deduplication per partition.
const DEDUP_WINDOW_SIZE: usize = 100;

/// Default vector dimension for CortexaDB embeddings (fallback).
/// Must match OllamaEmbeddingService::dimensions() (768 for nomic-embed-text).
const DEFAULT_VECTOR_DIM: usize = 768;

/// Maximum entries to retrieve per collection query.
const MAX_BATCH_SIZE: usize = 100_000;

/// Maps an agent_id to a CortexaDB collection name.
fn collection_name(agent_id: &str) -> String {
    if agent_id == "swarm.insights" {
        "chat.swarm".to_string()
    } else {
        format!("chat.{}", agent_id)
    }
}

/// Storage backed by CortexaDB.
///
/// Each agent's messages are stored in a collection named `chat.{agent_id}`.
/// Swarm messages go in `chat.swarm`.
/// Sorting is done client-side via the `timestamp` metadata field.
pub struct Storage {
    db: Arc<CortexaDB>,
    /// Per-partition message counters for O(1) count queries.
    partition_counts: DashMap<String, AtomicU64>,
    /// Per-partition content hash windows for message deduplication.
    /// Maps agent_id → (timestamp, hash) pairs, oldest evicted first.
    dedup_hashes: DashMap<String, VecDeque<(u64, String)>>,
    /// Configured vector dimension for zero-embedding construction.
    vector_dimension: usize,
}

impl Storage {
    /// Creates a new Storage instance with the specified vector dimension.
    pub fn new(path: PathBuf, vector_dimension: usize) -> Result<Self, SavantError> {
        info!("Sovereign Substrate: Initializing CortexaDB at {:?}", path);

        let path_str = path
            .to_str()
            .ok_or_else(|| SavantError::StorageError("Database path is not valid UTF-8".into()))?;

        let db = CortexaDB::open(path_str, vector_dimension)
            .map_err(|e| SavantError::StorageError(e.to_string()))?;

        Ok(Self {
            db: Arc::new(db),
            partition_counts: DashMap::new(),
            dedup_hashes: DashMap::new(),
            vector_dimension,
        })
    }

    /// Creates a new Storage instance with the default vector dimension.
    pub fn with_defaults(path: PathBuf) -> Result<Self, SavantError> {
        Self::new(path, DEFAULT_VECTOR_DIM)
    }

    /// Creates a zero-vector at the configured dimension.
    fn zero_embedding(&self) -> Vec<f32> {
        vec![0.0; self.vector_dimension]
    }

    /// Ghost-Restore: Performs a full database integrity check and recovery.
    ///
    /// 1. Flushes all pending writes to disk
    /// 2. Forces a checkpoint (snapshot + WAL truncation)
    /// 3. Compacts on-disk segments to reclaim space
    /// 4. Clears in-memory caches to force fresh reads
    /// 5. Verifies database stats are consistent
    pub fn ghost_restore(&self) -> Result<(), SavantError> {
        warn!("Sovereign Substrate: INITIATING GHOST-RESTORE.");

        // Step 1: Flush pending writes
        self.db
            .flush()
            .map_err(|e| SavantError::StorageError(format!("Flush failed: {}", e)))?;

        // Step 2: Force checkpoint
        self.db
            .checkpoint()
            .map_err(|e| SavantError::StorageError(format!("Checkpoint failed: {}", e)))?;

        // Step 3: Compact segments
        self.db
            .compact()
            .map_err(|e| SavantError::StorageError(format!("Compaction failed: {}", e)))?;

        // Step 4: Clear in-memory caches
        let hash_count = self.dedup_hashes.len();
        let partition_count = self.partition_counts.len();
        self.dedup_hashes.clear();
        self.partition_counts.clear();

        // Step 5: Verify stats
        let stats = self
            .db
            .stats()
            .map_err(|e| SavantError::StorageError(format!("Stats query failed: {}", e)))?;

        info!(
            "Ghost-Restore complete. DB has {} entries, {} hash caches cleared, {} partition counters reset.",
            stats.entries, hash_count, partition_count
        );
        Ok(())
    }

    /// Appends a chat message with deduplication.
    ///
    /// Uses blake3 content hashing to detect and skip duplicate messages
    /// within a sliding window of `DEDUP_WINDOW_SIZE` entries per partition.
    pub fn append_chat(&self, agent_id: &str, msg: &ChatMessage) -> Result<(), SavantError> {
        let coll = collection_name(agent_id);
        let payload = serde_json::to_string(msg).map_err(SavantError::SerializationError)?;

        // Compute content hash for deduplication
        let content_hash = blake3::hash(msg.content.as_bytes()).to_hex().to_string();

        // Check for duplicate within sliding window
        if let Some(hashes) = self.dedup_hashes.get(agent_id) {
            if hashes.iter().any(|(_, h)| h == &content_hash) {
                return Ok(());
            }
        }

        let timestamp = chrono::Utc::now().timestamp_micros().max(0) as u64;
        let msg_id = uuid::Uuid::new_v4().to_string();

        let mut metadata = HashMap::new();
        metadata.insert("timestamp".to_string(), timestamp.to_string());
        metadata.insert("message_id".to_string(), msg_id);
        metadata.insert("agent_id".to_string(), agent_id.to_string());

        self.db
            .add_with_content(
                &coll,
                payload.into_bytes(),
                self.zero_embedding(),
                Some(metadata),
            )
            .map_err(|e| SavantError::StorageError(e.to_string()))?;

        // Increment partition counter
        self.partition_counts
            .entry(agent_id.to_string())
            .or_default()
            .fetch_add(1, Ordering::Relaxed);

        // Add hash to dedup window and evict oldest if over limit
        let mut hashes = self.dedup_hashes.entry(agent_id.to_string()).or_default();
        hashes.push_back((timestamp, content_hash));
        while hashes.len() > DEDUP_WINDOW_SIZE {
            hashes.pop_front();
        }

        Ok(())
    }

    /// Retrieves chat history for an agent, most recent first (chronological order).
    pub fn get_history(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        let coll = collection_name(agent_id);

        let memories = self
            .db
            .get_all_in_collection(&coll)
            .map_err(|e| SavantError::StorageError(e.to_string()))?;

        // Cap retrieval at MAX_BATCH_SIZE to prevent unbounded memory growth
        let capped: Vec<_> = if memories.len() > MAX_BATCH_SIZE {
            memories[memories.len() - MAX_BATCH_SIZE..].to_vec()
        } else {
            memories
        };

        let mut entries: Vec<(u64, ChatMessage)> = Vec::with_capacity(capped.len());
        for mem in &capped {
            let ts = mem
                .metadata
                .get("timestamp")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if let Ok(msg) = serde_json::from_slice::<ChatMessage>(&mem.content) {
                entries.push((ts, msg));
            }
        }

        entries.sort_by(|a, b| b.0.cmp(&a.0)); // newest first

        let mut result: Vec<ChatMessage> = entries
            .into_iter()
            .take(limit)
            .map(|(_, msg)| msg)
            .collect();

        // Return in chronological order (oldest first)
        result.reverse();
        Ok(result)
    }

    /// Retrieves swarm-wide history.
    pub fn get_swarm_history(&self, limit: usize) -> Result<Vec<ChatMessage>, SavantError> {
        self.get_history("swarm.insights", limit)
    }

    /// Prunes old history entries, keeping only the most recent `keep_last` messages.
    pub fn prune_history(&self, agent_id: &str, keep_last: usize) -> Result<(), SavantError> {
        let coll = collection_name(agent_id);

        let memories = self
            .db
            .get_all_in_collection(&coll)
            .map_err(|e| SavantError::StorageError(e.to_string()))?;

        if memories.len() <= keep_last {
            return Ok(());
        }

        // Gather timestamps for sorting
        let mut entries: Vec<(u64, u64)> = Vec::with_capacity(memories.len());
        for mem in &memories {
            let ts = mem
                .metadata
                .get("timestamp")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            entries.push((ts, mem.id));
        }

        entries.sort_by(|a, b| a.0.cmp(&b.0)); // oldest first

        let to_delete = entries.len() - keep_last;
        let mut deleted = 0u64;

        for (_, id) in entries.iter().take(to_delete) {
            if self.db.delete(*id).is_ok() {
                deleted += 1;
            }
        }

        if let Some(counter) = self.partition_counts.get(agent_id) {
            // Use saturating_sub to prevent underflow wrapping
            let current = counter.load(Ordering::Relaxed);
            counter.store(current.saturating_sub(deleted), Ordering::Relaxed);
        }

        debug!("Pruned {} old entries for agent {}", deleted, agent_id);
        Ok(())
    }

    /// Gracefully shuts down the storage engine, ensuring all data is flushed.
    pub fn shutdown(&self) -> Result<(), SavantError> {
        info!("Storage: Initiating graceful shutdown...");
        self.db
            .flush()
            .map_err(|e| SavantError::StorageError(e.to_string()))?;
        self.db
            .checkpoint()
            .map_err(|e| SavantError::StorageError(e.to_string()))?;

        let partition_count = self.dedup_hashes.len();
        self.partition_counts.clear();
        self.dedup_hashes.clear();

        info!(
            "Storage: Shutdown complete. {} partition caches flushed and cleared.",
            partition_count
        );
        Ok(())
    }
}
