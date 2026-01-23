//! Network client for Solver (solver-tee3 Architecture)
//!
//! This module provides the network client for Solver to:
//! - Register with Validator
//! - Send heartbeats
//! - Submit execution results to Validator
//!
//! ## solver-tee3 Architecture Note
//!
//! In solver-tee3, Solver does NOT read state from Validator.
//! All state is provided in the SolverTask prepared by Validator.
//! This client is only for:
//! 1. Registration and heartbeat (lifecycle management)
//! 2. Submitting TeeExecutionResult back to Validator

use serde::{Serialize, Deserialize};
use setu_rpc::{
    RegisterSolverRequest, RegisterSolverResponse,
    HeartbeatRequest, HeartbeatResponse,
    HttpRegistrationClient,
    Result as RpcResult, RpcError,
};
use setu_types::event::Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn, error, debug};

/// Submit Event Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventRequest {
    pub event: Event,
}

/// Submit Event Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventResponse {
    pub success: bool,
    pub message: String,
    pub event_id: Option<String>,
    pub vlc_time: Option<u64>,
}

/// Solver network client configuration
#[derive(Debug, Clone)]
pub struct SolverNetworkConfig {
    /// Solver ID
    pub solver_id: String,
    /// Solver's listen address
    pub address: String,
    /// Solver's listen port
    pub port: u16,
    /// Maximum capacity (concurrent tasks)
    pub capacity: u32,
    /// Shard assignment
    pub shard_id: Option<String>,
    /// Resource types this solver handles
    pub resources: Vec<String>,
    /// Validator address to connect to
    pub validator_address: String,
    /// Validator HTTP port
    pub validator_port: u16,
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
    /// Account address for receiving fees
    pub account_address: String,
    /// Public key (secp256k1)
    pub public_key: Vec<u8>,
    /// Signature for registration
    pub signature: Vec<u8>,
}

impl Default for SolverNetworkConfig {
    fn default() -> Self {
        Self {
            solver_id: "solver-1".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9001,
            capacity: 100,
            shard_id: None,
            resources: vec![],
            validator_address: "127.0.0.1".to_string(),
            validator_port: 8080,
            heartbeat_interval_secs: 30,
            account_address: "0x0000000000000000000000000000000000000000".to_string(),
            public_key: vec![],
            signature: vec![],
        }
    }
}

/// Solver network client for connecting to Validator
///
/// Handles:
/// - Registration with Validator
/// - Periodic heartbeats
/// - Load tracking
///
/// Does NOT handle (in solver-tee3):
/// - State reads (provided in SolverTask)
/// - Balance queries (Validator prepares everything)
pub struct SolverNetworkClient {
    config: SolverNetworkConfig,
    http_client: HttpRegistrationClient,
    current_load: Arc<AtomicU32>,
    is_registered: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl SolverNetworkClient {
    /// Create a new solver network client
    pub fn new(config: SolverNetworkConfig) -> Self {
        let http_client = HttpRegistrationClient::new(
            &config.validator_address,
            config.validator_port,
        );
        
        info!(
            solver_id = %config.solver_id,
            validator = %format!("{}:{}", config.validator_address, config.validator_port),
            "Creating solver network client"
        );
        
        Self {
            config,
            http_client,
            current_load: Arc::new(AtomicU32::new(0)),
            is_registered: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
    
    /// Register with the validator
    pub async fn register(&self) -> RpcResult<RegisterSolverResponse> {
        info!(
            solver_id = %self.config.solver_id,
            validator = %format!("{}:{}", self.config.validator_address, self.config.validator_port),
            "Registering with validator"
        );
        
        let request = RegisterSolverRequest {
            solver_id: self.config.solver_id.clone(),
            network_address: self.config.address.clone(),
            network_port: self.config.port,
            account_address: self.config.account_address.clone(),
            public_key: self.config.public_key.clone(),
            signature: self.config.signature.clone(),
            capacity: self.config.capacity,
            shard_id: self.config.shard_id.clone(),
            resources: self.config.resources.clone(),
        };
        
        let response = self.http_client.register_solver(request).await?;
        
        if response.success {
            self.is_registered.store(true, Ordering::SeqCst);
            info!(
                solver_id = %self.config.solver_id,
                "Successfully registered with validator"
            );
        } else {
            warn!(
                solver_id = %self.config.solver_id,
                message = %response.message,
                "Failed to register with validator"
            );
        }
        
        Ok(response)
    }
    
    /// Send heartbeat to validator
    pub async fn heartbeat(&self) -> RpcResult<HeartbeatResponse> {
        let current_load = self.current_load.load(Ordering::Relaxed);
        
        debug!(
            solver_id = %self.config.solver_id,
            current_load = current_load,
            "Sending heartbeat"
        );
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let request = HeartbeatRequest {
            node_id: self.config.solver_id.clone(),
            current_load: Some(current_load),
            timestamp,
        };
        
        let url = format!(
            "http://{}:{}/api/v1/heartbeat",
            self.config.validator_address,
            self.config.validator_port
        );
        
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            return Ok(HeartbeatResponse {
                acknowledged: false,
                server_timestamp: 0,
            });
        }
        
        let response: HeartbeatResponse = response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        Ok(response)
    }
    
    /// Submit an event to the validator (for backward compatibility)
    ///
    /// In solver-tee3, prefer using `submit_execution_result` instead.
    pub async fn submit_event(&self, event: Event) -> RpcResult<SubmitEventResponse> {
        info!(
            solver_id = %self.config.solver_id,
            event_id = %event.id,
            event_type = %event.event_type.name(),
            "Submitting event to validator"
        );
        
        let request = SubmitEventRequest { event };
        
        let url = format!(
            "http://{}:{}/api/v1/event",
            self.config.validator_address,
            self.config.validator_port
        );
        
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcError::Network(e.to_string()))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(
                status = %status,
                body = %body,
                "Failed to submit event"
            );
            return Err(RpcError::Network(format!("HTTP error: {} - {}", status, body)));
        }
        
        let response: SubmitEventResponse = response
            .json()
            .await
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        
        if response.success {
            info!(
                event_id = ?response.event_id,
                vlc_time = ?response.vlc_time,
                "Event submitted successfully"
            );
        } else {
            warn!(
                message = %response.message,
                "Event submission failed"
            );
        }
        
        Ok(response)
    }
    
    /// Get validator base URL
    pub fn validator_url(&self) -> String {
        format!("http://{}:{}", self.config.validator_address, self.config.validator_port)
    }
    
    /// Start the heartbeat loop
    pub async fn start_heartbeat_loop(&self) {
        let interval = Duration::from_secs(self.config.heartbeat_interval_secs);
        
        info!(
            solver_id = %self.config.solver_id,
            interval_secs = self.config.heartbeat_interval_secs,
            "Starting heartbeat loop"
        );
        
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                info!("Heartbeat loop shutting down");
                break;
            }
            
            tokio::time::sleep(interval).await;
            
            if !self.is_registered.load(Ordering::Relaxed) {
                debug!("Not registered, skipping heartbeat");
                continue;
            }
            
            match self.heartbeat().await {
                Ok(response) => {
                    if response.acknowledged {
                        debug!("Heartbeat acknowledged");
                    } else {
                        warn!("Heartbeat not acknowledged, may need to re-register");
                    }
                }
                Err(e) => {
                    error!(error = %e, "Heartbeat failed");
                    self.is_registered.store(false, Ordering::SeqCst);
                }
            }
        }
    }
    
    /// Increment current load
    pub fn increment_load(&self) {
        self.current_load.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Decrement current load
    pub fn decrement_load(&self) {
        self.current_load.fetch_sub(1, Ordering::Relaxed);
    }
    
    /// Get current load
    pub fn get_load(&self) -> u32 {
        self.current_load.load(Ordering::Relaxed)
    }
    
    /// Check if registered
    pub fn is_registered(&self) -> bool {
        self.is_registered.load(Ordering::Relaxed)
    }
    
    /// Shutdown the client
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
    
    /// Get solver ID
    pub fn solver_id(&self) -> &str {
        &self.config.solver_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_default() {
        let config = SolverNetworkConfig::default();
        assert_eq!(config.solver_id, "solver-1");
        assert_eq!(config.capacity, 100);
        assert_eq!(config.heartbeat_interval_secs, 30);
    }
    
    #[test]
    fn test_client_creation() {
        let config = SolverNetworkConfig {
            solver_id: "test-solver".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9001,
            capacity: 50,
            shard_id: Some("shard-0".to_string()),
            resources: vec!["ETH".to_string()],
            validator_address: "127.0.0.1".to_string(),
            validator_port: 8080,
            heartbeat_interval_secs: 10,
            account_address: "0x0000000000000000000000000000000000000000".to_string(),
            public_key: vec![],
            signature: vec![],
        };
        
        let client = SolverNetworkClient::new(config);
        
        assert_eq!(client.solver_id(), "test-solver");
        assert_eq!(client.get_load(), 0);
        assert!(!client.is_registered());
    }
    
    #[test]
    fn test_load_tracking() {
        let config = SolverNetworkConfig::default();
        let client = SolverNetworkClient::new(config);
        
        assert_eq!(client.get_load(), 0);
        
        client.increment_load();
        client.increment_load();
        assert_eq!(client.get_load(), 2);
        
        client.decrement_load();
        assert_eq!(client.get_load(), 1);
    }
}

