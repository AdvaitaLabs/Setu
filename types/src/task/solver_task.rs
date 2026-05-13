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

use serde::{Deserialize, Serialize};
use crate::{Event, ObjectId, SubnetId};
use crate::dynamic_field::DfAccessMode;
use super::gas::GasBudget;

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

    /// Move module bytecode needed for MoveCall execution.
    /// Keys: "mod:{hex_addr}::{name}", values: raw bytecode.
    /// Empty for non-MoveCall operations.
    #[serde(default)]
    pub module_read_set: Vec<ReadSetEntry>,
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
            module_read_set: Vec::new(),
        }
    }
    
    /// Generate task_id from event and state context
    /// This creates a unique identifier for Attestation binding
    pub fn generate_task_id(event: &Event, pre_state_root: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_TASK_ID:");
        hasher.update(event.id.as_bytes());
        hasher.update(pre_state_root);
        hasher.update(&event.timestamp.to_le_bytes());
        
        *hasher.finalize().as_bytes()
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

    /// Resolved dynamic field accesses (DF FDP v1.3 — top-level field).
    ///
    /// Populated by `TaskPreparer` when the client declares
    /// `MoveCallPayload.dynamic_field_accesses`. Kept top-level (rather than
    /// nested inside `OperationType::MoveCall`) so other operations can adopt
    /// DF later without a breaking enum layout change, and `#[serde(default)]`
    /// preserves compatibility with payloads produced before M1.
    #[serde(default)]
    pub dynamic_fields: Vec<ResolvedDynamicField>,
}

impl ResolvedInputs {
    /// Create empty ResolvedInputs (for testing or non-transfer events)
    pub fn new() -> Self {
        Self {
            operation: OperationType::NoOp,
            input_objects: Vec::new(),
            dynamic_fields: Vec::new(),
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
            dynamic_fields: Vec::new(),
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
            dynamic_fields: Vec::new(),
        }
    }
    
    /// Create for split coin
    pub fn split_coin(source: ResolvedObject, amounts: Vec<u64>) -> Self {
        Self {
            operation: OperationType::SplitCoin {
                source_index: 0,
                amounts,
            },
            input_objects: vec![source],
            dynamic_fields: Vec::new(),
        }
    }
    
    /// Create for atomic merge-then-transfer
    pub fn merge_then_transfer(
        target: ResolvedObject,
        sources: Vec<ResolvedObject>,
        recipient: crate::object::Address,
        amount: u64,
    ) -> Self {
        let source_indices: Vec<usize> = (1..=sources.len()).collect();
        let mut objects = vec![target];
        objects.extend(sources);
        
        Self {
            operation: OperationType::MergeThenTransfer {
                target_index: 0,
                source_indices,
                recipient,
                amount,
            },
            input_objects: objects,
            dynamic_fields: Vec::new(),
        }
    }

    /// Create for Move function call
    pub fn move_call(
        package: String,
        module_name: String,
        function_name: String,
        type_args: Vec<String>,
        pure_args: Vec<Vec<u8>>,
        input_objects: Vec<ResolvedObject>,
        mutable_indices: Vec<usize>,
        consumed_indices: Vec<usize>,
    ) -> Self {
        Self {
            operation: OperationType::MoveCall {
                package,
                module_name,
                function_name,
                type_args,
                pure_args,
                mutable_indices,
                consumed_indices,
            },
            input_objects,
            dynamic_fields: Vec::new(),
        }
    }
    
    /// Get primary coin object (for Transfer)
    pub fn primary_coin(&self) -> Option<&ResolvedObject> {
        match &self.operation {
            OperationType::NoOp => None,
            OperationType::Transfer { from_coin_index, .. } => {
                self.input_objects.get(*from_coin_index)
            }
            OperationType::MergeCoins { target_index, .. } => {
                self.input_objects.get(*target_index)
            }
            OperationType::SplitCoin { source_index, .. } => {
                self.input_objects.get(*source_index)
            }
            OperationType::MergeThenTransfer { target_index, .. } => {
                self.input_objects.get(*target_index)
            }
            OperationType::MoveCall { .. } => None,
            OperationType::MovePublish { .. } => None,
            OperationType::ProgrammableTransaction(_) => None,
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
    
    /// Merge multiple coins into one
    MergeCoins {
        /// Index of target coin in input_objects
        target_index: usize,
        /// Indices of source coins to merge
        source_indices: Vec<usize>,
    },
    
    /// Split one coin into multiple
    SplitCoin {
        /// Index of source coin in input_objects
        source_index: usize,
        /// Amounts for new coins
        amounts: Vec<u64>,
    },
    
    /// Atomic merge-then-transfer: merge sources into target, then partial transfer
    MergeThenTransfer {
        /// Index of target coin in input_objects
        target_index: usize,
        /// Indices of source coins to merge
        source_indices: Vec<usize>,
        /// Recipient address
        recipient: crate::object::Address,
        /// Amount to transfer after merge
        amount: u64,
    },

    /// Move function call
    MoveCall {
        /// Module address (hex)
        package: String,
        /// Module name
        module_name: String,
        /// Function name
        function_name: String,
        /// Type arguments (Move TypeTag string representation)
        type_args: Vec<String>,
        /// Pure arguments (BCS serialized) — no object references
        pure_args: Vec<Vec<u8>>,
        /// Indices into input_objects that are mutable references (&mut T)
        mutable_indices: Vec<usize>,
        /// Indices into input_objects that are consumed (by value T)
        consumed_indices: Vec<usize>,
    },

    /// Move module publish (Phase 0-4: goes through ROOT/RootSubnetExecutor, not TEE)
    MovePublish {
        /// Module bytecode list
        modules: Vec<Vec<u8>>,
    },

    /// Programmable Transaction Block (PTB).
    ///
    /// Carries a fully-validated [`ProgrammableTransaction`](crate::ptb::ProgrammableTransaction)
    /// from the wire down to the executor. The dispatch chain is:
    ///   service.rs `submit_move_ptb` → `MovePtbHandler` →
    ///   `TaskPreparer::prepare_move_ptb_task` → `HybridExecutor` →
    ///   `engine.execute_ptb`. See
    ///   `docs/feat/move-vm-phase9-ptb-event-wire/design.md`.
    ///
    /// New tail variant — appended for BCS-additive forward compatibility.
    /// Existing serialized `OperationType` values (Transfer/MergeCoins/...)
    /// remain byte-identical.
    ProgrammableTransaction(crate::ptb::ProgrammableTransaction),
}

/// Resolved dynamic field access (DF FDP v1.3 — consumed by Enclave/VM via
/// `SetuObjectRuntime.df_cache`).
///
/// Produced by `TaskPreparer` from a client-declared `DynamicFieldAccess`:
/// - `df_object_id` is `derive_df_oid(parent, name_type_tag, name_bcs)`.
/// - `value_bytes` is `None` for `DfAccessMode::Create` (nothing exists yet)
///   and `Some(bcs_data)` for Read/Mutate/Delete (decoded from the on-disk
///   `ObjectEnvelope.data` segment).
/// - `name_type_tag` / `value_type_tag` are canonical Move type tag strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedDynamicField {
    pub parent_object_id: ObjectId,
    pub df_object_id: ObjectId,
    pub name_type_tag: String,
    pub name_bcs: Vec<u8>,
    pub value_bytes: Option<Vec<u8>>,
    pub value_type_tag: String,
    pub mode: DfAccessMode,
    /// Full on-disk `ObjectEnvelope::to_bytes()` for Read/Mutate/Delete modes.
    /// `None` for Create (nothing exists yet). Used by the Move VM engine to
    /// emit `StateChange.old_value` with bytes that exactly match current SMT,
    /// avoiding false stale_read at CF apply time.
    #[serde(default)]
    pub envelope_bytes: Option<Vec<u8>>,
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
        
        assert!(matches!(resolved.operation, OperationType::Transfer { amount: 100, .. }));
        assert_eq!(resolved.input_objects.len(), 1);
        assert_eq!(resolved.primary_coin().unwrap().object_id, coin.object_id);
    }
    
    #[test]
    fn test_gas_budget_default() {
        let budget = GasBudget::default();
        assert_eq!(budget.max_gas_units, u64::MAX);
        assert_eq!(budget.estimated_fee, 0);
    }

    // ── DF FDP M1 — ResolvedInputs.dynamic_fields coverage ──

    #[test]
    fn resolved_inputs_serde_default_dynamic_fields() {
        // Legacy client payload that predates M1 — no `dynamic_fields` field.
        // With `#[serde(default)]` on the new field this MUST round-trip to
        // an empty Vec instead of a decode error.
        // NoOp is a unit variant — serde's default externally-tagged repr
        // serializes unit variants as the bare variant string.
        let legacy_json = r#"{
            "operation": "NoOp",
            "input_objects": []
        }"#;
        let parsed: ResolvedInputs = serde_json::from_str(legacy_json)
            .expect("legacy payload without dynamic_fields should still decode");
        assert!(parsed.dynamic_fields.is_empty());
    }

    #[test]
    fn resolved_dynamic_field_bcs_roundtrip() {
        use crate::dynamic_field::DfAccessMode;

        // Create-mode entry (value_bytes = None).
        let create = ResolvedDynamicField {
            parent_object_id: ObjectId::new([1u8; 32]),
            df_object_id: ObjectId::new([2u8; 32]),
            name_type_tag: "u64".to_string(),
            name_bcs: vec![42, 0, 0, 0, 0, 0, 0, 0],
            value_bytes: None,
            value_type_tag: "0xcafe::pool::Liquidity".to_string(),
            mode: DfAccessMode::Create,
            envelope_bytes: None,
        };
        let bytes = bcs::to_bytes(&create).unwrap();
        let decoded: ResolvedDynamicField = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.parent_object_id, create.parent_object_id);
        assert_eq!(decoded.df_object_id, create.df_object_id);
        assert_eq!(decoded.name_type_tag, create.name_type_tag);
        assert_eq!(decoded.name_bcs, create.name_bcs);
        assert_eq!(decoded.value_bytes, None);
        assert_eq!(decoded.value_type_tag, create.value_type_tag);
        assert_eq!(decoded.mode, DfAccessMode::Create);

        // Read-mode entry (value_bytes = Some).
        let read = ResolvedDynamicField {
            parent_object_id: ObjectId::new([3u8; 32]),
            df_object_id: ObjectId::new([4u8; 32]),
            name_type_tag: "address".to_string(),
            name_bcs: vec![5; 32],
            value_bytes: Some(vec![10, 20, 30]),
            value_type_tag: "u64".to_string(),
            mode: DfAccessMode::Read,
            envelope_bytes: Some(vec![0xde, 0xad, 0xbe, 0xef]),
        };
        let bytes = bcs::to_bytes(&read).unwrap();
        let decoded: ResolvedDynamicField = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.value_bytes, Some(vec![10, 20, 30]));
        assert_eq!(decoded.mode, DfAccessMode::Read);
    }

    #[test]
    fn resolved_inputs_dynamic_fields_roundtrip() {
        use crate::dynamic_field::DfAccessMode;

        let coin = ResolvedObject::coin(ObjectId::new([9u8; 32])).with_version(7);
        let mut inputs = ResolvedInputs::transfer(coin, 100);
        inputs.dynamic_fields.push(ResolvedDynamicField {
            parent_object_id: ObjectId::new([1u8; 32]),
            df_object_id: ObjectId::new([2u8; 32]),
            name_type_tag: "u64".to_string(),
            name_bcs: vec![1, 0, 0, 0, 0, 0, 0, 0],
            value_bytes: Some(vec![0xab, 0xcd]),
            value_type_tag: "0xdead::beef::T".to_string(),
            mode: DfAccessMode::Mutate,
            envelope_bytes: Some(vec![1, 2, 3, 4]),
        });

        let bytes = bcs::to_bytes(&inputs).unwrap();
        let decoded: ResolvedInputs = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.dynamic_fields.len(), 1);
        let df = &decoded.dynamic_fields[0];
        assert_eq!(df.parent_object_id, ObjectId::new([1u8; 32]));
        assert_eq!(df.df_object_id, ObjectId::new([2u8; 32]));
        assert_eq!(df.name_type_tag, "u64");
        assert_eq!(df.name_bcs, vec![1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(df.value_bytes, Some(vec![0xab, 0xcd]));
        assert_eq!(df.value_type_tag, "0xdead::beef::T");
        assert_eq!(df.mode, DfAccessMode::Mutate);
    }
}
