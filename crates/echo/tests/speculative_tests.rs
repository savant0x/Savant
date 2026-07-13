//! ECHO protocol speculative execution tests.

#[cfg(test)]
mod echo_speculative_tests {
    use savant_echo::circuit_breaker::{CircuitState, ComponentMetrics};

    #[test]
    fn test_circuit_breaker_trip_and_block() {
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
    fn test_circuit_breaker_halfopen_recovery() {
        let metrics = ComponentMetrics::with_reset_config(0.1, 5, 0, 2);

        // Trip the circuit
        for _ in 0..5 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Transition to half-open (reset_duration=0 means immediate)
        metrics.record_outcome(true); // This triggers time-based check
                                      // After transition, need consecutive successes to reset
        metrics.record_outcome(true);
        metrics.record_outcome(true);

        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_high_frequency() {
        let metrics = ComponentMetrics::new(0.5, 10);

        // 50% failure rate should trip with threshold of 0.5 (actually > 0.5)
        for _ in 0..10 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);
        assert!(metrics.trip_count() > 0);
    }

    #[test]
    fn test_circuit_breaker_closed_under_threshold() {
        let metrics = ComponentMetrics::new(0.5, 5);

        // 20% failure rate should NOT trip with threshold of 0.5
        for _ in 0..4 {
            metrics.record_outcome(true);
        }
        metrics.record_outcome(false);

        assert_eq!(metrics.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let metrics = ComponentMetrics::new(0.1, 3);

        // Trip the circuit
        for _ in 0..3 {
            metrics.record_outcome(false);
        }
        assert_eq!(metrics.state(), CircuitState::Open);

        // Manual reset
        metrics.reset();
        assert_eq!(metrics.state(), CircuitState::Closed);
        assert_eq!(metrics.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_error_rate() {
        let metrics = ComponentMetrics::new(0.3, 5);

        // Record mixed outcomes
        metrics.record_outcome(true);
        metrics.record_outcome(true);
        metrics.record_outcome(false);
        metrics.record_outcome(true);
        metrics.record_outcome(false);

        // 2 failures out of 5 = 0.4 error rate > 0.3 threshold → tripped
        assert_eq!(metrics.state(), CircuitState::Open);
    }
}
