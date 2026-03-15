//! Runtime error types

use setu_types::ObjectId;
use thiserror::Error;

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Object not found: {0}")]
    ObjectNotFound(ObjectId),

    #[error("Insufficient balance for {address}: required {required}, available {available}")]
    InsufficientBalance {
        address: String,
        required: u64,
        available: u64,
    },

    #[error("Invalid ownership: object {object_id} is not owned by {address}")]
    InvalidOwnership {
        object_id: ObjectId,
        address: String,
    },

    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),

    #[error("State error: {0}")]
    StateError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Program execution error: {0}")]
    ProgramExecution(String),

    #[error("Program aborted: {0}")]
    ProgramAbort(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}
