//! Solver Task types for Validator → Solver communication.
//!
//! This module defines the data structures used when Validator sends
//! execution tasks to Solver nodes.
//!
//! ## Design Decisions
//!
//! - `coin_id` is in `ResolvedInputs`, not in `Event.transfer` (keeps Event generic)
//! - `ResolvedInputs` is required (not Option) for stateless execution
//! - Supports multiple objects via `Vec<ResolvedObject>` for future MergeCoins/SplitCoin

use super::gas::GasBudget;
use crate::{Event, ObjectId, SubnetId};
use serde::{Deserialize, Serialize};

/// Solver Task sent from Validator to Solver
///
/// Contains everything needed for TEE execution:
/// - Event to execute (account model semantic)
/// - Resolved inputs (object model references)
/// - Read set (actual object data with Merkle proofs)
/// - Gas budget
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverTask {
    /// Unique task ID (used for Attestation binding)
    /// This is a 32-byte hash for cryptographic binding
    pub task_id: [u8; 32],

    /// Event to execute
    pub event: Event,

    /// Validator-resolved object references (required)
    pub resolved_inputs: ResolvedInputs,

    /// Object data + Merkle proofs
    pub read_set: Vec<ReadSetEntry>,

    /// Pre-execution state root (for TEE verification)
    pub pre_state_root: [u8; 32],

    /// Target subnet
    pub subnet_id: SubnetId,

    /// Gas budget for execution
    pub gas_budget: GasBudget,
}

impl SolverTask {
    /// Create a new SolverTask
    pub fn new(
        task_id: [u8; 32],
        event: Event,
        resolved_inputs: ResolvedInputs,
        pre_state_root: [u8; 32],
        subnet_id: SubnetId,
    ) -> Self {
        Self {
            task_id,
            event,
            resolved_inputs,
            read_set: Vec::new(),
            pre_state_root,
            subnet_id,
            gas_budget: GasBudget::default(),
        }
    }

    /// Generate task_id from event and state context
    /// This creates a unique identifier for Attestation binding
    pub fn generate_task_id(event: &Event, pre_state_root: &[u8; 32]) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(event.id.as_bytes());
        hasher.update(pre_state_root);
        hasher.update(&event.timestamp.to_le_bytes());

        hasher.finalize().into()
    }

    /// Add read set entries
    pub fn with_read_set(mut self, read_set: Vec<ReadSetEntry>) -> Self {
        self.read_set = read_set;
        self
    }

    /// Set gas budget
    pub fn with_gas_budget(mut self, gas_budget: GasBudget) -> Self {
        self.gas_budget = gas_budget;
        self
    }
}

/// Resolved input object references
///
/// Contains the object IDs that Validator has resolved from account model.
/// This is required (not Option) because stateless TEE needs explicit object references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedInputs {
    /// Operation type with parameters
    pub operation: OperationType,

    /// Input objects (referenced by index in operation)
    pub input_objects: Vec<ResolvedObject>,
}

impl ResolvedInputs {
    /// Create empty ResolvedInputs (for testing or non-transfer events)
    pub fn new() -> Self {
        Self {
            operation: OperationType::NoOp,
            input_objects: Vec::new(),
        }
    }

    /// Create for a single-coin transfer
    pub fn transfer(coin: ResolvedObject, amount: u64) -> Self {
        Self {
            operation: OperationType::Transfer {
                from_coin_index: 0,
                amount,
            },
            input_objects: vec![coin],
        }
    }

    /// Create for merge coins (future)
    pub fn merge_coins(target: ResolvedObject, sources: Vec<ResolvedObject>) -> Self {
        let source_indices: Vec<usize> = (1..=sources.len()).collect();
        let mut objects = vec![target];
        objects.extend(sources);

        Self {
            operation: OperationType::MergeCoins {
                target_index: 0,
                source_indices,
            },
            input_objects: objects,
        }
    }

    /// Get primary coin object (for Transfer)
    pub fn primary_coin(&self) -> Option<&ResolvedObject> {
        match &self.operation {
            OperationType::NoOp => None,
            OperationType::Transfer {
                from_coin_index, ..
            } => self.input_objects.get(*from_coin_index),
            OperationType::MergeCoins { target_index, .. } => self.input_objects.get(*target_index),
            OperationType::SplitCoin { source_index, .. } => self.input_objects.get(*source_index),
        }
    }
}

impl Default for ResolvedInputs {
    fn default() -> Self {
        Self::new()
    }
}

/// Operation type (determines how to interpret input_objects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationType {
    /// No operation (for non-coin events like state registration)
    NoOp,

    /// Single coin transfer
    Transfer {
        /// Index of source coin in input_objects
        from_coin_index: usize,
        /// Amount to transfer
        amount: u64,
    },

    /// Merge multiple coins into one (future)
    MergeCoins {
        /// Index of target coin in input_objects
        target_index: usize,
        /// Indices of source coins to merge
        source_indices: Vec<usize>,
    },

    /// Split one coin into multiple (future)
    SplitCoin {
        /// Index of source coin in input_objects
        source_index: usize,
        /// Amounts for new coins
        amounts: Vec<u64>,
    },
}

/// Single resolved object reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedObject {
    /// Object ID
    pub object_id: ObjectId,

    /// Object type (e.g., "Coin", "NFT")
    pub object_type: String,

    /// Expected version (for optimistic locking)
    pub expected_version: u64,
}

impl ResolvedObject {
    /// Create a new resolved object reference
    pub fn new(object_id: ObjectId, object_type: impl Into<String>) -> Self {
        Self {
            object_id,
            object_type: object_type.into(),
            expected_version: 0,
        }
    }

    /// Create a Coin object reference
    pub fn coin(object_id: ObjectId) -> Self {
        Self::new(object_id, "Coin")
    }

    /// Set expected version
    pub fn with_version(mut self, version: u64) -> Self {
        self.expected_version = version;
        self
    }
}

/// An entry in the read set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadSetEntry {
    /// Object key (hashed to get SMT key)
    pub key: String,
    /// Current value (serialized object data)
    pub value: Vec<u8>,
    /// Merkle proof for this object (optional, for verification)
    pub proof: Option<Vec<u8>>,
}

impl ReadSetEntry {
    pub fn new(key: String, value: Vec<u8>) -> Self {
        Self {
            key,
            value,
            proof: None,
        }
    }

    pub fn with_proof(mut self, proof: Vec<u8>) -> Self {
        self.proof = Some(proof);
        self
    }
}

/// Merkle proof for read set verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    /// Sibling hashes on the path to root
    pub siblings: Vec<[u8; 32]>,

    /// Path direction bits (true = right, false = left)
    pub path_bits: Vec<bool>,

    /// Leaf index (optional, for verification)
    pub leaf_index: Option<u64>,
}

impl MerkleProof {
    /// Create a new Merkle proof
    pub fn new(siblings: Vec<[u8; 32]>, path_bits: Vec<bool>) -> Self {
        Self {
            siblings,
            path_bits,
            leaf_index: None,
        }
    }

    /// Create an empty proof (for MVP without actual verification)
    pub fn empty() -> Self {
        Self {
            siblings: Vec::new(),
            path_bits: Vec::new(),
            leaf_index: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_inputs_transfer() {
        let coin = ResolvedObject::coin(ObjectId::random()).with_version(1);
        let resolved = ResolvedInputs::transfer(coin.clone(), 100);

        assert!(matches!(
            resolved.operation,
            OperationType::Transfer { amount: 100, .. }
        ));
        assert_eq!(resolved.input_objects.len(), 1);
        assert_eq!(resolved.primary_coin().unwrap().object_id, coin.object_id);
    }

    #[test]
    fn test_gas_budget_default() {
        let budget = GasBudget::default();
        assert_eq!(budget.max_gas_units, u64::MAX);
        assert_eq!(budget.estimated_fee, 0);
    }
}
