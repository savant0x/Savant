use thiserror::Error;

/// IPC-specific errors for the zero-copy blackboard subsystem.
#[derive(Error, Debug)]
pub enum SwarmIpcError {
    #[error("Failed to initialize iceoryx2 node: {0}")]
    NodeCreation(String),

    #[error("Failed to create blackboard service: {0}")]
    ServiceCreation(String),

    #[error("Failed to access shared context: {0}")]
    AccessViolation(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Service shutdown failed: {0}")]
    Shutdown(String),

    #[error("Invalid service name: {0}")]
    InvalidServiceName(String),
}

impl SwarmIpcError {
    /// Check if the error is a fatal node creation failure.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::NodeCreation(_) | Self::ServiceCreation(_))
    }

    /// Check if the error is an access violation (non-fatal).
    pub fn is_access_violation(&self) -> bool {
        matches!(self, Self::AccessViolation(_))
    }
}
