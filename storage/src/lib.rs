//! Setu Storage Layer
//!
//! This crate provides the storage abstraction and implementations for Setu.
//!
//! ## Module Structure
//!
//! - `types`: Storage-specific types (BatchStoreResult, etc.)
//! - `backends`: Storage backend traits (EventStoreBackend, etc.)
//! - `memory`: In-memory implementations using DashMap
//! - `rocks`: RocksDB persistent implementations
//! - `state`: State management (GlobalStateManager, StateProvider)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use setu_storage::{EventStore, EventStoreBackend};  // Memory impl + trait
//! use setu_storage::{RocksDBEventStore, SetuDB};      // RocksDB impl
//! use setu_storage::{GlobalStateManager, StateProvider};  // State management
//! ```

// Module declarations
pub mod types;
pub mod backends;
pub mod memory;
pub mod rocks;
pub mod state;

// ============================================================================
// Re-exports for backward compatibility (100% API compatible)
// ============================================================================

// Storage types
pub use types::*;

// Backend traits
pub use backends::{EventStoreBackend, AnchorStoreBackend, CFStoreBackend, ObjectStore};

// Memory implementations
pub use memory::{EventStore, AnchorStore, CFStore, MemoryObjectStore};

// RocksDB types and implementations
pub use rocks::{SetuDB, RocksDBConfig, ColumnFamily, StorageError};
pub use rocks::{RocksDBEventStore, RocksDBAnchorStore, RocksDBCFStore};
pub use rocks::{RocksObjectStore, RebuildIndexResult, RocksDBMerkleStore};

// State management
pub use state::{SubnetStateSMT, GlobalStateManager, StateApplySummary, StateApplyError, RecoverySummary};
pub use state::{B4StoreExt}; // B4 scheme combined storage trait (extended from setu_merkle::B4Store)
pub use state::{StateProvider, MerkleStateProvider, CoinInfo, CoinState, SimpleMerkleProof};
pub use state::{init_coin, get_coin_state};
pub use state::{BatchStateSnapshot, BatchSnapshotStats};

// Re-export MerkleStore trait from setu-merkle for convenience
pub use setu_merkle::storage::{MerkleStore, MerkleLeafStore, MerkleMetaStore, B4Store};

// ============================================================================
// Backward compatibility: module path aliases
// ============================================================================

/// Backward compatibility alias for `state::manager`
pub mod subnet_state {
    pub use crate::state::manager::*;
}

/// Backward compatibility alias for `state::provider`
pub mod state_provider {
    pub use crate::state::provider::*;
}