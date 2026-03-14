use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("SSH connection failed: {0}")]
    SshConnectionError(String),

    #[error("Authentication failed: {0}")]
    AuthenticationError(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Invalid host configuration: {0}")]
    InvalidHostConfig(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("SSH error: {0}")]
    Ssh2Error(#[from] ssh2::Error),
}

impl From<AppError> for String {
    fn from(error: AppError) -> Self {
        error.to_string()
    }
}
