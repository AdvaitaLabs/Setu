// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! State synchronization for Setu network
//!
//! This module is modeled after Sui's state_sync system but adapted for Setu's
//! Event/ConsensusFrame/VLC-based consensus model instead of checkpoints.
//!
//! **Note**: This module uses deprecated types from setu-protocol for backward
//! compatibility. Future versions will use generic types defined in the
//! application layer.

#![allow(deprecated)]

//! ## Key Differences from Sui's Checkpoint-based Sync
//!
//! - **Event Sync**: Sync individual events between nodes
//! - **CF Sync**: Sync ConsensusFrames which aggregate events
//! - **VLC Tracking**: Track Vector Logical Clocks for causal ordering
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │        StateSyncEventLoop               │
//! │  ┌───────────────────────────────────┐  │
//! │  │  Periodic sync checks             │  │
//! │  │  - Compare local vs peer heights  │  │
//! │  │  - Request missing events/CFs     │  │
//! │  │  - Push local updates             │  │
//! │  └───────────────────────────────────┘  │
//! └─────────────────────────────────────────┘
//!                    │
//! ┌──────────────────┴──────────────────────┐
//! │             Server (RPC)                 │
//! │  - get_events                           │
//! │  - get_consensus_frames                 │
//! │  - push_events                          │
//! │  - push_consensus_frame                 │
//! └─────────────────────────────────────────┘
//! ```

mod builder;
pub mod metrics;
mod server;
mod store;

pub use builder::{Builder, UnstartedStateSync};
pub use server::{StateSync, StateSyncServer, Server};
pub use server::{
    GetEventsRequest, GetEventsResponse,
    GetConsensusFramesRequest, GetConsensusFramesResponse,
    PushEventsRequest, PushEventsResponse,
    PushConsensusFrameRequest, PushConsensusFrameResponse,
    GetSyncStateRequest, GetSyncStateResponse,
};
pub use store::{InMemoryStateSyncStore, InMemoryStoreError};

// Re-export from setu-protocol
pub use setu_protocol::{SerializedEvent, SerializedConsensusFrame, SerializedVote};

use anemo::PeerId;
use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

// ============================================================================
// StateSyncStore Trait
// ============================================================================

/// Storage trait for state synchronization
/// 
/// This trait abstracts the storage operations needed for state sync,
/// allowing the sync system to work with different storage backends.
#[async_trait]
pub trait StateSyncStore: Clone + Send + Sync + 'static {
    /// Error type for storage operations
    type Error: std::error::Error + Send + Sync + 'static;

    // ===== Event Storage =====
    
    /// Get events starting from a sequence number
    /// 
    /// Returns (events, has_more, highest_seq)
    async fn get_events_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedEvent>, bool, u64), Self::Error>;
    
    /// Store events received from peers
    /// 
    /// Returns (accepted_count, rejected_ids)
    async fn store_events(
        &self,
        events: Vec<SerializedEvent>,
    ) -> Result<(u32, Vec<String>), Self::Error>;
    
    /// Get events by their IDs
    async fn get_events_by_ids(
        &self,
        event_ids: &[String],
    ) -> Result<Vec<SerializedEvent>, Self::Error>;
    
    /// Get the highest event sequence number
    async fn highest_event_seq(&self) -> u64;

    // ===== ConsensusFrame Storage =====
    
    /// Get consensus frames starting from a sequence number
    /// 
    /// Returns (frames, has_more, highest_seq)
    async fn get_cfs_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedConsensusFrame>, bool, u64), Self::Error>;
    
    /// Store a consensus frame with its votes
    /// 
    /// Returns whether the CF was accepted
    async fn store_cf(
        &self,
        frame: SerializedConsensusFrame,
        votes: Vec<SerializedVote>,
    ) -> Result<(bool, Option<String>), Self::Error>;
    
    /// Get the highest finalized CF sequence number
    async fn highest_finalized_cf_seq(&self) -> u64;
}

// Blanket implementation for Arc<S> to allow sharing store across components
#[async_trait]
impl<S: StateSyncStore> StateSyncStore for Arc<S> {
    type Error = S::Error;

    async fn get_events_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedEvent>, bool, u64), Self::Error> {
        (**self).get_events_from_seq(start_seq, limit).await
    }

    async fn store_events(
        &self,
        events: Vec<SerializedEvent>,
    ) -> Result<(u32, Vec<String>), Self::Error> {
        (**self).store_events(events).await
    }

    async fn get_events_by_ids(
        &self,
        event_ids: &[String],
    ) -> Result<Vec<SerializedEvent>, Self::Error> {
        (**self).get_events_by_ids(event_ids).await
    }

    async fn highest_event_seq(&self) -> u64 {
        (**self).highest_event_seq().await
    }

    async fn get_cfs_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedConsensusFrame>, bool, u64), Self::Error> {
        (**self).get_cfs_from_seq(start_seq, limit).await
    }

    async fn store_cf(
        &self,
        frame: SerializedConsensusFrame,
        votes: Vec<SerializedVote>,
    ) -> Result<(bool, Option<String>), Self::Error> {
        (**self).store_cf(frame, votes).await
    }

    async fn highest_finalized_cf_seq(&self) -> u64 {
        (**self).highest_finalized_cf_seq().await
    }
}

/// Peer synchronization state tracking
///
/// Similar to Sui's PeerHeights but tracks Event/CF heights instead of checkpoints
#[derive(Debug, Default)]
pub struct PeerSyncState {
    /// Map from peer ID to their sync info
    peers: DashMap<PeerId, PeerSyncInfo>,
}

impl PeerSyncState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update peer's sync info
    pub fn update_peer(&self, peer_id: PeerId, info: PeerSyncInfo) {
        self.peers.insert(peer_id, info);
    }

    /// Remove a peer
    pub fn remove_peer(&self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
    }

    /// Get peer's sync info
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<PeerSyncInfo> {
        self.peers.get(peer_id).map(|r| r.clone())
    }

    /// Get all peers with their sync info
    pub fn all_peers(&self) -> Vec<(PeerId, PeerSyncInfo)> {
        self.peers.iter().map(|r| (*r.key(), r.value().clone())).collect()
    }

    /// Find the peer with the highest CF height
    pub fn highest_cf_peer(&self) -> Option<(PeerId, u64)> {
        self.peers
            .iter()
            .max_by_key(|r| r.highest_synced_cf)
            .map(|r| (*r.key(), r.highest_synced_cf))
    }
}

/// Sync information for a single peer
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PeerSyncInfo {
    /// Highest event sequence number this peer has
    pub highest_event_seq: u64,
    /// Highest CF (by sequence/epoch) this peer has finalized
    pub highest_synced_cf: u64,
    /// Highest CF this peer has verified but not finalized
    pub highest_verified_cf: u64,
    /// VLC snapshot for this peer (serialized VLC)
    pub vlc_snapshot: Option<Vec<u8>>,
    /// Timestamp of last update (ms since epoch)
    pub last_update_ms: u64,
}

/// Configuration for state synchronization
#[derive(Clone, Debug)]
pub struct StateSyncConfig {
    /// Interval between sync ticks (ms)
    pub tick_interval_ms: u64,
    /// Maximum events to request per batch
    pub max_events_per_request: u32,
    /// Maximum CFs to request per batch
    pub max_cfs_per_request: u32,
    /// Timeout for sync requests (ms)
    pub sync_timeout_ms: u64,
    /// Number of concurrent sync streams
    pub max_concurrent_syncs: usize,
    /// Rate limit for push_events RPC (per second)
    pub push_events_rate_limit: Option<u32>,
    /// Rate limit for get_events RPC (per second)
    pub get_events_rate_limit: Option<u32>,
    /// Inflight limit for get_consensus_frames
    pub get_cf_inflight_limit: Option<usize>,
}

impl Default for StateSyncConfig {
    fn default() -> Self {
        Self {
            tick_interval_ms: 5_000, // 5 seconds, matches MVP CF timeout
            max_events_per_request: 100, // Match MVP max 1000 events per CF
            max_cfs_per_request: 10,
            sync_timeout_ms: 30_000,
            max_concurrent_syncs: 3,
            push_events_rate_limit: Some(100),
            get_events_rate_limit: Some(50),
            get_cf_inflight_limit: Some(10),
        }
    }
}

/// Local state watermarks
#[derive(Clone, Debug, Default)]
pub struct LocalState {
    /// Highest event sequence we have
    pub highest_event_seq: u64,
    /// Highest CF we have finalized
    pub highest_finalized_cf: u64,
    /// Highest CF we have verified
    pub highest_verified_cf: u64,
    /// Current VLC state (serialized)
    pub current_vlc: Option<Vec<u8>>,
}

/// State for the sync system
pub struct SyncState {
    /// Local watermarks
    pub local: RwLock<LocalState>,
    /// Peer sync states
    pub peer_heights: Arc<PeerSyncState>,
    /// Pending sync requests
    pub pending_requests: DashMap<u64, SyncRequestState>,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            local: RwLock::new(LocalState::default()),
            peer_heights: Arc::new(PeerSyncState::new()),
            pending_requests: DashMap::new(),
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of a pending sync request
#[derive(Clone, Debug)]
pub struct SyncRequestState {
    pub request_id: u64,
    pub peer_id: PeerId,
    pub request_type: SyncRequestType,
    pub started_at: std::time::Instant,
}

#[derive(Clone, Debug)]
pub enum SyncRequestType {
    Events { from_seq: u64, to_seq: u64 },
    ConsensusFrames { from_cf: u64, to_cf: u64 },
}

/// Handle for controlling the state sync service
#[derive(Clone)]
pub struct Handle {
    sender: tokio::sync::mpsc::Sender<StateSyncMessage>,
}

impl Handle {
    /// Notify that a new event has been received
    pub fn notify_new_event(&self, event_seq: u64) {
        let _ = self.sender.try_send(StateSyncMessage::NewEvent(event_seq));
    }

    /// Notify that a new CF has been finalized
    pub fn notify_cf_finalized(&self, cf_seq: u64) {
        let _ = self.sender.try_send(StateSyncMessage::CFFinalized(cf_seq));
    }

    /// Request sync with a specific peer
    pub fn sync_with_peer(&self, peer_id: PeerId) {
        let _ = self.sender.try_send(StateSyncMessage::SyncWithPeer(peer_id));
    }
}

/// Internal messages for the state sync event loop
pub enum StateSyncMessage {
    NewEvent(u64),
    CFFinalized(u64),
    SyncWithPeer(PeerId),
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
}

/// Event loop for state synchronization
pub struct StateSyncEventLoop<S> {
    config: StateSyncConfig,
    state: Arc<SyncState>,
    #[allow(dead_code)]
    store: S,
    network: anemo::NetworkRef,
    message_rx: tokio::sync::mpsc::Receiver<StateSyncMessage>,
}

impl<S> StateSyncEventLoop<S>
where
    S: StateSyncStore,
{
    pub(crate) fn new(
        config: StateSyncConfig,
        state: Arc<SyncState>,
        store: S,
        network: anemo::NetworkRef,
        message_rx: tokio::sync::mpsc::Receiver<StateSyncMessage>,
    ) -> Self {
        Self {
            config,
            state,
            store,
            network,
            message_rx,
        }
    }

    /// Run the state sync event loop
    pub async fn run(mut self) {
        let tick_interval = Duration::from_millis(self.config.tick_interval_ms);
        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!("State sync event loop started with {:?} tick interval", tick_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.handle_tick().await;
                }
                Some(msg) = self.message_rx.recv() => {
                    self.handle_message(msg).await;
                }
                else => {
                    tracing::info!("State sync event loop shutting down");
                    break;
                }
            }
        }
    }

    async fn handle_tick(&self) {
        let Some(network) = self.network.upgrade() else {
            tracing::warn!("Network dropped, state sync stopping");
            return;
        };

        let connected_peers: Vec<PeerId> = network.peers().into_iter().collect();
        
        // Update peer list
        for peer_id in &connected_peers {
            if self.state.peer_heights.get_peer(peer_id).is_none() {
                self.state.peer_heights.update_peer(*peer_id, PeerSyncInfo::default());
            }
        }

        // Remove disconnected peers
        for (peer_id, _) in self.state.peer_heights.all_peers() {
            if !connected_peers.contains(&peer_id) {
                self.state.peer_heights.remove_peer(&peer_id);
            }
        }

        // Check if we need to sync
        self.check_and_sync(&connected_peers).await;
    }

    async fn check_and_sync(&self, peers: &[PeerId]) {
        let local = self.state.local.read().await;
        
        // Find peer with highest CF
        if let Some((best_peer, highest_cf)) = self.state.peer_heights.highest_cf_peer() {
            if highest_cf > local.highest_finalized_cf && peers.contains(&best_peer) {
                tracing::debug!(
                    "Need to sync: local CF {} < peer {} CF {}",
                    local.highest_finalized_cf,
                    best_peer,
                    highest_cf
                );
                // TODO: Initiate sync with best_peer
            }
        }
    }

    async fn handle_message(&self, msg: StateSyncMessage) {
        match msg {
            StateSyncMessage::NewEvent(seq) => {
                let mut local = self.state.local.write().await;
                local.highest_event_seq = local.highest_event_seq.max(seq);
            }
            StateSyncMessage::CFFinalized(seq) => {
                let mut local = self.state.local.write().await;
                local.highest_finalized_cf = local.highest_finalized_cf.max(seq);
            }
            StateSyncMessage::SyncWithPeer(peer_id) => {
                tracing::debug!("Requested sync with peer {:?}", peer_id);
                // TODO: Initiate sync
            }
            StateSyncMessage::PeerConnected(peer_id) => {
                self.state.peer_heights.update_peer(peer_id, PeerSyncInfo::default());
            }
            StateSyncMessage::PeerDisconnected(peer_id) => {
                self.state.peer_heights.remove_peer(&peer_id);
            }
        }
    }
}
