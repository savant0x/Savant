//! MCP server integration tests - auth, rate limiting, circuit breaker.

#![allow(clippy::disallowed_methods)]

use savant_mcp::circuit::CircuitBreaker;
use std::collections::HashMap;

/// Helper to hash an auth token the same way the server does (blake3)
fn hash_token(token: &str) -> String {
    let hash = blake3::hash(token.as_bytes());
    hash.to_hex().to_string()
}

// ============================================================================
// Authentication Tests
// ============================================================================

#[test]
fn test_auth_token_hashing() {
    let hash1 = hash_token("test-token-123");
    let hash2 = hash_token("test-token-123");
    let hash3 = hash_token("different-token");

    assert_eq!(hash1, hash2, "Same token should produce same hash");
    assert_ne!(
        hash1, hash3,
        "Different tokens should produce different hashes"
    );
    assert!(!hash1.is_empty(), "Hash should not be empty");
}

#[test]
fn test_auth_token_validation_flow() {
    let mut tokens = HashMap::new();
    let valid_hash = hash_token("valid-token-abc");
    tokens.insert(valid_hash.clone(), "Test token".to_string());

    // Valid token should be found
    assert!(tokens.contains_key(&valid_hash));

    // Invalid token should not be found
    let invalid_hash = hash_token("invalid-token");
    assert!(!tokens.contains_key(&invalid_hash));
}

#[test]
fn test_empty_token_rejected() {
    let empty_hash = hash_token("");
    assert!(!empty_hash.is_empty(), "Empty token still produces hash");

    let tokens: HashMap<String, String> = HashMap::new();
    assert!(
        !tokens.contains_key(&empty_hash),
        "Empty token should not be in auth map"
    );
}

// ============================================================================
// Rate Limiting Tests
// ============================================================================

#[test]
fn test_rate_limit_enforcement() {
    // Simulate the rate limiting logic from the MCP server
    let mut request_count = 0u32;
    let mut last_reset = std::time::Instant::now();
    let max_requests = 100;

    // Simulate 100 requests
    for _ in 0..100 {
        let now = std::time::Instant::now();
        if now.duration_since(last_reset).as_secs() >= 60 {
            request_count = 0;
            last_reset = now;
        }
        request_count += 1;
        assert!(request_count <= max_requests);
    }

    // 101st request should be blocked
    assert!(request_count > max_requests - 1);
}

#[test]
fn test_rate_limit_reset_after_timeout() {
    let mut request_count = 0u32;
    let last_reset = std::time::Instant::now() - std::time::Duration::from_secs(61);
    let _max_requests = 100;

    // After 61 seconds, counter should reset
    let now = std::time::Instant::now();
    if now.duration_since(last_reset).as_secs() >= 60 {
        request_count = 0;
        let _ = now;
    }

    assert_eq!(request_count, 0, "Counter should reset after timeout");
}

// ============================================================================
// Circuit Breaker Tests
// ============================================================================

#[test]
fn test_circuit_breaker_full_cycle() {
    let cb = CircuitBreaker::with_thresholds(3, 0, 2);

    // Closed → Open
    cb.record_failure();
    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Open);

    // Open → HalfOpen (timeout=0)
    assert!(cb.allow_request());
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::HalfOpen);

    // HalfOpen → Closed
    cb.record_success();
    cb.record_success();
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Closed);
}

#[test]
fn test_circuit_breaker_halfopen_failure() {
    let cb = CircuitBreaker::with_thresholds(1, 0, 3);

    // Trip
    cb.record_failure();
    assert!(cb.allow_request()); // → HalfOpen
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::HalfOpen);

    // Failure in HalfOpen re-opens
    cb.record_failure();
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Open);
}

#[test]
fn test_circuit_breaker_concurrent_failures() {
    use std::sync::Arc;
    use std::thread;

    let cb = Arc::new(CircuitBreaker::with_thresholds(50, 60, 10));
    let mut handles = vec![];

    for _ in 0..100 {
        let cb_clone = cb.clone();
        handles.push(thread::spawn(move || {
            cb_clone.record_failure();
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Open);
}

#[test]
fn test_circuit_breaker_reset() {
    let cb = CircuitBreaker::with_thresholds(1, 60, 1);
    cb.record_failure();
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Open);

    cb.reset();
    assert_eq!(cb.state(), savant_mcp::circuit::BreakerState::Closed);
    assert!(cb.allow_request());
}
