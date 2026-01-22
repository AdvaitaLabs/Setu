//! Network Adapter Module
//!
//! This module provides the bridge between the transport layer (setu-network-anemo)
//! and the application layer (ConsensusEngine). It handles:
//!
//! - Message routing: Directing incoming network messages to appropriate handlers
//! - Event synchronization: Protocol for syncing events with peers
//! - Consensus broadcasting: Sending CF proposals and votes
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      ConsensusValidator                          │
//! │  ┌─────────────────────────────────────────────────────────┐    │
//! │  │              ConsensusNetworkAdapter                     │    │
//! │  │  - Implements message routing                            │    │
//! │  │  - Handles NetworkEvent dispatch                         │    │
//! │  │  - Provides sync protocol implementation                 │    │
//! │  └─────────────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!                               ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    setu-network-anemo                            │
//! │  - Transport layer (QUIC)                                       │
//! │  - Connection management                                        │
//! │  - Basic message handler                                        │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use setu_validator::{ConsensusValidator, ConsensusValidatorConfig};
//! use crate::protocol::NetworkEvent;
//! use tokio::sync::mpsc;
//!
//! // Create validator
//! let config = ConsensusValidatorConfig::default();
//! let validator = ConsensusValidator::new(config);
//!
//! // Create network event channel
//! let (event_tx, event_rx) = mpsc::channel::<NetworkEvent>(1000);
//!
//! // Start network event handler (connects network to consensus)
//! let handle = validator.start_network_event_handler(event_rx);
//! ```

mod router;
mod setu_handler;
mod sync_protocol;

pub use router::{MessageRouter, NetworkEventHandler};
pub use setu_handler::{SetuMessageHandler, MessageHandlerStore, SETU_ROUTE};
pub use sync_protocol::{SyncProtocol, SyncStore, InMemorySyncStore};

