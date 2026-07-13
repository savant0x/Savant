//! Oneiros Dream Engine — Sleep-Time Compute for Savant.
//!
//! Implements NREM and REM analog processing for autonomous memory consolidation
//! and latent space exploration during idle periods.
//!
//! # Architecture
//! - **NREM Phase**: Structured replay of recent episodic memories, compression,
//!   contradiction resolution, and persistent storage.
//! - **REM Phase**: Adversarial latent space exploration via constrained navigator,
//!   cross-domain concept recombination.
//! - **Scheduler**: Triggers dream cycles during idle periods, yields on activity.
//! - **Filter**: Evaluates dream outputs for diversity (Vendi Score) before storage.

pub mod filter;
pub mod nrem;
pub mod rem;
pub mod scheduler;
pub mod vendi;

use std::sync::atomic::AtomicBool;

/// Global dreaming flag shared between dream engine and heartbeat pulse.
/// When true, heartbeat pulse should skip to avoid resource contention.
pub static IS_DREAMING: AtomicBool = AtomicBool::new(false);

/// Dream cycle configuration.
#[derive(Debug, Clone)]
pub struct DreamConfig {
    /// Whether dream engine is enabled.
    pub enabled: bool,
    /// NREM phase duration in seconds.
    pub nrem_duration_secs: u64,
    /// REM phase duration in seconds.
    pub rem_duration_secs: u64,
    /// Idle threshold (delta score) below which dreaming activates.
    pub idle_threshold: f32,
    /// Minutes of continuous idle before dream cycle starts.
    pub idle_minutes: u64,
    /// Vendi Score threshold for dream output diversity.
    pub vendi_threshold: f32,
    /// Interval between idle checks in seconds.
    pub check_interval_secs: u64,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            nrem_duration_secs: 300,
            rem_duration_secs: 180,
            idle_threshold: 0.1,
            idle_minutes: 10,
            vendi_threshold: 0.3,
            check_interval_secs: 30,
        }
    }
}

/// Result of a complete dream cycle (NREM + REM).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DreamCycleResult {
    /// Unique cycle identifier.
    pub cycle_id: String,
    /// Number of memories consolidated in NREM phase.
    pub nrem_consolidated: usize,
    /// Number of novel associations generated in REM phase.
    pub rem_associations: usize,
    /// Vendi Score of REM outputs (diversity metric).
    pub rem_vendi_score: f32,
    /// Total duration of the cycle in milliseconds.
    pub duration_ms: u64,
    /// Whether the cycle was interrupted by environment activity.
    pub interrupted: bool,
}

/// Theme cluster discovered during REM Phase 2.
/// Emitted to outbox for vault projection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThemeCluster {
    /// Unique cluster identifier.
    pub cluster_id: String,
    /// Concept IDs that belong to this cluster.
    pub concept_ids: Vec<String>,
    /// Human-readable label for the theme cluster.
    pub label: String,
    /// Vendi Score of the cluster (diversity metric).
    pub vendi_score: f32,
}

/// Dream engine error types.
#[derive(Debug, thiserror::Error)]
pub enum DreamError {
    #[error("Dream engine disabled")]
    Disabled,

    #[error("Memory engine error: {0}")]
    MemoryError(String),

    #[error("LLM provider error: {0}")]
    LlmError(String),

    #[error("Cycle interrupted by environment activity")]
    Interrupted,

    #[error("Vendi Score below threshold: {score:.2} < {threshold:.2}")]
    LowDiversity { score: f32, threshold: f32 },
}
