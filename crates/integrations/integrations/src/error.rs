//! Integration error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IntegrationError {
    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Sync error: {0}")]
    SyncError(String),
}

pub type IntegrationResult<T> = Result<T, IntegrationError>;
