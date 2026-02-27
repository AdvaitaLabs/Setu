//! Stateless Transition Function (STF) types.
//!
//! The STF is the core computation that runs inside the enclave:
//!
//! ```text
//! STF: (task_id, pre_state_root, events, read_set, resolved_inputs, gas_budget) 
//!      → (task_id, post_state_root, state_diff, gas_usage, attestation)
//! ```
//!
//! ## Key Properties
//!
//! - **Stateless**: The enclave holds no persistent state. All state is passed in/out.
//! - **Deterministic**: Same inputs always produce same outputs.
//! - **Verifiable**: Outputs are cryptographically attested.
//! - **Task-bound**: Each execution is bound to a unique task_id for replay protection.
//!
//! ## Data Flow (solver-tee3 Architecture)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          STF Execution Flow                             │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │  Validator prepares SolverTask:                                         │
//! │  - Selects coins for transfer operations                                │
//! │  - Builds read_set with Merkle proofs                                  │
//! │  - Sets gas_budget limits                                              │
//! │                                                                         │
//! │  Solver passes SolverTask → StfInput (pass-through)                    │
//! │                                                                         │
//! │  Input:                                                                 │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │ StfInput {                                                       │   │
//! │  │   task_id: TaskId,               // Unique task identifier       │   │
//! │  │   subnet_id: SubnetId,           // Target subnet                │   │
//! │  │   pre_state_root: [u8; 32],      // State root before execution  │   │
//! │  │   events: Vec<Event>,            // Events to execute            │   │
//! │  │   read_set: Vec<ReadSetEntry>,   // Objects needed for execution │   │
//! │  │   resolved_inputs: ResolvedInputs, // Coin selection results     │   │
//! │  │   gas_budget: GasBudget,         // Execution limits             │   │
//! │  │ }                                                                │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                              │                                         │
//! │                              ▼                                         │
//! │                    ┌──────────────────┐                                │
//! │                    │   STF Executor   │                                │
//! │                    │   (in Enclave)   │                                │
//! │                    │                  │                                │
//! │                    │ 1. Verify proofs │                                │
//! │                    │ 2. Build state   │                                │
//! │                    │ 3. Execute events│                                │
//! │                    │ 4. Compute root  │                                │
//! │                    │ 5. Generate attest│                               │
//! │                    └────────┬─────────┘                                │
//! │                              │                                         │
//! │                              ▼                                         │
//! │  Output:                                                               │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │ StfOutput {                                                      │   │
//! │  │   task_id: TaskId,               // Same as input (for binding)  │   │
//! │  │   post_state_root: [u8; 32],     // State root after execution   │   │
//! │  │   state_diff: StateDiff,         // Changes to apply             │   │
//! │  │   events_processed: Vec<EventId>,// Successfully processed       │   │
//! │  │   gas_usage: GasUsage,           // Actual gas consumed          │   │
//! │  │   attestation: Attestation,      // TEE proof (includes task_id) │   │
//! │  │ }                                                                │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use setu_types::{Event, EventId, SubnetId};
use thiserror::Error;
// Use types from setu-types (canonical source)
use setu_types::task::{
    Attestation,
    ResolvedInputs, GasBudget, GasUsage,
    ReadSetEntry,  // Use the canonical ReadSetEntry from setu-types
};

/// Hash type (32 bytes)
pub type Hash = [u8; 32];

/// Unique task identifier for binding attestation to specific execution
pub type TaskId = [u8; 32];

/// STF execution errors
#[derive(Debug, Error)]
pub enum StfError {
    #[error("Invalid pre-state root")]
    InvalidPreStateRoot,
    
    #[error("Read set verification failed: {0}")]
    ReadSetVerificationFailed(String),
    
    #[error("Merkle proof verification failed for object {object_id}: {reason}")]
    MerkleProofFailed { object_id: String, reason: String },
    
    #[error("Object version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u64, actual: u64 },
    
    #[error("Invalid resolved inputs: {0}")]
    InvalidResolvedInputs(String),
    
    #[error("Gas budget exceeded: limit={limit}, used={used}")]
    GasBudgetExceeded { limit: u64, used: u64 },
    
    #[error("Event execution failed: {event_id} - {reason}")]
    EventExecutionFailed { event_id: String, reason: String },
    
    #[error("State root computation failed: {0}")]
    StateRootComputationFailed(String),
    
    #[error("Attestation generation failed: {0}")]
    AttestationFailed(String),
    
    #[error("Execution timeout")]
    ExecutionTimeout,
    
    #[error("Memory limit exceeded")]
    MemoryLimitExceeded,
    
    #[error("Internal enclave error: {0}")]
    InternalError(String),
}

pub type StfResult<T> = Result<T, StfError>;

/// Input to the Stateless Transition Function
/// 
/// This structure is created by converting a SolverTask (prepared by Validator)
/// into the format needed for TEE execution. The Solver acts as a pass-through.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StfInput {
    /// Unique task identifier for this execution
    /// This binds the attestation to this specific task (replay protection)
    pub task_id: TaskId,
    
    /// Target subnet for this execution
    pub subnet_id: SubnetId,
    
    /// State root before execution (commitment to current state)
    pub pre_state_root: Hash,
    
    /// Events to execute (ordered by VLC)
    pub events: Vec<Event>,
    
    /// Read set: objects needed for execution with their current values
    /// The enclave verifies these against pre_state_root using Merkle proofs
    pub read_set: Vec<ReadSetEntry>,
    
    /// Resolved inputs from Validator (coin selection, object references)
    /// This is REQUIRED - the Validator must always provide coin selection results
    pub resolved_inputs: ResolvedInputs,
    
    /// Gas budget for this execution (limits on computation/storage)
    pub gas_budget: GasBudget,
    
    /// Optional: anchor ID for context
    pub anchor_id: Option<u64>,
}

impl StfInput {
    /// Create a new StfInput with required fields
    pub fn new(
        task_id: TaskId,
        subnet_id: SubnetId,
        pre_state_root: Hash,
        resolved_inputs: ResolvedInputs,
        gas_budget: GasBudget,
    ) -> Self {
        Self {
            task_id,
            subnet_id,
            pre_state_root,
            events: Vec::new(),
            read_set: Vec::new(),
            resolved_inputs,
            gas_budget,
            anchor_id: None,
        }
    }
    
    pub fn with_events(mut self, events: Vec<Event>) -> Self {
        self.events = events;
        self
    }
    
    pub fn with_read_set(mut self, read_set: Vec<ReadSetEntry>) -> Self {
        self.read_set = read_set;
        self
    }
    
    pub fn with_anchor(mut self, anchor_id: u64) -> Self {
        self.anchor_id = Some(anchor_id);
        self
    }
    
    /// Compute input hash for attestation binding
    /// This hash covers all inputs to ensure attestation is bound to specific execution
    pub fn input_hash(&self) -> Hash {
        use sha2::{Sha256, Digest};
        
        let mut hasher = Sha256::new();
        hasher.update(&self.task_id);
        hasher.update(&self.subnet_id.to_bytes());
        hasher.update(&self.pre_state_root);
        
        // Hash events
        for event in &self.events {
            hasher.update(event.id.as_bytes());
        }
        
        // Hash read_set keys (not values, to keep it efficient)
        for entry in &self.read_set {
            hasher.update(entry.key.as_bytes());
        }
        
        // Hash resolved inputs summary
        hasher.update(&(self.resolved_inputs.input_objects.len() as u64).to_le_bytes());
        
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}

/// Output from the Stateless Transition Function
/// 
/// The attestation in this output binds task_id, input_hash, and pre_state_root
/// to ensure validators can verify the execution was performed correctly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StfOutput {
    /// Task ID (must match input for validation)
    pub task_id: TaskId,
    
    /// Target subnet
    pub subnet_id: SubnetId,
    
    /// State root after execution
    pub post_state_root: Hash,
    
    /// State changes to apply
    pub state_diff: StateDiff,
    
    /// Events that were successfully processed
    pub events_processed: Vec<EventId>,
    
    /// Events that failed (with reasons)
    pub events_failed: Vec<FailedEvent>,
    
    /// Gas usage for this execution
    pub gas_usage: GasUsage,
    
    /// TEE attestation over this output
    /// The attestation binds: task_id, input_hash, pre_state_root, post_state_root
    pub attestation: Attestation,
    
    /// Execution statistics
    pub stats: ExecutionStats,
}

/// A state diff (collection of write set entries)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateDiff {
    /// Write set: objects to create or update
    pub writes: Vec<WriteSetEntry>,
    /// Delete set: objects to remove
    pub deletes: Vec<String>,
}

impl StateDiff {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn add_write(&mut self, entry: WriteSetEntry) {
        self.writes.push(entry);
    }
    
    pub fn add_delete(&mut self, key: String) {
        self.deletes.push(key);
    }
    
    /// Add writes from setu-runtime ExecutionOutput state changes
    /// 
    /// This converts runtime's object-based StateChange to enclave's WriteSetEntry
    pub fn add_state_changes(&mut self, state_changes: &[setu_runtime::StateChange]) {
        for change in state_changes {
            let entry = WriteSetEntry::from_state_change(
                &change.object_id,
                change.old_state.clone(),
                change.new_state.clone().unwrap_or_default(),
            );
            self.add_write(entry);
        }
    }
    
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes.is_empty()
    }
    
    pub fn len(&self) -> usize {
        self.writes.len() + self.deletes.len()
    }
    
    /// Compute commitment hash of this state diff
    pub fn commitment(&self) -> Hash {
        use sha2::{Sha256, Digest};
        
        let mut hasher = Sha256::new();
        
        // Hash writes
        for write in &self.writes {
            hasher.update(write.key.as_bytes());
            hasher.update(&write.new_value);
        }
        
        // Hash deletes
        for delete in &self.deletes {
            hasher.update(b"DELETE:");
            hasher.update(delete.as_bytes());
        }
        
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}

/// An entry in the write set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteSetEntry {
    /// Object key
    pub key: String,
    /// Old value (for verification, optional)
    pub old_value: Option<Vec<u8>>,
    /// New value
    pub new_value: Vec<u8>,
}

impl WriteSetEntry {
    pub fn new(key: String, new_value: Vec<u8>) -> Self {
        Self {
            key,
            old_value: None,
            new_value,
        }
    }
    
    pub fn with_old_value(mut self, old_value: Vec<u8>) -> Self {
        self.old_value = Some(old_value);
        self
    }
    
    /// Convert from setu-runtime StateChange to WriteSetEntry
    /// 
    /// This bridges the internal object-based state changes to the
    /// generic key-value format used by the TEE output.
    /// 
    /// ## Key Format
    /// 
    /// Uses `"oid:{hex}"` format where hex is the raw ObjectId bytes.
    /// This allows GlobalStateManager to reconstruct the exact SMT key
    /// without additional hashing (the hex IS the key).
    /// 
    /// Example: ObjectId([0xab, 0xcd, ...]) → key = "oid:abcd1234..."
    pub fn from_state_change(
        object_id: &setu_types::ObjectId,
        old_state: Option<Vec<u8>>,
        new_state: Vec<u8>,
    ) -> Self {
        // Use "oid:" prefix with raw hex (no 0x) so GlobalStateManager
        // can directly decode to the SMT key without SHA256
        let key = format!("oid:{}", hex::encode(object_id.as_bytes()));
        let mut entry = Self::new(key, new_state);
        if let Some(old) = old_state {
            entry = entry.with_old_value(old);
        }
        entry
    }
}

/// Information about a failed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedEvent {
    pub event_id: EventId,
    pub reason: String,
}

/// Execution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// Total execution time in microseconds
    pub execution_time_us: u64,
    /// Number of read operations
    pub reads: u64,
    /// Number of write operations
    pub writes: u64,
    /// Peak memory usage in bytes
    pub peak_memory_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_stf_input_builder() {
        let task_id = [1u8; 32];
        let resolved_inputs = ResolvedInputs::new();
        let gas_budget = GasBudget::default();
        
        let input = StfInput::new(
            task_id,
            SubnetId::ROOT,
            [0u8; 32],
            resolved_inputs,
            gas_budget,
        ).with_anchor(100);
        
        assert_eq!(input.subnet_id, SubnetId::ROOT);
        assert_eq!(input.anchor_id, Some(100));
        assert_eq!(input.task_id, task_id);
    }
    
    #[test]
    fn test_stf_input_hash_deterministic() {
        let task_id = [1u8; 32];
        let resolved_inputs = ResolvedInputs::new();
        let gas_budget = GasBudget::default();
        
        let input = StfInput::new(
            task_id,
            SubnetId::ROOT,
            [0u8; 32],
            resolved_inputs.clone(),
            gas_budget.clone(),
        );
        
        let hash1 = input.input_hash();
        let hash2 = input.input_hash();
        
        assert_eq!(hash1, hash2, "input_hash should be deterministic");
    }
    
    #[test]
    fn test_stf_input_hash_changes_with_task_id() {
        let resolved_inputs = ResolvedInputs::new();
        let gas_budget = GasBudget::default();
        
        let input1 = StfInput::new(
            [1u8; 32],
            SubnetId::ROOT,
            [0u8; 32],
            resolved_inputs.clone(),
            gas_budget.clone(),
        );
        
        let input2 = StfInput::new(
            [2u8; 32], // Different task_id
            SubnetId::ROOT,
            [0u8; 32],
            resolved_inputs,
            gas_budget,
        );
        
        assert_ne!(input1.input_hash(), input2.input_hash(), "different task_id should produce different hash");
    }
    
    #[test]
    fn test_state_diff_commitment() {
        let mut diff = StateDiff::new();
        diff.add_write(WriteSetEntry::new("key1".to_string(), vec![1, 2, 3]));
        diff.add_write(WriteSetEntry::new("key2".to_string(), vec![4, 5, 6]));
        
        let commitment1 = diff.commitment();
        let commitment2 = diff.commitment();
        
        assert_eq!(commitment1, commitment2);
        
        // Different diff should have different commitment
        let mut diff2 = StateDiff::new();
        diff2.add_write(WriteSetEntry::new("key1".to_string(), vec![7, 8, 9]));
        
        assert_ne!(commitment1, diff2.commitment());
    }
    
    #[test]
    fn test_read_set_entry() {
        let entry = ReadSetEntry::new("balance:alice".to_string(), vec![100])
            .with_proof(vec![1, 2, 3]);
        
        assert!(entry.proof.is_some());
    }
}
