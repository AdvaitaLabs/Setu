//! Registration types for validators, solvers, subnets, and users
//!
//! This module contains all registration-related data structures
//! used in the EventPayload.

use serde::{Deserialize, Serialize};

// Re-use SubnetType from subnet module (single source of truth)
pub use crate::subnet::SubnetType;

// ========== Validator Registration ==========

/// Validator registration data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRegistration {
    // ========== Identity Layer ==========
    /// Unique validator identifier (for logging, routing)
    pub validator_id: String,
    
    // ========== Network Layer ==========
    /// Network address (IP or hostname) for P2P communication
    pub network_address: String,
    /// Network port for P2P communication
    pub network_port: u16,
    
    // ========== Economic Layer ==========
    /// Ethereum-style account address (0x...) for staking and rewards
    pub account_address: String,
    /// Public key (secp256k1, 65 bytes uncompressed)
    pub public_key: Vec<u8>,
    /// Registration signature proving ownership of the account
    pub signature: Vec<u8>,
    
    // ========== Economic Parameters ==========
    /// Stake amount in Flux (minimum required for validator)
    pub stake_amount: u64,
    /// Commission rate (0-100, percentage)
    pub commission_rate: u8,
}

impl ValidatorRegistration {
    /// Create a new validator registration
    pub fn new(
        validator_id: impl Into<String>,
        network_address: impl Into<String>,
        network_port: u16,
        account_address: impl Into<String>,
        public_key: Vec<u8>,
        signature: Vec<u8>,
        stake_amount: u64,
    ) -> Self {
        Self {
            validator_id: validator_id.into(),
            network_address: network_address.into(),
            network_port,
            account_address: account_address.into(),
            public_key,
            signature,
            stake_amount,
            commission_rate: 10, // Default 10%
        }
    }
    
    pub fn with_commission_rate(mut self, rate: u8) -> Self {
        self.commission_rate = rate.min(100); // Cap at 100%
        self
    }
    
    /// Verify the registration signature
    pub fn verify_signature(&self) -> bool {
        // TODO: Implement actual ECDSA signature verification
        // Message format: "Register Validator: {validator_id}:{account_address}:{timestamp}"
        !self.signature.is_empty() && !self.public_key.is_empty()
    }
}

// ========== Solver Registration ==========

/// Solver registration data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverRegistration {
    // ========== Identity Layer ==========
    /// Unique solver identifier (for logging, routing)
    pub solver_id: String,
    
    // ========== Network Layer ==========
    /// Network address (IP or hostname) for P2P communication
    pub network_address: String,
    /// Network port for P2P communication
    pub network_port: u16,
    
    // ========== Economic Layer ==========
    /// Ethereum-style account address (0x...) for receiving task fees
    pub account_address: String,
    /// Public key (secp256k1, 65 bytes uncompressed)
    pub public_key: Vec<u8>,
    /// Registration signature proving ownership of the account
    pub signature: Vec<u8>,
    
    // ========== Capability Parameters ==========
    /// Maximum concurrent tasks capacity
    pub capacity: u32,
    /// Optional shard assignment
    pub shard_id: Option<String>,
    /// Resource types this solver can handle
    pub resources: Vec<String>,
}

impl SolverRegistration {
    /// Create a new solver registration
    pub fn new(
        solver_id: impl Into<String>,
        network_address: impl Into<String>,
        network_port: u16,
        account_address: impl Into<String>,
        public_key: Vec<u8>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            solver_id: solver_id.into(),
            network_address: network_address.into(),
            network_port,
            account_address: account_address.into(),
            public_key,
            signature,
            capacity: 100,
            shard_id: None,
            resources: vec![],
        }
    }
    
    pub fn with_capacity(mut self, capacity: u32) -> Self {
        self.capacity = capacity;
        self
    }
    
    pub fn with_shard(mut self, shard_id: impl Into<String>) -> Self {
        self.shard_id = Some(shard_id.into());
        self
    }
    
    pub fn with_resources(mut self, resources: Vec<String>) -> Self {
        self.resources = resources;
        self
    }
    
    /// Verify the registration signature
    pub fn verify_signature(&self) -> bool {
        // TODO: Implement actual ECDSA signature verification
        // Message format: "Register Solver: {solver_id}:{account_address}:{timestamp}"
        !self.signature.is_empty() && !self.public_key.is_empty()
    }
}

// ========== Unregistration ==========

/// Node type for unregistration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    Validator,
    Solver,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeType::Validator => write!(f, "validator"),
            NodeType::Solver => write!(f, "solver"),
        }
    }
}

/// Unregistration data (for both Validator and Solver)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unregistration {
    /// Node identifier to unregister
    pub node_id: String,
    /// Type of node
    pub node_type: NodeType,
}

impl Unregistration {
    pub fn validator(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            node_type: NodeType::Validator,
        }
    }
    
    pub fn solver(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            node_type: NodeType::Solver,
        }
    }
    
    pub fn is_validator(&self) -> bool {
        self.node_type == NodeType::Validator
    }
    
    pub fn is_solver(&self) -> bool {
        self.node_type == NodeType::Solver
    }
}

// ========== Subnet Registration ==========

/// Subnet resource limits
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetResourceLimits {
    /// Maximum transactions per second
    pub max_tps: Option<u64>,
    /// Maximum storage in bytes
    pub max_storage_bytes: Option<u64>,
    /// Maximum compute units per transaction
    pub max_compute_units: Option<u64>,
}

impl Default for SubnetResourceLimits {
    fn default() -> Self {
        Self {
            max_tps: None,
            max_storage_bytes: None,
            max_compute_units: None,
        }
    }
}

impl SubnetResourceLimits {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn with_tps(mut self, tps: u64) -> Self {
        self.max_tps = Some(tps);
        self
    }
    
    pub fn with_storage(mut self, bytes: u64) -> Self {
        self.max_storage_bytes = Some(bytes);
        self
    }
    
    pub fn with_compute(mut self, units: u64) -> Self {
        self.max_compute_units = Some(units);
        self
    }
}

/// Subnet registration data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetRegistration {
    /// Unique subnet identifier (will be assigned if not provided)
    pub subnet_id: String,
    /// Human-readable name for the subnet
    pub name: String,
    /// Description of the subnet's purpose
    pub description: Option<String>,
    /// Owner/creator of the subnet
    pub owner: String,
    /// Maximum number of users allowed (None = unlimited)
    pub max_users: Option<u64>,
    /// Resource limits for the subnet
    pub resource_limits: Option<SubnetResourceLimits>,
    /// List of Solver IDs assigned to this subnet
    pub assigned_solvers: Vec<String>,
    /// Subnet type (application, organization, etc.)
    pub subnet_type: SubnetType,
    /// Parent subnet ID (None = root subnet)
    pub parent_subnet_id: Option<String>,
}

impl SubnetRegistration {
    pub fn new(subnet_id: impl Into<String>, name: impl Into<String>, owner: impl Into<String>) -> Self {
        Self {
            subnet_id: subnet_id.into(),
            name: name.into(),
            description: None,
            owner: owner.into(),
            max_users: None,
            resource_limits: None,
            assigned_solvers: vec![],
            subnet_type: SubnetType::App,
            parent_subnet_id: None,
        }
    }
    
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
    
    pub fn with_max_users(mut self, max: u64) -> Self {
        self.max_users = Some(max);
        self
    }
    
    pub fn with_limits(mut self, limits: SubnetResourceLimits) -> Self {
        self.resource_limits = Some(limits);
        self
    }
    
    pub fn with_solvers(mut self, solvers: Vec<String>) -> Self {
        self.assigned_solvers = solvers;
        self
    }
    
    pub fn with_type(mut self, subnet_type: SubnetType) -> Self {
        self.subnet_type = subnet_type;
        self
    }
    
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_subnet_id = Some(parent_id.into());
        self
    }
}

// ========== User Registration ==========

/// User registration data
/// 
/// Supports two registration methods:
/// 1. MetaMask (Ethereum wallet): address + ECDSA signature
/// 2. Nostr: nostr_pubkey + Schnorr signature (address derived from pubkey)
/// 
/// The middle layer (Nostr Relay adapter) converts events to registration requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRegistration {
    /// User's Ethereum-style address (0x...)
    /// - For MetaMask: directly from wallet
    /// - For Nostr: derived from nostr_pubkey via Keccak256
    pub address: String,
    
    /// Optional: Nostr public key (32 bytes, Schnorr x-only public key)
    /// Only present for Nostr-based registrations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nostr_pubkey: Option<Vec<u8>>,
    
    /// Signature proving ownership
    /// - For MetaMask: ECDSA signature (65 bytes)
    /// - For Nostr: Schnorr signature (64 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Vec<u8>>,
    
    /// Signed message (for verification)
    /// Format: "Register to Setu: {timestamp}"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    
    /// Timestamp (for replay attack prevention)
    pub timestamp: u64,
    
    /// Subnet ID to register in (None = root subnet)
    pub subnet_id: Option<String>,
    
    /// Optional display name
    pub display_name: Option<String>,
    
    /// Optional metadata (JSON string)
    pub metadata: Option<String>,
    
    /// Inviter's address (resolved from invite code by middle layer)
    pub invited_by: Option<String>,
    
    /// Invite code used for registration
    pub invite_code: Option<String>,
}

impl UserRegistration {
    /// Create a new user registration from MetaMask (Ethereum wallet)
    pub fn from_metamask(
        address: impl Into<String>,
        timestamp: u64,
    ) -> Self {
        Self {
            address: address.into(),
            nostr_pubkey: None,
            signature: None,
            message: None,
            timestamp,
            subnet_id: None,
            display_name: None,
            metadata: None,
            invited_by: None,
            invite_code: None,
        }
    }
    
    /// Create a new user registration from Nostr credentials
    pub fn from_nostr(
        address: impl Into<String>,
        nostr_pubkey: Vec<u8>,
        signature: Vec<u8>,
        timestamp: u64,
    ) -> Self {
        Self {
            address: address.into(),
            nostr_pubkey: Some(nostr_pubkey),
            signature: Some(signature),
            message: None,
            timestamp,
            subnet_id: None,
            display_name: None,
            metadata: None,
            invited_by: None,
            invite_code: None,
        }
    }
    
    pub fn with_signature(mut self, signature: Vec<u8>) -> Self {
        self.signature = Some(signature);
        self
    }
    
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
    
    pub fn with_subnet(mut self, subnet_id: impl Into<String>) -> Self {
        self.subnet_id = Some(subnet_id.into());
        self
    }
    
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }
    
    pub fn with_invite(mut self, inviter: impl Into<String>, code: impl Into<String>) -> Self {
        self.invited_by = Some(inviter.into());
        self.invite_code = Some(code.into());
        self
    }
    
    pub fn with_metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }
    
    /// Get the subnet this user is registering in (defaults to root)
    pub fn get_subnet(&self) -> &str {
        self.subnet_id.as_deref().unwrap_or("subnet-0")
    }
    
    /// Check if this is a Nostr-based registration
    pub fn is_nostr(&self) -> bool {
        self.nostr_pubkey.is_some()
    }
    
    /// Check if this is a MetaMask-based registration
    pub fn is_metamask(&self) -> bool {
        self.nostr_pubkey.is_none()
    }
    
    /// Verify the signature and address derivation
    pub fn verify(&self) -> bool {
        if let Some(nostr_pubkey) = &self.nostr_pubkey {
            // Nostr registration: verify Schnorr signature and address derivation
            // TODO: Implement actual Schnorr signature verification
            // TODO: Verify address derivation from nostr_pubkey
            !nostr_pubkey.is_empty() && nostr_pubkey.len() == 32
        } else {
            // MetaMask registration: verify ECDSA signature
            // TODO: Implement actual ECDSA signature verification
            // For now, just check address format
            self.address.starts_with("0x") && self.address.len() == 42
        }
    }
}

// ========== Operation Payloads ==========

/// Power consumption data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerConsumption {
    /// User consuming power
    pub user_id: String,
    /// Amount of power consumed
    pub amount: u64,
    /// Reason for consumption
    pub reason: String,
}

impl PowerConsumption {
    pub fn new(user_id: impl Into<String>, amount: u64, reason: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            amount,
            reason: reason.into(),
        }
    }
}

/// Task submission data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSubmission {
    /// Unique task identifier
    pub task_id: String,
    /// Type of task
    pub task_type: String,
    /// User submitting the task
    pub submitter: String,
    /// Task payload (serialized data)
    pub payload: Vec<u8>,
}

impl TaskSubmission {
    pub fn new(task_id: impl Into<String>, task_type: impl Into<String>, submitter: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            task_type: task_type.into(),
            submitter: submitter.into(),
            payload: vec![],
        }
    }
    
    pub fn with_payload(mut self, payload: Vec<u8>) -> Self {
        self.payload = payload;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_validator_registration() {
        let reg = ValidatorRegistration::new(
            "v1",
            "127.0.0.1",
            8080,
            "0xabcd1234",
            vec![1, 2, 3],
            vec![4, 5, 6],
            10000,
        ).with_commission_rate(15);
        assert_eq!(reg.validator_id, "v1");
        assert_eq!(reg.account_address, "0xabcd1234");
        assert_eq!(reg.stake_amount, 10000);
        assert_eq!(reg.commission_rate, 15);
    }
    
    #[test]
    fn test_solver_registration() {
        let reg = SolverRegistration::new(
            "s1",
            "127.0.0.1",
            9000,
            "0xef123456",
            vec![1, 2, 3],
            vec![4, 5, 6],
        )
            .with_capacity(50)
            .with_shard("shard-0");
        assert_eq!(reg.solver_id, "s1");
        assert_eq!(reg.account_address, "0xef123456");
        assert_eq!(reg.capacity, 50);
        assert_eq!(reg.shard_id, Some("shard-0".to_string()));
    }
    
    #[test]
    fn test_unregistration() {
        let unreg = Unregistration::validator("v1");
        assert!(unreg.is_validator());
        assert!(!unreg.is_solver());
    }
    
    #[test]
    fn test_subnet_registration() {
        let reg = SubnetRegistration::new("subnet-1", "My App", "alice")
            .with_type(SubnetType::App)
            .with_max_users(1000);
        assert_eq!(reg.subnet_id, "subnet-1");
        assert_eq!(reg.subnet_type, SubnetType::App);
    }
    
    #[test]
    fn test_user_registration_metamask() {
        let reg = UserRegistration::from_metamask("0x1234abcd", 1234567890)
            .with_subnet("subnet-1")
            .with_display_name("Alice")
            .with_signature(vec![1, 2, 3])
            .with_message("Register to Setu: 1234567890");
        assert_eq!(reg.address, "0x1234abcd");
        assert_eq!(reg.get_subnet(), "subnet-1");
        assert!(reg.is_metamask());
        assert!(!reg.is_nostr());
        assert!(reg.verify());
    }
    
    #[test]
    fn test_user_registration_nostr() {
        let nostr_pubkey = vec![1; 32]; // 32 bytes Nostr pubkey
        let reg = UserRegistration::from_nostr("0x1234abcd", nostr_pubkey, vec![4, 5, 6], 1234567890)
            .with_subnet("subnet-1")
            .with_display_name("Alice");
        assert_eq!(reg.address, "0x1234abcd");
        assert_eq!(reg.get_subnet(), "subnet-1");
        assert!(reg.is_nostr());
        assert!(!reg.is_metamask());
        assert!(reg.verify());
    }
    
    #[test]
    fn test_user_default_subnet() {
        let reg = UserRegistration::from_metamask("0xabcd", 1234567890);
        assert_eq!(reg.get_subnet(), "subnet-0");
    }
}
