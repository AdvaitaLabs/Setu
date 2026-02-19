//! Merkle tree related types for consensus and state commitment.
//!
//! This module defines types used for:
//! - Object state values stored in Sparse Merkle Trees
//! - Anchor Merkle roots for consensus
//! - Subnet state aggregation
//!
//! # Design
//!
//! Based on mkt-3.md design document:
//! - Each subnet maintains its own Object State SMT
//! - All subnet roots are aggregated into a global state root
//! - Events are committed via Binary Merkle Tree
//! - Anchor chain uses append-only Binary Merkle Tree

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::subnet::SubnetId;

/// 32-byte hash value type alias
pub type HashValue = [u8; 32];

/// Zero hash constant
pub const ZERO_HASH: HashValue = [0u8; 32];

/// Object type tags for different object types in the SMT
pub mod object_type {
    /// Coin/Token object
    pub const COIN: u8 = 0;
    /// User profile object
    pub const PROFILE: u8 = 1;
    /// Credential/SBT object
    pub const CREDENTIAL: u8 = 2;
    /// Social relation graph object
    pub const RELATION_GRAPH: u8 = 3;

    // System objects (stored in ROOT subnet)
    /// Validator information
    pub const VALIDATOR_INFO: u8 = 10;
    /// Solver information
    pub const SOLVER_INFO: u8 = 11;
    /// Subnet configuration
    pub const SUBNET_CONFIG: u8 = 12;
    /// Global configuration
    pub const GLOBAL_CONFIG: u8 = 13;
    /// Staking position
    pub const STAKING_POSITION: u8 = 14;
    /// Cross-subnet lock record
    pub const CROSS_SUBNET_LOCK: u8 = 15;
    /// Bridge message
    pub const BRIDGE_MESSAGE: u8 = 16;
    /// Native token (SETU)
    pub const NATIVE_COIN: u8 = 17;
}

/// Object state value stored in SMT leaves.
///
/// This is the value stored in the Sparse Merkle Tree for each object.
/// The key is the ObjectId (32 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStateValue {
    /// Owner address (32 bytes)
    pub owner: [u8; 32],
    /// Object version number
    pub version: u64,
    /// Object type tag (see object_type module)
    pub type_tag: u8,
    /// Hash of the object's data
    pub data_hash: HashValue,
    /// Subnet this object belongs to
    pub subnet_id: SubnetId,
}

impl ObjectStateValue {
    /// Create a new object state value
    pub fn new(
        owner: [u8; 32],
        version: u64,
        type_tag: u8,
        data_hash: HashValue,
        subnet_id: SubnetId,
    ) -> Self {
        Self {
            owner,
            version,
            type_tag,
            data_hash,
            subnet_id,
        }
    }

    /// Create a system object (owned by the system)
    pub fn system_object(version: u64, type_tag: u8, data_hash: HashValue) -> Self {
        Self::new([0u8; 32], version, type_tag, data_hash, SubnetId::ROOT)
    }

    /// Compute the hash of this state value
    pub fn hash(&self) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(&self.owner);
        hasher.update(self.version.to_le_bytes());
        hasher.update([self.type_tag]);
        hasher.update(&self.data_hash);
        hasher.update(self.subnet_id.as_bytes());
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    /// Check if this is a system object
    pub fn is_system_object(&self) -> bool {
        self.type_tag >= object_type::VALIDATOR_INFO
    }

    /// Check if this object belongs to ROOT subnet
    pub fn is_root_object(&self) -> bool {
        self.subnet_id.is_root()
    }
}

impl Default for ObjectStateValue {
    fn default() -> Self {
        Self {
            owner: [0u8; 32],
            version: 0,
            type_tag: 0,
            data_hash: ZERO_HASH,
            subnet_id: SubnetId::ROOT,
        }
    }
}

/// Subnet state root with metadata.
///
/// Contains the root hash of a subnet's Object State SMT along with
/// metadata for verification and tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetStateRoot {
    /// Subnet identifier
    pub subnet_id: SubnetId,
    /// Root hash of the subnet's Object State SMT
    pub object_state_root: HashValue,
    /// Number of objects in this subnet
    pub object_count: u64,
    /// Last anchor where this subnet was updated
    pub last_updated_anchor: u64,
}

impl SubnetStateRoot {
    /// Create a new subnet state root
    pub fn new(
        subnet_id: SubnetId,
        object_state_root: HashValue,
        object_count: u64,
        last_updated_anchor: u64,
    ) -> Self {
        Self {
            subnet_id,
            object_state_root,
            object_count,
            last_updated_anchor,
        }
    }

    /// Create an empty subnet state root
    pub fn empty(subnet_id: SubnetId) -> Self {
        Self::new(subnet_id, ZERO_HASH, 0, 0)
    }

    /// Check if the subnet is empty
    pub fn is_empty(&self) -> bool {
        self.object_count == 0 && self.object_state_root == ZERO_HASH
    }
}

/// All Merkle roots for an Anchor.
///
/// This structure contains all the Merkle tree roots that are committed
/// in each anchor during DAG folding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AnchorMerkleRoots {
    /// Root of the Events Merkle Tree for this anchor
    pub events_root: HashValue,
    /// Root of the Subnet Aggregation Tree (global state)
    pub global_state_root: HashValue,
    /// Root of the Anchor Chain Tree (append-only history)
    pub anchor_chain_root: HashValue,
    /// Individual subnet state roots (for parallel verification)
    pub subnet_roots: HashMap<SubnetId, HashValue>,
}

impl AnchorMerkleRoots {
    /// Create empty Merkle roots
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with initial values
    pub fn with_roots(
        events_root: HashValue,
        global_state_root: HashValue,
        anchor_chain_root: HashValue,
    ) -> Self {
        Self {
            events_root,
            global_state_root,
            anchor_chain_root,
            subnet_roots: HashMap::new(),
        }
    }

    /// Check if the global state is empty
    pub fn is_empty(&self) -> bool {
        self.global_state_root == ZERO_HASH
    }

    /// Get a specific subnet's root
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<&HashValue> {
        self.subnet_roots.get(subnet_id)
    }

    /// Set a subnet's root
    pub fn set_subnet_root(&mut self, subnet_id: SubnetId, root: HashValue) {
        self.subnet_roots.insert(subnet_id, root);
    }

    /// Get ROOT subnet's state root
    pub fn root_subnet_root(&self) -> Option<&HashValue> {
        self.subnet_roots.get(&SubnetId::ROOT)
    }

    /// Compute a digest of all Merkle roots for signing
    pub fn digest(&self) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(&self.events_root);
        hasher.update(&self.global_state_root);
        hasher.update(&self.anchor_chain_root);

        // Include sorted subnet roots for determinism
        let mut subnet_ids: Vec<_> = self.subnet_roots.keys().collect();
        subnet_ids.sort();
        for subnet_id in subnet_ids {
            hasher.update(subnet_id.as_bytes());
            hasher.update(self.subnet_roots.get(subnet_id).unwrap());
        }

        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}

/// Execution result with Merkle proofs.
///
/// Extended execution result that includes state change information
/// for Merkle tree updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleExecutionResult {
    /// Event IDs processed
    pub event_ids: Vec<String>,
    /// Subnet this execution belongs to
    pub subnet_id: SubnetId,
    /// Objects read during execution (object_id, hash before)
    pub read_set: Vec<(HashValue, HashValue)>,
    /// Objects written during execution (object_id, hash after)
    pub write_set: Vec<(HashValue, HashValue)>,
    /// New subnet state root after execution
    pub subnet_state_root: HashValue,
    /// TEE attestation (None for ROOT subnet)
    pub tee_attestation: Option<Vec<u8>>,
    /// Whether execution was successful
    pub success: bool,
    /// Error message if failed
    pub error_message: Option<String>,
}

impl MerkleExecutionResult {
    /// Create a successful execution result
    pub fn success(
        event_ids: Vec<String>,
        subnet_id: SubnetId,
        read_set: Vec<(HashValue, HashValue)>,
        write_set: Vec<(HashValue, HashValue)>,
        subnet_state_root: HashValue,
    ) -> Self {
        Self {
            event_ids,
            subnet_id,
            read_set,
            write_set,
            subnet_state_root,
            tee_attestation: None,
            success: true,
            error_message: None,
        }
    }

    /// Create a failed execution result
    pub fn failure(event_ids: Vec<String>, subnet_id: SubnetId, error: String) -> Self {
        Self {
            event_ids,
            subnet_id,
            read_set: Vec::new(),
            write_set: Vec::new(),
            subnet_state_root: ZERO_HASH,
            tee_attestation: None,
            success: false,
            error_message: Some(error),
        }
    }

    /// Add TEE attestation
    pub fn with_attestation(mut self, attestation: Vec<u8>) -> Self {
        self.tee_attestation = Some(attestation);
        self
    }

    /// Check if this is a ROOT subnet execution
    pub fn is_root_execution(&self) -> bool {
        self.subnet_id.is_root()
    }

    /// Count the number of state changes
    pub fn change_count(&self) -> usize {
        self.write_set.len()
    }
}

/// Cross-subnet lock record for atomic cross-subnet operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSubnetLock {
    /// Unique lock ID
    pub lock_id: HashValue,
    /// Source subnet
    pub source_subnet: SubnetId,
    /// Target subnet
    pub target_subnet: SubnetId,
    /// Locked object IDs
    pub locked_objects: Vec<HashValue>,
    /// Lock expiry timestamp
    pub expiry: u64,
    /// Lock status
    pub status: CrossSubnetLockStatus,
    /// Creator of the lock
    pub creator: [u8; 32],
}

/// Status of a cross-subnet lock
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossSubnetLockStatus {
    /// Lock is active
    Active,
    /// Lock has been released (success)
    Released,
    /// Lock has expired
    Expired,
    /// Lock was cancelled
    Cancelled,
}

impl CrossSubnetLock {
    /// Create a new cross-subnet lock
    pub fn new(
        source_subnet: SubnetId,
        target_subnet: SubnetId,
        locked_objects: Vec<HashValue>,
        expiry: u64,
        creator: [u8; 32],
    ) -> Self {
        let lock_id =
            Self::compute_lock_id(&source_subnet, &target_subnet, &locked_objects, expiry);
        Self {
            lock_id,
            source_subnet,
            target_subnet,
            locked_objects,
            expiry,
            status: CrossSubnetLockStatus::Active,
            creator,
        }
    }

    fn compute_lock_id(
        source: &SubnetId,
        target: &SubnetId,
        objects: &[HashValue],
        expiry: u64,
    ) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(b"CROSS_SUBNET_LOCK:");
        hasher.update(source.as_bytes());
        hasher.update(target.as_bytes());
        for obj in objects {
            hasher.update(obj);
        }
        hasher.update(expiry.to_le_bytes());
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    /// Check if the lock is expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        current_time > self.expiry
    }

    /// Check if the lock is active
    pub fn is_active(&self) -> bool {
        self.status == CrossSubnetLockStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_state_value_hash() {
        let state =
            ObjectStateValue::new([1u8; 32], 1, object_type::COIN, [2u8; 32], SubnetId::ROOT);

        let hash = state.hash();
        assert_ne!(hash, ZERO_HASH);

        // Same state should produce same hash
        let state2 = state.clone();
        assert_eq!(state.hash(), state2.hash());

        // Different version should produce different hash
        let mut state3 = state.clone();
        state3.version = 2;
        assert_ne!(state.hash(), state3.hash());
    }

    #[test]
    fn test_anchor_merkle_roots() {
        let mut roots = AnchorMerkleRoots::new();
        assert!(roots.is_empty());

        roots.global_state_root = [1u8; 32];
        assert!(!roots.is_empty());

        roots.set_subnet_root(SubnetId::ROOT, [2u8; 32]);
        assert_eq!(roots.get_subnet_root(&SubnetId::ROOT), Some(&[2u8; 32]));

        let digest = roots.digest();
        assert_ne!(digest, ZERO_HASH);
    }

    #[test]
    fn test_subnet_state_root() {
        let root = SubnetStateRoot::empty(SubnetId::ROOT);
        assert!(root.is_empty());

        let root2 = SubnetStateRoot::new(SubnetId::ROOT, [1u8; 32], 10, 5);
        assert!(!root2.is_empty());
        assert_eq!(root2.object_count, 10);
    }

    #[test]
    fn test_cross_subnet_lock() {
        let source = SubnetId::ROOT;
        let target = SubnetId::from_str_id("app1");
        let objects = vec![[1u8; 32], [2u8; 32]];
        let expiry = 1000;

        let lock = CrossSubnetLock::new(source, target, objects, expiry, [0u8; 32]);

        assert!(lock.is_active());
        assert!(!lock.is_expired(500));
        assert!(lock.is_expired(1001));
    }
}
