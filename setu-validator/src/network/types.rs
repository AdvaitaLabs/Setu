//! Types for network service

use setu_types::event::Event;
use setu_rpc::ProcessingStep;
use std::net::SocketAddr;

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

/// Submit Event Request (from Solver)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubmitEventRequest {
    pub event: Event,
}

/// Submit Event Response
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubmitEventResponse {
    pub success: bool,
    pub message: String,
    pub event_id: Option<String>,
    pub vlc_time: Option<u64>,
}

/// Get balance response (Scheme B state query)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GetBalanceResponse {
    pub account: String,
    pub balance: u128,
    pub exists: bool,
}

/// Get object response (Scheme B state query)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GetObjectResponse {
    pub key: String,
    pub value: Option<Vec<u8>>,
    pub exists: bool,
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
