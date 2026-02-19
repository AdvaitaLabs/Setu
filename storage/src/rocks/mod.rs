//! RocksDB storage implementation for Setu
//!
//! This module provides persistent storage using RocksDB.
//!
//! ## Structure
//! - `core/`: Foundation infrastructure (SetuDB, config, errors)
//! - Store implementations: event_store, anchor_store, cf_store, object_store, merkle_store

// Core infrastructure
pub mod core;

// Store implementations
pub mod anchor_store;
pub mod cf_store;
pub mod event_store;
pub mod merkle_store;
pub mod object_store;

// Re-export core types for convenience
pub use core::{spawn_db_op, spawn_db_op_result, BlockingDbWrapper};
pub use core::{
    ColumnFamily, IntoSetuResult, RocksDBConfig, SetuDB, StorageError, StorageErrorKind,
    StorageOperation, StorageResultExt,
};

// Re-export store implementations
pub use anchor_store::RocksDBAnchorStore;
pub use cf_store::RocksDBCFStore;
pub use event_store::RocksDBEventStore;
pub use merkle_store::RocksDBMerkleStore;
pub use object_store::{RebuildIndexResult, RocksObjectStore};
