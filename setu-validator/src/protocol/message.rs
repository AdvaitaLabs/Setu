// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Network message definitions for Setu consensus
//!
//! This module defines the core `SetuMessage` enum that represents all
//! possible messages exchanged between Setu nodes using the VLC + ConsensusFrame
//! consensus model.
//!
//! ## Design Note
//!
//! These message types are specific to the current consensus implementation.
//! If switching to a different consensus (e.g., Mysticeti), a new set of
//! message types would be defined in a separate validator crate, while the
//! network layer remains unchanged.

use serde::{Deserialize, Serialize};
use setu_types::{ConsensusFrame, Event, Vote};

/// Network messages for Setu protocol
///
/// This enum defines all possible message types that can be sent between nodes.
/// Each variant carries the data needed for that specific operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SetuMessage {
    /// Event broadcast - propagate a new event to peers
    EventBroadcast { event: Event, sender_id: String },

    /// Consensus frame proposal - propose a new CF for voting
    CFProposal {
        cf: ConsensusFrame,
        proposer_id: String,
    },

    /// Vote for a consensus frame
    CFVote { vote: Vote },

    /// Notification that a consensus frame has been finalized
    CFFinalized {
        cf: ConsensusFrame,
        /// ID of the node that sent this notification
        sender_id: String,
    },

    /// Request specific events by their IDs
    RequestEvents {
        event_ids: Vec<String>,
        requester_id: String,
    },

    /// Response containing requested events
    EventsResponse {
        events: Vec<Event>,
        responder_id: String,
    },

    /// Ping message for health check and latency measurement
    Ping { timestamp: u64, nonce: u64 },

    /// Pong response to a ping
    Pong { timestamp: u64, nonce: u64 },
}

/// Message type identifier
///
/// Used for routing and filtering messages without deserializing the full payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    EventBroadcast,
    CFProposal,
    CFVote,
    CFFinalized,
    RequestEvents,
    EventsResponse,
    Ping,
    Pong,
}

impl SetuMessage {
    /// Get the message type without accessing the payload
    pub fn message_type(&self) -> MessageType {
        match self {
            SetuMessage::EventBroadcast { .. } => MessageType::EventBroadcast,
            SetuMessage::CFProposal { .. } => MessageType::CFProposal,
            SetuMessage::CFVote { .. } => MessageType::CFVote,
            SetuMessage::CFFinalized { .. } => MessageType::CFFinalized,
            SetuMessage::RequestEvents { .. } => MessageType::RequestEvents,
            SetuMessage::EventsResponse { .. } => MessageType::EventsResponse,
            SetuMessage::Ping { .. } => MessageType::Ping,
            SetuMessage::Pong { .. } => MessageType::Pong,
        }
    }

    /// Check if this message type expects a response
    pub fn expects_response(&self) -> bool {
        matches!(
            self,
            SetuMessage::RequestEvents { .. } | SetuMessage::Ping { .. }
        )
    }

    /// Check if this is a consensus-related message
    pub fn is_consensus_message(&self) -> bool {
        matches!(
            self,
            SetuMessage::CFProposal { .. }
                | SetuMessage::CFVote { .. }
                | SetuMessage::CFFinalized { .. }
        )
    }

    /// Get the route path for this message type
    ///
    /// Used by the network layer to route messages to appropriate handlers.
    pub fn route_path(&self) -> &'static str {
        match self {
            SetuMessage::EventBroadcast { .. } => "/setu/event",
            SetuMessage::CFProposal { .. } => "/setu/cf",
            SetuMessage::CFVote { .. } => "/setu/vote",
            SetuMessage::CFFinalized { .. } => "/setu/cf_finalized",
            SetuMessage::RequestEvents { .. } => "/setu/request_events",
            SetuMessage::EventsResponse { .. } => "/setu/events_response",
            SetuMessage::Ping { .. } => "/ping",
            SetuMessage::Pong { .. } => "/pong",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::VLCSnapshot;

    #[test]
    fn test_message_type() {
        let msg = SetuMessage::Ping {
            timestamp: 123,
            nonce: 456,
        };
        assert_eq!(msg.message_type(), MessageType::Ping);
        assert!(msg.expects_response());
        assert!(!msg.is_consensus_message());
    }

    #[test]
    fn test_consensus_message_check() {
        let vote = Vote::new("v1".to_string(), "cf1".to_string(), true);
        let msg = SetuMessage::CFVote { vote };
        assert!(msg.is_consensus_message());
    }

    #[test]
    fn test_route_paths() {
        let msg = SetuMessage::Ping {
            timestamp: 0,
            nonce: 0,
        };
        assert_eq!(msg.route_path(), "/ping");

        let event = Event::genesis("test".to_string(), VLCSnapshot::default());
        let msg = SetuMessage::EventBroadcast {
            event,
            sender_id: "s1".to_string(),
        };
        assert_eq!(msg.route_path(), "/setu/event");
    }
}
