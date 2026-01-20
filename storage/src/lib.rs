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

pub use state::*;
pub use event_store::*;
pub use anchor_store::*;
pub use object_store::*;
pub use subnet_state::{SubnetStateSMT, GlobalStateManager};

// StateProvider trait and implementations
pub use state_provider::{
    StateProvider, MerkleStateProvider, 
    CoinInfo, CoinState, SimpleMerkleProof,
    init_coin, get_coin_state,
};

// Re-export RocksDB types
pub use rocks::{SetuDB, RocksDBConfig, ColumnFamily, StorageError};
pub use rocks_object_store::RocksObjectStore;
pub use rocks_merkle_store::RocksDBMerkleStore;

