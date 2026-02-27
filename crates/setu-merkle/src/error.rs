//! Error types for merkle tree operations.

use thiserror::Error;

/// Result type for merkle operations
pub type MerkleResult<T> = Result<T, MerkleError>;

/// Errors that can occur during merkle tree operations
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MerkleError {
    /// The provided proof is invalid
    #[error("Invalid proof: {0}")]
    InvalidProof(String),

    /// Index out of bounds
    #[error("Index out of bounds: {index} >= {size}")]
    IndexOutOfBounds { index: usize, size: usize },

    /// Invalid hash length
    #[error("Invalid hash length: expected {expected}, got {got}")]
    InvalidHashLength { expected: usize, got: usize },

    /// Key not found in the tree
    #[error("Key not found")]
    KeyNotFound,

    /// Empty tree error
    #[error("Cannot perform operation on empty tree")]
    EmptyTree,

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Storage backend error
    #[error("Storage error: {0}")]
    StorageError(String),

    /// Invalid input
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Deserialization error  
    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    /// Data consistency error (e.g., root hash mismatch during recovery)
    #[error("Consistency error: {0}")]
    ConsistencyError(String),
}
