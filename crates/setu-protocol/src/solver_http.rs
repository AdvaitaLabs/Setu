//! Solver HTTP Communication Protocol
//!
//! This module defines request/response types for Validator ↔ Solver HTTP communication.
//!
//! ## Architecture (solver-tee3 Sync HTTP)
//!
//! ```text
//! Validator                              Solver
//!    │                                     │
//!    │  POST /api/v1/execute-task          │
//!    │  ─────────────────────────────────► │
//!    │  ExecuteTaskRequest                 │
//!    │                                     │ (TEE execution)
//!    │  ExecuteTaskResponse                │
//!    │  ◄───────────────────────────────── │
//!    │                                     │
//! ```
//!
//! Key benefit: Synchronous HTTP means Validator keeps Event in scope,
//! no pending_tasks mapping needed.

use serde::{Deserialize, Serialize};
use setu_enclave::{Attestation, GasUsage, SolverTask};
use setu_types::event::StateChange;
use setu_types::SubnetId;

// ============================================
// Request/Response Types
// ============================================

/// Request to execute a SolverTask synchronously
///
/// Validator sends this to Solver's `/api/v1/execute-task` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteTaskRequest {
    /// The solver task to execute (contains Event + all data needed for TEE)
    pub solver_task: SolverTask,
    /// Validator ID that sent this task
    pub validator_id: String,
    /// Request ID for tracing/correlation
    pub request_id: String,
}

/// Response containing TEE execution result
///
/// Solver returns this synchronously in the HTTP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteTaskResponse {
    /// Whether execution was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// TEE execution result (present on success)
    pub result: Option<TeeExecutionResultDto>,
    /// Total time including network overhead (microseconds)
    pub execution_time_us: u64,
}

// ============================================
// DTO Types for Serialization
// ============================================

/// DTO for TeeExecutionResult
///
/// This is the serializable version of `setu_solver::TeeExecutionResult`.
/// Both Validator and Solver use this for HTTP transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeeExecutionResultDto {
    /// Task ID (matches input SolverTask.task_id)
    pub task_id: [u8; 32],
    /// Subnet that was executed
    pub subnet_id: String,
    /// Post-execution state root
    pub post_state_root: [u8; 32],
    /// State changes to apply
    pub state_changes: Vec<StateChangeDto>,
    /// Number of events processed
    pub events_processed: usize,
    /// Number of events failed
    pub events_failed: usize,
    /// Gas used
    pub gas_used: u64,
    /// TEE attestation
    pub attestation: AttestationDto,
    /// TEE execution time (microseconds)
    pub execution_time_us: u64,
}

/// State change DTO (mirrors setu_types::event::StateChange)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChangeDto {
    pub key: String,
    pub old_value: Option<Vec<u8>>,
    pub new_value: Option<Vec<u8>>,
}

/// Attestation DTO (mirrors setu_enclave::Attestation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationDto {
    pub enclave_id: String,
    pub timestamp: u64,
    pub task_id_binding: [u8; 32],
    pub input_hash: [u8; 32],
    pub pre_state_root: [u8; 32],
    pub post_state_root: [u8; 32],
    pub signature: Vec<u8>,
}

// ============================================
// Conversion: StateChange ↔ StateChangeDto
// ============================================

impl From<&StateChange> for StateChangeDto {
    fn from(sc: &StateChange) -> Self {
        Self {
            key: sc.key.clone(),
            old_value: sc.old_value.clone(),
            new_value: sc.new_value.clone(),
        }
    }
}

impl From<StateChangeDto> for StateChange {
    fn from(dto: StateChangeDto) -> Self {
        Self {
            key: dto.key,
            old_value: dto.old_value,
            new_value: dto.new_value,
        }
    }
}

// ============================================
// Conversion: Attestation ↔ AttestationDto
// ============================================

impl From<&Attestation> for AttestationDto {
    fn from(att: &Attestation) -> Self {
        Self {
            enclave_id: att.enclave_id.clone(),
            timestamp: att.timestamp,
            task_id_binding: att.task_id_binding,
            input_hash: att.input_hash,
            pre_state_root: att.pre_state_root,
            post_state_root: att.post_state_root,
            signature: att.signature.clone(),
        }
    }
}

// ============================================
// Helper: Build TeeExecutionResultDto
// ============================================

impl TeeExecutionResultDto {
    /// Create from raw components
    pub fn new(
        task_id: [u8; 32],
        subnet_id: SubnetId,
        post_state_root: [u8; 32],
        state_changes: Vec<StateChange>,
        events_processed: usize,
        events_failed: usize,
        gas_usage: &GasUsage,
        attestation: &Attestation,
        execution_time_us: u64,
    ) -> Self {
        Self {
            task_id,
            subnet_id: subnet_id.to_string(),
            post_state_root,
            state_changes: state_changes.iter().map(StateChangeDto::from).collect(),
            events_processed,
            events_failed,
            gas_used: gas_usage.gas_used,
            attestation: AttestationDto::from(attestation),
            execution_time_us,
        }
    }

    /// Convert state_changes back to Vec<StateChange>
    pub fn to_state_changes(&self) -> Vec<StateChange> {
        self.state_changes
            .iter()
            .cloned()
            .map(StateChange::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_change_dto_roundtrip() {
        let original = StateChange {
            key: "test_key".to_string(),
            old_value: Some(vec![1, 2, 3]),
            new_value: Some(vec![4, 5, 6]),
        };

        let dto = StateChangeDto::from(&original);
        let restored: StateChange = dto.into();

        assert_eq!(original.key, restored.key);
        assert_eq!(original.old_value, restored.old_value);
        assert_eq!(original.new_value, restored.new_value);
    }
}
