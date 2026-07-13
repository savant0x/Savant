//! ECHO protocol tests - circuit breaker state transitions.

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod echo_tests {
    use savant_echo::circuit_breaker::{CircuitState, ComponentMetrics};

    #[test]
    fn test_circuit_breaker_full_lifecycle() {
        let metrics = ComponentMetrics::with_reset_config(0.5, 5, 0, 2);

        // Start closed
        assert_eq!(metrics.state(), CircuitState::Closed);

        // Accumulate failures - 100% error rate with threshold 0.5
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Time-based transition (reset_duration=0)
        metrics.record_outcome(true); // Triggers check, transitions to HalfOpen
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // Successes in HalfOpen
        metrics.record_outcome(true);
        metrics.record_outcome(true);
        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_halfopen_failure_reopens() {
        let metrics = ComponentMetrics::with_reset_config(0.5, 5, 0, 3);

        // Trip
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // → HalfOpen (timeout=0)
        metrics.record_outcome(true);
        assert_eq!(metrics.state(), CircuitState::HalfOpen);

        // Failure in HalfOpen immediately re-opens
        metrics.record_outcome(false);
        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_reset_clears_state() {
        let metrics = ComponentMetrics::new(0.1, 3);

        // Trip
        for _ in 0..3 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Reset
        metrics.reset();
        assert_eq!(metrics.state(), CircuitState::Closed);
        assert_eq!(metrics.failure_count(), 0);
    }

    #[test]
    fn test_concurrent_operations() {
        use std::sync::Arc;
        use std::thread;

        let metrics = Arc::new(ComponentMetrics::new(0.3, 10));
        let mut handles = vec![];

        // Spawn threads recording outcomes
        for _ in 0..50 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                m.record_outcome(false);
            }));
        }

        for _ in 0..20 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                m.record_outcome(true);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // With heavy failure rate, circuit should be open
        assert_eq!(metrics.state(), CircuitState::Open);
    }

    #[test]
    fn test_error_rate_tracking() {
        let metrics = ComponentMetrics::new(0.3, 5);

        for _ in 0..7 {
            metrics.record_outcome(false);
        }
        for _ in 0..3 {
            metrics.record_outcome(true);
        }

        // 70% error rate exceeds 0.3 threshold → tripped
        assert_eq!(metrics.state(), CircuitState::Open);
        assert!(metrics.trip_count() > 0);
        // After tripping at 5 min samples, subsequent calls are blocked
        assert!(metrics.total_count() >= 5);
    }
}
