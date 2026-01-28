//! Registration service for Solver and Validator nodes
//!
//! This module provides the RPC service for node registration,
//! allowing Solvers to register with Validators.

use crate::error::{Result, RpcError};
use crate::messages::*;
use anemo::{Network, PeerId, Request};
use bytes::Bytes;
use std::sync::Arc;
use tracing::{info, debug};

// ============================================
// Registration Service Trait
// ============================================

/// Trait for handling registration requests
#[async_trait::async_trait]
pub trait RegistrationHandler: Send + Sync {
    /// Handle solver registration
    async fn register_solver(&self, request: RegisterSolverRequest) -> RegisterSolverResponse;
    
    /// Handle validator registration
    async fn register_validator(&self, request: RegisterValidatorRequest) -> RegisterValidatorResponse;
    
    /// Handle unregister request
    async fn unregister(&self, request: UnregisterRequest) -> UnregisterResponse;
    
    /// Handle heartbeat
    async fn heartbeat(&self, request: HeartbeatRequest) -> HeartbeatResponse;
    
    /// Get solver list
    async fn get_solver_list(&self, request: GetSolverListRequest) -> GetSolverListResponse;
    
    /// Get validator list
    async fn get_validator_list(&self, request: GetValidatorListRequest) -> GetValidatorListResponse;
    
    /// Get node status
    async fn get_node_status(&self, request: GetNodeStatusRequest) -> GetNodeStatusResponse;
}

// ============================================
// Registration Server
// ============================================

/// Registration RPC server that handles incoming registration requests
pub struct RegistrationServer<H: RegistrationHandler> {
    handler: Arc<H>,
}

impl<H: RegistrationHandler> RegistrationServer<H> {
    /// Create a new registration server
    pub fn new(handler: Arc<H>) -> Self {
        Self { handler }
    }
    
    /// Handle incoming RPC request
    pub async fn handle_request(&self, request_bytes: Bytes) -> Result<Bytes> {
        let request = RpcRequest::from_bytes(&request_bytes)
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        let response = match request {
            RpcRequest::RegisterSolver(req) => {
                info!(solver_id = %req.solver_id, "Handling solver registration");
                RpcResponse::RegisterSolver(self.handler.register_solver(req).await)
            }
            RpcRequest::RegisterValidator(req) => {
                info!(validator_id = %req.validator_id, "Handling validator registration");
                RpcResponse::RegisterValidator(self.handler.register_validator(req).await)
            }
            RpcRequest::Unregister(req) => {
                info!(node_id = %req.node_id, "Handling unregister request");
                RpcResponse::Unregister(self.handler.unregister(req).await)
            }
            RpcRequest::Heartbeat(req) => {
                debug!(node_id = %req.node_id, "Handling heartbeat");
                RpcResponse::Heartbeat(self.handler.heartbeat(req).await)
            }
            RpcRequest::GetSolverList(req) => {
                debug!("Handling get solver list");
                RpcResponse::GetSolverList(self.handler.get_solver_list(req).await)
            }
            RpcRequest::GetValidatorList(req) => {
                debug!("Handling get validator list");
                RpcResponse::GetValidatorList(self.handler.get_validator_list(req).await)
            }
            RpcRequest::GetNodeStatus(req) => {
                debug!(node_id = %req.node_id, "Handling get node status");
                RpcResponse::GetNodeStatus(self.handler.get_node_status(req).await)
            }
        };
        
        let response_bytes = response.to_bytes()
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        Ok(Bytes::from(response_bytes))
    }
}

impl<H: RegistrationHandler> Clone for RegistrationServer<H> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
        }
    }
}

// ============================================
// Registration Client
// ============================================

/// Registration RPC client for connecting to validators
pub struct RegistrationClient {
    network: Network,
    peer_id: PeerId,
}

impl RegistrationClient {
    /// Create a new registration client
    pub fn new(network: Network, peer_id: PeerId) -> Self {
        Self { network, peer_id }
    }
    
    /// Send RPC request and get response
    async fn send_request(&self, request: RpcRequest) -> Result<RpcResponse> {
        let request_bytes = request.to_bytes()
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        let response = self.network
            .rpc(self.peer_id, Request::new(Bytes::from(request_bytes)))
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        let response = RpcResponse::from_bytes(response.body())
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        Ok(response)
    }
    
    /// Register a solver
    pub async fn register_solver(&self, request: RegisterSolverRequest) -> Result<RegisterSolverResponse> {
        info!(
            solver_id = %request.solver_id,
            address = %request.address,
            port = request.port,
            account_address = %request.account_address,
            "Registering solver"
        );
        
        let response = self.send_request(RpcRequest::RegisterSolver(request)).await?;
        
        match response {
            RpcResponse::RegisterSolver(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Register a validator
    pub async fn register_validator(&self, request: RegisterValidatorRequest) -> Result<RegisterValidatorResponse> {
        info!(
            validator_id = %request.validator_id,
            address = %request.address,
            port = request.port,
            account_address = %request.account_address,
            "Registering validator"
        );
        
        let response = self.send_request(RpcRequest::RegisterValidator(request)).await?;
        
        match response {
            RpcResponse::RegisterValidator(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Unregister a node
    pub async fn unregister(&self, node_id: String, node_type: NodeType) -> Result<UnregisterResponse> {
        info!(node_id = %node_id, node_type = %node_type, "Unregistering node");
        
        let request = UnregisterRequest { node_id, node_type };
        let response = self.send_request(RpcRequest::Unregister(request)).await?;
        
        match response {
            RpcResponse::Unregister(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Send heartbeat
    pub async fn heartbeat(&self, node_id: String, current_load: Option<u32>) -> Result<HeartbeatResponse> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let request = HeartbeatRequest {
            node_id,
            current_load,
            timestamp,
        };
        
        let response = self.send_request(RpcRequest::Heartbeat(request)).await?;
        
        match response {
            RpcResponse::Heartbeat(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Get solver list
    pub async fn get_solver_list(&self, shard_id: Option<String>) -> Result<GetSolverListResponse> {
        let request = GetSolverListRequest {
            shard_id,
            status_filter: None,
        };
        
        let response = self.send_request(RpcRequest::GetSolverList(request)).await?;
        
        match response {
            RpcResponse::GetSolverList(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Get validator list
    pub async fn get_validator_list(&self) -> Result<GetValidatorListResponse> {
        let request = GetValidatorListRequest {
            status_filter: None,
        };
        
        let response = self.send_request(RpcRequest::GetValidatorList(request)).await?;
        
        match response {
            RpcResponse::GetValidatorList(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
    
    /// Get node status
    pub async fn get_node_status(&self, node_id: String) -> Result<GetNodeStatusResponse> {
        let request = GetNodeStatusRequest { node_id };
        
        let response = self.send_request(RpcRequest::GetNodeStatus(request)).await?;
        
        match response {
            RpcResponse::GetNodeStatus(resp) => Ok(resp),
            RpcResponse::Error(msg) => Err(RpcError::InvalidRequest(msg)),
            _ => Err(RpcError::InvalidRequest("Unexpected response type".to_string())),
        }
    }
}

// ============================================
// Simple HTTP-based Registration Client (for CLI)
// ============================================

/// Simple registration client using HTTP (for CLI usage without P2P network)
pub struct HttpRegistrationClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpRegistrationClient {
    /// Create a new HTTP registration client
    pub fn new(validator_address: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}", validator_address, port),
            client: reqwest::Client::new(),
        }
    }
    
    /// Register a solver via HTTP
    pub async fn register_solver(&self, request: RegisterSolverRequest) -> Result<RegisterSolverResponse> {
        let url = format!("{}/api/v1/register/solver", self.base_url);
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(RpcError::Network(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        
        response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))
    }
    
    /// Register a validator via HTTP
    pub async fn register_validator(&self, request: RegisterValidatorRequest) -> Result<RegisterValidatorResponse> {
        let url = format!("{}/api/v1/register/validator", self.base_url);
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(RpcError::Network(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        
        response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))
    }
    
    /// Get solver list via HTTP
    pub async fn get_solver_list(&self) -> Result<GetSolverListResponse> {
        let url = format!("{}/api/v1/solvers", self.base_url);
        
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(RpcError::Network(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        
        response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))
    }
    
    /// Get validator list via HTTP
    pub async fn get_validator_list(&self) -> Result<GetValidatorListResponse> {
        let url = format!("{}/api/v1/validators", self.base_url);
        
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(RpcError::Network(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        
        response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_request_serialization() {
        let request = RegisterSolverRequest {
            solver_id: "solver-1".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9001,
            account_address: "0xabcd1234".to_string(),
            public_key: vec![1, 2, 3],
            signature: vec![4, 5, 6],
            capacity: 100,
            shard_id: Some("shard-0".to_string()),
            resources: vec!["ETH".to_string(), "BTC".to_string()],
        };
        
        let rpc_request = RpcRequest::RegisterSolver(request);
        let bytes = rpc_request.to_bytes().unwrap();
        let decoded = RpcRequest::from_bytes(&bytes).unwrap();
        
        match decoded {
            RpcRequest::RegisterSolver(req) => {
                assert_eq!(req.solver_id, "solver-1");
                assert_eq!(req.port, 9001);
            }
            _ => panic!("Wrong request type"),
        }
    }
    
    #[test]
    fn test_response_serialization() {
        let response = RegisterSolverResponse {
            success: true,
            message: "Registered successfully".to_string(),
            assigned_id: Some("solver-1".to_string()),
        };
        
        let rpc_response = RpcResponse::RegisterSolver(response);
        let bytes = rpc_response.to_bytes().unwrap();
        let decoded = RpcResponse::from_bytes(&bytes).unwrap();
        
        match decoded {
            RpcResponse::RegisterSolver(resp) => {
                assert!(resp.success);
                assert_eq!(resp.assigned_id, Some("solver-1".to_string()));
            }
            _ => panic!("Wrong response type"),
        }
    }
}

