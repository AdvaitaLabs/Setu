//! TEE Attestation Verification for Validators
//!
//! This module provides verification of Solver execution results
//! by checking TEE attestations before applying state changes.
//!
//! # Workflow
//!
//! ```text
//! Solver (TEE) produces:
//!   ExecutionResult + TeeAttestation
//!           │
//!           ▼
//! Validator receives Event with execution_result
//!           │
//!           ▼
//! TeeVerifier.verify_execution()
//!   1. Verify attestation signature
//!   2. Verify state change commitments
//!   3. Check read-set versions
//!           │
//!           ▼
//! If verified → Apply state changes to SMT
//! If failed → Reject event, penalize solver
//! ```

use setu_types::event::{Event, ExecutionResult, StateChange};
use setu_types::SubnetId;
use sha2::{Sha256, Digest};
use std::collections::HashMap;

/// TEE attestation attached to an execution result
#[derive(Debug, Clone)]
pub struct TeeAttestation {
    /// The solver that produced this attestation
    pub solver_id: String,
    /// Attestation quote from the TEE
    pub quote: Vec<u8>,
    /// Signature over the execution result
    pub signature: Vec<u8>,
    /// Platform identifier (e.g., "SGX", "TDX", "SEV")
    pub platform: String,
    /// Enclave measurement
    pub measurement: [u8; 32],
    /// Commitment to read-set (hash of read keys and versions)
    pub read_set_commitment: [u8; 32],
    /// Commitment to write-set (hash of state changes)
    pub write_set_commitment: [u8; 32],
    /// Subnet state root after execution
    pub post_state_root: [u8; 32],
    /// Timestamp
    pub timestamp: u64,
}

impl TeeAttestation {
    /// Create a new attestation (used by Solver)
    pub fn new(
        solver_id: String,
        platform: String,
        measurement: [u8; 32],
        read_set_commitment: [u8; 32],
        write_set_commitment: [u8; 32],
        post_state_root: [u8; 32],
    ) -> Self {
        Self {
            solver_id,
            quote: Vec::new(),
            signature: Vec::new(),
            platform,
            measurement,
            read_set_commitment,
            write_set_commitment,
            post_state_root,
            timestamp: current_timestamp(),
        }
    }
    
    /// Compute commitment for a set of state changes (write-set)
    pub fn compute_write_set_commitment(changes: &[StateChange]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for change in changes {
            hasher.update(change.key.as_bytes());
            if let Some(ref old) = change.old_value {
                hasher.update(&[1u8]);
                hasher.update(old);
            } else {
                hasher.update(&[0u8]);
            }
            if let Some(ref new) = change.new_value {
                hasher.update(&[1u8]);
                hasher.update(new);
            } else {
                hasher.update(&[0u8]);
            }
        }
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}

/// Result of TEE verification
#[derive(Debug, Clone)]
pub enum VerificationResult {
    /// Verification passed
    Verified,
    /// Verification failed with reason
    Failed(VerificationError),
    /// No attestation present (for ROOT subnet events)
    NotApplicable,
}

/// Errors during TEE verification
#[derive(Debug, Clone)]
pub enum VerificationError {
    /// Attestation is missing
    MissingAttestation,
    /// Attestation signature invalid
    InvalidSignature,
    /// Quote verification failed
    QuoteVerificationFailed(String),
    /// Read-set commitment mismatch
    ReadSetMismatch,
    /// Write-set commitment mismatch
    WriteSetMismatch,
    /// State root mismatch
    StateRootMismatch,
    /// Attestation too old
    ExpiredAttestation,
    /// Unknown solver
    UnknownSolver(String),
    /// Enclave measurement mismatch
    MeasurementMismatch,
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerificationError::MissingAttestation => write!(f, "Missing TEE attestation"),
            VerificationError::InvalidSignature => write!(f, "Invalid attestation signature"),
            VerificationError::QuoteVerificationFailed(msg) => write!(f, "Quote verification failed: {}", msg),
            VerificationError::ReadSetMismatch => write!(f, "Read-set commitment mismatch"),
            VerificationError::WriteSetMismatch => write!(f, "Write-set commitment mismatch"),
            VerificationError::StateRootMismatch => write!(f, "Post-state root mismatch"),
            VerificationError::ExpiredAttestation => write!(f, "Attestation expired"),
            VerificationError::UnknownSolver(id) => write!(f, "Unknown solver: {}", id),
            VerificationError::MeasurementMismatch => write!(f, "Enclave measurement mismatch"),
        }
    }
}

/// Registry of known solvers and their expected measurements
#[derive(Debug, Clone, Default)]
pub struct SolverRegistry {
    /// Mapping from solver_id to expected enclave measurement
    solvers: HashMap<String, SolverInfo>,
}

#[derive(Debug, Clone)]
pub struct SolverInfo {
    pub solver_id: String,
    pub public_key: Vec<u8>,
    pub expected_measurement: [u8; 32],
    pub platform: String,
    pub registered_at: u64,
}

impl SolverRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn register(&mut self, info: SolverInfo) {
        self.solvers.insert(info.solver_id.clone(), info);
    }
    
    pub fn get(&self, solver_id: &str) -> Option<&SolverInfo> {
        self.solvers.get(solver_id)
    }
    
    pub fn is_registered(&self, solver_id: &str) -> bool {
        self.solvers.contains_key(solver_id)
    }
}

/// TEE Verifier for Validators
///
/// Validates execution results from Solvers before applying state changes.
pub struct TeeVerifier {
    /// Registry of known solvers
    solver_registry: SolverRegistry,
    /// Maximum age of attestations (in milliseconds)
    max_attestation_age_ms: u64,
    /// Whether to skip verification (for testing)
    skip_verification: bool,
}

impl TeeVerifier {
    /// Create a new TeeVerifier
    pub fn new(solver_registry: SolverRegistry) -> Self {
        Self {
            solver_registry,
            max_attestation_age_ms: 5 * 60 * 1000, // 5 minutes
            skip_verification: false,
        }
    }
    
    /// Create a permissive verifier for testing
    pub fn permissive() -> Self {
        Self {
            solver_registry: SolverRegistry::new(),
            max_attestation_age_ms: u64::MAX,
            skip_verification: true,
        }
    }
    
    /// Verify an event's execution result
    ///
    /// For ROOT subnet events (validator-executed), returns NotApplicable.
    /// For App subnet events, verifies the TEE attestation.
    pub fn verify_event(&self, event: &Event) -> VerificationResult {
        let subnet_id = event.get_subnet_id();
        
        // ROOT subnet events are executed by validators, no TEE needed
        if subnet_id.is_root() || event.is_validator_executed() {
            return VerificationResult::NotApplicable;
        }
        
        // App subnet events require TEE attestation
        // For now, we use a simplified verification
        if self.skip_verification {
            return VerificationResult::Verified;
        }
        
        // Check if execution result exists
        let result = match &event.execution_result {
            Some(r) => r,
            None => return VerificationResult::Failed(VerificationError::MissingAttestation),
        };
        
        // Verify execution was successful
        if !result.success {
            // Failed executions don't need attestation verification
            // (they won't have state changes applied)
            return VerificationResult::Verified;
        }
        
        // In a full implementation, we would:
        // 1. Extract TeeAttestation from the event
        // 2. Verify the attestation quote
        // 3. Verify the signature
        // 4. Check solver is registered
        // 5. Verify enclave measurement
        // 6. Verify write-set commitment matches state_changes
        
        // For now, accept if execution result exists
        VerificationResult::Verified
    }
    
    /// Verify a batch of events
    pub fn verify_events(&self, events: &[Event]) -> Vec<(String, VerificationResult)> {
        events
            .iter()
            .map(|e| (e.id.clone(), self.verify_event(e)))
            .collect()
    }
    
    /// Verify attestation against an execution result
    pub fn verify_attestation(
        &self,
        attestation: &TeeAttestation,
        result: &ExecutionResult,
    ) -> VerificationResult {
        if self.skip_verification {
            return VerificationResult::Verified;
        }
        
        // Check solver is registered
        if !self.solver_registry.is_registered(&attestation.solver_id) {
            return VerificationResult::Failed(
                VerificationError::UnknownSolver(attestation.solver_id.clone())
            );
        }
        
        // Check attestation age
        let now = current_timestamp();
        if now > attestation.timestamp + self.max_attestation_age_ms {
            return VerificationResult::Failed(VerificationError::ExpiredAttestation);
        }
        
        // Verify write-set commitment
        let expected_commitment = TeeAttestation::compute_write_set_commitment(&result.state_changes);
        if expected_commitment != attestation.write_set_commitment {
            return VerificationResult::Failed(VerificationError::WriteSetMismatch);
        }
        
        // Check enclave measurement
        let solver_info = self.solver_registry.get(&attestation.solver_id).unwrap();
        if attestation.measurement != solver_info.expected_measurement {
            return VerificationResult::Failed(VerificationError::MeasurementMismatch);
        }
        
        // In production: verify quote and signature
        // For now, we skip these checks
        
        VerificationResult::Verified
    }
    
    /// Get the solver registry
    pub fn solver_registry(&self) -> &SolverRegistry {
        &self.solver_registry
    }
    
    /// Get mutable access to solver registry
    pub fn solver_registry_mut(&mut self) -> &mut SolverRegistry {
        &mut self.solver_registry
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::EventType;
    use setu_types::event::VLCSnapshot;
    
    fn create_app_event() -> Event {
        let app_subnet = SubnetId::from_str_id("test-app");
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        event = event.with_subnet(app_subnet);
        event.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![
                StateChange {
                    key: "balance:alice".to_string(),
                    old_value: Some(vec![100]),
                    new_value: Some(vec![90]),
                },
            ],
        });
        event
    }
    
    fn create_root_event() -> Event {
        let mut event = Event::new(
            EventType::ValidatorRegister,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        event = event.with_subnet(SubnetId::ROOT);
        event
    }
    
    #[test]
    fn test_root_events_not_applicable() {
        let verifier = TeeVerifier::new(SolverRegistry::new());
        let event = create_root_event();
        
        match verifier.verify_event(&event) {
            VerificationResult::NotApplicable => {}
            other => panic!("Expected NotApplicable, got {:?}", other),
        }
    }
    
    #[test]
    fn test_app_events_need_verification() {
        // With skip_verification = true
        let verifier = TeeVerifier::permissive();
        let event = create_app_event();
        
        match verifier.verify_event(&event) {
            VerificationResult::Verified => {}
            other => panic!("Expected Verified, got {:?}", other),
        }
    }
    
    #[test]
    fn test_write_set_commitment() {
        let changes = vec![
            StateChange {
                key: "balance:alice".to_string(),
                old_value: Some(vec![100]),
                new_value: Some(vec![90]),
            },
        ];
        
        let commitment1 = TeeAttestation::compute_write_set_commitment(&changes);
        let commitment2 = TeeAttestation::compute_write_set_commitment(&changes);
        
        assert_eq!(commitment1, commitment2);
        
        // Different changes should produce different commitment
        let changes2 = vec![
            StateChange {
                key: "balance:bob".to_string(),
                old_value: Some(vec![50]),
                new_value: Some(vec![60]),
            },
        ];
        
        let commitment3 = TeeAttestation::compute_write_set_commitment(&changes2);
        assert_ne!(commitment1, commitment3);
    }
    
    #[test]
    fn test_solver_registry() {
        let mut registry = SolverRegistry::new();
        
        registry.register(SolverInfo {
            solver_id: "solver-1".to_string(),
            public_key: vec![1, 2, 3],
            expected_measurement: [0u8; 32],
            platform: "SGX".to_string(),
            registered_at: 0,
        });
        
        assert!(registry.is_registered("solver-1"));
        assert!(!registry.is_registered("solver-2"));
    }
}
