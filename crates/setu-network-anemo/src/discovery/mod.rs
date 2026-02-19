// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Peer discovery and connection management for Anemo network
//!
//! This module is modeled after Sui's discovery system, providing:
//! - Peer discovery mechanisms with signed node information
//! - Connection establishment and management
//! - Event-loop based peer maintenance
//! - Support for validator and solver node topologies
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │        DiscoveryEventLoop               │
//! │  ┌───────────────────────────────────┐  │
//! │  │  Tick-based peer management       │  │
//! │  │  - Check peer health              │  │
//! │  │  - Reconnect to seeds             │  │
//! │  │  - Exchange peer info             │  │
//! │  └───────────────────────────────────┘  │
//! └─────────────────────────────────────────┘
//!                    │
//! ┌──────────────────┴──────────────────────┐
//! │             Server (RPC)                 │
//! │  - get_known_peers                       │
//! │  - push_peer_info                        │
//! └─────────────────────────────────────────┘
//! ```

mod builder;
pub mod metrics;
mod server;

pub use builder::{Builder, UnstartedDiscovery};
pub use server::{Discovery, DiscoveryServer, Server};

use anemo::PeerId;
use ed25519_consensus::{Signature, SigningKey, VerificationKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Information about a node in the network
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub node_id: String,
    /// Anemo peer ID derived from public key
    pub peer_id: PeerId,
    /// Network addresses (host:port)
    pub addresses: Vec<String>,
    /// Timestamp when this info was created (ms since epoch)
    pub timestamp_ms: u64,
    /// Node role (validator or solver)
    pub node_type: NodeType,
    /// Optional metadata
    pub metadata: HashMap<String, String>,
}

impl NodeInfo {
    /// Create new node info with current timestamp
    pub fn new(
        node_id: String,
        peer_id: PeerId,
        addresses: Vec<String>,
        node_type: NodeType,
    ) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            node_id,
            peer_id,
            addresses,
            timestamp_ms,
            node_type,
            metadata: HashMap::new(),
        }
    }

    /// Sign this node info with the given key
    pub fn sign(self, key: &SigningKey) -> SignedNodeInfo {
        let bytes = bcs::to_bytes(&self).expect("BCS serialization should not fail");
        let signature = key.sign(&bytes);
        SignedNodeInfo {
            info: self,
            signature,
        }
    }
}

/// Node type in the Setu network
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    /// Validator node (3 in MVP, full mesh connectivity)
    Validator,
    /// Solver node (10 in MVP, connects to all validators)
    Solver,
    /// Full node (read-only, syncs state)
    FullNode,
}

/// Node info with cryptographic signature for verification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedNodeInfo {
    /// The node information
    pub info: NodeInfo,
    /// Ed25519 signature over the serialized NodeInfo
    pub signature: Signature,
}

impl SignedNodeInfo {
    /// Verify the signature using the peer's public key
    pub fn verify(&self, key: &VerificationKey) -> bool {
        let bytes = match bcs::to_bytes(&self.info) {
            Ok(b) => b,
            Err(_) => return false,
        };
        key.verify(&self.signature, &bytes).is_ok()
    }
}

/// Shared state for the discovery system
pub struct State {
    /// Our own signed node info
    pub our_info: Option<SignedNodeInfo>,
    /// Currently connected peers with their info
    pub connected_peers: HashMap<PeerId, SignedNodeInfo>,
    /// All known peers (including disconnected)
    pub known_peers: HashMap<PeerId, SignedNodeInfo>,
}

impl State {
    /// Get count of connected validators
    pub fn connected_validators(&self) -> usize {
        self.connected_peers
            .values()
            .filter(|info| info.info.node_type == NodeType::Validator)
            .count()
    }

    /// Get count of connected solvers
    pub fn connected_solvers(&self) -> usize {
        self.connected_peers
            .values()
            .filter(|info| info.info.node_type == NodeType::Solver)
            .count()
    }
}

/// Handle for controlling the discovery service
#[derive(Clone)]
pub struct Handle {
    pub(crate) _shutdown_handle: Arc<tokio::sync::oneshot::Sender<()>>,
}

/// Configuration for discovery
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// Interval between discovery ticks (ms)
    pub tick_interval_ms: u64,
    /// Maximum allowed clock skew for signed info (ms)
    pub max_clock_skew_ms: u64,
    /// Seed peers to connect to on startup
    pub seed_peers: Vec<String>,
    /// Whether to enable peer exchange
    pub enable_peer_exchange: bool,
    /// Maximum number of peers to return in get_known_peers
    pub max_peers_to_return: usize,
    /// Rate limit for get_known_peers RPC (requests per second)
    pub get_known_peers_rate_limit: Option<u32>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            tick_interval_ms: 5_000,   // 5 seconds
            max_clock_skew_ms: 60_000, // 1 minute
            seed_peers: Vec::new(),
            enable_peer_exchange: true,
            max_peers_to_return: 50,
            get_known_peers_rate_limit: Some(10),
        }
    }
}

/// Event loop that manages peer discovery
pub struct DiscoveryEventLoop {
    config: DiscoveryConfig,
    state: Arc<RwLock<State>>,
    network: anemo::NetworkRef,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
}

impl DiscoveryEventLoop {
    pub(crate) fn new(
        config: DiscoveryConfig,
        state: Arc<RwLock<State>>,
        network: anemo::NetworkRef,
        shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            config,
            state,
            network,
            shutdown_rx,
        }
    }

    /// Run the discovery event loop
    pub async fn run(mut self) {
        let tick_interval = Duration::from_millis(self.config.tick_interval_ms);
        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            "Discovery event loop started with {:?} tick interval",
            tick_interval
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.handle_tick().await;
                }
                _ = &mut self.shutdown_rx => {
                    tracing::info!("Discovery event loop shutting down");
                    break;
                }
            }
        }
    }

    async fn handle_tick(&self) {
        // Check network is still alive
        let Some(network) = self.network.upgrade() else {
            tracing::warn!("Network has been dropped, discovery stopping");
            return;
        };

        // Get current connected peers
        let connected_peers: Vec<PeerId> = network.peers().into_iter().collect();

        // Update connected peers in state
        {
            let mut state = self.state.write().unwrap();
            state
                .connected_peers
                .retain(|peer_id, _| connected_peers.contains(peer_id));
        }

        // If peer exchange is enabled, query peers for their known peers
        if self.config.enable_peer_exchange {
            self.exchange_peer_info(&network, &connected_peers).await;
        }

        // Log current state
        let state = self.state.read().unwrap();
        tracing::debug!(
            "Discovery tick: {} connected ({} validators, {} solvers), {} known",
            state.connected_peers.len(),
            state.connected_validators(),
            state.connected_solvers(),
            state.known_peers.len(),
        );
    }

    async fn exchange_peer_info(&self, _network: &anemo::Network, peers: &[PeerId]) {
        for peer_id in peers {
            // TODO: Make RPC call to get_known_peers
            tracing::trace!("Would exchange peer info with {:?}", peer_id);
        }
    }
}
