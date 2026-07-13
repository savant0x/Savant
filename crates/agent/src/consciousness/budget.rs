//! Consciousness Budget — token/cost budget enforcement for the consciousness layer.
//!
//! Quiet hours default to 3AM–11AM UTC (11PM–7AM EDT).
//! Configurable via `[evolution].quiet_hours_start` / `quiet_hours_end` in savant.toml.

use std::time::Instant;

/// Token budget for consciousness operations.
pub struct ConsciousnessBudget {
    base_tokens_per_hour: u32,
    base_tokens_per_day: u32,
    tokens_per_hour: u32,
    tokens_per_day: u32,
    current_hour_tokens: u32,
    current_day_tokens: u32,
    /// Hour (UTC) when quiet hours begin. Default: 3 (3AM UTC = 11PM EDT).
    quiet_hours_start: u8,
    /// Hour (UTC) when quiet hours end. Default: 11 (11AM UTC = 7AM EDT).
    quiet_hours_end: u8,
    last_hourly_reset: Instant,
    last_daily_reset: Instant,
}

impl Default for ConsciousnessBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsciousnessBudget {
    /// Create a new budget with default quiet hours (3AM–11AM UTC = 11PM–7AM EDT).
    pub fn new() -> Self {
        Self::with_quiet_hours(3, 11)
    }

    /// Create a budget with custom quiet hours (UTC).
    pub fn with_quiet_hours(start_utc: u8, end_utc: u8) -> Self {
        let now = Instant::now();
        Self {
            base_tokens_per_hour: 100_000,
            base_tokens_per_day: 500_000,
            tokens_per_hour: 100_000,
            tokens_per_day: 500_000,
            current_hour_tokens: 0,
            current_day_tokens: 0,
            quiet_hours_start: start_utc.min(23),
            quiet_hours_end: end_utc.min(23),
            last_hourly_reset: now,
            last_daily_reset: now,
        }
    }

    /// Create a budget with no quiet hours (always active).
    pub fn always_active() -> Self {
        Self::with_quiet_hours(25, 25) // impossible hour = never quiet
    }

    /// Check if thinking is allowed right now.
    /// Automatically resets hourly/daily counters when the period elapses.
    pub fn can_think(&mut self) -> bool {
        self.auto_reset();

        if self.is_quiet_hours() {
            return false;
        }
        if self.current_hour_tokens >= self.tokens_per_hour {
            return false;
        }
        if self.current_day_tokens >= self.tokens_per_day {
            return false;
        }
        true
    }

    /// Record token usage.
    pub fn record_usage(&mut self, tokens: u32) {
        self.auto_reset();
        self.current_hour_tokens += tokens;
        self.current_day_tokens += tokens;
    }

    /// Reset hourly counter (called every hour).
    pub fn reset_hourly(&mut self) {
        self.current_hour_tokens = 0;
        self.last_hourly_reset = Instant::now();
    }

    /// Reset daily counter (called every midnight).
    pub fn reset_daily(&mut self) {
        self.current_day_tokens = 0;
        self.current_hour_tokens = 0;
        self.last_daily_reset = Instant::now();
        self.last_hourly_reset = Instant::now();
    }

    /// Automatically reset counters when the period elapses.
    fn auto_reset(&mut self) {
        let now = Instant::now();

        if now.duration_since(self.last_hourly_reset).as_secs() >= 3600 {
            self.current_hour_tokens = 0;
            self.last_hourly_reset = now;
            tracing::debug!("[consciousness] Hourly budget reset");
        }

        if now.duration_since(self.last_daily_reset).as_secs() >= 86400 {
            self.current_day_tokens = 0;
            self.current_hour_tokens = 0;
            self.last_daily_reset = now;
            self.last_hourly_reset = now;
            tracing::debug!("[consciousness] Daily budget reset");
        }
    }

    /// Check if we're in quiet hours (no consciousness operations).
    /// Compares current UTC hour against configured quiet window.
    fn is_quiet_hours(&self) -> bool {
        use chrono::Timelike;
        let hour = chrono::Utc::now().hour() as u8;

        // If start == end (or both > 23), quiet hours are disabled
        if self.quiet_hours_start == self.quiet_hours_end || self.quiet_hours_start > 23 {
            return false;
        }

        if self.quiet_hours_start > self.quiet_hours_end {
            // Overnight window: e.g., 22–7 means 22:00–06:59
            hour >= self.quiet_hours_start || hour < self.quiet_hours_end
        } else {
            // Same-day window: e.g., 3–11 means 03:00–10:59
            hour >= self.quiet_hours_start && hour < self.quiet_hours_end
        }
    }

    /// Adjust budget based on entropy level (0.0–1.0).
    /// Uses base values to prevent drift (M5 fix).
    pub fn set_budget_multiplier(&mut self, entropy: f64) {
        let multiplier = if entropy > 0.85 {
            1.0
        } else if entropy > 0.40 {
            0.4
        } else if entropy > 0.10 {
            0.15
        } else {
            0.02
        };

        self.tokens_per_hour = (self.base_tokens_per_hour as f64 * multiplier) as u32;
        self.tokens_per_day = (self.base_tokens_per_day as f64 * multiplier) as u32;
    }

    /// Percentage of hourly budget used.
    pub fn hourly_usage_pct(&self) -> f64 {
        if self.tokens_per_hour == 0 {
            return 1.0;
        }
        self.current_hour_tokens as f64 / self.tokens_per_hour as f64
    }

    /// Get current quiet hours config (for API exposure).
    pub fn quiet_hours(&self) -> (u8, u8) {
        (self.quiet_hours_start, self.quiet_hours_end)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_allows_thinking() {
        // Use always_active to avoid time-dependent failures
        let mut budget = ConsciousnessBudget::always_active();
        assert!(budget.can_think());
    }

    #[test]
    fn test_budget_blocks_at_limit() {
        let mut budget = ConsciousnessBudget::always_active();
        budget.current_hour_tokens = 100_000;
        assert!(!budget.can_think());
    }

    #[test]
    fn test_budget_records_usage() {
        let mut budget = ConsciousnessBudget::always_active();
        budget.record_usage(5000);
        assert_eq!(budget.current_hour_tokens, 5000);
        assert_eq!(budget.current_day_tokens, 5000);
    }

    #[test]
    fn test_budget_reset() {
        let mut budget = ConsciousnessBudget::always_active();
        budget.record_usage(5000);
        budget.reset_hourly();
        assert_eq!(budget.current_hour_tokens, 0);
        assert_eq!(budget.current_day_tokens, 5000);
    }

    #[test]
    fn test_budget_multiplier_uses_base() {
        let mut budget = ConsciousnessBudget::new();
        budget.set_budget_multiplier(0.9);
        assert_eq!(budget.tokens_per_hour, 100_000);

        budget.set_budget_multiplier(0.05);
        assert_eq!(budget.tokens_per_hour, 2000);

        assert_eq!(budget.base_tokens_per_hour, 100_000);
        assert_eq!(budget.base_tokens_per_day, 500_000);
    }

    #[test]
    fn test_budget_multiplier_no_drift() {
        let mut budget = ConsciousnessBudget::new();
        for _ in 0..10 {
            budget.set_budget_multiplier(0.5);
        }
        assert_eq!(budget.tokens_per_hour, (100_000.0 * 0.4) as u32);
    }

    #[test]
    fn test_quiet_hours_default() {
        let budget = ConsciousnessBudget::new();
        let (start, end) = budget.quiet_hours();
        assert_eq!(start, 3); // 3AM UTC = 11PM EDT
        assert_eq!(end, 11); // 11AM UTC = 7AM EDT
    }

    #[test]
    fn test_quiet_hours_custom() {
        let budget = ConsciousnessBudget::with_quiet_hours(22, 6);
        let (start, end) = budget.quiet_hours();
        assert_eq!(start, 22);
        assert_eq!(end, 6);
    }

    #[test]
    fn test_quiet_hours_disabled() {
        let mut budget = ConsciousnessBudget::always_active();
        // always_active sets start==end which disables quiet hours
        assert!(budget.can_think());
    }

    #[test]
    fn test_quiet_hours_overnight_window() {
        // 22–6 means overnight: hour 23 is quiet, hour 5 is quiet, hour 12 is not
        let budget = ConsciousnessBudget::with_quiet_hours(22, 6);
        assert_eq!(budget.quiet_hours_start, 22);
        assert_eq!(budget.quiet_hours_end, 6);
        // The logic: start > end → hour >= start || hour < end
        // hour=23 → 23>=22 → true (quiet)
        // hour=5 → 5<6 → true (quiet)
        // hour=12 → 12>=22=false, 12<6=false → false (active)
    }
}
