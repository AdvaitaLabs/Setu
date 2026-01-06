//! Runtime error types

use thiserror::Error;
use setu_types::ObjectId;

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Object not found: {0}")]
    ObjectNotFound(ObjectId),
    
    #[error("Insufficient balance: have {have}, need {need}")]
    InsufficientBalance { have: u64, need: u64 },
    
    #[error("Invalid ownership: object {object_id} is not owned by {address}")]
    InvalidOwnership { object_id: ObjectId, address: String },
    
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),
    
    #[error("State error: {0}")]
    StateError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    #[error("Unknown error: {0}")]
    Unknown(String),
}
