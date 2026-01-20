//! Solver RPC client and server (simplified, no protobuf)

use crate::error::{Result, RpcError};
use setu_types::Transfer;
use anemo::{Network, PeerId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::{info, debug};

// ============================================
// Request/Response types
// ============================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransferRequest {
    pub transfer: Transfer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransferResponse {
    pub transfer_id: String,
    pub accepted: bool,
    pub message: String,
}

// ============================================
// Solver RPC Client (used by Router)
// ============================================

pub struct SolverClient {
    network: Network,
    peer_id: PeerId,
}

impl SolverClient {
    pub fn new(network: Network, peer_id: PeerId) -> Self {
        Self { network, peer_id }
    }
    
    pub async fn submit_transfer(&self, transfer: Transfer) -> Result<String> {
        debug!(
            transfer_id = %transfer.id,
            "Submitting transfer to solver via RPC"
        );
        
        let request = SubmitTransferRequest { transfer };
        let bytes = bincode::serialize(&request)?;
        
        let response = self.network
            .rpc(self.peer_id, anemo::Request::new(bytes::Bytes::from(bytes)))
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        let response: SubmitTransferResponse = bincode::deserialize(response.body())?;
        
        if response.accepted {
            info!(transfer_id = %response.transfer_id, "Transfer accepted");
            Ok(response.transfer_id)
        } else {
            Err(RpcError::InvalidRequest(response.message))
        }
    }
}

// ============================================
// Solver RPC Server (receives transfers)
// ============================================

pub struct SolverServer {
    solver_id: String,
    transfer_tx: tokio::sync::mpsc::UnboundedSender<Transfer>,
    current_load: Arc<AtomicU32>,
    capacity: u32,
}

impl SolverServer {
    pub fn new(
        solver_id: String,
        transfer_tx: tokio::sync::mpsc::UnboundedSender<Transfer>,
        capacity: u32,
    ) -> Self {
        Self {
            solver_id,
            transfer_tx,
            current_load: Arc::new(AtomicU32::new(0)),
            capacity,
        }
    }
    
    pub async fn handle_request(&self, request_bytes: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: SubmitTransferRequest = bincode::deserialize(&request_bytes)?;
        
        info!(
            transfer_id = %request.transfer.id,
            from = %request.transfer.from,
            to = %request.transfer.to,
            "Received transfer via RPC"
        );
        
        // Check capacity
        let current = self.current_load.load(Ordering::Relaxed);
        if current >= self.capacity {
            let response = SubmitTransferResponse {
                transfer_id: request.transfer.id.clone(),
                accepted: false,
                message: "Solver at capacity".to_string(),
            };
            return Ok(bytes::Bytes::from(bincode::serialize(&response)?));
        }
        
        // Send to internal channel
        self.transfer_tx.send(request.transfer.clone())
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        self.current_load.fetch_add(1, Ordering::Relaxed);
        
        let response = SubmitTransferResponse {
            transfer_id: request.transfer.id,
            accepted: true,
            message: "Transfer accepted".to_string(),
        };
        
        Ok(bytes::Bytes::from(bincode::serialize(&response)?))
    }
    
    pub fn decrement_load(&self) {
        self.current_load.fetch_sub(1, Ordering::Relaxed);
    }
}
