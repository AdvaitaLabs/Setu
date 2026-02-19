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
pub mod backends;
pub mod memory;
pub mod rocks;
pub mod state;
pub mod types;

// ============================================================================
// Re-exports for backward compatibility (100% API compatible)
// ============================================================================

// Storage types
pub use types::*;

// Backend traits
pub use backends::{AnchorStoreBackend, CFStoreBackend, EventStoreBackend, ObjectStore};

// Memory implementations
pub use memory::{AnchorStore, CFStore, EventStore, MemoryObjectStore};

// RocksDB types and implementations
pub use rocks::{ColumnFamily, RocksDBConfig, SetuDB, StorageError};
pub use rocks::{RebuildIndexResult, RocksDBMerkleStore, RocksObjectStore};
pub use rocks::{RocksDBAnchorStore, RocksDBCFStore, RocksDBEventStore};

// State management
pub use state::{get_coin_state, init_coin};
pub use state::{CoinInfo, CoinState, MerkleStateProvider, SimpleMerkleProof, StateProvider};
pub use state::{GlobalStateManager, StateApplyError, StateApplySummary, SubnetStateSMT};

// Re-export MerkleStore trait from setu-merkle for convenience
pub use setu_merkle::storage::MerkleStore;

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
