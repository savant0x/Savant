use thiserror::Error;

/// Unified Error Type for Savant.
#[derive(Error, Debug)]
pub enum SavantError {
    /// Authentication failure. Always returns a generic message to prevent
    /// information leakage. Internal details are logged separately.
    #[error("Authentication failed")]
    AuthError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Action VETOED by swarm consensus: {0}")]
    ConsensusVeto(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Heuristic recovery failed: {0}")]
    HeuristicFailure(String),

    #[error("Ambiguity detected in autonomous intent: {0}")]
    AmbiguityDetected(String),

    #[error("Verification failure: {0}")]
    VerificationFailure(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Model/embedding error: {0}")]
    ModelError(String),

    #[error("Request timed out: {0}")]
    Timeout(String),

    #[error("Rate limited: {0}")]
    RateLimit(String),

    #[error("Operation failed: {0}")]
    OperationFailed(String),

    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    #[error("Circuit breaker tripped: {0}")]
    CircuitBreakerTripped(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}
