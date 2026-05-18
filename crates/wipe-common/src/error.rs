use thiserror::Error;

#[derive(Debug, Error)]
pub enum WipeError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("device frozen — issue ATA security freeze unlock first")]
    DeviceFrozen,
    #[error("method {0} not supported by this device")]
    MethodUnsupported(String),
    #[error("operation aborted by operator")]
    Aborted,
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("cert generation failed: {0}")]
    Cert(String),
    #[error("license check failed: {0}")]
    License(String),
}

pub type WipeResult<T> = Result<T, WipeError>;
