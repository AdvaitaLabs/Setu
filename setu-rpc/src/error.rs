//! RPC error types

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
}

pub type Result<T> = std::result::Result<T, RpcError>;
