// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Network events for application layer notification
//!
//! This module defines events that the network layer sends to the application
//! layer (e.g., ConsensusEngine) when network activities occur.
//!
//! ## Design Note
//!
//! `NetworkEvent` is the bridge between the transport layer and the consensus
//! engine. The network adapter receives raw messages, deserializes them, and
//! converts them into `NetworkEvent` variants for the consensus layer to process.

use setu_types::{ConsensusFrame, Event, NodeInfo, Vote};

/// Network events that are sent to the application layer
///
/// These events notify the application about network activities such as
/// peer connections, received messages, and state changes.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// A new peer has connected
    PeerConnected {
        peer_id: String,
        node_info: NodeInfo,
    },

    /// A peer has disconnected
    PeerDisconnected { peer_id: String },

    /// Received an event broadcast from a peer
    EventReceived { peer_id: String, event: Event },

    /// Received a consensus frame proposal
    CFProposal { peer_id: String, cf: ConsensusFrame },

    /// Received a vote for a consensus frame
    VoteReceived { peer_id: String, vote: Vote },

    /// Received notification that a CF was finalized
    CFFinalized { peer_id: String, cf: ConsensusFrame },
}

impl NetworkEvent {
    /// Get the peer ID associated with this event
    pub fn peer_id(&self) -> &str {
        match self {
            NetworkEvent::PeerConnected { peer_id, .. }
            | NetworkEvent::PeerDisconnected { peer_id }
            | NetworkEvent::EventReceived { peer_id, .. }
            | NetworkEvent::CFProposal { peer_id, .. }
            | NetworkEvent::VoteReceived { peer_id, .. }
            | NetworkEvent::CFFinalized { peer_id, .. } => peer_id,
        }
    }

    /// Check if this is a connection-related event
    pub fn is_connection_event(&self) -> bool {
        matches!(
            self,
            NetworkEvent::PeerConnected { .. } | NetworkEvent::PeerDisconnected { .. }
        )
    }

    /// Check if this is a consensus-related event
    pub fn is_consensus_event(&self) -> bool {
        matches!(
            self,
            NetworkEvent::CFProposal { .. }
                | NetworkEvent::VoteReceived { .. }
                | NetworkEvent::CFFinalized { .. }
        )
    }

    /// Check if this is an event-related notification
    pub fn is_event_notification(&self) -> bool {
        matches!(self, NetworkEvent::EventReceived { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::VLCSnapshot;

    #[test]
    fn test_peer_id() {
        let event = NetworkEvent::PeerDisconnected {
            peer_id: "peer1".to_string(),
        };
        assert_eq!(event.peer_id(), "peer1");
    }

    #[test]
    fn test_connection_event() {
        let event = NetworkEvent::PeerConnected {
            peer_id: "peer1".to_string(),
            node_info: NodeInfo::new_validator("peer1".to_string(), "127.0.0.1".to_string(), 8080),
        };
        assert!(event.is_connection_event());
        assert!(!event.is_consensus_event());
    }

    #[test]
    fn test_consensus_event() {
        let vote = Vote::new("v1".to_string(), "cf1".to_string(), true);
        let event = NetworkEvent::VoteReceived {
            peer_id: "peer1".to_string(),
            vote,
        };
        assert!(event.is_consensus_event());
        assert!(!event.is_connection_event());
    }

    #[test]
    fn test_event_notification() {
        let e = Event::genesis("creator".to_string(), VLCSnapshot::default());
        let event = NetworkEvent::EventReceived {
            peer_id: "peer1".to_string(),
            event: e,
        };
        assert!(event.is_event_notification());
        assert!(!event.is_consensus_event());
    }
}
