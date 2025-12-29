//! Router RPC client and server (simplified, no protobuf)

use crate::error::{Result, RpcError};
use anemo::{Network, PeerId};
use serde::{Deserialize, Serialize};
use tracing::{info, debug};

// ============================================
// Request/Response types
// ============================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSolverRequest {
    pub solver_id: String,
    pub address: String,
    pub port: u32,
    pub capacity: u32,
    pub shard_id: Option<String>,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSolverResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub solver_id: String,
    pub current_load: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub acknowledged: bool,
}

// ============================================
// Router RPC Client (used by Solver)
// ============================================

pub struct RouterClient {
    network: Network,
    peer_id: PeerId,
}

impl RouterClient {
    pub fn new(network: Network, peer_id: PeerId) -> Self {
        Self { network, peer_id }
    }
    
    pub async fn register_solver(
        &self,
        solver_id: String,
        address: String,
        port: u32,
        capacity: u32,
        shard_id: Option<String>,
        resources: Vec<String>,
    ) -> Result<bool> {
        info!(
            solver_id = %solver_id,
            address = %address,
            port = port,
            "Registering solver with router via RPC"
        );
        
        let request = RegisterSolverRequest {
            solver_id,
            address,
            port,
            capacity,
            shard_id,
            resources,
        };
        
        let bytes = bincode::serialize(&request)?;
        
        let response = self.network
            .rpc(self.peer_id, anemo::Request::new(bytes::Bytes::from(bytes)))
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        let response: RegisterSolverResponse = bincode::deserialize(response.body())?;
        
        if response.success {
            info!("Solver registered successfully");
        } else {
            info!("Solver registration failed: {}", response.message);
        }
        
        Ok(response.success)
    }
    
    pub async fn heartbeat(&self, solver_id: String, current_load: u32) -> Result<bool> {
        debug!(
            solver_id = %solver_id,
            current_load = current_load,
            "Sending heartbeat to router"
        );
        
        let request = HeartbeatRequest {
            solver_id,
            current_load,
        };
        
        let bytes = bincode::serialize(&request)?;
        
        let response = self.network
            .rpc(self.peer_id, anemo::Request::new(bytes::Bytes::from(bytes)))
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        let response: HeartbeatResponse = bincode::deserialize(response.body())?;
        
        Ok(response.acknowledged)
    }
}

// ============================================
// Router RPC Server (receives registrations)
// ============================================

pub struct RouterServer {
    router_id: String,
    on_solver_register: Box<dyn Fn(RegisterSolverRequest) -> bool + Send + Sync>,
}

impl RouterServer {
    pub fn new<F>(router_id: String, on_solver_register: F) -> Self
    where
        F: Fn(RegisterSolverRequest) -> bool + Send + Sync + 'static,
    {
        Self {
            router_id,
            on_solver_register: Box::new(on_solver_register),
        }
    }
    
    pub async fn handle_request(&self, request_bytes: bytes::Bytes) -> Result<bytes::Bytes> {
        // Try to deserialize as RegisterSolverRequest first
        if let Ok(request) = bincode::deserialize::<RegisterSolverRequest>(&request_bytes) {
            info!(
                solver_id = %request.solver_id,
                address = %request.address,
                "Received solver registration"
            );
            
            let success = (self.on_solver_register)(request);
            
            let response = RegisterSolverResponse {
                success,
                message: if success {
                    "Solver registered successfully".to_string()
                } else {
                    "Failed to register solver".to_string()
                },
            };
            
            return Ok(bytes::Bytes::from(bincode::serialize(&response)?));
        }
        
        // Try to deserialize as HeartbeatRequest
        if let Ok(request) = bincode::deserialize::<HeartbeatRequest>(&request_bytes) {
            debug!(
                solver_id = %request.solver_id,
                current_load = request.current_load,
                "Received heartbeat"
            );
            
            let response = HeartbeatResponse {
                acknowledged: true,
            };
            
            return Ok(bytes::Bytes::from(bincode::serialize(&response)?));
        }
        
        Err(RpcError::InvalidRequest("Unknown request type".to_string()))
    }
}
