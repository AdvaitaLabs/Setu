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
pub mod event_store;
pub mod anchor_store;
pub mod cf_store;
pub mod object_store;
pub mod merkle_store;

// Re-export core types for convenience
pub use core::{SetuDB, RocksDBConfig, ColumnFamily, StorageError, StorageErrorKind, StorageOperation, StorageResultExt, IntoSetuResult};
pub use core::{spawn_db_op, spawn_db_op_result, BlockingDbWrapper};

// Re-export store implementations
pub use event_store::RocksDBEventStore;
pub use anchor_store::RocksDBAnchorStore;
pub use cf_store::RocksDBCFStore;
pub use object_store::{RocksObjectStore, RebuildIndexResult};
pub use merkle_store::RocksDBMerkleStore;
