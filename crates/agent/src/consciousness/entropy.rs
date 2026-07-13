//! Entropy Calculator — computes Shannon entropy of the hivemind's global state.

use std::hash::Hash;

/// Tracks hivemind state changes and computes entropy score.
pub struct EntropyCalculator {
    previous_state_hash: u64,
    state_change_count: u32,
    tool_call_count: u32,
    tick_count: u32,
}

impl Default for EntropyCalculator {
    fn default() -> Self {
        Self::new()
    }
}

impl EntropyCalculator {
    pub fn new() -> Self {
        Self {
            previous_state_hash: 0,
            state_change_count: 0,
            tool_call_count: 0,
            tick_count: 0,
        }
    }

    /// Compute entropy from current hivemind state.
    /// Returns a value between 0.0 (dormant) and 1.0 (hyper-active).
    pub fn calculate<T: Hash + std::fmt::Debug>(&mut self, current_state: &T) -> f64 {
        // Use blake3 for collision-resistant state hashing
        let state_bytes = format!("{:?}", current_state);
        let hash = blake3::hash(state_bytes.as_bytes());
        let current_hash = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or([0u8; 8]));

        // State change detection
        if current_hash != self.previous_state_hash {
            self.state_change_count += 1;
        }
        self.previous_state_hash = current_hash;
        self.tick_count += 1;

        // Compute entropy factors
        let change_rate = if self.tick_count > 0 {
            (self.state_change_count as f64 / self.tick_count as f64).min(1.0)
        } else {
            0.0
        };

        let tool_activity = (self.tool_call_count as f64 / 10.0).min(1.0);

        // Weighted entropy
        (change_rate * 0.6) + (tool_activity * 0.4)
    }

    /// Record a tool call for activity tracking.
    pub fn record_tool_call(&mut self) {
        self.tool_call_count += 1;
    }

    /// Reset counters (called periodically).
    pub fn reset(&mut self) {
        self.state_change_count = 0;
        self.tool_call_count = 0;
        self.tick_count = 0;
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_initial() {
        let mut calc = EntropyCalculator::new();
        let entropy = calc.calculate(&"initial_state");
        // First tick: no previous state to compare, so change_rate = 0
        assert!((0.0..=1.0).contains(&entropy));
    }

    #[test]
    fn test_entropy_increases_with_activity() {
        let mut calc = EntropyCalculator::new();
        let _ = calc.calculate(&"state_1");

        for i in 0..20 {
            calc.record_tool_call();
            let _ = calc.calculate(&format!("state_{}", i));
        }

        let entropy = calc.calculate(&"state_final");
        assert!(entropy > 0.0);
    }

    #[test]
    fn test_entropy_resets() {
        let mut calc = EntropyCalculator::new();
        for i in 0..10 {
            calc.record_tool_call();
            let _ = calc.calculate(&format!("state_{}", i));
        }
        calc.reset();
        let entropy = calc.calculate(&"same_state");
        assert!((0.0..=1.0).contains(&entropy));
    }
}
