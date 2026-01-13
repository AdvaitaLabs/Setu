//! Stateless Transition Function (STF) types.
//!
//! The STF is the core computation that runs inside the enclave:
//!
//! ```text
//! STF: (pre_state_root, events, read_set) → (post_state_root, state_diff, attestation)
//! ```
//!
//! ## Key Properties
//!
//! - **Stateless**: The enclave holds no persistent state. All state is passed in/out.
//! - **Deterministic**: Same inputs always produce same outputs.
//! - **Verifiable**: Outputs are cryptographically attested.
//!
//! ## Data Flow
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          STF Execution Flow                             │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │  Input:                                                                 │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │ StfInput {                                                       │   │
//! │  │   subnet_id: SubnetId,           // Target subnet                │   │
//! │  │   pre_state_root: [u8; 32],      // State root before execution  │   │
//! │  │   events: Vec<Event>,            // Events to execute            │   │
//! │  │   read_set: Vec<ReadSetEntry>,   // Objects needed for execution │   │
//! │  │ }                                                                │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                              │                                         │
//! │                              ▼                                         │
//! │                    ┌──────────────────┐                                │
//! │                    │   STF Executor   │                                │
//! │                    │   (in Enclave)   │                                │
//! │                    └────────┬─────────┘                                │
//! │                              │                                         │
//! │                              ▼                                         │
//! │  Output:                                                               │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │ StfOutput {                                                      │   │
//! │  │   post_state_root: [u8; 32],     // State root after execution   │   │
//! │  │   state_diff: StateDiff,         // Changes to apply             │   │
//! │  │   events_processed: Vec<EventId>,// Successfully processed       │   │
//! │  │   attestation: Attestation,      // TEE proof                    │   │
//! │  │ }                                                                │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use setu_types::{Event, EventId, SubnetId};
use thiserror::Error;
use crate::attestation::Attestation;

/// Hash type (32 bytes)
pub type Hash = [u8; 32];

/// STF execution errors
#[derive(Debug, Error)]
pub enum StfError {
    #[error("Invalid pre-state root")]
    InvalidPreStateRoot,
    
    #[error("Read set verification failed: {0}")]
    ReadSetVerificationFailed(String),
    
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StfInput {
    /// Target subnet for this execution
    pub subnet_id: SubnetId,
    
    /// State root before execution (commitment to current state)
    pub pre_state_root: Hash,
    
    /// Events to execute (ordered by VLC)
    pub events: Vec<Event>,
    
    /// Read set: objects needed for execution with their current values
    /// The enclave verifies these against pre_state_root
    pub read_set: Vec<ReadSetEntry>,
    
    /// Optional: anchor ID for context
    pub anchor_id: Option<u64>,
}

impl StfInput {
    pub fn new(subnet_id: SubnetId, pre_state_root: Hash) -> Self {
        Self {
            subnet_id,
            pre_state_root,
            events: Vec::new(),
            read_set: Vec::new(),
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

/// Output from the Stateless Transition Function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StfOutput {
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
    
    /// TEE attestation over this output
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
        let input = StfInput::new(SubnetId::ROOT, [0u8; 32])
            .with_anchor(100);
        
        assert_eq!(input.subnet_id, SubnetId::ROOT);
        assert_eq!(input.anchor_id, Some(100));
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
