use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// The core state sequence points underlying a circuit breaker
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed = 0,
    Open = 1,
    HalfOpen = 2,
}

/// Provides localized fault isolation capabilities when wrapping unreliable clients.
///
/// The circuit breaker has three states:
/// - **Closed**: Normal operation. Requests pass through. Failures are counted.
/// - **Open**: Failure threshold exceeded. All requests are rejected immediately.
/// - **HalfOpen**: Recovery probe. A single request is allowed through to test recovery.
///
/// # Configuration
/// - `failure_threshold`: Number of failures before opening (default: 5)
/// - `recovery_timeout_secs`: Seconds to wait before transitioning Open→HalfOpen (default: 30)
/// - `success_threshold`: Successes in HalfOpen needed to close (default: 3)
pub struct CircuitBreaker {
    state: AtomicU8,
    failure_count: AtomicU64,
    success_count: AtomicU64,
    last_failure_time: AtomicU64,
    failure_threshold: u64,
    recovery_timeout_secs: u64,
    success_threshold: u64,
}

impl CircuitBreaker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(BreakerState::Closed as u8),
            failure_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_failure_time: AtomicU64::new(0),
            failure_threshold: 5,
            recovery_timeout_secs: 30,
            success_threshold: 3,
        }
    }

    /// Creates a circuit breaker with custom thresholds.
    #[must_use]
    pub fn with_thresholds(
        failure_threshold: u64,
        recovery_timeout_secs: u64,
        success_threshold: u64,
    ) -> Self {
        Self {
            failure_threshold,
            recovery_timeout_secs,
            success_threshold,
            ..Self::new()
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_else(|e| {
                tracing::warn!("System clock before UNIX epoch: {}, using 0", e);
                0
            })
    }

    /// Returns the current state of the circuit breaker.
    pub fn state(&self) -> BreakerState {
        match self.state.load(Ordering::Relaxed) {
            0 => BreakerState::Closed,
            1 => BreakerState::Open,
            2 => BreakerState::HalfOpen,
            _ => BreakerState::Open, // Fail-safe
        }
    }

    /// Checks if a request should be allowed through.
    /// Returns true if the request should proceed, false if blocked.
    pub fn allow_request(&self) -> bool {
        let current = self.state();
        match current {
            BreakerState::Closed => true,
            BreakerState::HalfOpen => true,
            BreakerState::Open => {
                // Check if recovery timeout has elapsed
                let last_failure = self.last_failure_time.load(Ordering::Relaxed);
                let now = Self::now_secs();
                if now.saturating_sub(last_failure) >= self.recovery_timeout_secs {
                    // Transition to HalfOpen via CAS
                    let _prev = self.state.compare_exchange(
                        BreakerState::Open as u8,
                        BreakerState::HalfOpen as u8,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );
                    info!("Circuit breaker transitioning Open → HalfOpen");
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Records a successful operation.
    pub fn record_success(&self) {
        match self.state() {
            BreakerState::HalfOpen => {
                let count = self.success_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= self.success_threshold {
                    self.state
                        .store(BreakerState::Closed as u8, Ordering::SeqCst);
                    self.failure_count.store(0, Ordering::SeqCst);
                    self.success_count.store(0, Ordering::SeqCst);
                    info!("Circuit breaker transitioning HalfOpen → Closed (recovered)");
                }
            }
            BreakerState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::SeqCst);
            }
            BreakerState::Open => {
                // Shouldn't happen (blocked), but don't change state
            }
        }
    }

    /// Records a failed operation.
    pub fn record_failure(&self) {
        self.last_failure_time
            .store(Self::now_secs(), Ordering::SeqCst);

        match self.state() {
            BreakerState::Closed => {
                let count = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= self.failure_threshold {
                    self.state.store(BreakerState::Open as u8, Ordering::SeqCst);
                    warn!(
                        "Circuit breaker transitioning Closed → Open (threshold reached: {})",
                        count
                    );
                }
            }
            BreakerState::HalfOpen => {
                // Single failure in HalfOpen re-opens the circuit
                self.state.store(BreakerState::Open as u8, Ordering::SeqCst);
                self.success_count.store(0, Ordering::SeqCst);
                warn!("Circuit breaker transitioning HalfOpen → Open (recovery failed)");
            }
            BreakerState::Open => {
                // Already open, just update timestamp
            }
        }
    }

    /// Resets the circuit breaker to closed state.
    pub fn reset(&self) {
        self.state
            .store(BreakerState::Closed as u8, Ordering::SeqCst);
        self.failure_count.store(0, Ordering::SeqCst);
        self.success_count.store(0, Ordering::SeqCst);
        self.last_failure_time.store(0, Ordering::SeqCst);
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_closed() {
        let breaker = CircuitBreaker::new();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow_request());
    }

    #[test]
    fn test_opens_after_threshold() {
        let breaker = CircuitBreaker::with_thresholds(3, 60, 2);

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Closed);

        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(!breaker.allow_request());
    }

    #[test]
    fn test_halfopen_closes_after_successes() {
        let breaker = CircuitBreaker::with_thresholds(1, 0, 2);

        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);

        // recovery_timeout=0 means immediate transition
        assert!(breaker.allow_request());
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
    }

    #[test]
    fn test_halfopen_reopens_on_failure() {
        let breaker = CircuitBreaker::with_thresholds(1, 0, 3);

        breaker.record_failure();
        assert!(breaker.allow_request()); // → HalfOpen
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
    }

    #[test]
    fn test_reset() {
        let breaker = CircuitBreaker::with_thresholds(1, 60, 1);
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);

        breaker.reset();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow_request());
    }
}
