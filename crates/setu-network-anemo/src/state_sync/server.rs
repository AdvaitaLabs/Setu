// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! RPC server implementation for state synchronization

use super::{PeerSyncInfo, StateSyncConfig, SyncState};
use anemo::{Request, Response, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Re-export from setu-protocol
pub use setu_protocol::{SerializedEvent, SerializedConsensusFrame, SerializedVote};

/// State Sync RPC trait
///
/// Defines the RPC methods for state synchronization between nodes.
#[anemo::async_trait]
pub trait StateSync: Send + Sync + 'static {
    /// Get events starting from a sequence number
    async fn get_events(
        &self,
        request: Request<GetEventsRequest>,
    ) -> Result<Response<GetEventsResponse>>;

    /// Get consensus frames starting from a sequence number
    async fn get_consensus_frames(
        &self,
        request: Request<GetConsensusFramesRequest>,
    ) -> Result<Response<GetConsensusFramesResponse>>;

    /// Push events to this node
    async fn push_events(
        &self,
        request: Request<PushEventsRequest>,
    ) -> Result<Response<PushEventsResponse>>;

    /// Push a consensus frame to this node
    async fn push_consensus_frame(
        &self,
        request: Request<PushConsensusFrameRequest>,
    ) -> Result<Response<PushConsensusFrameResponse>>;

    /// Get this node's sync state
    async fn get_sync_state(
        &self,
        request: Request<GetSyncStateRequest>,
    ) -> Result<Response<GetSyncStateResponse>>;
}

// ============================================================================
// Request/Response types for Events
// ============================================================================

/// Request for getting events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetEventsRequest {
    /// Start from this sequence number (exclusive)
    pub start_seq: u64,
    /// Maximum number of events to return
    pub limit: u32,
}

/// Response with events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetEventsResponse {
    /// Serialized events
    pub events: Vec<SerializedEvent>,
    /// Whether there are more events after this batch
    pub has_more: bool,
    /// The highest sequence number included
    pub highest_seq: u64,
}

/// Request for pushing events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushEventsRequest {
    /// Events to push
    pub events: Vec<SerializedEvent>,
}

/// Response for pushed events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushEventsResponse {
    /// Number of events accepted
    pub accepted: u32,
    /// Events that were rejected (by ID)
    pub rejected: Vec<String>,
}

// ============================================================================
// Request/Response types for Consensus Frames
// ============================================================================

/// Request for getting consensus frames
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetConsensusFramesRequest {
    /// Start from this CF sequence (exclusive)
    pub start_seq: u64,
    /// Maximum number of CFs to return
    pub limit: u32,
}

/// Response with consensus frames
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetConsensusFramesResponse {
    /// Serialized consensus frames
    pub frames: Vec<SerializedConsensusFrame>,
    /// Whether there are more CFs after this batch
    pub has_more: bool,
    /// The highest CF sequence included
    pub highest_seq: u64,
}

/// Request for pushing a consensus frame
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushConsensusFrameRequest {
    /// The consensus frame
    pub frame: SerializedConsensusFrame,
    /// Votes for this frame
    pub votes: Vec<SerializedVote>,
}

/// Response for pushed consensus frame
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushConsensusFrameResponse {
    /// Whether the CF was accepted
    pub accepted: bool,
    /// Reason if rejected
    pub reason: Option<String>,
}

// ============================================================================
// Sync State Query
// ============================================================================

/// Request for getting sync state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSyncStateRequest {}

/// Response with sync state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSyncStateResponse {
    /// This node's sync info
    pub sync_info: PeerSyncInfo,
}

// ============================================================================
// Server Implementation
// ============================================================================

/// State sync server implementation
pub struct Server<S> {
    pub(crate) state: Arc<SyncState>,
    pub(crate) store: S,
    pub(crate) config: StateSyncConfig,
}

impl<S> Server<S> {
    /// Create a new server instance
    pub fn new(state: Arc<SyncState>, store: S, config: StateSyncConfig) -> Self {
        Self { state, store, config }
    }
}

use super::StateSyncStore;

#[anemo::async_trait]
impl<S> StateSync for Server<S>
where
    S: StateSyncStore,
{
    async fn get_events(
        &self,
        request: Request<GetEventsRequest>,
    ) -> Result<Response<GetEventsResponse>> {
        let req = request.into_body();
        let limit = req.limit.min(self.config.max_events_per_request);
        
        match self.store.get_events_from_seq(req.start_seq, limit).await {
            Ok((events, has_more, highest_seq)) => {
                tracing::debug!(
                    "get_events: start_seq={}, limit={}, returned={} events",
                    req.start_seq,
                    limit,
                    events.len()
                );
                Ok(Response::new(GetEventsResponse {
                    events,
                    has_more,
                    highest_seq,
                }))
            }
            Err(e) => {
                tracing::error!("get_events error: {}", e);
                Ok(Response::new(GetEventsResponse {
                    events: Vec::new(),
                    has_more: false,
                    highest_seq: req.start_seq,
                }))
            }
        }
    }

    async fn get_consensus_frames(
        &self,
        request: Request<GetConsensusFramesRequest>,
    ) -> Result<Response<GetConsensusFramesResponse>> {
        let req = request.into_body();
        let limit = req.limit.min(self.config.max_cfs_per_request);
        
        match self.store.get_cfs_from_seq(req.start_seq, limit).await {
            Ok((frames, has_more, highest_seq)) => {
                tracing::debug!(
                    "get_consensus_frames: start_seq={}, limit={}, returned={} frames",
                    req.start_seq,
                    limit,
                    frames.len()
                );
                Ok(Response::new(GetConsensusFramesResponse {
                    frames,
                    has_more,
                    highest_seq,
                }))
            }
            Err(e) => {
                tracing::error!("get_consensus_frames error: {}", e);
                Ok(Response::new(GetConsensusFramesResponse {
                    frames: Vec::new(),
                    has_more: false,
                    highest_seq: req.start_seq,
                }))
            }
        }
    }

    async fn push_events(
        &self,
        request: Request<PushEventsRequest>,
    ) -> Result<Response<PushEventsResponse>> {
        let req = request.into_body();
        let event_count = req.events.len();
        
        match self.store.store_events(req.events).await {
            Ok((accepted, rejected)) => {
                tracing::debug!(
                    "push_events: received={}, accepted={}, rejected={}",
                    event_count,
                    accepted,
                    rejected.len()
                );
                
                // Update local state if we accepted any events
                if accepted > 0 {
                    let new_seq = self.store.highest_event_seq().await;
                    let mut local = self.state.local.write().await;
                    local.highest_event_seq = local.highest_event_seq.max(new_seq);
                }
                
                Ok(Response::new(PushEventsResponse { accepted, rejected }))
            }
            Err(e) => {
                tracing::error!("push_events error: {}", e);
                Ok(Response::new(PushEventsResponse {
                    accepted: 0,
                    rejected: Vec::new(),
                }))
            }
        }
    }

    async fn push_consensus_frame(
        &self,
        request: Request<PushConsensusFrameRequest>,
    ) -> Result<Response<PushConsensusFrameResponse>> {
        let req = request.into_body();
        let cf_id = req.frame.id.clone();
        
        match self.store.store_cf(req.frame, req.votes).await {
            Ok((accepted, reason)) => {
                tracing::debug!(
                    "push_consensus_frame: cf_id={}, accepted={}",
                    cf_id,
                    accepted
                );
                
                // Update local state if accepted
                if accepted {
                    let new_seq = self.store.highest_finalized_cf_seq().await;
                    let mut local = self.state.local.write().await;
                    local.highest_finalized_cf = local.highest_finalized_cf.max(new_seq);
                }
                
                Ok(Response::new(PushConsensusFrameResponse { accepted, reason }))
            }
            Err(e) => {
                tracing::error!("push_consensus_frame error: {}", e);
                Ok(Response::new(PushConsensusFrameResponse {
                    accepted: false,
                    reason: Some(e.to_string()),
                }))
            }
        }
    }

    async fn get_sync_state(
        &self,
        _request: Request<GetSyncStateRequest>,
    ) -> Result<Response<GetSyncStateResponse>> {
        let local = self.state.local.read().await;
        
        let sync_info = PeerSyncInfo {
            highest_event_seq: local.highest_event_seq,
            highest_synced_cf: local.highest_finalized_cf,
            highest_verified_cf: local.highest_verified_cf,
            vlc_snapshot: local.current_vlc.clone(),
            last_update_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        
        Ok(Response::new(GetSyncStateResponse { sync_info }))
    }
}

/// Wrapper for the state sync server
pub struct StateSyncServer<T> {
    inner: T,
}

impl<T: StateSync> StateSyncServer<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}
