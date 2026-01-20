//! Transfer types for routing and processing
//!
//! This module contains transfer-related types that were previously
//! scattered across core-types and event.rs.

use serde::{Deserialize, Serialize};

// Re-export VLC from setu-vlc (single source of truth)
pub use setu_vlc::VectorClock;

// ========== Type Aliases ==========

/// Unique identifier for a transfer.
pub type TransferId = String;

/// Logical clock key (e.g., node id).
pub type ClockKey = String;

/// Object or resource key used for routing and conflict detection.
pub type ResourceKey = String;

// ========== Transfer Types ==========

/// High-level transfer kind (semantic label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransferType {
    /// Flux token transfer between accounts
    FluxTransfer,
    /// Power consumption for computation
    PowerConsume,
    /// Task submission to solvers
    TaskSubmit,
}

impl Default for TransferType {
    fn default() -> Self {
        TransferType::FluxTransfer
    }
}

/// VLC assigned by Validator to a Transfer.
///
/// This is passed to Solver and should be used in the resulting Event,
/// ensuring consistent ordering across the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignedVlc {
    /// The logical time assigned by Validator
    pub logical_time: u64,
    /// The physical time when assigned (milliseconds since epoch)
    pub physical_time: u64,
    /// The Validator ID that assigned this VLC
    pub validator_id: String,
}

/// Complete transfer representation for routing and processing.
///
/// A Transfer is the user-facing transaction request that gets
/// routed to a Solver for execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// Unique transfer identifier
    pub id: TransferId,
    
    /// Logical sender (owner) of the transfer
    pub from: String,
    
    /// Logical recipient; may be empty for some system transfers
    pub to: String,
    
    /// Transfer amount in smallest unit (e.g., 1 Flux = 10^18)
    pub amount: u64,
    
    /// Application-level classification of this transfer
    pub transfer_type: TransferType,
    
    /// Resources affected by this transfer (for conflict detection)
    pub resources: Vec<ResourceKey>,
    
    /// Power/work score used for tie-breaks in ordering
    pub power: u64,
    
    // ========== Routing Hints ==========
    
    /// Optional: Preferred solver ID for manual routing
    pub preferred_solver: Option<String>,
    
    /// Optional: Shard ID for shard-based routing
    pub shard_id: Option<String>,
    
    /// Optional: Subnet ID for subnet-based routing
    pub subnet_id: Option<String>,
    
    // ========== VLC Fields ==========
    
    /// VLC assigned by Validator when receiving the transfer.
    /// Solver should use this VLC when creating Event, NOT generate its own.
    pub assigned_vlc: Option<AssignedVlc>,
}

impl Transfer {
    /// Create a new transfer with minimal required fields
    pub fn new(id: impl Into<String>, from: impl Into<String>, to: impl Into<String>, amount: u64) -> Self {
        Self {
            id: id.into(),
            from: from.into(),
            to: to.into(),
            amount,
            transfer_type: TransferType::FluxTransfer,
            resources: vec![],
            power: 0,
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
            assigned_vlc: None,
        }
    }
    
    /// Set transfer type
    pub fn with_type(mut self, transfer_type: TransferType) -> Self {
        self.transfer_type = transfer_type;
        self
    }
    
    /// Set resources for conflict detection
    pub fn with_resources(mut self, resources: Vec<ResourceKey>) -> Self {
        self.resources = resources;
        self
    }
    
    /// Set power score
    pub fn with_power(mut self, power: u64) -> Self {
        self.power = power;
        self
    }
    
    /// Set preferred solver
    pub fn with_preferred_solver(mut self, solver_id: impl Into<String>) -> Self {
        self.preferred_solver = Some(solver_id.into());
        self
    }
    
    /// Set preferred solver (Option variant)
    pub fn with_preferred_solver_opt(mut self, solver_id: Option<String>) -> Self {
        self.preferred_solver = solver_id;
        self
    }
    
    /// Set shard ID
    pub fn with_shard_id(mut self, shard_id: Option<String>) -> Self {
        self.shard_id = shard_id;
        self
    }
    
    /// Set subnet ID
    pub fn with_subnet(mut self, subnet_id: impl Into<String>) -> Self {
        self.subnet_id = Some(subnet_id.into());
        self
    }
    
    /// Set subnet ID (Option variant)
    pub fn with_subnet_id(mut self, subnet_id: Option<String>) -> Self {
        self.subnet_id = subnet_id;
        self
    }
    
    /// Set assigned VLC from validator
    pub fn with_assigned_vlc(mut self, vlc: AssignedVlc) -> Self {
        self.assigned_vlc = Some(vlc);
        self
    }
    
    /// Get affected account resources
    pub fn affected_accounts(&self) -> Vec<String> {
        vec![
            format!("account:{}", self.from),
            format!("account:{}", self.to),
        ]
    }
}

impl Default for Transfer {
    fn default() -> Self {
        Self {
            id: String::new(),
            from: String::new(),
            to: String::new(),
            amount: 0,
            transfer_type: TransferType::default(),
            resources: vec![],
            power: 0,
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
            assigned_vlc: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_transfer_creation() {
        let transfer = Transfer::new("tx-1", "alice", "bob", 1000);
        assert_eq!(transfer.id, "tx-1");
        assert_eq!(transfer.from, "alice");
        assert_eq!(transfer.to, "bob");
        assert_eq!(transfer.amount, 1000);
        assert_eq!(transfer.transfer_type, TransferType::FluxTransfer);
    }
    
    #[test]
    fn test_transfer_builder() {
        let transfer = Transfer::new("tx-2", "alice", "bob", 500)
            .with_type(TransferType::PowerConsume)
            .with_power(10)
            .with_subnet("subnet-1");
        
        assert_eq!(transfer.transfer_type, TransferType::PowerConsume);
        assert_eq!(transfer.power, 10);
        assert_eq!(transfer.subnet_id, Some("subnet-1".to_string()));
    }
    
    #[test]
    fn test_affected_accounts() {
        let transfer = Transfer::new("tx-1", "alice", "bob", 100);
        let accounts = transfer.affected_accounts();
        assert!(accounts.contains(&"account:alice".to_string()));
        assert!(accounts.contains(&"account:bob".to_string()));
    }
}
