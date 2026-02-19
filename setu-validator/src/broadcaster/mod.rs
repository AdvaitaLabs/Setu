//! Consensus Broadcaster Implementations
//!
//! This module provides implementations of the `ConsensusBroadcaster` trait
//! from the consensus crate. The main implementation uses the Anemo P2P
//! network layer for actual message delivery.

mod anemo_adapter;

pub use anemo_adapter::AnemoConsensusBroadcaster;

// Re-export from consensus for convenience
pub use consensus::{
    BroadcastError, BroadcastResult, ConsensusBroadcaster, MockBroadcaster, NoOpBroadcaster,
};
