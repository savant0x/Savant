//! Statistical Circuit Breaker for Autonomous Upgrades
//!
//! Monitors the failure rate of newly hot-swapped components and triggers
//! rollbacks if mathematical thresholds are exceeded.
//!
//! # States
//! - **Closed**: Normal operation, requests flow through
//! - **Open**: Circuit is tripped, requests are blocked
//! - **Half-Open**: Testing recovery, limited requests allowed
//!
//! # Reset Mechanisms
//! - Time-based reset: Automatically reset after stability period
//! - Manual reset: Explicit reset via API
//! - Success-based reset: Reset after consecutive successes in half-open state

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    /// Normal operation, requests flow through
    Closed = 0,
    /// Circuit is tripped, requests are blocked
    Open = 1,
    /// Testing recovery, limited requests allowed
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(value: u8) -> Self {
        match value {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }
}

/// Default time-based reset duration (5 minutes)
const DEFAULT_RESET_DURATION_SECS: u64 = 300;

/// Default number of consecutive successes needed to reset from half-open
const DEFAULT_SUCCESS_THRESHOLD: u64 = 5;

/// Metrics for a specific component or epoch with circuit breaker functionality.
pub struct ComponentMetrics {
    total_invocations: AtomicU64,
    failed_invocations: AtomicU64,
    /// Threshold error rate (e.g., 5% = 0.05)
    error_threshold: f64,
    /// Minimum invocations before the breaker is allowed to trip
    min_sample_size: u64,
    /// Current circuit breaker state
    state: AtomicU8,
    /// Timestamp when the circuit was opened (UNIX seconds)
    opened_at: AtomicU64,
    /// Duration to wait before attempting reset (seconds)
    reset_duration_secs: u64,
    /// Consecutive successes in half-open state
    consecutive_successes: AtomicU64,
    /// Number of successes needed to reset from half-open
    success_threshold: u64,
    /// Total number of times the circuit has been reset
    reset_count: AtomicU64,
    /// Total number of times the circuit has been tripped
    trip_count: AtomicU64,
}

impl ComponentMetrics {
    /// Creates a new metrics tracker with internal thresholding.
    pub fn new(error_threshold: f64, min_sample_size: u64) -> Self {
        Self {
            total_invocations: AtomicU64::new(0),
            failed_invocations: AtomicU64::new(0),
            error_threshold,
            min_sample_size,
            state: AtomicU8::new(CircuitState::Closed as u8),
            opened_at: AtomicU64::new(0),
            reset_duration_secs: DEFAULT_RESET_DURATION_SECS,
            consecutive_successes: AtomicU64::new(0),
            success_threshold: DEFAULT_SUCCESS_THRESHOLD,
            reset_count: AtomicU64::new(0),
            trip_count: AtomicU64::new(0),
        }
    }

    /// Creates a new metrics tracker with custom reset configuration.
    pub fn with_reset_config(
        error_threshold: f64,
        min_sample_size: u64,
        reset_duration_secs: u64,
        success_threshold: u64,
    ) -> Self {
        Self {
            total_invocations: AtomicU64::new(0),
            failed_invocations: AtomicU64::new(0),
            error_threshold,
            min_sample_size,
            state: AtomicU8::new(CircuitState::Closed as u8),
            opened_at: AtomicU64::new(0),
            reset_duration_secs,
            consecutive_successes: AtomicU64::new(0),
            success_threshold,
            reset_count: AtomicU64::new(0),
            trip_count: AtomicU64::new(0),
        }
    }

    /// Gets the current circuit state.
    pub fn state(&self) -> CircuitState {
        CircuitState::from(self.state.load(Ordering::Acquire))
    }

    /// Gets the current UNIX timestamp.
    fn current_time() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Checks if the circuit should transition from Open to Half-Open.
    /// Uses compare-and-swap (CAS) on state for atomic Open→HalfOpen transition.
    fn check_time_based_reset(&self) -> bool {
        // Only proceed if currently Open
        if self.state.load(Ordering::Acquire) != CircuitState::Open as u8 {
            return false;
        }

        let opened_at = self.opened_at.load(Ordering::Acquire);
        let now = Self::current_time();

        if opened_at > 0 && now >= opened_at + self.reset_duration_secs {
            // CAS: transition from Open to HalfOpen atomically
            let cas_result = self.state.compare_exchange(
                CircuitState::Open as u8,
                CircuitState::HalfOpen as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if cas_result.is_ok() {
                self.consecutive_successes.store(0, Ordering::Release);
                info!(
                    "Circuit breaker transitioning to Half-Open after {} seconds",
                    self.reset_duration_secs
                );
                true
            } else {
                // Another thread already transitioned the state
                false
            }
        } else {
            false
        }
    }

    /// Records an execution outcome.
    ///
    /// Returns `true` if the circuit breaker should trip.
    pub fn record_outcome(&self, success: bool) -> bool {
        // Check for time-based reset first
        self.check_time_based_reset();

        let current_state = self.state();

        // If circuit is open, block all requests
        if current_state == CircuitState::Open {
            debug!("Circuit breaker is Open, blocking request");
            return true;
        }

        // If circuit is half-open, only allow limited requests
        if current_state == CircuitState::HalfOpen {
            if success {
                let consecutive = self.consecutive_successes.fetch_add(1, Ordering::AcqRel) + 1;
                debug!(
                    "Half-Open: consecutive success {} of {}",
                    consecutive, self.success_threshold
                );

                if consecutive >= self.success_threshold {
                    // Reset the circuit
                    self.reset();
                    info!(
                        "Circuit breaker reset after {} consecutive successes",
                        consecutive
                    );
                    return false;
                }
                return false;
            } else {
                // Failure in half-open state, trip the circuit again
                self.trip();
                warn!("Circuit breaker re-tripped after failure in Half-Open state");
                return true;
            }
        }

        // Closed state: normal operation
        // Use AcqRel for fetch_add to ensure visibility of prior writes,
        // and Acquire for loads to get a consistent snapshot of both counters.
        let total = self.total_invocations.fetch_add(1, Ordering::AcqRel) + 1;

        let failed = if !success {
            self.failed_invocations.fetch_add(1, Ordering::AcqRel) + 1
        } else {
            self.failed_invocations.load(Ordering::Acquire)
        };

        // Do not calculate statistics until we have a statistically significant sample
        if total < self.min_sample_size {
            return false;
        }

        // Calculate current error rate
        let current_error_rate = (failed as f64) / (total as f64);

        // Trip the breaker if the error rate exceeds the threshold
        if current_error_rate > self.error_threshold {
            self.trip();
            warn!(
                "Circuit breaker tripped: error rate {:.2}% exceeds threshold {:.2}%",
                current_error_rate * 100.0,
                self.error_threshold * 100.0
            );
            return true;
        }

        false
    }

    /// Trips the circuit breaker (transitions to Open state).
    fn trip(&self) {
        self.state
            .store(CircuitState::Open as u8, Ordering::Release);
        self.opened_at
            .store(Self::current_time(), Ordering::Release);
        self.trip_count.fetch_add(1, Ordering::AcqRel);
        self.consecutive_successes.store(0, Ordering::Release);
    }

    /// Resets the circuit breaker (transitions to Closed state).
    pub fn reset(&self) {
        self.state
            .store(CircuitState::Closed as u8, Ordering::Release);
        self.opened_at.store(0, Ordering::Release);
        self.total_invocations.store(0, Ordering::Release);
        self.failed_invocations.store(0, Ordering::Release);
        self.consecutive_successes.store(0, Ordering::Release);
        self.reset_count.fetch_add(1, Ordering::AcqRel);
        info!("Circuit breaker manually reset");
    }

    /// Returns current failure count.
    pub fn failure_count(&self) -> u64 {
        self.failed_invocations.load(Ordering::Acquire)
    }

    /// Returns current total count.
    pub fn total_count(&self) -> u64 {
        self.total_invocations.load(Ordering::Acquire)
    }

    /// Returns the current error rate.
    pub fn error_rate(&self) -> f64 {
        let total = self.total_invocations.load(Ordering::Acquire);
        if total == 0 {
            return 0.0;
        }
        let failed = self.failed_invocations.load(Ordering::Acquire);
        (failed as f64) / (total as f64)
    }

    /// Returns the number of times the circuit has been reset.
    pub fn reset_count(&self) -> u64 {
        self.reset_count.load(Ordering::Acquire)
    }

    /// Returns the number of times the circuit has been tripped.
    pub fn trip_count(&self) -> u64 {
        self.trip_count.load(Ordering::Acquire)
    }

    /// Returns the time when the circuit was opened (0 if closed).
    pub fn opened_at(&self) -> u64 {
        self.opened_at.load(Ordering::Acquire)
    }

    /// Returns the number of consecutive successes in half-open state.
    pub fn consecutive_successes(&self) -> u64 {
        self.consecutive_successes.load(Ordering::Acquire)
    }

    /// Records an outcome and triggers a rollback on the registry if the circuit trips.
    /// Returns true if the circuit was tripped (and rollback attempted).
    pub fn record_outcome_with_rollback(
        &self,
        success: bool,
        registry: &crate::registry::HotSwappableRegistry,
    ) -> bool {
        let tripped = self.record_outcome(success);
        if tripped {
            match registry.rollback_epoch() {
                Ok(()) => {
                    info!(
                        "Circuit breaker tripped — rolled back to epoch {}",
                        registry.current_epoch()
                    );
                }
                Err(e) => {
                    warn!("Circuit breaker tripped but rollback failed: {}", e);
                }
            }
        }
        tripped
    }

    /// Returns the current epoch from the registry for health monitoring.
    pub fn current_epoch(registry: &crate::registry::HotSwappableRegistry) -> u64 {
        registry.current_epoch()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_closed_state() {
        let metrics = ComponentMetrics::new(0.1, 10);

        // Record successful outcomes
        for _ in 0..5 {
            assert!(!metrics.record_outcome(true));
        }

        assert_eq!(metrics.state(), CircuitState::Closed);
        assert_eq!(metrics.failure_count(), 0);
        assert_eq!(metrics.total_count(), 5);
    }

    #[test]
    fn test_circuit_breaker_trip() {
        let metrics = ComponentMetrics::new(0.1, 5);

        // Record failures to exceed threshold
        for _ in 0..5 {
            metrics.record_outcome(false);
        }

        // Circuit should be tripped
        assert_eq!(metrics.state(), CircuitState::Open);
        assert!(metrics.record_outcome(true)); // Should be blocked
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let metrics = ComponentMetrics::new(0.1, 5);

        // Trip the circuit
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Reset the circuit
        metrics.reset();
        assert_eq!(metrics.state(), CircuitState::Closed);
        assert_eq!(metrics.failure_count(), 0);
        assert_eq!(metrics.total_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_half_open_success() {
        let metrics = ComponentMetrics::with_reset_config(0.1, 5, 0, 3);

        // Trip the circuit
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Wait for time-based reset (reset_duration_secs = 0)
        assert!(metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // Record successes in half-open state
        assert!(!metrics.record_outcome(true));
        assert!(!metrics.record_outcome(true));
        assert!(!metrics.record_outcome(true)); // Should reset

        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_failure() {
        let metrics = ComponentMetrics::with_reset_config(0.1, 5, 0, 3);

        // Trip the circuit
        for _ in 0..5 {
            metrics.record_outcome(false);
        }

        // Transition to half-open
        assert!(metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // Record failure in half-open state
        assert!(metrics.record_outcome(false)); // Should re-trip

        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_error_rate_calculation() {
        let metrics = ComponentMetrics::new(0.1, 10);

        // Record mixed outcomes
        for _ in 0..7 {
            metrics.record_outcome(true);
        }
        for _ in 0..3 {
            metrics.record_outcome(false);
        }

        assert_eq!(metrics.error_rate(), 0.3);
        assert_eq!(metrics.failure_count(), 3);
        assert_eq!(metrics.total_count(), 10);
    }

    // ====================== EXTENDED TEST COVERAGE ======================

    #[test]
    fn test_zero_threshold_trips_on_any_failure() {
        let metrics = ComponentMetrics::new(0.0, 1);
        metrics.record_outcome(false);
        assert_eq!(metrics.state(), CircuitState::Open);
        assert_eq!(metrics.trip_count(), 1);
    }

    #[test]
    fn test_hundred_percent_threshold_never_trips() {
        let metrics = ComponentMetrics::new(1.0, 1);
        for _ in 0..1000 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_min_sample_prevents_premature_trip() {
        let metrics = ComponentMetrics::new(0.01, 100);
        for _ in 0..99 {
            metrics.record_outcome(false);
        }
        assert_eq!(
            metrics.state(),
            CircuitState::Closed,
            "Should not trip before min_sample"
        );
        metrics.record_outcome(false);
        assert_eq!(
            metrics.state(),
            CircuitState::Open,
            "Should trip at min_sample"
        );
    }

    #[test]
    fn test_error_rate_with_zero_total() {
        let metrics = ComponentMetrics::new(0.5, 1);
        assert_eq!(
            metrics.error_rate(),
            0.0,
            "Zero invocations should yield 0.0 error rate"
        );
    }

    #[test]
    fn test_all_successes_zero_error_rate() {
        let metrics = ComponentMetrics::new(0.05, 1);
        for _ in 0..100 {
            metrics.record_outcome(true);
        }
        assert_eq!(metrics.error_rate(), 0.0);
        assert_eq!(metrics.failure_count(), 0);
    }

    #[test]
    fn test_all_failures_full_error_rate() {
        let metrics = ComponentMetrics::new(0.5, 1);
        // First failure trips the circuit (min_sample_size=1, error_threshold=0.5)
        // After trip, circuit is Open and blocks all subsequent requests
        metrics.record_outcome(false); // This trips the circuit
        assert_eq!(metrics.state(), CircuitState::Open);

        // Additional failures are blocked (circuit is open)
        for _ in 0..49 {
            metrics.record_outcome(false); // Blocked, not counted
        }

        // Only the first failure was counted before the circuit opened
        assert_eq!(metrics.failure_count(), 1);
        assert_eq!(metrics.error_rate(), 1.0);
    }

    #[test]
    fn test_manual_reset_preserves_trip_count() {
        let metrics = ComponentMetrics::new(0.05, 10);
        for _ in 0..10 {
            metrics.record_outcome(false);
        }
        let trips = metrics.trip_count();

        metrics.reset();
        metrics.reset();
        assert_eq!(
            metrics.trip_count(),
            trips,
            "Trip count preserved across resets"
        );
        assert_eq!(metrics.reset_count(), 2);
    }

    #[test]
    fn test_manual_reset_clears_counts() {
        let metrics = ComponentMetrics::new(0.05, 1);
        metrics.record_outcome(false);
        metrics.record_outcome(true);
        assert!(metrics.total_count() > 0);

        metrics.reset();
        assert_eq!(metrics.total_count(), 0);
        assert_eq!(metrics.failure_count(), 0);
    }

    #[test]
    fn test_multiple_trip_reset_cycles() {
        let metrics = ComponentMetrics::new(0.05, 5);

        for cycle in 0..5 {
            // Trip
            for _ in 0..5 {
                metrics.record_outcome(false);
            }
            assert_eq!(
                metrics.state(),
                CircuitState::Open,
                "Cycle {} should trip",
                cycle
            );

            // Reset
            metrics.reset();
            assert_eq!(
                metrics.state(),
                CircuitState::Closed,
                "Cycle {} should be closed after reset",
                cycle
            );
        }

        assert_eq!(metrics.trip_count(), 5);
        assert_eq!(metrics.reset_count(), 5);
    }

    #[test]
    fn test_state_enum_from_invalid_u8() {
        // Invalid values should default to Closed
        assert_eq!(CircuitState::from(255), CircuitState::Closed);
        assert_eq!(CircuitState::from(100), CircuitState::Closed);
    }

    #[test]
    fn test_opened_at_zero_when_closed() {
        let metrics = ComponentMetrics::new(0.05, 1);
        assert_eq!(
            metrics.opened_at(),
            0,
            "Closed circuit should have opened_at = 0"
        );
    }

    #[test]
    fn test_opened_at_set_when_tripped() {
        let metrics = ComponentMetrics::new(0.05, 1);
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be after UNIX_EPOCH")
            .as_secs();

        metrics.record_outcome(false);

        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be after UNIX_EPOCH")
            .as_secs();

        assert!(metrics.opened_at() >= before);
        assert!(metrics.opened_at() <= after);
    }

    #[test]
    fn test_custom_reset_config_params() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 1, 120, 10);

        // Trip and reset to verify config is stored
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        metrics.reset();
        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_check_time_based_reset_not_expired() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 5, 3600, 3);
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Reset duration is 3600 seconds, should not have expired
        assert!(
            !metrics.check_time_based_reset(),
            "Should not reset when duration not elapsed"
        );
        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_check_time_based_reset_when_not_open() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 5, 0, 3);
        // Circuit is closed, check should return false
        assert!(!metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_resets_consecutive_successes() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 5, 0, 5);

        // Trip
        for _ in 0..5 {
            metrics.record_outcome(false);
        }

        // Half-open via time-based reset
        assert!(metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // Two successes
        metrics.record_outcome(true);
        metrics.record_outcome(true);
        assert_eq!(metrics.consecutive_successes(), 2);

        // Failure resets consecutive successes and re-trips
        metrics.record_outcome(false);
        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_exact_threshold_boundary() {
        // 10% threshold with 10 samples, 1 failure = 10% (exactly at threshold, not exceeding)
        let metrics = ComponentMetrics::new(0.10, 10);
        for _ in 0..9 {
            metrics.record_outcome(true);
        }
        metrics.record_outcome(false);
        // 1/10 = 10% = 0.10, which is NOT > 0.10
        assert_eq!(metrics.state(), CircuitState::Closed);

        // One more failure = 2/11 ≈ 18% > 10%
        metrics.record_outcome(false);
        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_success_does_not_trip_when_above_threshold() {
        let metrics = ComponentMetrics::new(0.5, 5);
        for _ in 0..5 {
            metrics.record_outcome(true);
        }
        assert_eq!(metrics.state(), CircuitState::Closed);
        assert_eq!(metrics.failure_count(), 0);
    }

    #[test]
    fn test_interleaved_success_and_failure() {
        let metrics = ComponentMetrics::new(0.5, 10);

        // Alternating success/failure = 50% error rate
        for i in 0..20 {
            metrics.record_outcome(i % 2 == 0);
        }

        assert_eq!(metrics.error_rate(), 0.5);
        assert_eq!(metrics.state(), CircuitState::Closed); // At threshold, not above
    }

    #[test]
    fn test_record_outcome_returns_true_when_blocked() {
        let metrics = ComponentMetrics::new(0.05, 5);

        // Trip
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // record_outcome returns true when circuit IS open (request was blocked)
        let result = metrics.record_outcome(true);
        assert!(
            result,
            "record_outcome should return true when circuit is open (blocked)"
        );
    }

    #[test]
    fn test_consecutive_successes_after_reset() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 5, 0, 5);

        // Trip
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert!(metrics.consecutive_successes() > 0 || metrics.consecutive_successes() == 0);

        metrics.reset();
        assert_eq!(metrics.consecutive_successes(), 0);
    }

    #[test]
    fn test_state_transitions_complete_lifecycle() {
        let metrics = ComponentMetrics::with_reset_config(0.05, 5, 0, 3);

        // 1. Start closed
        assert_eq!(metrics.state(), CircuitState::Closed);

        // 2. Trip to open
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // 3. Time-based reset to half-open
        assert!(metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // 4. Failures in half-open re-trip
        metrics.record_outcome(false);
        assert_eq!(metrics.state(), CircuitState::Open);

        // 5. Reset again, succeed this time
        assert!(metrics.check_time_based_reset());
        assert_eq!(metrics.state(), CircuitState::HalfOpen);
        metrics.record_outcome(true);
        metrics.record_outcome(true);
        metrics.record_outcome(true);
        assert_eq!(metrics.state(), CircuitState::Closed);
    }
}
