//! RPC message types for Setu network communication
//!
//! This module defines all request/response types used in RPC communication.

use serde::{Deserialize, Serialize};

// ============================================
// Message Type Discriminator
// ============================================

/// Message type for routing RPC requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageType {
    // Registration messages (0x1x)
    RegisterSolver = 0x10,
    RegisterValidator = 0x11,
    Unregister = 0x12,
    Heartbeat = 0x13,
    
    // Query messages (0x2x)
    GetSolverList = 0x20,
    GetValidatorList = 0x21,
    GetNodeStatus = 0x22,
    
    // Transfer messages (0x3x)
    SubmitTransfer = 0x30,
    TransferResult = 0x31,
    SubmitTransfersBatch = 0x32,
    
    // Event messages (0x4x)
    SubmitEvent = 0x40,
    EventResult = 0x41,
}

// ============================================
// Registration Request/Response Types
// ============================================

/// Request to register a solver with the validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSolverRequest {
    /// Unique solver identifier
    pub solver_id: String,
    /// Network address (IP or hostname)
    pub address: String,
    /// Port number
    pub port: u16,
    /// Ethereum-style account address for receiving fees
    pub account_address: String,
    /// Public key (secp256k1, 65 bytes uncompressed)
    pub public_key: Vec<u8>,
    /// Registration signature
    pub signature: Vec<u8>,
    /// Maximum capacity (concurrent transfers)
    pub capacity: u32,
    /// Optional shard assignment
    pub shard_id: Option<String>,
    /// Resource types this solver can handle
    pub resources: Vec<String>,
}

/// Response to solver registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSolverResponse {
    /// Whether registration was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Assigned solver ID (may differ from requested)
    pub assigned_id: Option<String>,
}

/// Request to register a validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterValidatorRequest {
    /// Unique validator identifier
    pub validator_id: String,
    /// Network address (IP or hostname)
    pub address: String,
    /// Port number
    pub port: u16,
    /// Ethereum-style account address for staking and rewards
    pub account_address: String,
    /// Public key (secp256k1, 65 bytes uncompressed)
    pub public_key: Vec<u8>,
    /// Registration signature
    pub signature: Vec<u8>,
    /// Stake amount in Flux
    pub stake_amount: u64,
    /// Commission rate (0-100)
    pub commission_rate: u8,
}

/// Response to validator registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterValidatorResponse {
    /// Whether registration was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
}

/// Request to unregister a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnregisterRequest {
    /// Node ID to unregister
    pub node_id: String,
    /// Node type (solver or validator)
    pub node_type: NodeType,
}

/// Response to unregister request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnregisterResponse {
    pub success: bool,
    pub message: String,
}

/// Heartbeat request to keep registration alive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    /// Node ID
    pub node_id: String,
    /// Current load (for solvers)
    pub current_load: Option<u32>,
    /// Timestamp
    pub timestamp: u64,
}

/// Heartbeat response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    /// Whether heartbeat was acknowledged
    pub acknowledged: bool,
    /// Server timestamp
    pub server_timestamp: u64,
}

// ============================================
// Query Request/Response Types
// ============================================

/// Request to get list of registered solvers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSolverListRequest {
    /// Optional filter by shard
    pub shard_id: Option<String>,
    /// Optional filter by status
    pub status_filter: Option<String>,
}

/// Solver info in list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverListItem {
    pub solver_id: String,
    pub address: String,
    pub port: u16,
    pub account_address: Option<String>,
    pub capacity: u32,
    pub current_load: u32,
    pub status: String,
    pub shard_id: Option<String>,
}

/// Response with solver list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSolverListResponse {
    pub solvers: Vec<SolverListItem>,
}

/// Request to get list of validators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValidatorListRequest {
    /// Optional filter by status
    pub status_filter: Option<String>,
}

/// Validator info in list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorListItem {
    pub validator_id: String,
    pub address: String,
    pub port: u16,
    pub account_address: Option<String>,
    pub status: String,
}

/// Response with validator list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetValidatorListResponse {
    pub validators: Vec<ValidatorListItem>,
}

/// Request to get node status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNodeStatusRequest {
    pub node_id: String,
}

/// Response with node status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNodeStatusResponse {
    pub found: bool,
    pub node_id: String,
    pub node_type: Option<NodeType>,
    pub status: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub uptime_seconds: Option<u64>,
}

// ============================================
// Transfer Request/Response Types
// ============================================

/// Request to submit a transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransferRequest {
    /// Sender address
    pub from: String,
    /// Receiver address
    pub to: String,
    /// Amount to transfer (non-negative)
    pub amount: u64,
    /// Transfer type (flux, instant, etc.)
    pub transfer_type: String,
    /// Optional preferred solver
    pub preferred_solver: Option<String>,
    /// Optional shard assignment
    pub shard_id: Option<String>,
    /// Optional subnet ID for subnet-based routing
    pub subnet_id: Option<String>,
    /// Resources involved in this transfer
    pub resources: Vec<String>,
}

/// Response to transfer submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransferResponse {
    /// Whether submission was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Assigned transfer ID
    pub transfer_id: Option<String>,
    /// Assigned solver ID
    pub solver_id: Option<String>,
    /// Processing steps (for debugging/visualization)
    pub processing_steps: Vec<ProcessingStep>,
}

// ============================================
// Batch Transfer Request/Response Types
// ============================================

/// Request to submit multiple transfers in a batch
/// 
/// Use this for high-throughput scenarios (>100 TPS).
/// Batch processing reduces lock acquisitions from 5-6N to just 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransfersBatchRequest {
    /// List of transfers to submit
    pub transfers: Vec<SubmitTransferRequest>,
}

/// Result for a single transfer in batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTransferResult {
    /// Original transfer index in the batch
    pub index: usize,
    /// Whether this transfer was successful
    pub success: bool,
    /// Assigned transfer ID (if successful)
    pub transfer_id: Option<String>,
    /// Assigned solver ID (if successful)
    pub solver_id: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
}

/// Response to batch transfer submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransfersBatchResponse {
    /// Whether the batch submission was accepted
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Number of successfully submitted transfers
    pub submitted: usize,
    /// Number of failed transfers
    pub failed: usize,
    /// Results for each transfer in the batch
    pub results: Vec<BatchTransferResult>,
    /// Batch preparation statistics
    pub stats: BatchPrepareStatsResponse,
}

/// Batch preparation statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchPrepareStatsResponse {
    /// Total transfers in the batch
    pub total_transfers: usize,
    /// Unique (sender, subnet) pairs queried
    pub unique_sender_subnet_pairs: usize,
    /// Coins selected for transfers
    pub coins_selected: usize,
    /// Same-sender balance conflicts detected (overdraft prevention)
    pub same_sender_conflicts: usize,
}

/// A processing step in the transfer pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingStep {
    /// Step name
    pub step: String,
    /// Step status
    pub status: String,
    /// Additional details
    pub details: Option<String>,
    /// Timestamp
    pub timestamp: u64,
}

/// Request to get transfer status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTransferStatusRequest {
    /// Transfer ID to query
    pub transfer_id: String,
}

/// Response with transfer status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTransferStatusResponse {
    /// Whether transfer was found
    pub found: bool,
    /// Transfer ID
    pub transfer_id: String,
    /// Current status
    pub status: Option<String>,
    /// Assigned solver
    pub solver_id: Option<String>,
    /// Event ID (if completed)
    pub event_id: Option<String>,
    /// Processing steps
    pub processing_steps: Vec<ProcessingStep>,
}

// ============================================
// Common Types
// ============================================

/// Node type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    Solver,
    Validator,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeType::Solver => write!(f, "solver"),
            NodeType::Validator => write!(f, "validator"),
        }
    }
}

// ============================================
// Wrapper for all RPC messages
// ============================================

/// Wrapper enum for all RPC request types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcRequest {
    RegisterSolver(RegisterSolverRequest),
    RegisterValidator(RegisterValidatorRequest),
    Unregister(UnregisterRequest),
    Heartbeat(HeartbeatRequest),
    GetSolverList(GetSolverListRequest),
    GetValidatorList(GetValidatorListRequest),
    GetNodeStatus(GetNodeStatusRequest),
}

/// Wrapper enum for all RPC response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcResponse {
    RegisterSolver(RegisterSolverResponse),
    RegisterValidator(RegisterValidatorResponse),
    Unregister(UnregisterResponse),
    Heartbeat(HeartbeatResponse),
    GetSolverList(GetSolverListResponse),
    GetValidatorList(GetValidatorListResponse),
    GetNodeStatus(GetNodeStatusResponse),
    Error(String),
}

impl RpcRequest {
    /// Serialize request to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }
    
    /// Deserialize request from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

impl RpcResponse {
    /// Serialize response to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }
    
    /// Deserialize response from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}
