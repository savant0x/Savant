pub mod block_quota;
pub mod oci_verifier;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("platform not supported: {0}")]
    UnsupportedPlatform(String),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("signature invalid: {0}")]
    SignatureInvalid(String),
}
