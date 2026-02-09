//! State management module
//!
//! This module provides state management functionality:
//! - `GlobalStateManager`: Manages per-subnet Sparse Merkle Trees
//! - `SubnetStateSMT`: Individual subnet state SMT
//! - `StateProvider`: Trait for reading blockchain state
//! - `MerkleStateProvider`: Production implementation backed by SMT

pub mod manager;
pub mod provider;

pub use manager::{SubnetStateSMT, GlobalStateManager, StateApplySummary, StateApplyError};
pub use provider::{
    StateProvider, MerkleStateProvider,
    CoinInfo, CoinState, SimpleMerkleProof,
    init_coin, get_coin_state,
};
