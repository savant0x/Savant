//! Loop Detector — multi-layered tool call loop detection.
//!
//! Prevents sub-agents from entering infinite loops when calling tools.
//! Inspired by Mercury Agent's multi-layered detection:
//! - Identical call threshold: same tool + same params = abort
//! - Failing call threshold: consecutive failures = abort
//! - Absolute max calls: total calls = abort
//! - Absolute max failures: total failures = abort

use std::collections::VecDeque;

/// Detected loop result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDetection {
    /// No loop detected — continue.
    Clear,
    /// Identical call detected N times.
    IdenticalLoop { tool_name: String, count: usize },
    /// Failing calls detected N times consecutively.
    FailingLoop { count: usize },
    /// Absolute max calls exceeded.
    MaxCallsExceeded { total: usize, max: usize },
    /// Absolute max failures exceeded.
    MaxFailuresExceeded { total: usize, max: usize },
}

/// Multi-layered loop detector for tool calls.
pub struct LoopDetector {
    call_history: VecDeque<(String, String)>, // (tool_name, params_hash)
    failure_streak: usize,
    total_calls: usize,
    total_failures: usize,
    identical_threshold: usize,
    failing_threshold: usize,
    absolute_max_calls: usize,
    absolute_max_failures: usize,
}

impl LoopDetector {
    pub fn new(
        identical_threshold: usize,
        failing_threshold: usize,
        absolute_max_calls: usize,
        absolute_max_failures: usize,
    ) -> Self {
        Self {
            call_history: VecDeque::new(),
            failure_streak: 0,
            total_calls: 0,
            total_failures: 0,
            identical_threshold,
            failing_threshold,
            absolute_max_calls,
            absolute_max_failures,
        }
    }

    /// Record a tool call and check for loops.
    pub fn record_call(
        &mut self,
        tool_name: &str,
        params_hash: &str,
        success: bool,
    ) -> LoopDetection {
        self.total_calls += 1;

        if !success {
            self.failure_streak += 1;
            self.total_failures += 1;
        } else {
            self.failure_streak = 0;
        }

        let key = (tool_name.to_string(), params_hash.to_string());
        self.call_history.push_back(key.clone());

        // Check absolute max calls
        if self.total_calls > self.absolute_max_calls {
            return LoopDetection::MaxCallsExceeded {
                total: self.total_calls,
                max: self.absolute_max_calls,
            };
        }

        // Check absolute max failures
        if self.total_failures > self.absolute_max_failures {
            return LoopDetection::MaxFailuresExceeded {
                total: self.total_failures,
                max: self.absolute_max_failures,
            };
        }

        // Check consecutive failure streak
        if self.failure_streak >= self.failing_threshold {
            return LoopDetection::FailingLoop {
                count: self.failure_streak,
            };
        }

        // Check identical call threshold
        let identical_count = self
            .call_history
            .iter()
            .rev()
            .take_while(|k| *k == &key)
            .count();
        if identical_count >= self.identical_threshold {
            return LoopDetection::IdenticalLoop {
                tool_name: tool_name.to_string(),
                count: identical_count,
            };
        }

        LoopDetection::Clear
    }

    /// Total calls recorded.
    pub fn total_calls(&self) -> usize {
        self.total_calls
    }

    /// Total failures recorded.
    pub fn total_failures(&self) -> usize {
        self.total_failures
    }

    /// Reset the detector.
    pub fn reset(&mut self) {
        self.call_history.clear();
        self.failure_streak = 0;
        self.total_calls = 0;
        self.total_failures = 0;
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_loop_detection() {
        let mut detector = LoopDetector::new(4, 6, 75, 20);

        assert_eq!(
            detector.record_call("read", "file.rs", true),
            LoopDetection::Clear
        );
        assert_eq!(
            detector.record_call("read", "file.rs", true),
            LoopDetection::Clear
        );
        assert_eq!(
            detector.record_call("read", "file.rs", true),
            LoopDetection::Clear
        );
        let result = detector.record_call("read", "file.rs", true);
        assert!(matches!(
            result,
            LoopDetection::IdenticalLoop { count: 4, .. }
        ));
    }

    #[test]
    fn test_failing_loop_detection() {
        let mut detector = LoopDetector::new(4, 3, 75, 20);

        assert_eq!(
            detector.record_call("write", "a.rs", false),
            LoopDetection::Clear
        );
        assert_eq!(
            detector.record_call("write", "b.rs", false),
            LoopDetection::Clear
        );
        let result = detector.record_call("write", "c.rs", false);
        assert!(matches!(result, LoopDetection::FailingLoop { count: 3 }));
    }

    #[test]
    fn test_failure_streak_resets_on_success() {
        let mut detector = LoopDetector::new(4, 3, 75, 20);

        assert_eq!(
            detector.record_call("write", "a.rs", false),
            LoopDetection::Clear
        );
        assert_eq!(
            detector.record_call("write", "b.rs", false),
            LoopDetection::Clear
        );
        assert_eq!(
            detector.record_call("read", "c.rs", true),
            LoopDetection::Clear
        ); // reset
        assert_eq!(
            detector.record_call("write", "d.rs", false),
            LoopDetection::Clear
        ); // streak = 1
    }

    #[test]
    fn test_max_calls_exceeded() {
        let mut detector = LoopDetector::new(4, 6, 3, 20);

        detector.record_call("a", "1", true);
        detector.record_call("b", "2", true);
        detector.record_call("c", "3", true);
        let result = detector.record_call("d", "4", true);
        assert!(matches!(
            result,
            LoopDetection::MaxCallsExceeded { total: 4, max: 3 }
        ));
    }
}
