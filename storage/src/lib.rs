pub mod state;
pub mod event_store;
pub mod anchor_store;
pub mod object_store;
pub mod subnet_state;
pub mod state_provider;

// RocksDB storage implementation
pub mod rocks;
pub mod rocks_object_store;
pub mod rocks_merkle_store;
pub mod rocks_event_store;
pub mod rocks_anchor_store;
pub mod rocks_cf_store;

// Backend traits for storage abstraction
pub mod event_store_backend;
pub mod anchor_store_backend;
pub mod cf_store_backend;

pub use state::*;
pub use event_store::*;
pub use anchor_store::*;
pub use object_store::*;
pub use subnet_state::{SubnetStateSMT, GlobalStateManager, StateApplySummary, StateApplyError};

// Backend traits
pub use event_store_backend::EventStoreBackend;
pub use anchor_store_backend::AnchorStoreBackend;
pub use cf_store_backend::CFStoreBackend;

// StateProvider trait and implementations
pub use state_provider::{
    StateProvider, MerkleStateProvider, 
    CoinInfo, CoinState, SimpleMerkleProof,
    init_coin, get_coin_state,
};

// Re-export RocksDB types
pub use rocks::{SetuDB, RocksDBConfig, ColumnFamily, StorageError};
pub use rocks_object_store::{RocksObjectStore, RebuildIndexResult};
pub use rocks_merkle_store::RocksDBMerkleStore;
pub use rocks_event_store::RocksDBEventStore;
pub use rocks_anchor_store::RocksDBAnchorStore;
pub use rocks_cf_store::RocksDBCFStore;

// Re-export MerkleStore trait from setu-merkle for convenience
pub use setu_merkle::storage::MerkleStore;

