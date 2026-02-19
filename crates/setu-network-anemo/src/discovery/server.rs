// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! RPC server implementation for discovery

use super::{DiscoveryConfig, SignedNodeInfo, State};
use anemo::{Request, Response, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Discovery RPC trait
///
/// This trait defines the RPC methods for the discovery service.
/// It can be used with anemo-build to generate the server implementation.
#[anemo::async_trait]
pub trait Discovery: Send + Sync + 'static {
    /// Get known peers from this node
    async fn get_known_peers(
        &self,
        request: Request<GetKnownPeersRequest>,
    ) -> Result<Response<GetKnownPeersResponse>>;

    /// Push peer info to this node
    async fn push_peer_info(
        &self,
        request: Request<PushPeerInfoRequest>,
    ) -> Result<Response<PushPeerInfoResponse>>;
}

/// Request for getting known peers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetKnownPeersRequest {
    /// Maximum number of peers to return
    pub limit: Option<usize>,
    /// Filter by node type (optional)
    pub node_type_filter: Option<String>,
}

/// Response with known peers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetKnownPeersResponse {
    /// Our own node info
    pub own_info: Option<SignedNodeInfo>,
    /// List of known peers
    pub known_peers: Vec<SignedNodeInfo>,
}

/// Request for pushing peer info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushPeerInfoRequest {
    /// The node info to share
    pub info: SignedNodeInfo,
}

/// Response for push peer info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushPeerInfoResponse {
    /// Whether the info was accepted
    pub accepted: bool,
    /// Reason if not accepted
    pub reason: Option<String>,
}

/// Discovery server implementation
pub struct Server {
    pub(crate) state: Arc<RwLock<State>>,
    pub(crate) config: DiscoveryConfig,
}

#[anemo::async_trait]
impl Discovery for Server {
    async fn get_known_peers(
        &self,
        request: Request<GetKnownPeersRequest>,
    ) -> Result<Response<GetKnownPeersResponse>> {
        let req = request.into_body();
        let limit = req.limit.unwrap_or(self.config.max_peers_to_return);

        let state = self.state.read().unwrap();

        let own_info = state.our_info.clone();

        let known_peers: Vec<SignedNodeInfo> =
            state.known_peers.values().take(limit).cloned().collect();

        Ok(Response::new(GetKnownPeersResponse {
            own_info,
            known_peers,
        }))
    }

    async fn push_peer_info(
        &self,
        request: Request<PushPeerInfoRequest>,
    ) -> Result<Response<PushPeerInfoResponse>> {
        let req = request.into_body();
        let info = req.info;

        // Validate timestamp is not too old or in the future
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let skew = if info.info.timestamp_ms > now_ms {
            info.info.timestamp_ms - now_ms
        } else {
            now_ms - info.info.timestamp_ms
        };

        if skew > self.config.max_clock_skew_ms {
            return Ok(Response::new(PushPeerInfoResponse {
                accepted: false,
                reason: Some("Timestamp too far from current time".to_string()),
            }));
        }

        // TODO: Verify signature with peer's public key

        // Store the peer info
        {
            let mut state = self.state.write().unwrap();

            // Check if we already have newer info
            if let Some(existing) = state.known_peers.get(&info.info.peer_id) {
                if existing.info.timestamp_ms >= info.info.timestamp_ms {
                    return Ok(Response::new(PushPeerInfoResponse {
                        accepted: false,
                        reason: Some("Have newer info".to_string()),
                    }));
                }
            }

            state.known_peers.insert(info.info.peer_id, info);
        }

        Ok(Response::new(PushPeerInfoResponse {
            accepted: true,
            reason: None,
        }))
    }
}

/// Wrapper to make Server work with anemo's routing
pub struct DiscoveryServer<T> {
    inner: T,
}

impl<T: Discovery> DiscoveryServer<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}
