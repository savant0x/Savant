use thiserror::Error;

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Memory error: {0}")]
    Memory(#[from] savant_memory::MemoryError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Injection detected in vault file: {0}")]
    InjectionDetected(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("Vault path not configured")]
    VaultPathNotConfigured,
}
