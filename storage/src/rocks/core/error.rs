//! Storage error types with rich context for debugging and error handling.
//!
//! This module provides structured error types that preserve:
//! - Error categorization (RocksDB, serialization, I/O, etc.)
//! - Operation context (what operation failed)
//! - Key context (which key was involved)
//! - Column family context (which CF was accessed)
//!
//! ## Usage
//!
//! ```ignore
//! use crate::rocks::error::{StorageError, StorageOperation};
//!
//! // Basic error
//! let err = StorageError::key_not_found("user:123");
//!
//! // Error with context
//! let err = StorageError::rocksdb(rocks_err)
//!     .with_operation(StorageOperation::Get)
//!     .with_key("user:123")
//!     .with_cf("users");
//! ```

use thiserror::Error;
use std::fmt;

/// The type of storage operation that failed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOperation {
    /// Get/read operation
    Get,
    /// Put/write operation
    Put,
    /// Delete operation
    Delete,
    /// Batch write operation
    BatchWrite,
    /// Iteration/scan operation
    Iterate,
    /// Open database
    Open,
    /// Column family operation
    ColumnFamily,
    /// Flush operation
    Flush,
    /// Compact operation
    Compact,
    /// Other operation
    Other,
}

impl fmt::Display for StorageOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageOperation::Get => write!(f, "get"),
            StorageOperation::Put => write!(f, "put"),
            StorageOperation::Delete => write!(f, "delete"),
            StorageOperation::BatchWrite => write!(f, "batch_write"),
            StorageOperation::Iterate => write!(f, "iterate"),
            StorageOperation::Open => write!(f, "open"),
            StorageOperation::ColumnFamily => write!(f, "column_family"),
            StorageOperation::Flush => write!(f, "flush"),
            StorageOperation::Compact => write!(f, "compact"),
            StorageOperation::Other => write!(f, "other"),
        }
    }
}

/// The underlying cause of a storage error
#[derive(Error, Debug)]
pub enum StorageErrorKind {
    #[error("RocksDB error: {0}")]
    RocksDB(#[from] rocksdb::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Column family not found: {0}")]
    ColumnFamilyNotFound(String),

    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

/// Rich storage error with context information
#[derive(Debug)]
pub struct StorageError {
    /// The underlying error kind
    pub kind: StorageErrorKind,
    /// The operation that failed (optional)
    pub operation: Option<StorageOperation>,
    /// The key involved (optional, truncated for display)
    pub key: Option<String>,
    /// The column family involved (optional)
    pub column_family: Option<String>,
}

impl StorageError {
    /// Create a new StorageError from an error kind
    pub fn new(kind: StorageErrorKind) -> Self {
        Self {
            kind,
            operation: None,
            key: None,
            column_family: None,
        }
    }

    // =========================================================================
    // Convenience constructors
    // =========================================================================

    /// Create a RocksDB error
    pub fn rocksdb(err: rocksdb::Error) -> Self {
        Self::new(StorageErrorKind::RocksDB(err))
    }

    /// Create a serialization error
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Serialization(msg.into()))
    }

    /// Create a deserialization error
    pub fn deserialization(msg: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Deserialization(msg.into()))
    }

    /// Create a column family not found error
    pub fn cf_not_found(cf_name: impl Into<String>) -> Self {
        let name = cf_name.into();
        Self::new(StorageErrorKind::ColumnFamilyNotFound(name.clone()))
            .with_cf(name)
    }

    /// Create a key not found error
    pub fn key_not_found(key: impl Into<String>) -> Self {
        let k = key.into();
        Self::new(StorageErrorKind::KeyNotFound(k.clone()))
            .with_key(k)
    }

    /// Create an invalid data error
    pub fn invalid_data(msg: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::InvalidData(msg.into()))
    }

    /// Create an IO error
    pub fn io(err: std::io::Error) -> Self {
        Self::new(StorageErrorKind::Io(err))
    }

    /// Create a generic other error
    pub fn other(msg: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Other(msg.into()))
    }

    // =========================================================================
    // Context builders (fluent API)
    // =========================================================================

    /// Add operation context
    pub fn with_operation(mut self, op: StorageOperation) -> Self {
        self.operation = Some(op);
        self
    }

    /// Add key context (automatically truncates long keys)
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        let k = key.into();
        // Truncate long keys for display
        self.key = Some(if k.len() > 64 {
            format!("{}...", &k[..61])
        } else {
            k
        });
        self
    }

    /// Add key context from bytes
    pub fn with_key_bytes(mut self, key: &[u8]) -> Self {
        // Try to display as UTF-8, otherwise show hex
        let k = match std::str::from_utf8(key) {
            Ok(s) => s.to_string(),
            Err(_) => {
                if key.len() <= 32 {
                    hex::encode(key)
                } else {
                    format!("{}...", hex::encode(&key[..29]))
                }
            }
        };
        self.key = Some(if k.len() > 64 {
            format!("{}...", &k[..61])
        } else {
            k
        });
        self
    }

    /// Add column family context
    pub fn with_cf(mut self, cf: impl Into<String>) -> Self {
        self.column_family = Some(cf.into());
        self
    }

    // =========================================================================
    // Query methods
    // =========================================================================

    /// Check if this is a "not found" error
    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, StorageErrorKind::KeyNotFound(_))
    }

    /// Check if this is a RocksDB error
    pub fn is_rocksdb_error(&self) -> bool {
        matches!(self.kind, StorageErrorKind::RocksDB(_))
    }

    /// Check if this is a serialization/deserialization error
    pub fn is_serde_error(&self) -> bool {
        matches!(
            self.kind,
            StorageErrorKind::Serialization(_) | StorageErrorKind::Deserialization(_)
        )
    }

    /// Get a brief error code for metrics/logging
    pub fn error_code(&self) -> &'static str {
        match &self.kind {
            StorageErrorKind::RocksDB(_) => "ROCKS_DB",
            StorageErrorKind::Serialization(_) => "SERIALIZE",
            StorageErrorKind::Deserialization(_) => "DESERIALIZE",
            StorageErrorKind::ColumnFamilyNotFound(_) => "CF_NOT_FOUND",
            StorageErrorKind::KeyNotFound(_) => "KEY_NOT_FOUND",
            StorageErrorKind::InvalidData(_) => "INVALID_DATA",
            StorageErrorKind::Io(_) => "IO_ERROR",
            StorageErrorKind::Other(_) => "OTHER",
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Build context string
        let mut context_parts = Vec::new();
        
        if let Some(op) = &self.operation {
            context_parts.push(format!("op={}", op));
        }
        if let Some(cf) = &self.column_family {
            context_parts.push(format!("cf={}", cf));
        }
        if let Some(key) = &self.key {
            context_parts.push(format!("key={}", key));
        }

        if context_parts.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{} [{}]", self.kind, context_parts.join(", "))
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            StorageErrorKind::RocksDB(e) => Some(e),
            StorageErrorKind::Io(e) => Some(e),
            _ => None,
        }
    }
}

// =========================================================================
// From implementations for automatic conversion
// =========================================================================

impl From<rocksdb::Error> for StorageError {
    fn from(e: rocksdb::Error) -> Self {
        Self::rocksdb(e)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::io(e)
    }
}

impl From<bcs::Error> for StorageError {
    fn from(e: bcs::Error) -> Self {
        Self::deserialization(e.to_string())
    }
}

impl From<bincode::error::DecodeError> for StorageError {
    fn from(e: bincode::error::DecodeError) -> Self {
        Self::deserialization(e.to_string())
    }
}

impl From<bincode::error::EncodeError> for StorageError {
    fn from(e: bincode::error::EncodeError) -> Self {
        Self::serialization(e.to_string())
    }
}

// Allow creating from StorageErrorKind directly
impl From<StorageErrorKind> for StorageError {
    fn from(kind: StorageErrorKind) -> Self {
        Self::new(kind)
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

// =========================================================================
// Extension trait for Result to add context
// =========================================================================

/// Extension trait for adding context to storage results
pub trait StorageResultExt<T> {
    /// Add operation context to the error
    fn with_operation(self, op: StorageOperation) -> Result<T>;
    
    /// Add key context to the error
    fn with_key(self, key: impl Into<String>) -> Result<T>;
    
    /// Add key context from bytes
    fn with_key_bytes(self, key: &[u8]) -> Result<T>;
    
    /// Add column family context
    fn with_cf(self, cf: impl Into<String>) -> Result<T>;
    
    /// Add full context
    fn with_context(
        self,
        op: StorageOperation,
        cf: impl Into<String>,
        key: impl Into<String>,
    ) -> Result<T>;
}

impl<T> StorageResultExt<T> for Result<T> {
    fn with_operation(self, op: StorageOperation) -> Result<T> {
        self.map_err(|e| e.with_operation(op))
    }
    
    fn with_key(self, key: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.with_key(key))
    }
    
    fn with_key_bytes(self, key: &[u8]) -> Result<T> {
        self.map_err(|e| e.with_key_bytes(key))
    }
    
    fn with_cf(self, cf: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.with_cf(cf))
    }
    
    fn with_context(
        self,
        op: StorageOperation,
        cf: impl Into<String>,
        key: impl Into<String>,
    ) -> Result<T> {
        self.map_err(|e| e.with_operation(op).with_cf(cf).with_key(key))
    }
}

// =========================================================================
// SetuError conversion (storage crate depends on types, so we can implement From here)
// =========================================================================

use setu_types::SetuError;

impl From<StorageError> for SetuError {
    fn from(err: StorageError) -> Self {
        // Use the Display implementation which includes context
        SetuError::StorageError(err.to_string())
    }
}

/// Extension trait to convert storage results to SetuResult
pub trait IntoSetuResult<T> {
    /// Convert a storage Result to SetuResult, preserving error context
    fn into_setu_result(self) -> setu_types::SetuResult<T>;
}

impl<T> IntoSetuResult<T> for Result<T> {
    fn into_setu_result(self) -> setu_types::SetuResult<T> {
        self.map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_without_context() {
        let err = StorageError::key_not_found("test_key");
        let display = format!("{}", err);
        assert!(display.contains("Key not found"));
        assert!(display.contains("test_key"));
    }

    #[test]
    fn test_error_display_with_full_context() {
        let err = StorageError::other("something went wrong")
            .with_operation(StorageOperation::Put)
            .with_cf("users")
            .with_key("user:123");
        
        let display = format!("{}", err);
        assert!(display.contains("something went wrong"));
        assert!(display.contains("op=put"));
        assert!(display.contains("cf=users"));
        assert!(display.contains("key=user:123"));
    }

    #[test]
    fn test_long_key_truncation() {
        let long_key = "a".repeat(100);
        let err = StorageError::key_not_found(&long_key);
        
        let display = format!("{}", err);
        assert!(display.contains("..."));
        assert!(display.len() < 200); // Should be truncated
    }

    #[test]
    fn test_error_code() {
        assert_eq!(StorageError::key_not_found("x").error_code(), "KEY_NOT_FOUND");
        assert_eq!(StorageError::serialization("x").error_code(), "SERIALIZE");
        assert_eq!(StorageError::cf_not_found("x").error_code(), "CF_NOT_FOUND");
    }

    #[test]
    fn test_is_not_found() {
        assert!(StorageError::key_not_found("x").is_not_found());
        assert!(!StorageError::other("x").is_not_found());
    }

    #[test]
    fn test_result_extension() {
        let result: Result<()> = Err(StorageError::other("test"));
        let result = result.with_operation(StorageOperation::Get).with_cf("test_cf");
        
        if let Err(e) = result {
            assert_eq!(e.operation, Some(StorageOperation::Get));
            assert_eq!(e.column_family, Some("test_cf".to_string()));
        }
    }

    #[test]
    fn test_bytes_key_utf8() {
        let err = StorageError::other("test").with_key_bytes(b"hello");
        assert_eq!(err.key, Some("hello".to_string()));
    }

    #[test]
    fn test_bytes_key_hex() {
        let err = StorageError::other("test").with_key_bytes(&[0x00, 0x01, 0x02, 0xFF]);
        assert_eq!(err.key, Some("000102ff".to_string()));
    }

    #[test]
    fn test_setu_error_conversion() {
        let storage_err = StorageError::key_not_found("user:123")
            .with_operation(StorageOperation::Get)
            .with_cf("users");
        
        let setu_err: SetuError = storage_err.into();
        let msg = format!("{}", setu_err);
        
        // The SetuError message should contain the full context
        assert!(msg.contains("Storage error"));
        assert!(msg.contains("Key not found"));
        assert!(msg.contains("op=get"));
        assert!(msg.contains("cf=users"));
    }

    #[test]
    fn test_into_setu_result() {
        let result: Result<i32> = Ok(42);
        let setu_result = result.into_setu_result();
        assert_eq!(setu_result.unwrap(), 42);

        let result: Result<i32> = Err(StorageError::other("test error"));
        let setu_result = result.into_setu_result();
        assert!(setu_result.is_err());
    }
}
