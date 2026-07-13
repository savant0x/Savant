//! Self-Repair — detects stuck agents and broken tools.
//!
//! Two detection mechanisms:
//! 1. **Tool Health Tracker** — tracks consecutive failures per tool across turns.
//!    Tools with ≥ threshold failures are excluded from execution.
//! 2. **Stuck Detector** — tracks if the agent produces the same output repeatedly.
//!    If N consecutive outputs hash the same, the agent is stuck.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Default threshold for broken tool detection.
const DEFAULT_BROKEN_THRESHOLD: usize = 5;

/// Default threshold for stuck agent detection.
const DEFAULT_STUCK_THRESHOLD: usize = 3;

/// Tracks tool failure counts across turns.
#[derive(Debug, Clone, Default)]
pub struct ToolHealthTracker {
    /// tool_name → consecutive failure count
    failure_counts: HashMap<String, usize>,
    /// tool_name → last error message
    last_errors: HashMap<String, String>,
}

impl ToolHealthTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a successful tool execution — resets the failure counter.
    pub fn record_success(&mut self, tool_name: &str) {
        self.failure_counts.remove(tool_name);
        self.last_errors.remove(tool_name);
    }

    /// Records a failed tool execution — increments the failure counter.
    pub fn record_failure(&mut self, tool_name: &str, error: &str) {
        *self
            .failure_counts
            .entry(tool_name.to_string())
            .or_insert(0) += 1;
        self.last_errors
            .insert(tool_name.to_string(), error.to_string());
    }

    /// Returns tool names with ≥ threshold consecutive failures.
    pub fn broken_tools(&self, threshold: usize) -> Vec<String> {
        self.failure_counts
            .iter()
            .filter(|(_, &count)| count >= threshold)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Gets the failure count for a specific tool.
    pub fn get_failure_count(&self, tool_name: &str) -> usize {
        self.failure_counts.get(tool_name).copied().unwrap_or(0)
    }

    /// Gets the last error for a specific tool.
    pub fn get_last_error(&self, tool_name: &str) -> Option<&str> {
        self.last_errors.get(tool_name).map(|s| s.as_str())
    }
}

/// Detects stuck agents by tracking output hash repetition.
pub struct StuckDetector {
    /// Consecutive iterations with the same content hash
    no_progress_count: usize,
    /// Hash of the last content produced
    last_content_hash: u64,
    /// Number of consecutive same-hash outputs before triggering stuck
    threshold: usize,
}

impl StuckDetector {
    pub fn new(threshold: usize) -> Self {
        Self {
            no_progress_count: 0,
            last_content_hash: 0,
            threshold,
        }
    }

    /// Checks if the current content represents progress.
    /// Returns true if the agent appears stuck (same output N times).
    pub fn check(&mut self, content_hash: u64) -> bool {
        if content_hash == self.last_content_hash && content_hash != 0 {
            self.no_progress_count += 1;
        } else {
            self.no_progress_count = 0;
            self.last_content_hash = content_hash;
        }
        self.no_progress_count >= self.threshold
    }

    /// Resets the stuck detector.
    pub fn reset(&mut self) {
        self.no_progress_count = 0;
    }

    /// Gets the current no-progress count.
    pub fn progress_count(&self) -> usize {
        self.no_progress_count
    }
}

/// Self-repair outcome.
pub enum RepairOutcome {
    /// Recovery successful
    Recovered,
    /// Manual intervention required
    ManualRequired(String),
    /// Retry with modified context
    Retry,
}

/// Self-repair engine combining tool health tracking and stuck detection.
#[derive(Clone)]
pub struct SelfRepair {
    pub tool_health: Arc<RwLock<ToolHealthTracker>>,
    pub stuck_detector: Arc<RwLock<StuckDetector>>,
    broken_threshold: usize,
}

impl SelfRepair {
    pub fn new(broken_threshold: usize, stuck_threshold: usize) -> Self {
        Self {
            tool_health: Arc::new(RwLock::new(ToolHealthTracker::new())),
            stuck_detector: Arc::new(RwLock::new(StuckDetector::new(stuck_threshold))),
            broken_threshold,
        }
    }

    /// Creates a SelfRepair with default thresholds.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_BROKEN_THRESHOLD, DEFAULT_STUCK_THRESHOLD)
    }

    /// Called after each tool execution to track health.
    pub async fn on_tool_result(
        &self,
        tool_name: &str,
        result: &Result<String, savant_core::error::SavantError>,
    ) {
        let mut health = self.tool_health.write().await;
        match result {
            Ok(_) => health.record_success(tool_name),
            Err(e) => health.record_failure(tool_name, &e.to_string()),
        }
    }

    /// Gets tools that should be excluded due to repeated failures.
    pub async fn get_excluded_tools(&self) -> Vec<String> {
        let health = self.tool_health.read().await;
        health.broken_tools(self.broken_threshold)
    }

    /// Checks if the agent appears stuck based on content hash.
    pub async fn check_stuck(&self, content_hash: u64) -> bool {
        let mut detector = self.stuck_detector.write().await;
        detector.check(content_hash)
    }

    /// Resets stuck detection after recovery attempt.
    pub async fn reset_stuck(&self) {
        let mut detector = self.stuck_detector.write().await;
        detector.reset();
    }

    /// Returns a structured repair outcome based on current state.
    /// This is the enterprise-grade version of `recovery_hint()` that returns
    /// a typed enum instead of a raw string.
    pub async fn recovery_outcome(&self) -> RepairOutcome {
        let excluded = self.get_excluded_tools().await;
        if excluded.is_empty() {
            RepairOutcome::Retry
        } else {
            RepairOutcome::ManualRequired(format!(
                "Tools disabled: {}. Try a different approach.",
                excluded.join(", ")
            ))
        }
    }

    /// Generates a recovery hint message for the LLM.
    pub async fn recovery_hint(&self) -> String {
        let excluded = self.get_excluded_tools().await;
        if excluded.is_empty() {
            return "Your previous approach didn't work. Try a different strategy or tool."
                .to_string();
        }
        format!(
            "The following tools have repeatedly failed and are temporarily disabled: {}. \
             Try a different tool or approach to complete your task.",
            excluded.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_health_success_resets() {
        let mut tracker = ToolHealthTracker::new();
        tracker.record_failure("shell", "error 1");
        tracker.record_failure("shell", "error 2");
        assert_eq!(tracker.get_failure_count("shell"), 2);

        tracker.record_success("shell");
        assert_eq!(tracker.get_failure_count("shell"), 0);
    }

    #[test]
    fn test_tool_health_broken_threshold() {
        let mut tracker = ToolHealthTracker::new();
        for i in 0..5 {
            tracker.record_failure("shell", &format!("error {}", i));
        }
        let broken = tracker.broken_tools(5);
        assert_eq!(broken, vec!["shell".to_string()]);
    }

    #[test]
    fn test_stuck_detector_not_stuck() {
        let mut detector = StuckDetector::new(3);
        assert!(!detector.check(123));
        assert!(!detector.check(456));
        assert!(!detector.check(789));
    }

    #[test]
    fn test_stuck_detector_stuck() {
        let mut detector = StuckDetector::new(3);
        assert!(!detector.check(123)); // 1st: new hash, count=0
        assert!(!detector.check(123)); // 2nd: same hash, count=1
        assert!(!detector.check(123)); // 3rd: same hash, count=2
        assert!(detector.check(123)); // 4th: same hash, count=3 >= threshold
    }

    #[test]
    fn test_stuck_detector_reset() {
        let mut detector = StuckDetector::new(3);
        detector.check(123);
        detector.check(123);
        detector.reset();
        assert!(!detector.check(123));
    }
}
