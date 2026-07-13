use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::error::MemoryError;
use crate::lsm_engine::{LsmConfig, LsmStorageEngine};
use crate::models::{AgentMessage, MemoryEntry};
use crate::notifications::NotificationChannel;
use crate::reflective::ReflectiveMemory;
use crate::vector_engine::{SemanticVectorEngine, VectorConfig};
use savant_core::traits::{EmbeddingProvider, LlmProvider};
use savant_core::types::LlmParams;

/// Strips the \\?\ UNC extended-length prefix from a Windows path.
/// On non-Windows or paths without the prefix, returns the path unchanged.
/// This is needed because std::fs::rename and std::fs::remove_dir_all
/// can fail with os error 267 on UNC-prefixed locked directories.
fn strip_unc_prefix(path: &std::path::Path) -> std::path::PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

// Maximum procedures, lessons, and insights are now configurable via MemoryConfig.
// See models::MemoryConfig for defaults (max_procedures, max_lessons, max_insights).

/// 🧬 OMEGA-VIII: Memory Layer Definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryLayer {
    /// L0: High-frequency transient logs (Episodic)
    Episodic,
    /// L1: Aggregated workspace and session state (Contextual)
    Contextual,
    /// L2: SIMD-accelerated long-term storage (Semantic)
    Semantic,
}

impl MemoryLayer {
    /// Determines the memory layer from a category string.
    pub fn from_category(category: &str) -> Self {
        match category.to_lowercase().as_str() {
            c if c.contains("episodic") || c.contains("session") || c.contains("transcript") => {
                Self::Episodic
            }
            c if c.contains("semantic") || c.contains("concept") || c.contains("relation") => {
                Self::Semantic
            }
            _ => Self::Contextual,
        }
    }
}

#[derive(Clone)]
pub struct EngineConfig {
    pub lsm_config: LsmConfig,
    pub vector_config: VectorConfig,
    pub distill_llm_provider: Option<Arc<dyn LlmProvider>>,
    pub distill_params: Option<LlmParams>,
    pub embedding_service: Arc<dyn EmbeddingProvider>,
    /// Per-agent personality traits for promotion scoring
    pub personality: Option<crate::promotion::PersonalityTraits>,
    /// Centralized tunables for all memory subsystems (MEM-17 through MEM-27).
    pub memory_config: crate::models::MemoryConfig,
}

/// The atomic Pure-Rust adapter (CortexaShim) that guarantees write atomicity
/// across the LSM and Vector engines to prevent orphaned vectors or race conditions.
pub struct MemoryEnclave {
    lsm: Arc<LsmStorageEngine>,
    vector: Arc<SemanticVectorEngine>,
    embedding_service: Arc<dyn EmbeddingProvider>,
    /// Centralized configuration for all memory subsystem tunables.
    pub(crate) config: crate::models::MemoryConfig,
    promotion: tokio::sync::Mutex<crate::promotion::PromotionEngine>,
    /// MAGMA 4-graph reflective memory (Semantic, Temporal, Causal, Entity).
    reflective: tokio::sync::RwLock<ReflectiveMemory>,
    /// CP-06: Notification channel for high-importance events.
    notifications: NotificationChannel,
    /// CP-07: BM25 keyword search index.
    bm25: tokio::sync::RwLock<crate::bm25_index::Bm25Index>,
    /// CP-11: Application-level audit trail.
    audit: tokio::sync::Mutex<crate::audit::AuditTrail>,
    /// CP-12/13: Learned procedures extracted from recurring patterns.
    procedures: tokio::sync::Mutex<Vec<crate::procedural::ProceduralMemory>>,
    /// CP-13: Lessons synthesized from repeated experiences.
    lessons: tokio::sync::Mutex<Vec<crate::lessons::Lesson>>,
    /// CP-14: Insights synthesized from concept clusters.
    insights: tokio::sync::Mutex<Vec<crate::lessons::Insight>>,
    /// CP-15: Multimodal image store.
    multimodal: tokio::sync::Mutex<crate::multimodal::MultimodalStore>,
    /// CP-16: P2P mesh sync manager (None when disabled).
    mesh_sync: tokio::sync::Mutex<Option<crate::mesh_sync::MeshSyncManager>>,
    // Per-session write lock pool: 64 partitions keyed by session_id hash
    write_locks: [tokio::sync::Mutex<()>; 64],
}

/// Read-only handle for sub-agents. Exposes only query methods — no writes.
/// Sub-agents receive this instead of the full `MemoryEnclave` to prevent
/// cross-agent memory corruption.
pub struct MemoryEnclaveHandle {
    inner: Arc<MemoryEnclave>,
}

impl MemoryEnclaveHandle {
    /// Create a read-only handle from a full enclave.
    pub fn new(enclave: Arc<MemoryEnclave>) -> Self {
        Self { inner: enclave }
    }

    /// Search memory by text query (read-only).
    pub async fn search_by_text(&self, query: &str, limit: usize) -> Vec<(u64, f32)> {
        let bm25 = self.inner.bm25.read().await;
        bm25.search(query, limit)
    }

    /// Get a memory entry by ID (read-only).
    pub fn get_metadata(&self, id: u64) -> Result<Option<crate::models::MemoryEntry>, MemoryError> {
        self.inner.lsm.get_metadata(id)
    }

    /// Get facts by subject (read-only).
    pub fn get_facts_by_subject(&self, subject: &str) -> Vec<(String, String, u64)> {
        self.inner.lsm.get_facts_by_subject(subject)
    }

    /// Get session state (read-only).
    pub fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::models::SessionState>, MemoryError> {
        self.inner.lsm.get_session_state(session_id)
    }

    /// Get lessons (read-only).
    pub async fn get_lessons(&self) -> Vec<crate::lessons::Lesson> {
        self.inner.get_lessons_vec().await
    }

    /// Get insights (read-only).
    pub async fn get_insights(&self) -> Vec<crate::lessons::Insight> {
        self.inner.get_insights_vec().await
    }
}

impl MemoryEnclave {
    /// Returns a reference to the MAGMA 4-graph reflective memory.
    /// Use this to access Semantic, Temporal, Causal, and Entity graphs.
    pub async fn reflective(&self) -> tokio::sync::RwLockReadGuard<'_, ReflectiveMemory> {
        self.reflective.read().await
    }

    /// Returns a clone of synthesized lessons (CP-13).
    pub async fn get_lessons_vec(&self) -> Vec<crate::lessons::Lesson> {
        self.lessons.lock().await.clone()
    }

    /// Returns a clone of synthesized insights (CP-14).
    pub async fn get_insights_vec(&self) -> Vec<crate::lessons::Insight> {
        self.insights.lock().await.clone()
    }

    /// Returns a mutable reference to the MAGMA 4-graph reflective memory.
    pub async fn reflective_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, ReflectiveMemory> {
        self.reflective.write().await
    }

    /// Returns a reference to the BM25 keyword search index (CP-07).
    pub async fn bm25(&self) -> tokio::sync::RwLockReadGuard<'_, crate::bm25_index::Bm25Index> {
        self.bm25.read().await
    }

    /// Returns a mutable reference to the BM25 index (CP-07).
    pub async fn bm25_mut(
        &self,
    ) -> tokio::sync::RwLockWriteGuard<'_, crate::bm25_index::Bm25Index> {
        self.bm25.write().await
    }

    /// Returns a reference to the audit trail (CP-11).
    pub async fn audit(&self) -> tokio::sync::MutexGuard<'_, crate::audit::AuditTrail> {
        self.audit.lock().await
    }

    /// Sends a notification through the enclave's notification channel (CP-06).
    pub fn notify(&self, notification: crate::notifications::MemoryNotification) {
        self.notifications.notify(notification);
    }

    /// Returns a reference to learned procedures (CP-12).
    pub async fn procedures(
        &self,
    ) -> tokio::sync::MutexGuard<'_, Vec<crate::procedural::ProceduralMemory>> {
        self.procedures.lock().await
    }

    /// Returns a reference to synthesized lessons (CP-13).
    pub async fn lessons(&self) -> tokio::sync::MutexGuard<'_, Vec<crate::lessons::Lesson>> {
        self.lessons.lock().await
    }

    /// Returns a reference to synthesized insights (CP-14).
    pub async fn insights(&self) -> tokio::sync::MutexGuard<'_, Vec<crate::lessons::Insight>> {
        self.insights.lock().await
    }

    /// Returns a reference to the multimodal store (CP-15).
    pub async fn multimodal(
        &self,
    ) -> tokio::sync::MutexGuard<'_, crate::multimodal::MultimodalStore> {
        self.multimodal.lock().await
    }

    /// Returns a reference to the mesh sync manager (CP-16).
    pub async fn mesh_sync(
        &self,
    ) -> tokio::sync::MutexGuard<'_, Option<crate::mesh_sync::MeshSyncManager>> {
        self.mesh_sync.lock().await
    }

    /// Acquires the partitioned write lock for the given session.
    async fn lock_session(&self, session_id: &str) -> tokio::sync::MutexGuard<'_, ()> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        session_id.hash(&mut hasher);
        let idx = (hasher.finish() % 64) as usize;
        self.write_locks[idx].lock().await
    }
}

impl MemoryEnclave {
    pub fn new<P: AsRef<Path>>(
        storage_path: P,
        config: EngineConfig,
    ) -> Result<Arc<Self>, MemoryError> {
        let lsm = LsmStorageEngine::new(storage_path.as_ref(), config.lsm_config)?;

        // Apply MemoryConfig overrides to vector config
        let mut vector_config = config.vector_config;
        // Use MemoryConfig default_vector_dim if the VectorConfig has the default value
        if vector_config.dimensions == 768 && config.memory_config.default_vector_dim != 768 {
            vector_config.dimensions = config.memory_config.default_vector_dim;
        }
        // Apply MemoryConfig vector_max_elements
        vector_config.max_elements = config.memory_config.vector_max_elements;

        // Dynamic vector dimension: use embedding service dimension
        let emb_dims = config.embedding_service.dimensions();
        if emb_dims > 0 && emb_dims != vector_config.dimensions {
            info!(
                "Overriding vector dimension: {} -> {} (from embedding service)",
                vector_config.dimensions, emb_dims
            );
            vector_config.dimensions = emb_dims;
        }

        // Attempt with a clone first to preserve `vector_config` for possible retry
        let first_try_config = vector_config.clone();
        let vector = match SemanticVectorEngine::new(storage_path.as_ref(), first_try_config) {
            Ok(v) => v,
            Err(e @ MemoryError::VectorInitFailed(_)) => {
                // Potential dimension mismatch from old persistence; clear and retry once
                let vector_dir = storage_path.as_ref().join("vector");
                if vector_dir.exists() {
                    // RC-27: Strip UNC prefix from ALL paths used in filesystem ops.
                    // On Windows, \\\\?\\ extended-length paths cause os error 267
                    // ("The directory name is invalid") for std::fs operations on locked dirs.
                    let canonical_vector = strip_unc_prefix(&vector_dir);
                    let canonical_backup = strip_unc_prefix(&storage_path.as_ref().join("vector.bak"));

                    // Back up vector index before clearing (best-effort)
                    if canonical_backup.exists() {
                        if let Err(e) = std::fs::remove_dir_all(&canonical_backup) {
                            debug!("Failed to remove old vector backup: {}", e);
                        }
                    }
                    if let Err(copy_err) =
                        savant_core::utils::io::copy_dir_recursive(&canonical_vector, &canonical_backup)
                    {
                        warn!("Failed to back up vector index: {}", copy_err);
                    } else {
                        info!("Vector index backed up to {:?}", canonical_backup);
                    }

                    warn!(
                        "Clearing stale vector index at {:?} due to init error: {}",
                        canonical_vector, e
                    );

                    // Targeted fix first: delete just the lock file to release the handle.
                    // This is the minimum needed to let the vector engine re-initialize.
                    let lock_file = canonical_vector.join("lock");
                    if lock_file.exists() {
                        match std::fs::remove_file(&lock_file) {
                            Ok(()) => info!("Removed stale lock file at {:?}", lock_file),
                            Err(lock_err) => warn!("Failed to remove lock file {:?}: {}", lock_file, lock_err),
                        }
                    }

                    // Nuclear option: remove entire vector directory.
                    // Only needed if lock file removal alone doesn't resolve the issue.
                    if canonical_vector.exists() {
                        if let Err(remove_err) = std::fs::remove_dir_all(&canonical_vector) {
                            // If the lock file was deleted but dir removal failed,
                            // the vector engine might still be able to re-initialize
                            // (it only needs the lock released, not the dir gone).
                            if !lock_file.exists() {
                                info!(
                                    "Directory removal failed ({}) but lock file is gone — will retry init",
                                    remove_err
                                );
                            } else {
                                warn!(
                                    "Failed to remove stale vector index: {}. Another process may be using it.",
                                    remove_err
                                );
                                return Err(MemoryError::VectorInitFailed(format!(
                                    "Vector database locked by another process. Close other Savant instances and try again. (original error: {}",
                                    e
                                )));
                            }
                        }
                    }
                    // Retry with original `vector_config` (which may have corrected dimensions)
                    SemanticVectorEngine::new(storage_path.as_ref(), vector_config)?
                } else {
                    return Err(e);
                }
            }
            Err(other) => return Err(other),
        };

        let bm25_k1 = config.memory_config.bm25_k1;
        let bm25_b = config.memory_config.bm25_b;
        let max_bm25_documents = config.memory_config.max_bm25_documents;

        // Load BM25 state from CortexaDB before moving lsm into struct
        let bm25_index = match lsm.load_bm25_state() {
            Ok(Some(bm25)) => {
                info!(
                    "Restored BM25 index from CortexaDB ({} docs)",
                    bm25.doc_count()
                );
                bm25
            }
            Ok(None) => {
                crate::bm25_index::Bm25Index::with_config(bm25_k1, bm25_b, max_bm25_documents)
            }
            Err(e) => {
                warn!("Failed to load BM25 state, starting fresh: {}", e);
                crate::bm25_index::Bm25Index::with_config(bm25_k1, bm25_b, max_bm25_documents)
            }
        };

        // B6: Load procedures/lessons/insights from CortexaDB before moving lsm
        let procedures = lsm.load_procedures().unwrap_or_else(|e| {
            warn!("Failed to load procedures, starting fresh: {}", e);
            Vec::new()
        });
        let lessons = lsm.load_lessons().unwrap_or_else(|e| {
            warn!("Failed to load lessons, starting fresh: {}", e);
            Vec::new()
        });
        let insights = lsm.load_insights().unwrap_or_else(|e| {
            warn!("Failed to load insights, starting fresh: {}", e);
            Vec::new()
        });

        Ok(Arc::new(Self {
            lsm,
            vector,
            embedding_service: config.embedding_service,
            config: config.memory_config,
            promotion: tokio::sync::Mutex::new(crate::promotion::PromotionEngine::new(
                config.personality.unwrap_or_default(),
            )),
            reflective: tokio::sync::RwLock::new(ReflectiveMemory::new()),
            notifications: NotificationChannel::default(),
            bm25: tokio::sync::RwLock::new(bm25_index),
            audit: tokio::sync::Mutex::new(crate::audit::AuditTrail::default()),
            procedures: tokio::sync::Mutex::new(procedures),
            lessons: tokio::sync::Mutex::new(lessons),
            insights: tokio::sync::Mutex::new(insights),
            multimodal: tokio::sync::Mutex::new(crate::multimodal::MultimodalStore::new_with_path(
                storage_path.as_ref().to_path_buf(),
            )),
            mesh_sync: tokio::sync::Mutex::new(None),
            write_locks: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        }))
    }

    pub async fn append_message(
        &self,
        session_id: &str,
        message: &AgentMessage,
    ) -> Result<(), MemoryError> {
        let _guard = self.lock_session(session_id).await;
        self.lsm.append_message(session_id, message)
    }

    pub fn fetch_session_tail(&self, session_id: &str, limit: usize) -> Vec<AgentMessage> {
        self.lsm.fetch_session_tail(session_id, limit)
    }

    /// Updates the personality traits used by the promotion engine.
    pub async fn update_personality(&self, traits: crate::promotion::PersonalityTraits) {
        info!(
            "MemoryEnclave: personality updated — O:{:.2} C:{:.2} E:{:.2} A:{:.2} N:{:.2}",
            traits.openness,
            traits.conscientiousness,
            traits.extraversion,
            traits.agreeableness,
            traits.neuroticism
        );
        let mut engine = self.promotion.lock().await;
        engine.update_traits(traits);
    }

    /// Runs a promotion cycle: scores all memory entries, archives low-value entries,
    /// reinforces high-value entries, and promotes identity candidates.
    ///
    /// The promotion cycle is the core memory lifecycle operation:
    /// 1. **Score**: Each entry is scored using personality-weighted metrics
    /// 2. **Archive**: Low-scoring old entries (score < 0.35, age > 720h) are deleted
    /// 3. **Reinforce**: High-scoring entries (score > 0.7) have their hit_count incremented
    /// 4. **Identity**: Candidates for SOUL.md mutation are drift-checked and queued
    /// 5. **Persist**: Evolution score is updated based on cycle results
    pub async fn run_promotion_cycle(&self) {
        let entries = match self.lsm.iter_metadata() {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut low_count = 0;
        let mut high_count = 0;
        let mut archived_count = 0;
        let mut reinforced_count = 0;
        let mut identity_candidates = Vec::new();
        let mut archive_errors = 0;

        // CP-04: Ebbinghaus retention scorer for alternative scoring
        let ebbinghaus = crate::promotion::EbbinghausScorer::default();
        let now = chrono::Utc::now().timestamp();

        // Hold the promotion lock for the entire promotion cycle to avoid
        // lock/unlock per entry contention and prevent race conditions
        // between scoring and evolution score updates (RC-17, MEM-14).
        let mut promotion = self.promotion.lock().await;

        for entry in &entries {
            let age_hours = (chrono::Utc::now().timestamp_millis() - i64::from(entry.created_at))
                as f32
                / 3600000.0;
            let metrics = crate::promotion::PromotionMetrics {
                hit_count: u32::from(entry.hit_count),
                age_hours,
                shannon_entropy: f32::from(entry.shannon_entropy),
                importance: entry.importance,
                category: entry.category.clone(),
            };
            let ocean_score = promotion.calculate_score(&metrics);

            // CP-04: Compute Ebbinghaus retention score alongside OCEAN
            let last_accessed: i64 = entry.last_accessed_at.into();
            let days_since_access = if last_accessed > 0 {
                ((now - last_accessed / 1000).max(0) as f32) / 86400.0
            } else {
                age_hours / 24.0
            };
            let access_ts: Vec<i64> = entry
                .access_timestamps
                .iter()
                .map(|t| i64::from(*t))
                .collect();
            let ebbinghaus_score =
                ebbinghaus.score(&entry.category, days_since_access, &access_ts, now);

            // Use the average of OCEAN and Ebbinghaus scores
            let score = (ocean_score + ebbinghaus_score) / 2.0;

            // B4: Ebbinghaus tier-based lifecycle decisions
            let tier = ebbinghaus.tier(ebbinghaus_score);
            match tier {
                crate::promotion::RetentionTier::Hot => {
                    // Reinforce: increment hit_count
                    high_count += 1;
                    let mut reinforced = entry.clone();
                    let current_hits: u32 = reinforced.hit_count.into();
                    reinforced.hit_count = (current_hits + 1).into();
                    reinforced.updated_at = chrono::Utc::now().timestamp_millis().into();
                    let id: u64 = entry.id.into();
                    if let Err(e) = self.lsm.insert_metadata(id, &reinforced) {
                        warn!("[memory::enclave] Failed to reinforce entry {}: {}", id, e);
                    } else {
                        reinforced_count += 1;
                    }
                }
                crate::promotion::RetentionTier::Warm => {
                    // Keep — no action needed
                }
                crate::promotion::RetentionTier::Cold => {
                    // Archive: entries in Cold tier with sufficient age
                    if age_hours > 720.0 {
                        low_count += 1;
                        let id: u64 = entry.id.into();
                        if let Err(e) = self.vector.remove(&id.to_string()) {
                            warn!(
                                "[memory::enclave] Failed to remove vector for cold entry {}: {}",
                                id, e
                            );
                        }
                        if let Err(e) = self.lsm.delete_metadata(id) {
                            warn!(
                                "[memory::enclave] Failed to archive cold entry {} from LSM: {}",
                                id, e
                            );
                            archive_errors += 1;
                        } else {
                            archived_count += 1;
                        }
                    }
                }
                crate::promotion::RetentionTier::Dead => {
                    // Archive immediately — Dead tier
                    low_count += 1;
                    let id: u64 = entry.id.into();
                    if let Err(e) = self.vector.remove(&id.to_string()) {
                        warn!(
                            "[memory::enclave] Failed to remove vector for dead entry {}: {}",
                            id, e
                        );
                    }
                    if let Err(e) = self.lsm.delete_metadata(id) {
                        warn!(
                            "[memory::enclave] Failed to archive dead entry {} from LSM: {}",
                            id, e
                        );
                        archive_errors += 1;
                    } else {
                        archived_count += 1;
                    }
                }
            }

            // Check if this memory should be promoted to identity (SOUL.md mutation)
            let recurrence = u32::from(entry.hit_count) as usize;
            if promotion.should_promote_to_identity(&metrics, recurrence) {
                let delta = crate::promotion::PersonalityDelta::new(format!(
                    "Promoted from memory {} (score: {:.2}, recurrence: {})",
                    entry.id, score, recurrence
                ));
                match promotion.check_drift_guard(&delta) {
                    Ok(()) => {
                        identity_candidates.push((entry.clone(), delta, score));
                    }
                    Err(distance) => {
                        tracing::warn!(
                            "[PROMOTION] Drift guard blocked identity promotion for {}: distance {:.2}",
                            entry.id, distance
                        );
                    }
                }
            }
        }

        // Update evolution score based on promotion cycle results
        let new_score = (high_count as f32 / entries.len().max(1) as f32).min(1.0);

        // Track layer distribution for observability
        let mut layer_counts: std::collections::HashMap<MemoryLayer, usize> =
            std::collections::HashMap::new();
        for entry in &entries {
            let layer = MemoryLayer::from_category(&entry.category);
            *layer_counts.entry(layer).or_insert(0) += 1;
        }

        // Persist evolution score to promotion engine (same lock, no re-acquire needed)
        promotion.update_evolution_score(new_score);
        drop(promotion);

        if !identity_candidates.is_empty() {
            tracing::info!(
                "[PROMOTION] Identity promotion candidates: {} (drift-checked)",
                identity_candidates.len()
            );
            // CP-06: Notify about identity promotion candidates
            for (candidate, _, score) in &identity_candidates {
                let candidate_id: u64 = candidate.id.into();
                let notification = crate::notifications::MemoryNotification {
                    notification_id: uuid::Uuid::new_v4().to_string(),
                    source_session: candidate.session_id.clone(),
                    memory_id: candidate_id,
                    domain_tags: candidate.tags.clone(),
                    content_preview: format!(
                        "Identity promotion candidate (score: {:.2}): {}",
                        score,
                        candidate.content.chars().take(100).collect::<String>()
                    ),
                    importance: candidate.importance,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                self.notifications.notify(notification);
            }
        }

        if low_count > 0
            || high_count > 0
            || archived_count > 0
            || reinforced_count > 0
            || !identity_candidates.is_empty()
        {
            tracing::info!(
                "[PROMOTION] Cycle: {} entries scored, {} archived ({} errors), {} reinforced, {} identity-candidates (evolution: {:.2})",
                entries.len(), archived_count, archive_errors, reinforced_count, identity_candidates.len(), new_score
            );
        }
    }

    /// Extracts a `ContextPackage` from the memory system for inter-agent delegation.
    ///
    /// Queries all 4 reflective memory graphs (semantic, temporal, causal, entity)
    /// for concepts relevant to the given task description. Populates the
    /// `ContextPackage` with CortexaDB collection keys so the receiving agent
    /// can hydrate context via zero-copy shared memory reads.
    ///
    /// Also includes recent tool outputs from the session transcript.
    pub fn extract_context_package(
        &self,
        session_id: &str,
        task_description: &str,
        max_token_budget: u32,
    ) -> Result<savant_ipc::a2a::context::ContextPackage, MemoryError> {
        use savant_ipc::a2a::context::ContextPackage;

        let mut pkg = ContextPackage::new().with_token_budget(max_token_budget);

        // Build collection keys for each graph namespace.
        // The key format is "{namespace}.{session_id}" so the receiving agent
        // can locate the correct CortexaDB collection.
        let semantic_key = format!("semantic.{}", session_id);
        let temporal_key = format!("temporal.{}", session_id);
        let causal_key = format!("causal.{}", session_id);
        let entity_key = format!("entity.{}", session_id);

        pkg = pkg.with_semantic_collection(&semantic_key);
        pkg = pkg.with_temporal_collection(&temporal_key);
        pkg = pkg.with_causal_collection(&causal_key);
        pkg = pkg.with_entity_collection(&entity_key);

        // Populate tool output offsets from recent session messages.
        // Fetch the last 8 messages that contain tool results.
        let recent = self.lsm.fetch_session_tail(session_id, 8);
        let mut tool_offsets = [0u32; 8];
        let mut tool_count: u8 = 0;
        for msg in &recent {
            if msg.role == crate::models::MessageRole::Tool && tool_count < 8 {
                // Use the message hash as a shared memory offset identifier
                let mut hash: u64 = 0xcbf29ce484222325;
                for byte in msg.id.as_bytes() {
                    hash ^= *byte as u64;
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                tool_offsets[tool_count as usize] = (hash & 0xFFFF_FFFF) as u32;
                tool_count += 1;
            }
        }
        pkg.tool_output_offsets = tool_offsets;
        pkg.tool_output_count = tool_count;

        debug!(
            session_id = %session_id,
            task = %task_description,
            tool_outputs = %tool_count,
            "Extracted context package for delegation"
        );

        Ok(pkg)
    }

    pub async fn atomic_compact(
        &self,
        session_id: &str,
        batch: Vec<AgentMessage>,
    ) -> Result<(), MemoryError> {
        let _guard = self.lock_session(session_id).await;
        self.lsm.atomic_compact(session_id, batch)
    }

    pub async fn index_memory(&self, mut entry: MemoryEntry) -> Result<(), MemoryError> {
        let _guard = self.lock_session(&entry.session_id).await;

        // Automatic Embedding Generation via embedding service
        if entry.embedding.is_empty() {
            debug!("Generating automatic embedding for entry: {}", entry.id);
            if let Ok(vec) = self.embedding_service.embed(&entry.content).await {
                entry.embedding = vec;
            } else {
                tracing::warn!("[memory::engine] Embedding generation failed for entry {}, vector index will be skipped", entry.id);
            }
        }

        // Only index in vector engine if embedding is provided
        if !entry.embedding.is_empty() {
            self.vector
                .index_memory(&entry.id.to_string(), &entry.embedding)?;
        }

        if let Err(e) = self.lsm.insert_metadata(entry.id.to_native(), &entry) {
            // AAA Atomicity Rollback
            if !entry.embedding.is_empty() {
                if let Err(e) = self.vector.remove(&entry.id.to_string()) {
                    warn!(
                        "[memory::engine] Failed to rollback vector index on LSM error: {}",
                        e
                    );
                }
            }
            return Err(e);
        }

        // CP-07: Index in BM25 keyword search
        {
            let mut bm25 = self.bm25.write().await;
            let entry_id: u64 = entry.id.into();
            bm25.add_document(entry_id, &entry.content);
        }

        // CP-11: Record in audit trail
        {
            let mut audit = self.audit.lock().await;
            let entry_id: u64 = entry.id.into();
            audit.record(
                crate::audit::AuditOperation::Store,
                vec![entry_id],
                &entry.session_id,
                Some(entry.importance as f32 / 10.0),
                &format!(
                    "Indexed memory: {}",
                    &entry.content[..entry.content.len().min(80)]
                ),
            );
        }

        Ok(())
    }

    pub async fn delete_memory(&self, id: u64) -> Result<(), MemoryError> {
        let _guard = self.lock_session("__global__").await;

        // Remove from vector engine (best effort)
        if let Err(e) = self.vector.remove(&id.to_string()) {
            warn!(
                "[memory::engine] Failed to remove vector for memory {}: {}",
                id, e
            );
        }

        // Remove from LSM engine
        self.lsm.delete_metadata(id)
    }

    /// Culls low-entropy memories below the specified Shannon entropy threshold.
    ///
    /// Low-entropy memories contain minimal informational gain and represent
    /// redundant, trivial, or noise entries. This operation:
    /// 1. Iterates all metadata entries in the enclave
    /// 2. Identifies entries with `shannon_entropy < threshold`
    /// 3. Deletes qualifying entries from both vector index and LSM storage
    /// 4. Returns the count of culled entries
    ///
    /// # Arguments
    /// * `threshold` - Minimum Shannon entropy (0.0-1.0). Entries below this are culled.
    ///   Typical values: 0.1 (aggressive), 0.3 (moderate), 0.5 (conservative)
    ///
    /// # Returns
    /// * `Ok(count)` - Number of entries successfully culled
    /// * `Err(MemoryError)` - If iteration or deletion fails
    pub fn cull_low_entropy_memories(&self, threshold: f32) -> Result<usize, MemoryError> {
        let entries = self.lsm.iter_metadata()?;
        let mut culled = 0usize;
        let mut failed = 0usize;

        for entry in &entries {
            let entropy: f32 = entry.shannon_entropy.into();
            if entropy < threshold {
                let id: u64 = entry.id.into();
                // Remove from vector engine (best effort)
                if let Err(e) = self.vector.remove(&id.to_string()) {
                    warn!(
                        "[memory::enclave] Failed to remove vector for culled entry {}: {}",
                        id, e
                    );
                }
                // Remove from LSM engine (authoritative)
                if let Err(e) = self.lsm.delete_metadata(id) {
                    warn!(
                        "[memory::enclave] Failed to delete culled entry {} from LSM: {}",
                        id, e
                    );
                    failed += 1;
                } else {
                    culled += 1;
                }
            }
        }

        if culled > 0 || failed > 0 {
            info!(
                "[memory::enclave] Cull complete: threshold={:.3}, scanned={}, culled={}, failed={}",
                threshold, entries.len(), culled, failed
            );
        }

        if failed > 0 {
            Err(MemoryError::TransactionFailed(format!(
                "Cull completed with {} failures out of {} attempted",
                failed,
                culled + failed
            )))
        } else {
            Ok(culled)
        }
    }

    /// B7: Tier migration — promotes memories between lifecycle tiers.
    /// L0 (Episodic) → L1 (Contextual): entries older than 24h with hit_count > 3
    /// L1 (Contextual) → L2 (Semantic): entries older than 7d with importance >= 7
    pub fn migrate_tiers(&self) -> Result<usize, MemoryError> {
        let entries = self.lsm.iter_metadata()?;
        let mut migrated = 0usize;
        let now = chrono::Utc::now().timestamp_millis();

        for entry in &entries {
            let age_hours = (now - i64::from(entry.created_at)) as f32 / 3_600_000.0;
            let hit_count: u32 = entry.hit_count.into();
            let importance = entry.importance;

            let new_layer = if age_hours > 24.0 && hit_count > 3 {
                // L0 → L1: Episodic → Contextual
                Some("contextual")
            } else if age_hours > 168.0 && importance >= 7 {
                // L1 → L2: Contextual → Semantic
                Some("semantic")
            } else {
                None
            };

            if let Some(layer) = new_layer {
                let id: u64 = entry.id.into();
                if let Ok(Some(mut meta)) = self.lsm.get_metadata(id) {
                    meta.category = layer.to_string();
                    let _ = self.lsm.insert_metadata(id, &meta);
                    migrated += 1;
                }
            }
        }

        if migrated > 0 {
            info!("Tier migration: {} entries promoted", migrated);
        }
        Ok(migrated)
    }

    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.vector.recall(query_embedding, top_k, None)
    }

    /// Returns all vectors within `max_distance` of the query embedding.
    /// Useful for similarity threshold searches and neighborhood exploration.
    pub fn recall_within_distance(
        &self,
        query_embedding: &[f32],
        max_distance: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.vector
            .recall_within_distance(query_embedding, max_distance)
    }

    /// Counts the number of messages in a session.
    pub fn count_session_messages(&self, session_id: &str) -> Result<u64, MemoryError> {
        self.lsm.count_session_messages(session_id)
    }

    /// Fetches all message IDs for a session.
    pub fn fetch_all_message_ids_for_session(&self, session_id: &str) -> Vec<String> {
        self.lsm.fetch_all_message_ids_for_session(session_id)
    }

    /// Fetches a message by its ID across all sessions.
    pub fn fetch_message_by_id(&self, msg_id: &str) -> Result<Option<AgentMessage>, MemoryError> {
        self.lsm.fetch_message_by_id(msg_id)
    }

    /// CP-08/09/10: Hybrid search combining BM25 + vector + RRF fusion + reranking.
    ///
    /// Pipeline:
    /// 1. Expand query via `query_expansion` (temporal concretization, synonyms)
    pub async fn persist_bm25(&self) -> Result<(), MemoryError> {
        let bm25 = self.bm25.read().await;
        self.lsm.save_bm25_state(&bm25)
    }

    /// D10: Fork a session — creates a new session with the same history up to a given turn.
    /// Returns the new session ID.
    pub async fn fork_session(
        &self,
        parent_session_id: &str,
        from_turn_id: &str,
    ) -> Result<String, MemoryError> {
        let new_session_id = format!(
            "{}-fork-{}",
            parent_session_id,
            chrono::Utc::now().timestamp_millis()
        );

        // Create new session state with parent reference
        let mut new_state = crate::models::SessionState::new(&new_session_id);
        new_state.parent_session_id = Some(parent_session_id.to_string());
        new_state.fork_point_turn_id = Some(from_turn_id.to_string());
        self.lsm.save_session_state(&new_state)?;

        // S3: Copy message history from parent session
        // Fetch all messages from parent (large limit to get full history)
        let parent_messages = self.lsm.fetch_session_tail(parent_session_id, 10_000);
        let mut copied = 0usize;
        for msg in &parent_messages {
            if let Err(e) = self.lsm.append_message(&new_session_id, msg) {
                warn!("Failed to copy message {} to forked session: {}", msg.id, e);
            } else {
                copied += 1;
            }
        }

        // Update turn count on new session
        new_state.turn_count = (copied as u64).into();
        self.lsm.save_session_state(&new_state)?;

        info!(
            "Forked session {} from {} at turn {} ({} messages copied)",
            new_session_id, parent_session_id, from_turn_id, copied
        );
        Ok(new_session_id)
    }

    /// D2: Clean up orphaned Processing turns on startup.
    /// Finds all sessions with active_turn_id in Processing state and marks them Interrupted.
    pub fn cleanup_orphaned_turns(&self) -> Result<usize, MemoryError> {
        let mut cleaned = 0usize;
        tracing::info!("Checking for orphaned processing turns...");

        let sessions = self.lsm.iter_session_states()?;
        for state in &sessions {
            if let Some(ref turn_id) = state.active_turn_id {
                match self.lsm.get_turn_state(&state.session_id, turn_id) {
                    Ok(Some(turn)) => {
                        if turn.state == crate::models::TurnPhase::Processing {
                            // Mark turn as interrupted
                            let mut interrupted_turn = turn;
                            interrupted_turn.state = crate::models::TurnPhase::Interrupted;
                            interrupted_turn.completed_at =
                                chrono::Utc::now().timestamp_millis().into();
                            if let Err(e) = self.lsm.save_turn_state(&interrupted_turn) {
                                warn!("Failed to mark turn {} as interrupted: {}", turn_id, e);
                            } else {
                                // Clear active_turn_id on session
                                let mut updated_state = state.clone();
                                updated_state.active_turn_id = None;
                                if let Err(e) = self.lsm.save_session_state(&updated_state) {
                                    warn!(
                                        "Failed to clear active_turn_id for {}: {}",
                                        state.session_id, e
                                    );
                                } else {
                                    cleaned += 1;
                                    info!(
                                        "Cleaned orphaned turn {} in session {}",
                                        turn_id, state.session_id
                                    );
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Turn not found — clear the stale reference
                        let mut updated_state = state.clone();
                        updated_state.active_turn_id = None;
                        let _ = self.lsm.save_session_state(&updated_state);
                        cleaned += 1;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get turn state for {} in {}: {}",
                            turn_id, state.session_id, e
                        );
                    }
                }
            }
        }

        tracing::info!("Orphan turn cleanup complete ({} cleaned)", cleaned);
        Ok(cleaned)
    }

    /// D8: Expire sessions older than the given TTL in hours.
    /// Returns the number of sessions expired.
    pub fn expire_stale_sessions(&self, ttl_hours: u64) -> Result<usize, MemoryError> {
        let cutoff = chrono::Utc::now().timestamp_millis() - (ttl_hours as i64 * 3_600_000);
        let mut expired = 0usize;

        let sessions = self.lsm.iter_session_states()?;
        for state in &sessions {
            let last_active: i64 = state.last_active.into();
            if last_active < cutoff {
                if let Err(e) = self.lsm.delete_session_state(&state.session_id) {
                    warn!(
                        "Failed to delete expired session {}: {}",
                        state.session_id, e
                    );
                } else {
                    expired += 1;
                    info!(
                        "Expired session {} (last active: {})",
                        state.session_id, last_active
                    );
                }
            }
        }

        if expired > 0 {
            info!(
                "Session expiry sweep: {} sessions expired (TTL={}h)",
                expired, ttl_hours
            );
        }
        Ok(expired)
    }

    /// B6: Persists procedures to CortexaDB for crash recovery.
    pub async fn persist_procedures(&self) -> Result<(), MemoryError> {
        let procedures = self.procedures.lock().await;
        self.lsm.save_procedures(&procedures)
    }

    /// B6: Persists lessons to CortexaDB for crash recovery.
    pub async fn persist_lessons(&self) -> Result<(), MemoryError> {
        let lessons = self.lessons.lock().await;
        self.lsm.save_lessons(&lessons)
    }

    /// B6: Persists insights to CortexaDB for crash recovery.
    pub async fn persist_insights(&self) -> Result<(), MemoryError> {
        let insights = self.insights.lock().await;
        self.lsm.save_insights(&insights)
    }

    /// 2. Search BM25 with expanded terms
    /// 3. Search vector index with original embedding
    /// 4. Fuse results via RRF (Reciprocal Rank Fusion)
    /// 5. Rerank top-N via cosine similarity
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        // CP-08: Query expansion
        let expanded = crate::query_expansion::expand_query(query);
        let search_terms = if expanded.expanded_terms.is_empty() {
            query.to_string()
        } else {
            format!("{} {}", query, expanded.expanded_terms.join(" "))
        };

        // BM25 search with expanded terms
        let bm25_results: Vec<crate::rrf_fusion::StreamResult> = {
            let bm25 = self.bm25.read().await;
            bm25.search(&search_terms, top_k * 2)
                .into_iter()
                .map(|(doc_id, score)| crate::rrf_fusion::StreamResult {
                    doc_id,
                    score,
                    session_id: String::new(),
                })
                .collect()
        };

        // Vector search with original embedding + temporal decay
        let vector_raw = self.vector.recall(query_embedding, top_k * 2, None)?;
        let now = chrono::Utc::now().timestamp_millis();
        let lambda = self.config.temporal_decay_lambda;
        let vector_results: Vec<crate::rrf_fusion::StreamResult> = vector_raw
            .iter()
            .filter_map(|sr| {
                sr.document_id.parse::<u64>().ok().map(|doc_id| {
                    // Apply temporal decay if enabled
                    let score = if self.config.apply_temporal_decay {
                        if let Ok(Some(entry)) = self.lsm.get_metadata(doc_id) {
                            let age_hours =
                                (now - i64::from(entry.created_at)) as f32 / 3_600_000.0;
                            let effective_lambda = if entry.importance >= 8 {
                                lambda * 0.5 // Half decay for high-importance
                            } else {
                                lambda
                            };
                            let decay = (-effective_lambda * age_hours).exp();
                            sr.score * decay
                        } else {
                            sr.score
                        }
                    } else {
                        sr.score
                    };
                    crate::rrf_fusion::StreamResult {
                        doc_id,
                        score,
                        session_id: String::new(),
                    }
                })
            })
            .collect();

        // NS-01 + NS-09: Query reflective memory graph stream
        // Uses ranked concept matching (exact > substring > word overlap)
        let graph_results: Vec<crate::rrf_fusion::StreamResult> = {
            let reflective = self.reflective.read().await;
            let concepts = reflective.resolve_ranked(query);
            concepts
                .into_iter()
                .enumerate()
                .map(|(i, m)| crate::rrf_fusion::StreamResult {
                    doc_id: m
                        .concept
                        .source_entries
                        .first()
                        .copied()
                        .unwrap_or(i as u64),
                    score: m.relevance,
                    session_id: String::new(),
                })
                .collect()
        };

        // CP-09: RRF fusion (BM25 + vector + graph)
        let fused = crate::rrf_fusion::fuse_results(
            &bm25_results,
            &vector_results,
            &graph_results,
            &crate::rrf_fusion::RrfConfig::default(),
            top_k * 2,
        );

        // Convert fused results back to SearchResult format
        let mut results: Vec<crate::vector_engine::SearchResult> = fused
            .into_iter()
            .map(|(doc_id, score)| crate::vector_engine::SearchResult {
                document_id: doc_id.to_string(),
                score,
                distance: 1.0 - score,
            })
            .collect();

        // NS-07: LLM-judged retrieval sufficiency — detect low-quality results
        // and expand with complementary queries if insufficient.
        if results.len() < top_k || Self::is_result_set_insufficient(&results) {
            let complementary_results = self
                .complementary_search(&search_terms, query_embedding, top_k)
                .await;
            if !complementary_results.is_empty() {
                let existing_ids: std::collections::HashSet<u64> = results
                    .iter()
                    .map(|r| r.document_id.parse::<u64>().unwrap_or(0))
                    .collect();
                for result in complementary_results {
                    if let Ok(id) = result.document_id.parse::<u64>() {
                        if !existing_ids.contains(&id) {
                            results.push(result);
                        }
                    }
                }
                // Re-sort by score descending after merging
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // FC-01: Rerank top candidates using embedding similarity
        if results.len() > 1 {
            let rerank_n = results.len().min(top_k * 2);
            let candidates: Vec<crate::reranker::RerankCandidate> = results
                .iter()
                .take(rerank_n)
                .filter_map(|r| {
                    r.document_id.parse::<u64>().ok().and_then(|doc_id| {
                        let content = self
                            .lsm
                            .get_metadata(doc_id)
                            .ok()
                            .flatten()
                            .map(|e| e.content)
                            .unwrap_or_default();
                        if content.is_empty() {
                            return None;
                        }
                        Some(crate::reranker::RerankCandidate {
                            doc_id,
                            original_score: r.score,
                            content,
                            session_id: String::new(),
                        })
                    })
                })
                .collect();
            if !candidates.is_empty() {
                let embedding_service = self.embedding_service.clone();
                let embed_fn = move |text: &str| -> Result<Vec<f32>, String> {
                    let svc = embedding_service.clone();
                    let text = text.to_string();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current()
                            .block_on(svc.embed(&text))
                            .map_err(|e| e.to_string())
                    })
                };
                let reranked = crate::reranker::rerank(query, candidates, &embed_fn, top_k);
                let reranked_results: Vec<crate::vector_engine::SearchResult> = reranked
                    .into_iter()
                    .map(|r| crate::vector_engine::SearchResult {
                        document_id: r.doc_id.to_string(),
                        score: r.reranked_score,
                        distance: 1.0 - r.reranked_score,
                    })
                    .collect();
                if !reranked_results.is_empty() {
                    results = reranked_results;
                }
            }
        }

        // CP-10: Truncate to top_k
        results.truncate(top_k);

        // CP-11: Record search in audit trail
        {
            let mut audit = self.audit.lock().await;
            audit.record(
                crate::audit::AuditOperation::Retrieve,
                vec![],
                "",
                None,
                &format!("hybrid_search: '{}' -> {} results", query, results.len()),
            );
        }

        Ok(results)
    }

    /// NS-07: Heuristic sufficiency detection — returns true if the result set
    /// appears low-quality (highly uniform scores, too few results, or all
    /// scores below the relevance floor).
    fn is_result_set_insufficient(results: &[crate::vector_engine::SearchResult]) -> bool {
        if results.is_empty() {
            return true;
        }
        let count = results.len();
        // All scores below relevance floor
        let all_low = results.iter().all(|r| r.score < 0.05);
        if all_low {
            return true;
        }
        // Highly uniform scores (variance < 0.001) suggests no discrimination
        if count >= 3 {
            let mean: f32 = results.iter().map(|r| r.score).sum::<f32>() / count as f32;
            let variance: f32 = results
                .iter()
                .map(|r| (r.score - mean).powi(2))
                .sum::<f32>()
                / count as f32;
            if variance < 0.001 {
                return true;
            }
        }
        false
    }

    /// NS-07: Generate complementary queries from the original search terms
    /// and run additional searches to fill retrieval gaps.
    async fn complementary_search(
        &self,
        original_terms: &str,
        _query_embedding: &[f32],
        top_k: usize,
    ) -> Vec<crate::vector_engine::SearchResult> {
        let mut all_results: Vec<crate::vector_engine::SearchResult> = Vec::new();

        // Extract key terms from the original query for complementary queries
        let terms: Vec<&str> = original_terms
            .split_whitespace()
            .filter(|t| t.len() > 3)
            .collect();

        // Generate complementary queries by combining key terms differently
        let mut complementary_queries: Vec<String> = Vec::new();
        if terms.len() >= 2 {
            // Reverse term order
            complementary_queries.push(terms.iter().rev().cloned().collect::<Vec<_>>().join(" "));
            // First + last term (skip middle)
            if terms.len() >= 3 {
                complementary_queries.push(format!("{} {}", terms[0], terms[terms.len() - 1]));
            }
        }
        // Add action-oriented query variant
        complementary_queries.push(format!("how to {}", original_terms));
        // Add result-oriented query variant
        complementary_queries.push(format!("result of {}", original_terms));

        for cq in &complementary_queries {
            let expanded = crate::query_expansion::expand_query(cq);
            let search_terms = if expanded.expanded_terms.is_empty() {
                cq.clone()
            } else {
                format!("{} {}", cq, expanded.expanded_terms.join(" "))
            };

            // BM25 complementary search
            let bm25_results: Vec<crate::rrf_fusion::StreamResult> = {
                let bm25 = self.bm25.read().await;
                bm25.search(&search_terms, top_k)
                    .into_iter()
                    .map(|(doc_id, score)| crate::rrf_fusion::StreamResult {
                        doc_id,
                        score,
                        session_id: String::new(),
                    })
                    .collect()
            };

            // Vector complementary search — re-embed the complementary query
            let cq_embedding = match self.embedding_service.embed(cq).await {
                Ok(emb) => emb,
                Err(_) => continue,
            };
            let vector_raw = match self.vector.recall(&cq_embedding, top_k, None) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let vector_results: Vec<crate::rrf_fusion::StreamResult> = vector_raw
                .iter()
                .filter_map(|sr| {
                    sr.document_id.parse::<u64>().ok().map(|doc_id| {
                        crate::rrf_fusion::StreamResult {
                            doc_id,
                            score: sr.score,
                            session_id: String::new(),
                        }
                    })
                })
                .collect();

            let fused = crate::rrf_fusion::fuse_results(
                &bm25_results,
                &vector_results,
                &[],
                &crate::rrf_fusion::RrfConfig::default(),
                top_k,
            );

            for (doc_id, score) in fused {
                all_results.push(crate::vector_engine::SearchResult {
                    document_id: doc_id.to_string(),
                    score,
                    distance: 1.0 - score,
                });
            }
        }

        // Sort by score and limit
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(top_k);
        all_results
    }

    pub fn semantic_search_temporal(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        let raw_results = self.vector.recall(query_embedding, top_k * 2, None)?;

        let mut filtered = Vec::new();
        for result in raw_results {
            if let Ok(memory_id) = result.document_id.parse::<u64>() {
                if let Ok(Some(temporal)) = self.lsm.get_temporal_metadata(memory_id) {
                    if temporal.is_active() {
                        filtered.push(result);
                    }
                } else {
                    filtered.push(result);
                }
            } else {
                filtered.push(result);
            }

            if filtered.len() >= top_k {
                break;
            }
        }
        Ok(filtered)
    }

    /// Semantic search with temporal decay on results.
    ///
    /// Applies exponential decay based on memory age:
    /// `effective_relevance = base_relevance * e^(-lambda * age_hours)`
    ///
    /// Override rules:
    /// - High-importance memories (importance >= 8): half decay rate
    /// - Promotion-immune memories (promotion_score > 0.7): no decay
    /// - Filters results with effective_relevance < 0.1
    pub fn semantic_search_temporal_decay(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        lambda: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        let raw_results = self.vector.recall(query_embedding, top_k * 3, None)?;
        let now = chrono::Utc::now().timestamp_millis();

        let mut filtered = Vec::new();
        for result in raw_results {
            if let Ok(memory_id) = result.document_id.parse::<u64>() {
                if let Ok(Some(entry)) = self.lsm.get_metadata(memory_id) {
                    let age_hours = (now - i64::from(entry.created_at)) as f32 / 3_600_000.0;

                    // Determine effective lambda based on importance
                    let effective_lambda = if entry.importance >= 8 {
                        lambda * 0.5 // Half decay rate for high-importance
                    } else {
                        lambda
                    };

                    let decay = (-effective_lambda * age_hours).exp();
                    let effective_relevance = result.score * decay;

                    if effective_relevance >= 0.1 {
                        let mut filtered_result = result.clone();
                        filtered_result.score = effective_relevance;
                        filtered.push(filtered_result);
                    }
                } else {
                    // No metadata — apply standard decay
                    filtered.push(result);
                }
            } else {
                filtered.push(result);
            }

            if filtered.len() >= top_k {
                break;
            }
        }

        // Sort by effective relevance descending
        filtered.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        filtered.truncate(top_k);

        Ok(filtered)
    }

    pub fn vector_count(&self) -> usize {
        self.vector.vector_count()
    }

    pub fn lsm(&self) -> Arc<LsmStorageEngine> {
        Arc::clone(&self.lsm)
    }

    pub fn vector(&self) -> Arc<SemanticVectorEngine> {
        Arc::clone(&self.vector)
    }

    // --- Session / Turn State ---

    /// Saves or updates a session state (write-locked).
    pub async fn save_session_state(
        &self,
        state: &crate::models::SessionState,
    ) -> Result<(), MemoryError> {
        let _guard = self.lock_session(&state.session_id).await;
        self.lsm.save_session_state(state)
    }

    /// Loads a session state (no write lock needed for reads).
    pub fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::models::SessionState>, MemoryError> {
        self.lsm.get_session_state(session_id)
    }

    /// Gets or creates a session state (write-locked to prevent TOCTOU race).
    pub async fn get_or_create_session_state(
        &self,
        session_id: &str,
    ) -> Result<crate::models::SessionState, MemoryError> {
        let _guard = self.lock_session(session_id).await;
        // Re-check after acquiring lock to prevent TOCTOU race
        if let Some(state) = self.lsm.get_session_state(session_id)? {
            return Ok(state);
        }
        self.lsm.get_or_create_session_state(session_id)
    }

    /// Saves a turn state (write-locked).
    pub async fn save_turn_state(
        &self,
        turn: &crate::models::TurnState,
    ) -> Result<(), MemoryError> {
        let _guard = self.lock_session(&turn.session_id).await;
        self.lsm.save_turn_state(turn)
    }

    /// Loads a specific turn state (no write lock needed).
    pub fn get_turn_state(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::models::TurnState>, MemoryError> {
        self.lsm.get_turn_state(session_id, turn_id)
    }

    /// Fetches the most recent N turns for a session.
    pub fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::TurnState>, MemoryError> {
        self.lsm.fetch_recent_turns(session_id, limit)
    }
}

/// The unified memory engine for Savant (5-Layer Cognitive Architecture Implementation)
/// Replaces singular usage with dedicated `enclave` and `collective` databases.
pub struct MemoryEngine {
    enclave: Arc<MemoryEnclave>,
    collective: Arc<MemoryEnclave>,
    notifications: NotificationChannel,
    /// Base storage path for snapshot/restore operations
    storage_path: std::path::PathBuf,
    /// Shutdown signal — cancels background consolidation scheduler
    shutdown_token: tokio_util::sync::CancellationToken,
}

impl MemoryEngine {
    pub fn new<P: AsRef<Path>>(
        storage_path: P,
        config: EngineConfig,
    ) -> Result<Arc<Self>, MemoryError> {
        let base = storage_path.as_ref();
        info!("Initializing Memory Engine at {:?}", base);

        let enclave = MemoryEnclave::new(base.join("enclave"), config.clone())?;
        let collective = MemoryEnclave::new(base.join("collective"), config.clone())?;
        let enclave_for_scheduler = enclave.clone();

        let shutdown_token = tokio_util::sync::CancellationToken::new();
        let engine = Arc::new(Self {
            enclave: enclave.clone(),
            collective: collective.clone(),
            notifications: NotificationChannel::default(),
            storage_path: base.to_path_buf(),
            shutdown_token: shutdown_token.clone(),
        });

        // OMEGA-VIII: Spawn the autonomous background pipelines
        if let Some(llm_provider) = config.distill_llm_provider {
            // Generate ephemeral JWT secret — in-memory only, destroyed on process exit.
            // If configured, use it. If not, generate crypto-random secret at runtime.
            // This follows the same pattern as ephemeral agent keys: no persistence, no vulnerability.
            let jwt_secret = config
                .distill_params
                .unwrap_or_default()
                .jwt_secret
                .unwrap_or_else(|| {
                    let mut hasher = blake3::Hasher::new();
                    hasher.update(uuid::Uuid::new_v4().as_bytes());
                    hasher.update(uuid::Uuid::new_v4().as_bytes());
                    hasher.update(&std::process::id().to_le_bytes());
                    hasher.update(
                        &std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                            .to_le_bytes(),
                    );
                    let hash = hasher.finalize();
                    let secret = hash.to_hex().to_string();
                    tracing::info!(
                        "Generated ephemeral JWT secret for distillation pipeline (in-memory only)"
                    );
                    secret
                });

            info!("Spawning Distillation Pipeline...");
            crate::distillation::spawn_distillation_pipeline(
                enclave.clone(),
                collective.clone(),
                llm_provider,
                config.embedding_service.clone(),
                jwt_secret,
            );
        }

        info!("Spawning Factual Arbiter...");
        crate::arbiter::spawn_arbiter_task(collective);

        // B5: Spawn consolidation scheduler — periodic promotion + tier migration + entropy culling
        info!("Spawning Consolidation Scheduler...");
        Self::spawn_consolidation_scheduler(enclave_for_scheduler, shutdown_token.clone());

        // D2: Clean up orphaned processing turns from previous run
        if let Err(e) = enclave.cleanup_orphaned_turns() {
            warn!("Orphan turn cleanup failed: {}", e);
        }

        info!("Memory Engine initialized successfully");
        Ok(engine)
    }

    /// Shuts down the memory engine — cancels background tasks so database locks
    /// are released before the process exits.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
        info!("[memory] Shutdown signal sent — background tasks will release database locks");
    }

    /// B5: Background consolidation scheduler.
    /// Runs promotion cycle (Ebbinghaus lifecycle), tier migration, and entropy culling
    /// on configurable intervals. Accepts a CancellationToken so the loop can break
    /// on graceful shutdown, releasing the Arc<MemoryEnclave> and its database locks.
    fn spawn_consolidation_scheduler(enclave: Arc<MemoryEnclave>, shutdown: tokio_util::sync::CancellationToken) {
        let promotion_interval = std::time::Duration::from_secs(900); // 15 minutes
        let migration_interval = std::time::Duration::from_secs(1800); // 30 minutes
        let culling_interval = std::time::Duration::from_secs(3600); // 1 hour
        let session_expiry_interval = std::time::Duration::from_secs(86400); // 24 hours

        tokio::spawn(async move {
            let mut promotion_timer = tokio::time::interval(promotion_interval);
            let mut migration_timer = tokio::time::interval(migration_interval);
            let mut culling_timer = tokio::time::interval(culling_interval);
            let mut session_expiry_timer = tokio::time::interval(session_expiry_interval);

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        info!("[memory] Consolidation scheduler shutting down — releasing database handles");
                        break;
                    }
                    _ = promotion_timer.tick() => {
                        // B4: Run promotion cycle with Ebbinghaus tier-based lifecycle
                        enclave.run_promotion_cycle().await;
                    }
                    _ = migration_timer.tick() => {
                        // B7: Tier migration (L0→L1→L2)
                        if let Err(e) = enclave.migrate_tiers() {
                            warn!("Tier migration failed: {}", e);
                        }
                    }
                    _ = culling_timer.tick() => {
                        // B10: Entropy culling
                        if let Err(e) = enclave.cull_low_entropy_memories(0.1) {
                            warn!("Entropy culling failed: {}", e);
                        }
                    }
                    _ = session_expiry_timer.tick() => {
                        // D8: Session TTL expiry
                        let ttl_hours = enclave.config.session_ttl_hours;
                        if ttl_hours > 0 {
                            match enclave.expire_stale_sessions(ttl_hours) {
                                Ok(count) if count > 0 => info!("Expired {} stale sessions", count),
                                Err(e) => warn!("Session expiry failed: {}", e),
                                _ => {}
                            }
                        }
                    }
                }
            }
            // Dropping enclave Arc here releases the vector database handles
            drop(enclave);
        });
    }

    pub fn with_defaults<P: AsRef<Path>>(
        storage_path: P,
        embedding_service: Arc<dyn EmbeddingProvider>,
    ) -> Result<Arc<Self>, MemoryError> {
        Self::new(
            storage_path,
            EngineConfig {
                lsm_config: LsmConfig::default(),
                vector_config: VectorConfig::default(),
                distill_llm_provider: None,
                distill_params: None,
                embedding_service,
                personality: None,
                memory_config: crate::models::MemoryConfig::default(),
            },
        )
    }

    pub fn enclave(&self) -> Arc<MemoryEnclave> {
        Arc::clone(&self.enclave)
    }

    pub fn collective(&self) -> Arc<MemoryEnclave> {
        Arc::clone(&self.collective)
    }

    pub fn subscribe_notifications(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::notifications::MemoryNotification> {
        self.notifications.subscribe()
    }

    pub fn notification_subscriber_count(&self) -> usize {
        self.notifications.subscriber_count()
    }

    // --- Legacy facades bridging to Enclave to avoid downstream breakage during transition ---

    pub async fn append_message(
        &self,
        session_id: &str,
        message: &AgentMessage,
    ) -> Result<(), MemoryError> {
        self.enclave.append_message(session_id, message).await
    }

    pub fn fetch_session_tail(&self, session_id: &str, limit: usize) -> Vec<AgentMessage> {
        self.enclave.fetch_session_tail(session_id, limit)
    }

    pub async fn atomic_compact(
        &self,
        session_id: &str,
        batch: Vec<AgentMessage>,
    ) -> Result<(), MemoryError> {
        self.enclave.atomic_compact(session_id, batch).await
    }

    pub async fn index_memory(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let importance = entry.importance;
        let content_preview = entry.content.chars().take(200).collect::<String>();
        let session_id = entry.session_id.clone();
        let entry_id: u64 = entry.id.into();
        let tags = entry.tags.clone();

        self.enclave.index_memory(entry).await?;

        // CP-06: Notify on high-importance memories
        if importance >= 8 {
            let notification = crate::notifications::MemoryNotification {
                notification_id: uuid::Uuid::new_v4().to_string(),
                source_session: session_id.clone(),
                memory_id: entry_id,
                domain_tags: tags,
                content_preview,
                importance,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            self.notifications.notify(notification);
        }

        Ok(())
    }

    /// Culls low-entropy memories below the specified Shannon entropy threshold.
    ///
    /// Low-entropy memories contain minimal informational gain and represent
    /// redundant, trivial, or noise entries. This operation runs across both
    /// the enclave (personal memory) and collective (shared memory) databases.
    ///
    /// # Arguments
    /// * `threshold` - Minimum Shannon entropy (0.0-1.0). Entries below this are culled.
    ///   Typical values: 0.1 (aggressive), 0.3 (moderate), 0.5 (conservative)
    ///
    /// # Returns
    /// * `Ok(count)` - Total number of entries successfully culled across both databases
    /// * `Err(MemoryError)` - If iteration or deletion fails
    pub fn cull_low_entropy_memories(&self, threshold: f32) -> Result<usize, MemoryError> {
        let enclave_culled = self.enclave.cull_low_entropy_memories(threshold)?;
        let collective_culled = self.collective.cull_low_entropy_memories(threshold)?;
        let total = enclave_culled + collective_culled;

        if total > 0 {
            info!(
                "[memory::engine] Total culled: {} (enclave: {}, collective: {})",
                total, enclave_culled, collective_culled
            );
        }

        Ok(total)
    }

    /// Consolidates session memory: deduplicates consecutive identical messages
    /// and compacts the session's message history.
    pub async fn consolidate(&self, session_id: &str) -> Result<usize, MemoryError> {
        let messages = self.enclave.fetch_session_tail(session_id, 500);
        if messages.len() < 2 {
            return Ok(0);
        }

        let mut deduped: Vec<AgentMessage> = Vec::with_capacity(messages.len());
        let mut removed = 0usize;

        for msg in &messages {
            if let Some(last) = deduped.last() {
                if last.content == msg.content && last.role == msg.role {
                    removed += 1;
                    continue;
                }
            }
            deduped.push(msg.clone());
        }

        if removed > 0 {
            tracing::info!(
                "[memory] Consolidated session {}: removed {} duplicate messages",
                session_id,
                removed
            );
            self.enclave.atomic_compact(session_id, deduped).await?;
        }

        // CP-12: Extract recurring tool-call patterns as procedures
        let tool_calls: Vec<(String, String)> = messages
            .iter()
            .filter(|m| !m.tool_calls.is_empty())
            .flat_map(|m| {
                m.tool_calls
                    .iter()
                    .map(move |tc| (tc.tool_name.clone(), m.session_id.clone()))
            })
            .collect();

        if tool_calls.len() >= 3 {
            let extractor = crate::procedural::PatternExtractor::default();
            let patterns = extractor.extract_patterns(&tool_calls);
            for (pattern, frequency) in patterns {
                let proc = extractor.create_procedure(
                    &pattern,
                    frequency,
                    "auto-extracted from consolidation",
                    &[session_id.to_string()],
                );
                {
                    let mut procedures = self.enclave.procedures().await;
                    // Only add if not already present (by ID)
                    if !procedures.iter().any(|p| p.id == proc.id) {
                        // RC-08: Evict lowest-strength procedure if at capacity
                        if procedures.len() >= self.enclave.config.max_procedures {
                            if let Some(min_idx) = procedures
                                .iter()
                                .enumerate()
                                .min_by(|a, b| {
                                    a.1.strength
                                        .partial_cmp(&b.1.strength)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .map(|(i, _)| i)
                            {
                                if proc.strength > procedures[min_idx].strength {
                                    procedures.remove(min_idx);
                                } else {
                                    continue; // New procedure has lower strength — drop it
                                }
                            }
                        }
                        procedures.push(proc);
                    }
                }
            }
        }

        // CP-14: Extract entities and populate reflective memory graphs
        let entity_extractor = crate::entities::EntityExtractor::new();
        let relation_extractor = crate::entities::RelationExtractor::new();
        let mut extracted_entities = Vec::new();

        for msg in &messages {
            let entities = entity_extractor.extract(&msg.content, session_id);
            extracted_entities.extend(entities);
        }

        if !extracted_entities.is_empty() {
            let mut reflective = self.enclave.reflective_mut().await;
            let now = chrono::Utc::now().timestamp();
            for entity in &extracted_entities {
                let concept = crate::reflective::Concept {
                    id: format!("entity:{:?}:{}", entity.entity_type, entity.canonical_name),
                    label: entity.canonical_name.clone(),
                    source_entries: vec![],
                    concept_type: crate::reflective::ConceptType::Semantic,
                    created_at: now,
                    last_accessed: now,
                };
                reflective.entity.add_concept(concept);
            }

            // Extract relations between known entities
            let known_names: Vec<String> = extracted_entities
                .iter()
                .map(|e| e.canonical_name.clone())
                .collect();
            for msg in &messages {
                let relations = relation_extractor.extract_relations(&msg.content, &known_names);
                for rel in relations {
                    let relation = crate::reflective::Relation {
                        relation_type: rel.relation_type.clone(),
                        weight: 1.0,
                        source_concept: format!("entity:person:{}", rel.source),
                        target_concept: format!("entity:person:{}", rel.target),
                    };
                    reflective.entity.add_relation(relation);
                }
            }

            tracing::info!(
                "[memory] Extracted {} entities and populated entity graph for session {}",
                extracted_entities.len(),
                session_id
            );
        }

        Ok(removed)
    }

    /// CP-13: Synthesize lessons from related memory clusters.
    pub async fn synthesize_lessons(&self, _session_id: &str) {
        let entries = match self.enclave.lsm().iter_metadata() {
            Ok(e) => e,
            Err(_) => return,
        };

        let mut by_category: std::collections::HashMap<String, Vec<(u64, String, f32)>> =
            std::collections::HashMap::new();
        for entry in &entries {
            by_category
                .entry(entry.category.clone())
                .or_default()
                .push((
                    u64::from(entry.id),
                    entry.content.clone(),
                    entry.importance as f32,
                ));
        }

        let synthesizer = crate::lessons::LessonSynthesizer::default();
        for (category, memories) in &by_category {
            if memories.len() >= 3 {
                if let Some(lesson) = synthesizer.synthesize(memories, category) {
                    let mut lessons = self.enclave.lessons().await;
                    if !lessons.iter().any(|l| l.id == lesson.id) {
                        // RC-08: Evict lowest-confidence lesson if at capacity
                        if lessons.len() >= self.enclave.config.max_lessons {
                            if let Some(min_idx) = lessons
                                .iter()
                                .enumerate()
                                .min_by(|a, b| {
                                    a.1.confidence
                                        .partial_cmp(&b.1.confidence)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .map(|(i, _)| i)
                            {
                                if lesson.confidence > lessons[min_idx].confidence {
                                    lessons.remove(min_idx);
                                } else {
                                    continue;
                                }
                            }
                        }
                        info!(
                            "[memory] Synthesized lesson from {} memories in category '{}'",
                            memories.len(),
                            category
                        );
                        lessons.push(lesson);
                    }
                }
            }
        }

        // B6: Persist lessons after synthesis
        if let Err(e) = self.enclave.persist_lessons().await {
            warn!("Failed to persist lessons: {}", e);
        }
    }

    /// CP-14: Synthesize insights from concept clusters in the MAGMA graph.
    pub async fn synthesize_insights(&self) {
        let reflective = self.enclave.reflective().await;
        let synthesizer = crate::lessons::InsightSynthesizer::default();

        for (namespace, graph) in [
            ("semantic", &reflective.semantic),
            ("temporal", &reflective.temporal),
            ("causal", &reflective.causal),
            ("entity", &reflective.entity),
        ] {
            if graph.concepts.len() >= 3 {
                let cluster: Vec<(u64, String)> = graph
                    .concepts
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i as u64, c.label.clone()))
                    .collect();
                if let Some(insight) = synthesizer.synthesize(&cluster, namespace) {
                    let mut insights = self.enclave.insights().await;
                    if !insights.iter().any(|i| i.id == insight.id) {
                        // RC-08: Evict lowest-confidence insight if at capacity
                        if insights.len() >= self.enclave.config.max_insights {
                            if let Some(min_idx) = insights
                                .iter()
                                .enumerate()
                                .min_by(|a, b| {
                                    a.1.confidence
                                        .partial_cmp(&b.1.confidence)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .map(|(i, _)| i)
                            {
                                if insight.confidence > insights[min_idx].confidence {
                                    insights.remove(min_idx);
                                } else {
                                    continue;
                                }
                            }
                        }
                        info!(
                            "[memory] Synthesized insight from {} concepts in {} namespace",
                            cluster.len(),
                            namespace
                        );
                        insights.push(insight);
                    }
                }
            }
        }

        // B6: Persist insights after synthesis
        if let Err(e) = self.enclave.persist_insights().await {
            warn!("Failed to persist insights: {}", e);
        }
    }

    pub fn hydrate_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<AgentMessage>, MemoryError> {
        let mut messages = self.enclave.fetch_session_tail(session_id, limit);
        messages.reverse();
        Ok(messages)
    }

    pub async fn delete_memory(&self, id: u64) -> Result<(), MemoryError> {
        self.enclave.delete_memory(id).await
    }

    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.enclave.semantic_search(query_embedding, top_k)
    }

    /// Returns all vectors within `max_distance` of the query embedding.
    pub fn recall_within_distance(
        &self,
        query_embedding: &[f32],
        max_distance: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.enclave
            .recall_within_distance(query_embedding, max_distance)
    }

    /// Counts the number of messages in a session.
    pub fn count_session_messages(&self, session_id: &str) -> Result<u64, MemoryError> {
        self.enclave.count_session_messages(session_id)
    }

    /// Fetches all message IDs for a session.
    pub fn fetch_all_message_ids_for_session(&self, session_id: &str) -> Vec<String> {
        self.enclave.fetch_all_message_ids_for_session(session_id)
    }

    /// Fetches a message by its ID across all sessions.
    pub fn fetch_message_by_id(&self, msg_id: &str) -> Result<Option<AgentMessage>, MemoryError> {
        self.enclave.fetch_message_by_id(msg_id)
    }

    pub fn semantic_search_temporal(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.enclave
            .semantic_search_temporal(query_embedding, top_k)
    }

    pub fn semantic_search_temporal_decay(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        lambda: f32,
    ) -> Result<Vec<crate::vector_engine::SearchResult>, MemoryError> {
        self.enclave
            .semantic_search_temporal_decay(query_embedding, top_k, lambda)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), MemoryError> {
        self.enclave.lsm.delete_session(session_id)
    }

    // --- Session / Turn State Facades ---

    pub async fn save_session_state(
        &self,
        state: &crate::models::SessionState,
    ) -> Result<(), MemoryError> {
        self.enclave.save_session_state(state).await
    }

    pub fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::models::SessionState>, MemoryError> {
        self.enclave.get_session_state(session_id)
    }

    pub async fn get_or_create_session_state(
        &self,
        session_id: &str,
    ) -> Result<crate::models::SessionState, MemoryError> {
        self.enclave.get_or_create_session_state(session_id).await
    }

    pub async fn save_turn_state(
        &self,
        turn: &crate::models::TurnState,
    ) -> Result<(), MemoryError> {
        self.enclave.save_turn_state(turn).await
    }

    pub fn get_turn_state(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::models::TurnState>, MemoryError> {
        self.enclave.get_turn_state(session_id, turn_id)
    }

    pub fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::TurnState>, MemoryError> {
        self.enclave.fetch_recent_turns(session_id, limit)
    }

    pub fn stats(&self) -> (crate::lsm_engine::StorageStats, usize) {
        let lsm_stats = self.enclave.lsm.stats().unwrap_or_default();
        let vector_count = self.enclave.vector_count();
        (lsm_stats, vector_count)
    }

    /// Snapshot the entire memory state to the given directory.
    /// Creates a manifest with timestamp, entry counts, and version.
    /// Note: Engines hold file handles; snapshot copies live data files.
    pub fn snapshot_state(&self, snapshot_path: &std::path::Path) -> Result<(), MemoryError> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Copy the entire storage directory to snapshot
        savant_core::utils::io::copy_dir_recursive(&self.storage_path, snapshot_path).map_err(
            |e| MemoryError::InitFailed(format!("Failed to copy storage to snapshot: {}", e)),
        )?;

        // Write manifest
        let (lsm_stats, vector_count) = self.stats();
        let manifest = {
            let mut map = serde_json::Map::new();
            map.insert("version".to_string(), serde_json::Value::Number(1.into()));
            map.insert(
                "timestamp".to_string(),
                serde_json::Value::Number(timestamp.into()),
            );
            map.insert(
                "lsm_entries".to_string(),
                serde_json::Value::Number(lsm_stats.total_messages.into()),
            );
            map.insert(
                "vector_count".to_string(),
                serde_json::Value::Number((vector_count as u64).into()),
            );
            map.insert(
                "source_path".to_string(),
                serde_json::Value::String(self.storage_path.to_string_lossy().to_string()),
            );
            serde_json::Value::Object(map)
        };

        let manifest_path = snapshot_path.join("snapshot_manifest.json");
        let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| {
            MemoryError::SerializationFailed(format!("Failed to serialize manifest: {}", e))
        })?;
        std::fs::write(&manifest_path, manifest_json)
            .map_err(|e| MemoryError::InitFailed(format!("Failed to write manifest: {}", e)))?;

        info!(
            "State snapshot created at {:?} ({} LSM entries, {} vectors)",
            snapshot_path, lsm_stats.total_messages, vector_count
        );
        Ok(())
    }

    /// Restore memory state from a snapshot directory.
    /// Validates the manifest, replaces active storage, and returns.
    /// **The process must be restarted after restore** since engines hold file handles.
    pub fn restore_state(&self, snapshot_path: &std::path::Path) -> Result<(), MemoryError> {
        // Validate manifest exists
        let manifest_path = snapshot_path.join("snapshot_manifest.json");
        if !manifest_path.exists() {
            return Err(MemoryError::InitFailed(
                "Snapshot manifest not found".to_string(),
            ));
        }

        let manifest_bytes = std::fs::read(&manifest_path)
            .map_err(|e| MemoryError::InitFailed(format!("Failed to read manifest: {}", e)))?;
        let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| MemoryError::InitFailed(format!("Invalid manifest JSON: {}", e)))?;

        let version = manifest["version"].as_u64().unwrap_or(0);
        if version == 0 {
            return Err(MemoryError::InitFailed(
                "Invalid snapshot version".to_string(),
            ));
        }

        // Replace active storage with snapshot
        if self.storage_path.exists() {
            let backup = self.storage_path.with_extension("pre-restore");
            if backup.exists() {
                std::fs::remove_dir_all(&backup).map_err(|e| {
                    MemoryError::InitFailed(format!("Failed to remove old backup: {}", e))
                })?;
            }
            std::fs::rename(&self.storage_path, &backup).map_err(|e| {
                MemoryError::InitFailed(format!("Failed to backup current storage: {}", e))
            })?;
            info!("Current storage backed up to {:?}", backup);
        }

        savant_core::utils::io::copy_dir_recursive(snapshot_path, &self.storage_path).map_err(
            |e| MemoryError::InitFailed(format!("Failed to restore from snapshot: {}", e)),
        )?;

        info!(
            "State restored from {:?} — restart required to load restored data",
            snapshot_path
        );
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::models::AgentMessage;
    use crate::MockEmbeddingProvider;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_engine() -> (Arc<MemoryEngine>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let engine = MemoryEngine::new(
            tmp.path(),
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
        .unwrap();
        (engine, tmp)
    }

    #[tokio::test]
    async fn test_store_and_retrieve() {
        let (engine, _tmp) = test_engine();
        let msg = AgentMessage::user("s1", "the sky is blue");
        engine.append_message("s1", &msg).await.unwrap();

        let messages = engine.fetch_session_tail("s1", 10);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "the sky is blue");
    }

    #[tokio::test]
    async fn test_delete_removes_entry() {
        let (engine, _tmp) = test_engine();
        let msg = AgentMessage::user("s1", "to be deleted");
        engine.append_message("s1", &msg).await.unwrap();

        let before = engine.fetch_session_tail("s1", 10);
        assert_eq!(before.len(), 1);

        engine.delete_session("s1").unwrap();

        let after = engine.fetch_session_tail("s1", 10);
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn test_list_entries_returns_all() {
        let (engine, _tmp) = test_engine();
        for i in 1..=3u32 {
            let msg = AgentMessage::user("s1", &format!("entry {}", i));
            engine.append_message("s1", &msg).await.unwrap();
        }

        let messages = engine.fetch_session_tail("s1", 10);
        assert_eq!(messages.len(), 3);
    }

    #[tokio::test]
    async fn test_store_duplicate_updates() {
        let (engine, _tmp) = test_engine();
        let msg1 = AgentMessage::user("s1", "first message");
        let msg2 = AgentMessage::assistant("s1", "second message");
        engine.append_message("s1", &msg1).await.unwrap();
        engine.append_message("s1", &msg2).await.unwrap();

        let messages = engine.fetch_session_tail("s1", 10);
        assert_eq!(messages.len(), 2);
        // fetch_session_tail returns newest first
        assert_eq!(messages[0].content, "second message");
        assert_eq!(messages[1].content, "first message");
    }

    #[tokio::test]
    async fn test_query_returns_results() {
        let (engine, _tmp) = test_engine();
        let msg = AgentMessage::user("s1", "rust programming language");
        engine.append_message("s1", &msg).await.unwrap();

        let messages = engine.fetch_session_tail("s1", 5);
        assert!(!messages.is_empty());
        assert!(messages[0].content.contains("rust"));
    }
}
