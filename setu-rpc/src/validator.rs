//! Validator RPC client and server (simplified, no protobuf)

use crate::error::{Result, RpcError};
use anemo::{Network, PeerId};
use serde::{Deserialize, Serialize};
use setu_types::event::Event;
use tracing::{debug, info};

// ============================================
// Request/Response types
// ============================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventRequest {
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventResponse {
    pub event_id: String,
    pub accepted: bool,
    pub message: String,
}

// ============================================
// Validator RPC Client (used by Solver)
// ============================================

pub struct ValidatorClient {
    network: Network,
    peer_id: PeerId,
}

impl ValidatorClient {
    pub fn new(network: Network, peer_id: PeerId) -> Self {
        Self { network, peer_id }
    }

    pub async fn submit_event(&self, event: Event) -> Result<String> {
        debug!(
            event_id = %event.id,
            "Submitting event to validator via RPC"
        );

        let request = SubmitEventRequest { event };
        let bytes = bincode::serialize(&request)?;

        let response = self
            .network
            .rpc(self.peer_id, anemo::Request::new(bytes::Bytes::from(bytes)))
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;

        let response: SubmitEventResponse = bincode::deserialize(response.body())?;

        if response.accepted {
            info!(event_id = %response.event_id, "Event accepted");
            Ok(response.event_id)
        } else {
            Err(RpcError::InvalidRequest(response.message))
        }
    }
}

// ============================================
// Validator RPC Server (receives events)
// ============================================

pub struct ValidatorServer {
    validator_id: String,
    event_tx: tokio::sync::mpsc::UnboundedSender<Event>,
}

impl ValidatorServer {
    pub fn new(validator_id: String, event_tx: tokio::sync::mpsc::UnboundedSender<Event>) -> Self {
        Self {
            validator_id,
            event_tx,
        }
    }

    pub async fn handle_request(&self, request_bytes: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: SubmitEventRequest = bincode::deserialize(&request_bytes)?;

        info!(
            event_id = %request.event.id,
            creator = %request.event.creator,
            "Received event via RPC"
        );

        // Send to internal channel
        self.event_tx
            .send(request.event.clone())
            .map_err(|e| RpcError::Network(e.to_string()))?;

        let response = SubmitEventResponse {
            event_id: request.event.id,
            accepted: true,
            message: "Event accepted for verification".to_string(),
        };

        Ok(bytes::Bytes::from(bincode::serialize(&response)?))
    }
}
