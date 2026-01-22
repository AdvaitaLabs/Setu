//! Anemo-based Consensus Broadcaster
//!
//! This adapter wraps the AnemoNetworkService to implement the
//! ConsensusBroadcaster trait, providing actual P2P message delivery
//! for consensus messages.
//!
//! Message serialization is handled by the application layer using
//! the SetuMessage type from setu_validator::protocol.

use bytes::Bytes;
use consensus::{BroadcastError, BroadcastResult, ConsensusBroadcaster};
use setu_network_anemo::{AnemoNetworkService, PeerId};
use setu_types::{ConsensusFrame, Event, EventId, Vote};
use std::fmt;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::protocol::{SetuMessage, MessageCodec};

/// The route used for Setu consensus messages
const SETU_ROUTE: &str = "/setu";

/// Adapter that implements ConsensusBroadcaster using Anemo P2P network
pub struct AnemoConsensusBroadcaster {
    /// The underlying Anemo network service
    network: Arc<AnemoNetworkService>,
    /// Local validator ID
    local_validator_id: String,
}

impl fmt::Debug for AnemoConsensusBroadcaster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnemoConsensusBroadcaster")
            .field("local_validator_id", &self.local_validator_id)
            .field("peer_count", &self.network.get_peer_count())
            .finish()
    }
}

impl AnemoConsensusBroadcaster {
    /// Create a new Anemo consensus broadcaster
    pub fn new(network: Arc<AnemoNetworkService>, local_validator_id: String) -> Self {
        Self {
            network,
            local_validator_id,
        }
    }

    /// Serialize a message to bytes
    fn serialize(message: &SetuMessage) -> Result<Bytes, BroadcastError> {
        MessageCodec::encode(message)
            .map_err(|e| BroadcastError::NetworkError(format!("Serialization failed: {}", e)))
    }

    /// Deserialize bytes to a message
    fn deserialize(bytes: &[u8]) -> Result<SetuMessage, BroadcastError> {
        MessageCodec::decode(bytes)
            .map_err(|e| BroadcastError::NetworkError(format!("Deserialization failed: {}", e)))
    }
}

#[async_trait::async_trait]
impl ConsensusBroadcaster for AnemoConsensusBroadcaster {
    async fn broadcast_cf(&self, cf: &ConsensusFrame) -> Result<BroadcastResult, BroadcastError> {
        let peers = self.network.get_connected_peers();
        let total_peers = peers.len();
        
        if total_peers == 0 {
            debug!(cf_id = %cf.id, "No peers to broadcast CF to");
            return Ok(BroadcastResult::success(0, 0));
        }

        let message = SetuMessage::CFProposal {
            cf: cf.clone(),
            proposer_id: self.local_validator_id.clone(),
        };
        let bytes = Self::serialize(&message)?;

        // Broadcast to all peers
        match self.network.broadcast(SETU_ROUTE, bytes).await {
            Ok((success, total)) => {
                info!(cf_id = %cf.id, success = success, total = total, "CF broadcasted");
                Ok(BroadcastResult::success(success, total))
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                warn!(cf_id = %cf.id, error = %error_msg, "Failed to broadcast CF");
                Err(BroadcastError::AllFailed(error_msg))
            }
        }
    }

    async fn broadcast_vote(&self, vote: &Vote) -> Result<BroadcastResult, BroadcastError> {
        let peers = self.network.get_connected_peers();
        let total_peers = peers.len();
        
        if total_peers == 0 {
            debug!(cf_id = %vote.cf_id, "No peers to broadcast vote to");
            return Ok(BroadcastResult::success(0, 0));
        }

        let message = SetuMessage::CFVote { vote: vote.clone() };
        let bytes = Self::serialize(&message)?;

        // Broadcast to all peers
        match self.network.broadcast(SETU_ROUTE, bytes).await {
            Ok((success, total)) => {
                info!(cf_id = %vote.cf_id, success = success, total = total, "Vote broadcasted");
                Ok(BroadcastResult::success(success, total))
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                warn!(cf_id = %vote.cf_id, error = %error_msg, "Failed to broadcast vote");
                Err(BroadcastError::AllFailed(error_msg))
            }
        }
    }

    async fn broadcast_finalized(&self, cf_id: &str) -> Result<BroadcastResult, BroadcastError> {
        // For finalization, we notify the network layer
        let peers = self.network.get_connected_peers();
        let total_peers = peers.len();
        
        if total_peers == 0 {
            debug!(cf_id = %cf_id, "No peers to broadcast finalization to");
            return Ok(BroadcastResult::success(0, 0));
        }

        // For MVP, finalization is implicitly communicated through vote quorum
        debug!(cf_id = %cf_id, peer_count = total_peers, "CF finalization acknowledged (implicit via votes)");
        
        Ok(BroadcastResult::success(total_peers, total_peers))
    }

    async fn broadcast_event(&self, event: &Event) -> Result<BroadcastResult, BroadcastError> {
        let total_peers = self.network.get_peer_count();
        
        if total_peers == 0 {
            debug!(event_id = %event.id, "No peers to broadcast event to");
            return Ok(BroadcastResult::success(0, 0));
        }

        let message = SetuMessage::EventBroadcast {
            event: event.clone(),
            sender_id: self.local_validator_id.clone(),
        };
        let bytes = Self::serialize(&message)?;

        // Use the generic broadcast method
        match self.network.broadcast(SETU_ROUTE, bytes).await {
            Ok((success, total)) => {
                info!(
                    event_id = %event.id,
                    success = success,
                    total = total,
                    "Event broadcasted"
                );
                Ok(BroadcastResult::success(success, total))
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                warn!(event_id = %event.id, error = %error_msg, "Failed to broadcast event");
                Err(BroadcastError::AllFailed(error_msg))
            }
        }
    }

    async fn request_events(&self, event_ids: &[EventId]) -> Result<Vec<Event>, BroadcastError> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            count = event_ids.len(),
            "Requesting missing events from peers"
        );

        // Convert EventId to String for the request
        let event_id_strings: Vec<String> = event_ids.iter().cloned().collect();

        let message = SetuMessage::RequestEvents {
            event_ids: event_id_strings.clone(),
            requester_id: self.local_validator_id.clone(),
        };
        let bytes = Self::serialize(&message)?;

        // Try each peer until we get the events
        let peers = self.network.get_connected_peers();
        let mut fetched_events: Vec<Event> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for peer_id_str in peers {
            // Parse peer ID (simplified - in production use proper peer ID type)
            let peer_id = match parse_peer_id(&peer_id_str) {
                Ok(id) => id,
                Err(_) => continue,
            };

            match self.network.send_to_peer(peer_id, SETU_ROUTE, bytes.clone()).await {
                Ok(response) => {
                    if let Ok(SetuMessage::EventsResponse { events, .. }) = Self::deserialize(&response) {
                        for event in events {
                            if !seen_ids.contains(&event.id) {
                                seen_ids.insert(event.id.clone());
                                fetched_events.push(event);
                            }
                        }
                    }

                    // Check if we got all requested events
                    if fetched_events.len() >= event_id_strings.len() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(peer = %peer_id_str, error = %e, "Failed to request events from peer");
                }
            }
        }

        info!(
            requested = event_ids.len(),
            fetched = fetched_events.len(),
            "Event fetch completed"
        );

        Ok(fetched_events)
    }

    fn peer_count(&self) -> usize {
        self.network.get_peer_count()
    }

    fn local_validator_id(&self) -> &str {
        &self.local_validator_id
    }
}

/// Parse a hex-encoded peer ID string to PeerId
fn parse_peer_id(peer_id_str: &str) -> Result<PeerId, BroadcastError> {
    let bytes = hex::decode(peer_id_str)
        .map_err(|e| BroadcastError::NetworkError(format!("Invalid peer ID: {}", e)))?;
    
    if bytes.len() != 32 {
        return Err(BroadcastError::NetworkError("Peer ID must be 32 bytes".to_string()));
    }

    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);
    Ok(PeerId(array))
}

#[cfg(test)]
mod tests {
    // Tests would require mocking AnemoNetworkService
    // For now, integration tests should cover this functionality
}
