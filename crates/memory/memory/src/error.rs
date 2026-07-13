//! Memory subsystem error types.
//!
//! All errors from the memory layer are funneled through this module
//! to provide structured, actionable error information.

use ruvector_core::error::RuvectorError;
use thiserror::Error;

/// Unified error type for all memory operations.
///
/// This enum covers failures from:
/// - CortexaDB storage operations
/// - ruvector-core vector indexing
/// - rkyv serialization/deserialization
/// - Configuration and validation
#[derive(Error, Debug)]
pub enum MemoryError {
    /// Storage initialization failed (e.g., permission denied, invalid path)
    #[error("Storage initialization failed: {0}")]
    InitFailed(String),

    /// Transaction failed (optimistic concurrency conflict, etc.)
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    /// Serialization or deserialization error
    #[error("Serialization error: {0}")]
    SerializationFailed(String),

    /// Orphaned tool_result detected during compaction (OpenClaw Issue #39609)
    #[error("Orphaned tool_result: tool_use_id={tool_use_id}, session={session_id}")]
    OrphanedToolResult {
        tool_use_id: String,
        session_id: String,
    },

    /// Vector engine initialization failed
    #[error("Vector engine initialization failed: {0}")]
    VectorInitFailed(String),

    /// Vector insertion failed
    #[error("Vector insertion failed: {0}")]
    VectorInsertFailed(String),

    /// Vector deletion failed
    #[error("Vector deletion failed: {0}")]
    VectorDeleteFailed(String),

    /// Vector query failed
    #[error("Vector query failed: {0}")]
    VectorQueryFailed(String),

    /// Dimension mismatch between expected and actual vector size
    #[error("Vector dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    /// Operation is not supported in the current configuration
    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    /// General I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// rkyv validation error
    #[error("Validation error: {0}")]
    Validation(String),
}

/// Result type alias for memory operations.
// pub type MemoryResult<T> = Result<T, MemoryError>;
impl From<rkyv::rancor::Error> for MemoryError {
    fn from(err: rkyv::rancor::Error) -> Self {
        MemoryError::SerializationFailed(err.to_string())
    }
}

impl From<RuvectorError> for MemoryError {
    fn from(err: RuvectorError) -> Self {
        let msg = err.to_string();
        let msg_lower = msg.to_lowercase();
        if msg_lower.contains("delete") || msg_lower.contains("remove") {
            MemoryError::VectorDeleteFailed(msg)
        } else if msg_lower.contains("search") || msg_lower.contains("query") {
            MemoryError::VectorQueryFailed(msg)
        } else {
            MemoryError::VectorInsertFailed(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = MemoryError::InitFailed("test".to_string());
        assert!(format!("{}", err).contains("initialization failed"));

        let err = MemoryError::OrphanedToolResult {
            tool_use_id: "call123".to_string(),
            session_id: "sess456".to_string(),
        };
        let s = format!("{}", err);
        assert!(s.contains("Orphaned"));
        assert!(s.contains("call123"));
        assert!(s.contains("sess456"));
    }
}
