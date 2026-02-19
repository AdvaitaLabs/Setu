// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Consensus Broadcaster Trait
//!
//! This module defines the abstraction for broadcasting consensus messages
//! to peer validators. The actual network implementation is provided by
//! the validator module, keeping consensus decoupled from networking.
//!
//! ## Design Pattern: Dependency Inversion
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    ConsensusEngine                          │
//! │                          │                                  │
//! │                          │ uses                             │
//! │                          ▼                                  │
//! │              trait ConsensusBroadcaster                     │
//! │                          ▲                                  │
//! │                          │ implements                       │
//! │                          │                                  │
//! └──────────────────────────┼──────────────────────────────────┘
//!                            │
//!              ┌─────────────┴─────────────┐
//!              │                           │
//!     AnemoConsensusBroadcaster    MockConsensusBroadcaster
//!     (production)                 (testing)
//! ```

use setu_types::{ConsensusFrame, Event, EventId, Vote};
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

/// Error type for broadcast operations
#[derive(Debug, Error)]
pub enum BroadcastError {
    /// Network layer error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Broadcast timeout
    #[error("Broadcast timeout after {0}ms")]
    Timeout(u64),

    /// No peers available
    #[error("No peer validators available")]
    NoPeers,

    /// All broadcast attempts failed
    #[error("All broadcasts failed: {0}")]
    AllFailed(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

/// Result of a broadcast operation
#[derive(Debug, Clone)]
pub struct BroadcastResult {
    /// Number of peers successfully sent to
    pub success_count: usize,
    /// Total number of peers attempted
    pub total_peers: usize,
    /// Details of failures (peer_id, error message)
    pub failures: Vec<(String, String)>,
}

impl BroadcastResult {
    /// Create a successful result
    pub fn success(count: usize, total: usize) -> Self {
        Self {
            success_count: count,
            total_peers: total,
            failures: Vec::new(),
        }
    }

    /// Create a result with failures
    pub fn with_failures(
        success_count: usize,
        total: usize,
        failures: Vec<(String, String)>,
    ) -> Self {
        Self {
            success_count,
            total_peers: total,
            failures,
        }
    }

    /// Check if all broadcasts succeeded
    pub fn all_succeeded(&self) -> bool {
        self.success_count == self.total_peers
    }

    /// Check if we reached a quorum (2/3 + 1)
    pub fn reached_quorum(&self, validator_count: usize) -> bool {
        let threshold = (validator_count * 2) / 3 + 1;
        self.success_count >= threshold
    }
}

/// Trait for broadcasting consensus messages to peer validators
///
/// This trait abstracts the network layer from consensus logic.
/// Implementations are provided by the validator module using
/// the actual network transport (e.g., Anemo P2P).
///
/// # Example
///
/// ```ignore
/// struct MyBroadcaster { /* ... */ }
///
/// #[async_trait::async_trait]
/// impl ConsensusBroadcaster for MyBroadcaster {
///     async fn broadcast_cf(&self, cf: &ConsensusFrame) -> Result<BroadcastResult, BroadcastError> {
///         // Send CF to all peers
///         Ok(BroadcastResult::success(3, 3))
///     }
///     // ... other methods
/// }
/// ```
#[async_trait::async_trait]
pub trait ConsensusBroadcaster: Send + Sync + Debug {
    /// Broadcast a ConsensusFrame proposal to all peer validators
    ///
    /// Called when this validator (as leader) creates a new CF.
    /// All other validators should receive this to vote on it.
    async fn broadcast_cf(&self, cf: &ConsensusFrame) -> Result<BroadcastResult, BroadcastError>;

    /// Broadcast a vote to all peer validators
    ///
    /// Called after a validator decides to approve/reject a CF.
    /// The leader collects votes to reach quorum.
    async fn broadcast_vote(&self, vote: &Vote) -> Result<BroadcastResult, BroadcastError>;

    /// Broadcast a finalization notification
    ///
    /// Called when a CF is finalized (quorum reached).
    /// This helps late-joining nodes sync up.
    async fn broadcast_finalized(&self, cf_id: &str) -> Result<BroadcastResult, BroadcastError>;

    /// Broadcast an event to all peer validators
    ///
    /// Called when a new event is added to the local DAG.
    /// This ensures all validators have the same events before CF creation.
    /// This is the PRIMARY path for event propagation.
    async fn broadcast_event(&self, event: &Event) -> Result<BroadcastResult, BroadcastError>;

    /// Request specific events from peers (fallback/recovery)
    ///
    /// Called when receiving a CF that references unknown events.
    /// Returns the events that were successfully fetched.
    async fn request_events(&self, event_ids: &[EventId]) -> Result<Vec<Event>, BroadcastError>;

    /// Get the number of connected peer validators
    fn peer_count(&self) -> usize;

    /// Get the local validator ID
    fn local_validator_id(&self) -> &str;
}

/// A no-op broadcaster for testing or single-node mode
#[derive(Debug, Clone)]
pub struct NoOpBroadcaster {
    local_id: String,
}

impl NoOpBroadcaster {
    /// Create a new no-op broadcaster
    pub fn new(local_id: String) -> Self {
        Self { local_id }
    }
}

#[async_trait::async_trait]
impl ConsensusBroadcaster for NoOpBroadcaster {
    async fn broadcast_cf(&self, _cf: &ConsensusFrame) -> Result<BroadcastResult, BroadcastError> {
        // No-op: just return success with 0 peers
        Ok(BroadcastResult::success(0, 0))
    }

    async fn broadcast_vote(&self, _vote: &Vote) -> Result<BroadcastResult, BroadcastError> {
        Ok(BroadcastResult::success(0, 0))
    }

    async fn broadcast_finalized(&self, _cf_id: &str) -> Result<BroadcastResult, BroadcastError> {
        Ok(BroadcastResult::success(0, 0))
    }

    async fn broadcast_event(&self, _event: &Event) -> Result<BroadcastResult, BroadcastError> {
        // No-op: just return success with 0 peers
        Ok(BroadcastResult::success(0, 0))
    }

    async fn request_events(&self, _event_ids: &[EventId]) -> Result<Vec<Event>, BroadcastError> {
        // No-op: return empty vec (no peers to request from)
        Ok(Vec::new())
    }

    fn peer_count(&self) -> usize {
        0
    }

    fn local_validator_id(&self) -> &str {
        &self.local_id
    }
}

/// A mock broadcaster for testing that records all broadcasts
#[derive(Debug)]
pub struct MockBroadcaster {
    local_id: String,
    peer_count: usize,
    /// Recorded CF broadcasts
    pub cf_broadcasts: std::sync::Mutex<Vec<ConsensusFrame>>,
    /// Recorded vote broadcasts
    pub vote_broadcasts: std::sync::Mutex<Vec<Vote>>,
    /// Recorded finalization broadcasts
    pub finalized_broadcasts: std::sync::Mutex<Vec<String>>,
    /// Recorded event broadcasts
    pub event_broadcasts: std::sync::Mutex<Vec<Event>>,
    /// Whether to simulate failures
    pub simulate_failure: std::sync::atomic::AtomicBool,
}

impl MockBroadcaster {
    /// Create a new mock broadcaster
    pub fn new(local_id: String, peer_count: usize) -> Self {
        Self {
            local_id,
            peer_count,
            cf_broadcasts: std::sync::Mutex::new(Vec::new()),
            vote_broadcasts: std::sync::Mutex::new(Vec::new()),
            finalized_broadcasts: std::sync::Mutex::new(Vec::new()),
            event_broadcasts: std::sync::Mutex::new(Vec::new()),
            simulate_failure: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Set whether to simulate failures
    pub fn set_simulate_failure(&self, fail: bool) {
        self.simulate_failure
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get recorded CF broadcasts
    pub fn get_cf_broadcasts(&self) -> Vec<ConsensusFrame> {
        self.cf_broadcasts.lock().unwrap().clone()
    }

    /// Get recorded vote broadcasts
    pub fn get_vote_broadcasts(&self) -> Vec<Vote> {
        self.vote_broadcasts.lock().unwrap().clone()
    }

    /// Get recorded finalization broadcasts
    pub fn get_finalized_broadcasts(&self) -> Vec<String> {
        self.finalized_broadcasts.lock().unwrap().clone()
    }

    /// Get recorded event broadcasts
    pub fn get_event_broadcasts(&self) -> Vec<Event> {
        self.event_broadcasts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ConsensusBroadcaster for MockBroadcaster {
    async fn broadcast_cf(&self, cf: &ConsensusFrame) -> Result<BroadcastResult, BroadcastError> {
        if self
            .simulate_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BroadcastError::AllFailed("Simulated failure".to_string()));
        }
        self.cf_broadcasts.lock().unwrap().push(cf.clone());
        Ok(BroadcastResult::success(self.peer_count, self.peer_count))
    }

    async fn broadcast_vote(&self, vote: &Vote) -> Result<BroadcastResult, BroadcastError> {
        if self
            .simulate_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BroadcastError::AllFailed("Simulated failure".to_string()));
        }
        self.vote_broadcasts.lock().unwrap().push(vote.clone());
        Ok(BroadcastResult::success(self.peer_count, self.peer_count))
    }

    async fn broadcast_finalized(&self, cf_id: &str) -> Result<BroadcastResult, BroadcastError> {
        if self
            .simulate_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BroadcastError::AllFailed("Simulated failure".to_string()));
        }
        self.finalized_broadcasts
            .lock()
            .unwrap()
            .push(cf_id.to_string());
        Ok(BroadcastResult::success(self.peer_count, self.peer_count))
    }

    async fn broadcast_event(&self, event: &Event) -> Result<BroadcastResult, BroadcastError> {
        if self
            .simulate_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BroadcastError::AllFailed("Simulated failure".to_string()));
        }
        self.event_broadcasts.lock().unwrap().push(event.clone());
        Ok(BroadcastResult::success(self.peer_count, self.peer_count))
    }

    async fn request_events(&self, _event_ids: &[EventId]) -> Result<Vec<Event>, BroadcastError> {
        if self
            .simulate_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BroadcastError::AllFailed("Simulated failure".to_string()));
        }
        // Mock: return empty vec (no events to fetch)
        Ok(Vec::new())
    }

    fn peer_count(&self) -> usize {
        self.peer_count
    }

    fn local_validator_id(&self) -> &str {
        &self.local_id
    }
}

/// Helper type for optional broadcaster
pub type OptionalBroadcaster = Option<Arc<dyn ConsensusBroadcaster>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_broadcaster() {
        let broadcaster = NoOpBroadcaster::new("validator-1".to_string());
        assert_eq!(broadcaster.peer_count(), 0);
        assert_eq!(broadcaster.local_validator_id(), "validator-1");
    }

    #[tokio::test]
    async fn test_mock_broadcaster() {
        let broadcaster = MockBroadcaster::new("validator-1".to_string(), 3);
        assert_eq!(broadcaster.peer_count(), 3);

        // Test CF broadcast
        let anchor = setu_types::Anchor::new(
            vec!["event-1".to_string()],
            setu_vlc::VLCSnapshot::default(),
            "state_root".to_string(),
            None,
            0,
        );
        let cf = setu_types::ConsensusFrame::new(anchor, "validator-1".to_string());

        let result = broadcaster.broadcast_cf(&cf).await.unwrap();
        assert_eq!(result.success_count, 3);
        assert!(result.all_succeeded());
        assert_eq!(broadcaster.get_cf_broadcasts().len(), 1);
    }

    #[tokio::test]
    async fn test_broadcast_result_quorum() {
        // For 3 validators: (3*2)/3 + 1 = 3 needed for quorum
        let result = BroadcastResult::success(3, 3);
        assert!(result.reached_quorum(3)); // 3 >= 3 ✓

        let result = BroadcastResult::success(2, 3);
        assert!(!result.reached_quorum(3)); // 2 < 3 ✗

        // For 4 validators: (4*2)/3 + 1 = 3 needed for quorum (rounded down)
        let result = BroadcastResult::success(3, 4);
        assert!(result.reached_quorum(4)); // 3 >= 3 ✓

        let result = BroadcastResult::success(2, 4);
        assert!(!result.reached_quorum(4)); // 2 < 3 ✗
    }
}
