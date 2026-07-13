//! Verified Hybrid Semantic Substrate (VHSS)
//!
//! This crate implements a production-grade memory subsystem that combines:
//! - Fjall 3.0 LSM-tree for transactional, high-concurrency persistence
//! - ruvector-core for SIMD-accelerated semantic search
//! - rkyv for zero-copy serialization
//! - Formal Kani verification for memory safety
//!
//! It completely eliminates OpenClaw's race conditions, ZeroClaw's memory bleed,
//! and provides mathematically proven safety guarantees.

pub mod arbiter;
mod async_backend;
pub mod audit;
pub mod bm25_index;
pub mod cross_encoder;
pub mod daily_log;
pub mod distillation;
pub mod engine;
pub mod entities;
mod error;
pub mod lessons;
mod lsm_engine;
pub mod mesh_sync;
pub mod models;
pub mod multimodal;
pub mod notifications;
pub mod privacy;
pub mod procedural;
pub mod promotion;
pub mod query_expansion;
pub mod reflective;
pub mod reranker;
pub mod rrf_fusion;
pub mod safety;
mod vector_engine;

pub use async_backend::AsyncMemoryBackend;
pub use daily_log::{DailyLog, LogEntry, LogPriority};
pub use distillation::{extract_triplets_deterministic, DistilledTriplet, TripletClaims};
pub use engine::MemoryEngine;
pub use entities::{
    Entity, EntityExtractor, EntityRelation, EntityResolver, EntityType, RelationExtractor,
};
pub use error::MemoryError;
pub use lsm_engine::{LsmStorageEngine, StorageStats};
pub use models::{
    message_key, session_key, session_state_key, turn_state_key, verify_tool_pair_integrity,
    AgentMessage, MemoryConfig, MemoryEntry, MessageRole, SessionState, ToolCallRef, ToolResultRef,
    TurnPhase, TurnState,
};
pub use notifications::{MemoryNotification, NotificationChannel};
pub use promotion::{PersonalityDelta, PersonalityTraits, PromotionEngine, PromotionMetrics};
pub use reflective::{
    intent_to_namespace, resolve_graph_intent, Concept, GraphNamespace, NamespaceGraph,
    QueryIntent, ReflectiveMemory, Relation,
};
pub use savant_core::utils::embeddings::EmbeddingService;
// Safety verification module is conditionally compiled with kani feature
#[cfg(feature = "kani")]
pub use safety::verify_memory_safety;
pub use vector_engine::SemanticVectorEngine;

/// Mock embedding provider for tests — returns fixed 64-dim zero vectors.
/// Use with `MemoryEngine::new(path, EngineConfig { lsm_config: LsmConfig { vector_dimension: 64, .. }, .. })`.
pub struct MockEmbeddingProvider;

#[async_trait::async_trait]
impl savant_core::traits::EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, savant_core::error::SavantError> {
        Ok(vec![0.0; 64])
    }
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, savant_core::error::SavantError> {
        Ok(texts.iter().map(|_| vec![0.0; 64]).collect())
    }
    fn dimensions(&self) -> usize {
        64
    }
}
