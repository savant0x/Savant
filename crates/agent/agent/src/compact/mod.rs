//! Compact — Dual-Stage Tool Output Compression Engine
//!
//! L1: Deterministic rule-based compression at tool output insertion time.
//! L2: Semantic context window compression at threshold.
//! L1.5: Cross-tool semantic deduplication via HNSW.
//!
//! Research basis: Synthesizes OpenHuman TokenJuice (deterministic rules),
//! Hermes Agent (LLM summarization + smart tool summaries), and OpenClaw
//! (preventive caps + RTK rewriting). Enhanced with Savant-specific capabilities:
//! OCEAN personality-driven compression, HNSW deduplication, Nexus telemetry.

pub mod classify;
pub mod engine;
pub mod integration;
pub mod l2;
pub mod ocean;
pub mod overlay;
pub mod reduce;
pub mod rules;
pub mod schema;
pub mod semantic;
pub mod telemetry;

pub use classify::{ClassificationResult, RuleMatcher};
pub use engine::CompactEngine;
pub use integration::{compact_output, compact_output_sync, init, reload_rules, rule_count};
pub use l2::{L2Compressor, L2Stage, L2Thresholds};
pub use ocean::OceanScaler;
pub use overlay::ThreeLayerOverlay;
pub use reduce::ReductionPipeline;
pub use rules::RuleRegistry;
pub use schema::CompactionResult;
pub use schema::*;
pub use semantic::SemanticDeduplicator;
pub use telemetry::CompressionEvent;
