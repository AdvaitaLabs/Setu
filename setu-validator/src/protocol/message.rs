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
    EventBroadcast {
        event: Event,
        sender_id: String,
    },

    /// Consensus frame proposal - propose a new CF for voting
    CFProposal {
        cf: ConsensusFrame,
        proposer_id: String,
    },

    /// Vote for a consensus frame
    CFVote {
        vote: Vote,
    },

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
    Ping {
        timestamp: u64,
        nonce: u64,
    },

    /// Pong response to a ping
    Pong {
        timestamp: u64,
        nonce: u64,
    },
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
        let vote = Vote::new(
            "v1".to_string(),
            "cf1".to_string(),
            true,
        );
        let msg = SetuMessage::CFVote { vote };
        assert!(msg.is_consensus_message());
    }
    
    #[test]
    fn test_route_paths() {
        let msg = SetuMessage::Ping { timestamp: 0, nonce: 0 };
        assert_eq!(msg.route_path(), "/ping");
        
        let event = Event::genesis("test".to_string(), VLCSnapshot::default());
        let msg = SetuMessage::EventBroadcast { 
            event, 
            sender_id: "s1".to_string() 
        };
        assert_eq!(msg.route_path(), "/setu/event");
    }

    /// Reproduce P2P event propagation bug: EventBroadcast deserialization fails
    /// with "tag for enum is not valid" while CFProposal/CFVote work fine.
    #[test]
    fn test_event_broadcast_bincode_roundtrip() {
        // Control: CFVote roundtrip (known working)
        let vote = Vote::new("v1".to_string(), "cf1".to_string(), true);
        let vote_msg = SetuMessage::CFVote { vote };
        let vote_bytes = bincode::serialize(&vote_msg).expect("CFVote serialize");
        let _: SetuMessage = bincode::deserialize(&vote_bytes).expect("CFVote deserialize");

        // Test: EventBroadcast with genesis event
        let event = Event::genesis("test-validator".to_string(), VLCSnapshot::default());
        let msg = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("EventBroadcast serialize");
        eprintln!("EventBroadcast (genesis) serialized size: {} bytes", bytes.len());
        let decoded: SetuMessage = bincode::deserialize(&bytes)
            .expect("EventBroadcast (genesis) deserialize failed!");

        match decoded {
            SetuMessage::EventBroadcast { event: decoded_event, sender_id } => {
                assert_eq!(sender_id, "validator-1");
                assert_eq!(decoded_event.id, event.id);
                assert_eq!(decoded_event.creator, "test-validator");
            }
            other => panic!("Expected EventBroadcast, got {:?}", other.message_type()),
        }
    }

    /// Test EventBroadcast with governance payload (the actual payload type from production)
    #[test]
    fn test_event_broadcast_governance_roundtrip() {
        use setu_types::governance::{GovernancePayload, GovernanceAction, ProposalContent, ProposalType, ProposalEffect};
        use setu_types::EventPayload;

        let content = ProposalContent {
            proposer: "alice".to_string(),
            proposal_type: ProposalType::ParameterChange,
            title: "Test Proposal".to_string(),
            description: "A test".to_string(),
            action: ProposalEffect::UpdateParameter {
                key: "min_transfer_amount".to_string(),
                value: 100u64.to_le_bytes().to_vec(),
            },
        };
        let payload = GovernancePayload {
            proposal_id: [1u8; 32],
            action: GovernanceAction::Propose(content),
        };

        let mut event = Event::new(
            setu_types::EventType::Governance,
            vec![],
            VLCSnapshot::default(),
            "validator-1".to_string(),
        );
        event.payload = EventPayload::Governance(payload);

        let msg = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("Governance EventBroadcast serialize");
        eprintln!("EventBroadcast (governance) serialized size: {} bytes", bytes.len());
        let decoded: SetuMessage = bincode::deserialize(&bytes)
            .expect("Governance EventBroadcast deserialization failed!");

        match decoded {
            SetuMessage::EventBroadcast { event: decoded_event, .. } => {
                assert_eq!(decoded_event.id, event.id);
            }
            other => panic!("Expected EventBroadcast, got {:?}", other.message_type()),
        }
    }

    /// Test EventBroadcast with transfer payload
    #[test]
    fn test_event_broadcast_transfer_roundtrip() {
        use setu_types::transfer::{Transfer, TransferType};

        let transfer = Transfer {
            id: "tx-001".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 1000,
            transfer_type: TransferType::SetuTransfer,
            resources: vec!["coin:alice".to_string()],
            power: 10,
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
            assigned_vlc: None,
        };

        let event = Event::transfer(
            transfer,
            vec![],
            VLCSnapshot::default(),
            "validator-1".to_string(),
        );
        let msg = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("Transfer EventBroadcast serialize");
        eprintln!("EventBroadcast (transfer) serialized size: {} bytes", bytes.len());
        let decoded: SetuMessage = bincode::deserialize(&bytes)
            .expect("Transfer EventBroadcast deserialization failed!");

        match decoded {
            SetuMessage::EventBroadcast { event: decoded_event, .. } => {
                assert_eq!(decoded_event.id, event.id);
            }
            other => panic!("Expected EventBroadcast, got {:?}", other.message_type()),
        }
    }

    /// Test CFProposal roundtrip as control group
    #[test]
    fn test_cf_proposal_bincode_roundtrip() {
        use setu_types::consensus::Anchor;

        let anchor = Anchor::new(
            vec!["genesis-event".to_string()],
            VLCSnapshot::default(),
            "state-root-hash".to_string(),
            None,
            0,
        );
        let cf = ConsensusFrame::new(anchor, "validator-1".to_string());

        let msg = SetuMessage::CFProposal {
            cf: cf.clone(),
            proposer_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("CFProposal serialize");
        eprintln!("CFProposal serialized size: {} bytes", bytes.len());
        let decoded: SetuMessage = bincode::deserialize(&bytes)
            .expect("CFProposal deserialization failed!");

        match decoded {
            SetuMessage::CFProposal { cf: decoded_cf, .. } => {
                assert_eq!(decoded_cf.id, cf.id);
            }
            other => panic!("Expected CFProposal, got {:?}", other.message_type()),
        }
    }

    #[test]
    fn test_cf_finalized_bincode_roundtrip() {
        use setu_types::consensus::Anchor;

        let anchor = Anchor::new(
            vec!["genesis-event".to_string()],
            VLCSnapshot::default(),
            "state-root-hash".to_string(),
            None,
            0,
        );
        let mut cf = ConsensusFrame::new(anchor, "validator-1".to_string());
        cf.add_vote(Vote::new("validator-1".to_string(), cf.id.clone(), true));
        cf.add_vote(Vote::new("validator-2".to_string(), cf.id.clone(), true));
        cf.add_vote(Vote::new("validator-3".to_string(), cf.id.clone(), true));
        cf.finalize();

        let msg = SetuMessage::CFFinalized {
            cf: cf.clone(),
            sender_id: "validator-2".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("CFFinalized serialize");
        let decoded: SetuMessage = bincode::deserialize(&bytes)
            .expect("CFFinalized deserialization failed!");

        match decoded {
            SetuMessage::CFFinalized { cf: decoded_cf, sender_id } => {
                assert_eq!(sender_id, "validator-2");
                assert_eq!(decoded_cf.id, cf.id);
                assert_eq!(decoded_cf.approve_count(), 3);
            }
            other => panic!("Expected CFFinalized, got {:?}", other.message_type()),
        }
    }

    /// CRITICAL TEST: Event with execution_result containing StateChange
    /// StateChange.target_subnet has #[serde(skip_serializing_if = "Option::is_none")]
    /// which is incompatible with bincode.
    #[test]
    fn test_event_with_execution_result_roundtrip() {
        use setu_types::event::{ExecutionResult, StateChange};
        use setu_types::EventPayload;
        use setu_types::governance::{GovernancePayload, GovernanceAction, ProposalContent, ProposalType, ProposalEffect};

        let content = ProposalContent {
            proposer: "alice".to_string(),
            proposal_type: ProposalType::ParameterChange,
            title: "Test".to_string(),
            description: "Test".to_string(),
            action: ProposalEffect::UpdateParameter {
                key: "min_transfer_amount".to_string(),
                value: 100u64.to_le_bytes().to_vec(),
            },
        };
        let payload = GovernancePayload {
            proposal_id: [1u8; 32],
            action: GovernanceAction::Propose(content),
        };

        let mut event = Event::new(
            setu_types::EventType::Governance,
            vec![],
            VLCSnapshot::default(),
            "validator-1".to_string(),
        );
        event.payload = EventPayload::Governance(payload);

        // Add execution_result with StateChange where target_subnet = None
        // This triggers #[serde(skip_serializing_if = "Option::is_none")] on target_subnet
        event.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![
                StateChange::insert(
                    "oid:0101010101010101010101010101010101010101010101010101010101010101".to_string(),
                    b"some data".to_vec(),
                ),
            ],
        });

        let msg = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("Serialize EventBroadcast with exec result");
        eprintln!("EventBroadcast (with exec result, target_subnet=None) serialized size: {} bytes", bytes.len());

        // THIS IS THE CRITICAL TEST — does deserialization fail?
        let result = bincode::deserialize::<SetuMessage>(&bytes);
        match result {
            Ok(decoded) => {
                eprintln!("Deserialization SUCCEEDED (skip_serializing_if did NOT break bincode)");
                match decoded {
                    SetuMessage::EventBroadcast { event: decoded_event, .. } => {
                        assert_eq!(decoded_event.id, event.id);
                        assert!(decoded_event.execution_result.is_some());
                    }
                    _ => panic!("Wrong variant"),
                }
            }
            Err(e) => {
                eprintln!("Deserialization FAILED as hypothesized: {}", e);
                panic!("CONFIRMED: skip_serializing_if breaks bincode! Error: {}", e);
            }
        }
    }

    /// Test UserRegistration roundtrip — also has skip_serializing_if
    #[test]
    fn test_event_user_register_roundtrip() {
        use setu_types::registration::UserRegistration;

        let reg = UserRegistration::from_metamask("0xAlice", 12345);
        let event = Event::user_register(
            reg,
            vec![],
            VLCSnapshot::default(),
            "validator-1".to_string(),
        );

        let msg = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: "validator-1".to_string(),
        };
        let bytes = bincode::serialize(&msg).expect("Serialize UserRegister EventBroadcast");
        eprintln!("EventBroadcast (UserRegister) serialized size: {} bytes", bytes.len());

        let result = bincode::deserialize::<SetuMessage>(&bytes);
        match result {
            Ok(decoded) => {
                eprintln!("UserRegister deserialization SUCCEEDED");
                match decoded {
                    SetuMessage::EventBroadcast { event: decoded_event, .. } => {
                        assert_eq!(decoded_event.id, event.id);
                    }
                    _ => panic!("Wrong variant"),
                }
            }
            Err(e) => {
                eprintln!("UserRegister deserialization FAILED: {}", e);
                panic!("CONFIRMED: UserRegistration skip_serializing_if breaks bincode! Error: {}", e);
            }
        }
    }
}
