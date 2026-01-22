// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! # Setu Network - Anemo Implementation
//!
//! This module provides a production-grade P2P networking layer for Setu based on
//! Mysten Labs' Anemo framework. The architecture follows Sui's network patterns
//! while adapting to Setu's Event/ConsensusFrame/VLC-based consensus model.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────┐
//! │                    Setu Application Layer                       │
//! │  (Consensus, Event Processing, VLC Management)                  │
//! └─────────────────────────────┬──────────────────────────────────┘
//!                               │
//! ┌─────────────────────────────┴──────────────────────────────────┐
//! │                    Network Service                              │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌─────────────────┐  ┌──────────────────────┐ │
//! │  │  Discovery  │  │   State Sync    │  │     Validator/       │ │
//! │  │             │  │                 │  │     Solver RPC       │ │
//! │  │ - Peer info │  │ - Event sync    │  │                      │ │
//! │  │ - Topology  │  │ - CF sync       │  │ - Submit events      │ │
//! │  │ - Health    │  │ - VLC tracking  │  │ - Propose CFs        │ │
//! │  └─────────────┘  └─────────────────┘  └──────────────────────┘ │
//! └─────────────────────────────┬──────────────────────────────────┘
//!                               │
//! ┌─────────────────────────────┴──────────────────────────────────┐
//! │                    Transport (Anemo)                            │
//! │  - QUIC connections with mTLS                                   │
//! │  - Ed25519 peer authentication                                  │
//! │  - Automatic reconnection                                       │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Network Topology (MVP)
//!
//! Based on Setu's MVP architecture:
//! - **3 Validators**: Full mesh connectivity for BFT consensus
//! - **10 Solvers**: Connect to all validators for event submission
//!
//! ## Module Organization
//!
//! Following Sui's network structure:
//!
//! - [`discovery`] - Peer discovery and connection management
//! - [`state_sync`] - Event and ConsensusFrame synchronization
//! - [`metrics`] - Prometheus metrics for monitoring
//! - [`transport`] - QUIC transport layer
//! - [`config`] - Network configuration
//! - [`error`] - Error types
//!
//! ## Features
//!
//! - **QUIC-based transport**: Low latency, multiplexed connections
//! - **mTLS authentication**: Ed25519-based peer authentication
//! - **Automatic reconnection**: Built-in connection management
//! - **RPC support**: Request-response pattern for synchronous operations
//! - **Builder pattern**: Sui-style service construction
//! - **Prometheus metrics**: Full observability

pub mod config;
pub mod discovery;
pub mod error;
pub mod generic_handler;
pub mod metrics;
pub mod node_info;
pub mod peer_manager;
pub mod service;
pub mod state_sync;
pub mod transport;

// Re-export generic handler types
pub use generic_handler::{
    GenericMessageHandler, HandlerError, HandleResult,
    create_router_from_handler,
};

// Re-export node info types
pub use node_info::{NodeInfo as NetworkNodeInfo, NodeRole, NodeStatus};

// Re-export main types
pub use config::{AnemoConfig, NetworkConfig};
pub use error::{AnemoError, Result};
pub use metrics::NetworkMetrics;
pub use peer_manager::AnemoPeerManager;
pub use service::AnemoNetworkService;
pub use transport::AnemoTransport;

// Re-export discovery types
pub use discovery::{
    Builder as DiscoveryBuilder,
    DiscoveryConfig,
    DiscoveryEventLoop,
    DiscoveryServer,
    Handle as DiscoveryHandle,
    NodeInfo,
    NodeType,
    SignedNodeInfo,
    State as DiscoveryState,
    UnstartedDiscovery,
};

// Re-export state sync types
pub use state_sync::{
    Builder as StateSyncBuilder,
    Handle as StateSyncHandle,
    LocalState,
    PeerSyncInfo,
    PeerSyncState,
    StateSyncConfig,
    StateSyncEventLoop,
    StateSyncServer,
    SyncState,
    UnstartedStateSync,
};

// Re-export core protocol types from setu-protocol
pub use setu_protocol::{GenericCodec, BincodeCodec, ProtocolError, ProtocolVersion};

// Re-export commonly used Anemo types
pub use anemo::{Network, PeerId, Request, Response};

/// Protocol version for Setu over Anemo
pub const PROTOCOL_VERSION: u16 = 1;

/// Default server name for Anemo network
pub const DEFAULT_SERVER_NAME: &str = "setu";

/// Maximum number of validators in MVP
pub const MVP_MAX_VALIDATORS: usize = 3;

/// Maximum number of solvers in MVP
pub const MVP_MAX_SOLVERS: usize = 10;

/// VLC delta threshold for CF generation (from MVP spec)
pub const MVP_VLC_DELTA_THRESHOLD: u64 = 10;

/// CF timeout in milliseconds (from MVP spec)
pub const MVP_CF_TIMEOUT_MS: u64 = 5000;

/// Maximum events per CF (from MVP spec)
pub const MVP_MAX_EVENTS_PER_CF: usize = 1000;
