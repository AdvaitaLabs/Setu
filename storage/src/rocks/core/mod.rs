//! RocksDB core infrastructure
//!
//! This module provides the foundational components for RocksDB storage:
//! - `SetuDB`: Main database wrapper with column family support
//! - `RocksDBConfig`: Configuration options
//! - `ColumnFamily`: Column family definitions
//! - `StorageError`: Rich error types with context
//! - Async helpers for blocking operations

pub mod db;
pub mod error;
pub mod config;
pub mod column_family;
pub mod async_wrapper;

pub use db::SetuDB;
pub use error::{StorageError, StorageErrorKind, StorageOperation, StorageResultExt, IntoSetuResult};
pub use config::RocksDBConfig;
pub use column_family::ColumnFamily;
pub use async_wrapper::{spawn_db_op, spawn_db_op_result, BlockingDbWrapper};
