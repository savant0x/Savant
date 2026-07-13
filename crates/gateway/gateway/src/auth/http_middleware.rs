//! HTTP Authentication Middleware
//!
//! Provides axum Tower middleware for REST API authentication.
//! Validates `Authorization: Bearer <key>` or `X-API-Key: <key>` headers
//! against the configured `dashboard_api_key`.
//!
//! Health/readiness endpoints (`/health`, `/live`, `/ready`) and WebSocket
//! routes (`/ws`, `/ws/canvas`) are exempt from authentication.

#![allow(clippy::disallowed_methods)] // json!() macro internal .unwrap() is provably infallible

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::atomic::{AtomicI64, Ordering};

/// Minimum interval (ms) between WARN log messages for repeated 401s.
/// Prevents log spam from polling endpoints without auth tokens.
const AUTH_WARN_INTERVAL_MS: i64 = 10_000;

/// Last timestamp (millis since epoch) a WARN was emitted for unauthorized requests.
/// Atomic to avoid requiring a Mutex in the hot path.
static LAST_AUTH_WARN_MS: AtomicI64 = AtomicI64::new(0);

/// Paths that do not require authentication.
const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/live",
    "/ready",
    "/ws",
    "/ws/canvas",
    "/api/setup",
    "/api/config",
    "/api/agents",
];

/// Constant-time byte comparison to prevent timing attacks.
///
/// Always iterates over the full length of the expected key (`b`).
/// If the provided key (`a`) is shorter, missing bytes are XORed with 0
/// (which produces a mismatch). If longer, excess bytes are ignored but
/// the function still processes all of `b`. This ensures the comparison
/// time depends only on the expected key length, never the provided one.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let expected_len = b.len();
    let mut result = 0u8;

    // Always iterate over the expected key length.
    // For indices beyond `a`, XOR with 0 (which is a mismatch if b[i] != 0).
    for i in 0..expected_len {
        let a_byte = if i < a.len() { a[i] } else { 0u8 };
        result |= a_byte ^ b[i];
    }

    // Also OR in the length difference so that extra bytes in `a` cause rejection.
    result |= (a.len() != expected_len) as u8;

    result == 0
}

/// Extract the API key from request headers.
///
/// Supports two header formats:
/// - `Authorization: Bearer <key>`
/// - `X-API-Key: <key>`
fn extract_api_key(req: &Request<Body>) -> Option<String> {
    // Try Authorization: Bearer <key>
    if let Some(auth_header) = req.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(key) = auth_str.strip_prefix("Bearer ") {
                return Some(key.to_string());
            }
        }
    }

    // Try X-API-Key: <key>
    if let Some(api_key_header) = req.headers().get("x-api-key") {
        if let Ok(key) = api_key_header.to_str() {
            return Some(key.to_string());
        }
    }

    None
}

/// Authentication middleware for REST API endpoints.
///
/// Validates the API key from request headers against the configured
/// `dashboard_api_key`. Returns 401 Unauthorized if the key is missing
/// or invalid.
pub async fn auth_middleware(
    axum::extract::State(expected_key): axum::extract::State<Option<String>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();

    // Allow public endpoints without authentication
    if PUBLIC_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
    {
        return next.run(req).await;
    }

    // If no dashboard API key is configured, allow all requests (development mode)
    let expected = match expected_key {
        Some(key) if !key.is_empty() => key,
        _ => return next.run(req).await,
    };

    // Extract API key from request
    let provided_key = extract_api_key(&req);

    match provided_key {
        Some(key) if constant_time_eq(key.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => {
            // Rate-limit WARN logging to prevent log spam from polling endpoints
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let last = LAST_AUTH_WARN_MS.load(Ordering::Relaxed);
            if now_ms - last > AUTH_WARN_INTERVAL_MS {
                LAST_AUTH_WARN_MS.store(now_ms, Ordering::Relaxed);
                tracing::warn!(
                    "[auth] Unauthorized request to {} from {} (suppressing further warnings for {}s)",
                    path,
                    req.headers()
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown"),
                    AUTH_WARN_INTERVAL_MS / 1000
                );
            }
            (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({
                    "error": "Unauthorized",
                    "message": "Valid API key required. Provide via 'Authorization: Bearer <key>' or 'X-API-Key: <key>' header."
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"secret123", b"secret123"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq(b"secret123", b"secret124"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"muchlongervalue"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }
}
