//! Types for network service

use setu_rpc::ProcessingStep;
use std::net::SocketAddr;

// Re-export types from api module
pub use setu_api::{GetBalanceResponse, GetObjectResponse, SubmitEventRequest, SubmitEventResponse};

/// Validator info for registration.
///
/// G12: Built from `ValidatorRegistration`. Dropped fields:
///   - account_address, public_key, signature (crypto — not needed for API display)
///   - stake_amount, commission_rate (economic — not indexed in-memory)
#[derive(Debug, Clone)]
pub struct ValidatorInfo {
    pub validator_id: String,
    pub address: String,
    pub port: u16,
    pub status: String,
    pub registered_at: u64,
}

impl ValidatorInfo {
    /// Build from a `ValidatorRegistration` event payload.
    ///
    /// `status` is caller-determined ("online" for live, "online" for replay).
    /// `timestamp_ms` is the event timestamp in milliseconds.
    pub fn from_registration(
        reg: &setu_types::registration::ValidatorRegistration,
        status: &str,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            validator_id: reg.validator_id.clone(),
            address: reg.address.clone(),
            port: reg.port,
            status: status.to_string(),
            registered_at: timestamp_ms / 1000,
        }
    }
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

/// Registered Solver information.
///
/// G12: Built from `SolverRegistration`. Dropped fields:
///   - account_address, public_key, signature (crypto — not needed for API display)
/// Retained capability fields (capacity, shard_id, assigned_shard, resources) are used for routing.
#[derive(Debug, Clone)]
pub struct SolverInfo {
    pub solver_id: String,
    pub address: String,
    pub port: u16,
    pub capacity: u32,
    pub shard_id: Option<String>,
    /// Assigned shard ID (numeric, for shard-based routing)
    pub assigned_shard: Option<u16>,
    pub resources: Vec<String>,
    pub status: String,
    pub registered_at: u64,
}

impl SolverInfo {
    /// Build from a `SolverRegistration` event payload.
    ///
    /// `status` is caller-determined ("active" for live, "replayed" for replay).
    /// `timestamp_ms` is the event timestamp in milliseconds.
    pub fn from_registration(
        reg: &setu_types::registration::SolverRegistration,
        status: &str,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            solver_id: reg.solver_id.clone(),
            address: reg.address.clone(),
            port: reg.port,
            capacity: reg.capacity,
            shard_id: reg.shard_id.clone(),
            assigned_shard: reg.assigned_shard,
            resources: reg.resources.clone(),
            status: status.to_string(),
            registered_at: timestamp_ms / 1000,
        }
    }

    /// Get HTTP URL for this solver
    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
    
    /// Get execute-task endpoint URL
    pub fn execute_task_url(&self) -> String {
        format!("{}/api/v1/execute-task", self.http_url())
    }

    /// Get batch execute-task endpoint URL
    pub fn execute_batch_url(&self) -> String {
        format!("{}/api/v1/execute-task-batch", self.http_url())
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

/// Registered subnet information.
///
/// G12: Built from `SubnetRegistration`. Dropped fields:
///   - description (display-only, low priority)
///   - max_users, resource_limits (governance — not indexed in-memory)
///   - assigned_solvers (routing — managed separately by RouterManager)
///   - parent_subnet_id (hierarchy — not needed for flat API query)
///   - initial_token_supply, token_config, user_airdrop_amount (economics — one-time init)
#[derive(Debug, Clone)]
pub struct SubnetInfo {
    pub subnet_id: String,
    pub name: String,
    pub owner: String,
    pub subnet_type: String,
    pub token_symbol: String,
    pub status: String,
    pub registered_at: u64,
}

impl SubnetInfo {
    /// Build from a `SubnetRegistration` event payload.
    ///
    /// `timestamp_ms` is the event timestamp in milliseconds.
    pub fn from_registration(
        reg: &setu_types::registration::SubnetRegistration,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            subnet_id: reg.subnet_id.clone(),
            name: reg.name.clone(),
            owner: reg.owner.clone(),
            subnet_type: format!("{:?}", reg.subnet_type),
            token_symbol: reg.token_symbol.clone().unwrap_or_default(),
            status: "active".to_string(),
            registered_at: timestamp_ms / 1000,
        }
    }
}
