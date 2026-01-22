// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Network-level node information types
//!
//! This module defines network-specific node information types that are
//! independent of the consensus layer. These types contain only the
//! information needed for network operations (addressing, connection, etc.).

use serde::{Deserialize, Serialize};

/// Role of a node in the network
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    /// Validator node - participates in consensus
    Validator,
    /// Solver node - processes transactions
    Solver,
    /// Light node - only syncs headers
    LightNode,
}

/// Status of a node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// Node is initializing
    Initializing,
    /// Node is syncing with the network
    Syncing,
    /// Node is active and participating
    Active,
    /// Node is temporarily inactive
    Inactive,
    /// Node is disconnected
    Disconnected,
}

/// Network-level node information
///
/// Contains the essential information needed for network operations:
/// - Identity (id)
/// - Addressing (address, port)
/// - Role and status
///
/// This type is intentionally minimal and does not include consensus-specific
/// fields like stake amounts or TEE configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique identifier for the node
    pub id: String,
    /// Node's role in the network
    pub role: NodeRole,
    /// Current status
    pub status: NodeStatus,
    /// IP address or hostname
    pub address: String,
    /// Port number
    pub port: u16,
    /// Optional public key for authentication
    pub public_key: Option<Vec<u8>>,
}

impl NodeInfo {
    /// Create a new NodeInfo with the given parameters
    pub fn new(id: String, role: NodeRole, address: String, port: u16) -> Self {
        Self {
            id,
            role,
            status: NodeStatus::Initializing,
            address,
            port,
            public_key: None,
        }
    }

    /// Create a new validator NodeInfo
    pub fn new_validator(id: String, address: String, port: u16) -> Self {
        Self::new(id, NodeRole::Validator, address, port)
    }

    /// Create a new solver NodeInfo
    pub fn new_solver(id: String, address: String, port: u16) -> Self {
        Self::new(id, NodeRole::Solver, address, port)
    }

    /// Create a new light node NodeInfo
    pub fn new_light_node(id: String, address: String, port: u16) -> Self {
        Self::new(id, NodeRole::LightNode, address, port)
    }

    /// Get the endpoint address (address:port)
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }

    /// Check if this is a validator node
    pub fn is_validator(&self) -> bool {
        self.role == NodeRole::Validator
    }

    /// Check if this is a solver node
    pub fn is_solver(&self) -> bool {
        self.role == NodeRole::Solver
    }

    /// Check if the node is active
    pub fn is_active(&self) -> bool {
        self.status == NodeStatus::Active
    }

    /// Set the node status
    pub fn set_status(&mut self, status: NodeStatus) {
        self.status = status;
    }

    /// Set the public key
    pub fn with_public_key(mut self, public_key: Vec<u8>) -> Self {
        self.public_key = Some(public_key);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_validator() {
        let info = NodeInfo::new_validator(
            "validator-1".to_string(),
            "127.0.0.1".to_string(),
            8080,
        );
        assert!(info.is_validator());
        assert_eq!(info.endpoint(), "127.0.0.1:8080");
    }

    #[test]
    fn test_new_solver() {
        let info = NodeInfo::new_solver(
            "solver-1".to_string(),
            "192.168.1.1".to_string(),
            9090,
        );
        assert!(info.is_solver());
        assert!(!info.is_validator());
    }

    #[test]
    fn test_status() {
        let mut info = NodeInfo::new_validator(
            "test".to_string(),
            "localhost".to_string(),
            8080,
        );
        assert!(!info.is_active());
        
        info.set_status(NodeStatus::Active);
        assert!(info.is_active());
    }
}
