//! V1 API error type with `IntoResponse` impl for axum.
//!
//! Used by all `/v1/*` endpoints to return structured JSON errors with
//! appropriate HTTP status codes. Mirrors the existing `api/*` error pattern
//! but is distinct so that v1 errors can evolve independently.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

/// V1 API error variants. Each maps to a specific HTTP status code.
#[derive(Debug, Error)]
pub enum V1ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("unprocessable: {0}")]
    Unprocessable(String),
    #[error("upstream error: {0}")]
    BadGateway(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// FID-031 impl: 501 Not Implemented — used by stub handlers that
    /// are mapped but not yet implemented. The dashboard surfaces this
    /// as "endpoint pending" rather than 500.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

impl IntoResponse for V1ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            Self::Unprocessable(_) => (StatusCode::UNPROCESSABLE_ENTITY, "UNPROCESSABLE_ENTITY"),
            Self::BadGateway(_) => (StatusCode::BAD_GATEWAY, "BAD_GATEWAY"),
            Self::ServiceUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            Self::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED"),
        };
        let body = Json(json!({
            "error": self.to_string(),
            "code": code,
            "version": env!("CARGO_PKG_VERSION"),
        }));
        (status, body).into_response()
    }
}

impl From<std::io::Error> for V1ApiError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(format!("io: {e}"))
    }
}

impl From<serde_json::Error> for V1ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::BadRequest(format!("invalid json: {e}"))
    }
}

/// V1Result<T> — a Result alias used by all `/v1/*` handlers so the
/// happy path returns the value (`IntoResponse`) and the error path
/// returns a structured `V1ApiError` JSON body. Enables the stub
/// pattern: `Err(V1ApiError::NotImplemented("foo".into()))` is the
/// full body of a stub handler.
///
/// **IntoResponse impl**: axum 0.7 provides a blanket `IntoResponse`
/// impl for `Result<T, E>` where both `T: IntoResponse` and `E: IntoResponse`.
/// Since `Json<Value>` implements `IntoResponse` (via axum::Json) and
/// `V1ApiError` implements `IntoResponse` (above), the `Result<Json<Value>,
/// V1ApiError>` returned by handlers gets `IntoResponse` for free.
/// We do NOT provide a custom impl here because that would violate Rust's
/// orphan rule (Result is from `std`, not the current crate).
pub type V1Result<T> = Result<T, V1ApiError>;
