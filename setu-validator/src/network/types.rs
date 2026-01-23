//! Types for network service

use setu_rpc::ProcessingStep;
use std::net::SocketAddr;

// Re-export types from api module
pub use setu_api::{GetBalanceResponse, GetObjectResponse, SubmitEventRequest, SubmitEventResponse};

/// Validator info for registration
#[derive(Debug, Clone)]
pub struct ValidatorInfo {
    pub validator_id: String,
    pub address: String,
    pub port: u16,
    pub status: String,
    pub registered_at: u64,
}

/// Network service configuration
#[derive(Debug, Clone)]
pub struct NetworkServiceConfig {
    /// Listen address for HTTP API
    pub http_listen_addr: SocketAddr,
    /// Listen address for Anemo P2P
    pub p2p_listen_addr: SocketAddr,
}

impl Default for NetworkServiceConfig {
    fn default() -> Self {
        Self {
            http_listen_addr: "127.0.0.1:8080".parse().unwrap(),
            p2p_listen_addr: "127.0.0.1:9000".parse().unwrap(),
        }
    }
}

/// Transfer tracking information
#[derive(Debug, Clone)]
pub struct TransferTracker {
    pub transfer_id: String,
    pub status: String,
    pub solver_id: Option<String>,
    pub event_id: Option<String>,
    pub processing_steps: Vec<ProcessingStep>,
    pub created_at: u64,
}

/// Registered Solver information
#[derive(Debug, Clone)]
pub struct SolverInfo {
    pub solver_id: String,
    pub address: String,
    pub port: u16,
    pub capacity: u32,
    pub shard_id: Option<String>,
    pub resources: Vec<String>,
    pub status: String,
    pub registered_at: u64,
}

impl SolverInfo {
    /// Get HTTP URL for this solver
    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
    
    /// Get execute-task endpoint URL
    pub fn execute_task_url(&self) -> String {
        format!("{}/api/v1/execute-task", self.http_url())
    }
}

/// Helper to get current timestamp in seconds
#[inline]
pub fn current_timestamp_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Helper to get current timestamp in milliseconds
#[inline]
pub fn current_timestamp_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
