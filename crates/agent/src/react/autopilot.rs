use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Verdict from the autopilot loop detector.
#[derive(Debug, Clone, PartialEq)]
pub enum AutopilotVerdict {
    /// Agent is making progress with diverse tool calls.
    Productive,
    /// Agent may be stuck — prompt a self-check.
    Suspicious,
    /// Agent is definitely stuck — terminate after max rounds.
    Stuck,
}

/// Record of a single tool call for diversity tracking.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub args_hash: u64,
    pub success: bool,
}

/// Tracks tool call patterns to detect stuck loops.
/// Complements SelfRepair's content-hash stuck detection with
/// parameter diversity analysis — catches "same tool, slightly
/// different args, still going nowhere" patterns.
/// Uses Arc internally so it can be cloned for async blocks.
#[derive(Clone)]
pub struct ToolCallTracker {
    window: Vec<ToolCallRecord>,
    max_window: usize,
    diversity_threshold: f64,
    success_threshold: f64,
    stuck_count: usize,
    max_stuck_rounds: usize,
}

impl Default for ToolCallTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCallTracker {
    pub fn new() -> Self {
        Self {
            window: Vec::new(),
            max_window: 10,
            diversity_threshold: 0.6,
            success_threshold: 0.7,
            stuck_count: 0,
            max_stuck_rounds: 3,
        }
    }

    /// Record a tool call result.
    pub fn record(&mut self, tool_name: &str, args: &str, success: bool) {
        let args_hash = {
            let mut hasher = DefaultHasher::new();
            args.hash(&mut hasher);
            tool_name.hash(&mut hasher);
            hasher.finish()
        };
        self.window.push(ToolCallRecord {
            tool_name: tool_name.to_string(),
            args_hash,
            success,
        });
        if self.window.len() > self.max_window {
            self.window.remove(0);
        }
    }

    /// Parameter diversity: unique args hashes / total in window.
    /// High diversity (>0.6) = agent is trying different approaches.
    /// Low diversity (<0.3) = agent is repeating itself.
    pub fn parameter_diversity(&self) -> f64 {
        if self.window.is_empty() {
            return 1.0;
        }
        let unique: std::collections::HashSet<u64> =
            self.window.iter().map(|r| r.args_hash).collect();
        unique.len() as f64 / self.window.len() as f64
    }

    /// Success rate: successful calls / total in window.
    pub fn success_rate(&self) -> f64 {
        if self.window.is_empty() {
            return 1.0;
        }
        let successes = self.window.iter().filter(|r| r.success).count();
        successes as f64 / self.window.len() as f64
    }

    /// Compute the current verdict based on diversity and success metrics.
    pub fn verdict(&self) -> AutopilotVerdict {
        if self.window.len() < 3 {
            return AutopilotVerdict::Productive;
        }
        let diversity = self.parameter_diversity();
        let success = self.success_rate();

        if diversity > self.diversity_threshold && success > self.success_threshold {
            AutopilotVerdict::Productive
        } else if diversity < 0.3 || success < 0.3 {
            AutopilotVerdict::Stuck
        } else {
            AutopilotVerdict::Suspicious
        }
    }

    /// Record a stuck verdict. Returns true if max stuck rounds exceeded.
    pub fn record_stuck(&mut self) -> bool {
        self.stuck_count += 1;
        self.stuck_count >= self.max_stuck_rounds
    }

    /// Reset stuck counter (called when agent is productive).
    pub fn reset_stuck(&mut self) {
        self.stuck_count = 0;
    }

    /// Whether the agent has been stuck for too many rounds.
    pub fn is_max_stuck(&self) -> bool {
        self.stuck_count >= self.max_stuck_rounds
    }

    /// Current stuck round count.
    pub fn stuck_rounds(&self) -> usize {
        self.stuck_count
    }

    /// Configured maximum stuck rounds before termination.
    pub fn max_stuck_rounds(&self) -> usize {
        self.max_stuck_rounds
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_productive_diverse_calls() {
        let mut tracker = ToolCallTracker::new();
        // Diverse tool calls with high success
        tracker.record("git", "status", true);
        tracker.record("cargo", "check", true);
        tracker.record("grep", "pattern_a", true);
        tracker.record("git", "diff", true);
        assert_eq!(tracker.verdict(), AutopilotVerdict::Productive);
        assert!(tracker.parameter_diversity() > 0.6);
    }

    #[test]
    fn test_stuck_same_args_low_success() {
        let mut tracker = ToolCallTracker::new();
        // Same tool, same args, all failing
        for _ in 0..5 {
            tracker.record("cargo", "build", false);
        }
        assert_eq!(tracker.verdict(), AutopilotVerdict::Stuck);
        assert!(tracker.parameter_diversity() < 0.3);
        assert!(tracker.success_rate() < 0.3);
    }

    #[test]
    fn test_suspicious_mixed_pattern() {
        let mut tracker = ToolCallTracker::new();
        // Some diversity, some failures
        tracker.record("cargo", "build", false);
        tracker.record("cargo", "check", true);
        tracker.record("cargo", "build", false);
        tracker.record("cargo", "check", true);
        // diversity = 0.5, success = 0.5 → Suspicious
        assert_eq!(tracker.verdict(), AutopilotVerdict::Suspicious);
    }

    #[test]
    fn test_window_eviction() {
        let mut tracker = ToolCallTracker::new();
        tracker.max_window = 3;
        tracker.record("a", "1", true);
        tracker.record("b", "2", true);
        tracker.record("c", "3", true);
        tracker.record("d", "4", true); // evicts "a"
        assert_eq!(tracker.window.len(), 3);
        assert_eq!(tracker.window.first().unwrap().tool_name, "b");
    }

    #[test]
    fn test_stuck_counter() {
        let mut tracker = ToolCallTracker::new();
        assert!(!tracker.is_max_stuck());
        assert!(!tracker.record_stuck()); // count=1
        assert!(!tracker.record_stuck()); // count=2
        assert!(tracker.record_stuck()); // count=3 >= max
        assert!(tracker.is_max_stuck());
        tracker.reset_stuck();
        assert!(!tracker.is_max_stuck());
    }

    #[test]
    fn test_empty_window_is_productive() {
        let tracker = ToolCallTracker::new();
        assert_eq!(tracker.verdict(), AutopilotVerdict::Productive);
        assert_eq!(tracker.parameter_diversity(), 1.0);
        assert_eq!(tracker.success_rate(), 1.0);
    }
}
