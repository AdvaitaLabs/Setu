//! State management module
//!
//! This module provides state management functionality:
//! - `GlobalStateManager`: Manages per-subnet Sparse Merkle Trees
//! - `SubnetStateSMT`: Individual subnet state SMT
//! - `StateProvider`: Trait for reading blockchain state
//! - `MerkleStateProvider`: Production implementation backed by SMT
//! - `BatchStateSnapshot`: Optimized batch state querying for high-throughput

pub mod manager;
pub mod provider;
pub mod batch_snapshot;

pub use manager::{SubnetStateSMT, GlobalStateManager, StateApplySummary, StateApplyError, RecoverySummary, B4StoreExt};
pub use provider::{
    StateProvider, MerkleStateProvider,
    CoinInfo, CoinState, SimpleMerkleProof,
    init_coin, get_coin_state,
};
pub use batch_snapshot::{BatchStateSnapshot, BatchSnapshotStats};
