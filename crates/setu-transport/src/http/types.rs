//! HTTP Request/Response types for Validator ↔ Solver communication
//!
//! These types are the **single source of truth** for HTTP transport DTOs.
//! Both Validator (client) and Solver (server) use these same types.
//!
//! # Design Principles
//!
//! 1. **Pure Data**: These are DTOs (Data Transfer Objects) only - no business logic
//! 2. **Serializable**: All types derive Serialize/Deserialize for JSON transport
//! 3. **Validated**: Use builder pattern for validated construction where needed
//! 4. **Versioned**: API versioning through URL path (e.g., /api/v1/execute-task)

use serde::{Deserialize, Serialize};
use setu_types::task::SolverTask;

// ============================================
// Request/Response Types
// ============================================

/// Request to execute a SolverTask synchronously
///
/// Sent by Validator to Solver via `POST /api/v1/execute-task`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteTaskRequest {
    /// The solver task to execute in TEE
    pub solver_task: SolverTask,
    /// Validator ID that sent this task (for logging/tracing)
    pub validator_id: String,
    /// Request ID for correlation (UUID recommended)
    pub request_id: String,
}

impl ExecuteTaskRequest {
    /// Create a new execute task request
    pub fn new(
        solver_task: SolverTask,
        validator_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            solver_task,
            validator_id: validator_id.into(),
            request_id: request_id.into(),
        }
    }
}

/// Response containing TEE execution result
///
/// Returned by Solver to Validator in HTTP response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteTaskResponse {
    /// Whether execution was successful
    pub success: bool,
    /// Human-readable message describing outcome
    pub message: String,
    /// TEE execution result (present on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TeeExecutionResultDto>,
    /// Total execution time in microseconds
    pub execution_time_us: u64,
}

impl ExecuteTaskResponse {
    /// Create a success response
    pub fn success(
        result: TeeExecutionResultDto,
        message: impl Into<String>,
        execution_time_us: u64,
    ) -> Self {
        Self {
            success: true,
            message: message.into(),
            result: Some(result),
            execution_time_us,
        }
    }

    /// Create an error response
    pub fn error(message: impl Into<String>, execution_time_us: u64) -> Self {
        Self {
            success: false,
            message: message.into(),
            result: None,
            execution_time_us,
        }
    }
}

// ============================================
// DTO Types for Serialization
// ============================================

/// DTO for TeeExecutionResult (serializable for HTTP transport)
///
/// This mirrors the internal TeeExecutionResult structure but uses
/// only serializable types suitable for JSON transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeeExecutionResultDto {
    /// Task ID (matches input SolverTask.task_id)
    pub task_id: [u8; 32],
    /// Subnet that was executed
    pub subnet_id: String,
    /// Post-execution state root (Merkle root after state changes)
    pub post_state_root: [u8; 32],
    /// State changes to apply (key-value mutations)
    pub state_changes: Vec<StateChangeDto>,
    /// Number of events processed successfully
    pub events_processed: usize,
    /// Number of events that failed processing
    pub events_failed: usize,
    /// Total gas consumed
    pub gas_used: u64,
    /// TEE attestation proving execution integrity
    pub attestation: AttestationDto,
    /// Execution time in microseconds (TEE internal measurement)
    pub execution_time_us: u64,
}

/// State change DTO
///
/// Represents a single key-value mutation in the state store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChangeDto {
    /// State key
    pub key: String,
    /// Previous value (None if key didn't exist)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<Vec<u8>>,
    /// New value (None if key is being deleted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<Vec<u8>>,
}

impl StateChangeDto {
    /// Create a new state change
    pub fn new(
        key: impl Into<String>,
        old_value: Option<Vec<u8>>,
        new_value: Option<Vec<u8>>,
    ) -> Self {
        Self {
            key: key.into(),
            old_value,
            new_value,
        }
    }

    /// Create an insert (new key)
    pub fn insert(key: impl Into<String>, value: Vec<u8>) -> Self {
        Self {
            key: key.into(),
            old_value: None,
            new_value: Some(value),
        }
    }

    /// Create an update (existing key)
    pub fn update(key: impl Into<String>, old_value: Vec<u8>, new_value: Vec<u8>) -> Self {
        Self {
            key: key.into(),
            old_value: Some(old_value),
            new_value: Some(new_value),
        }
    }

    /// Create a delete
    pub fn delete(key: impl Into<String>, old_value: Vec<u8>) -> Self {
        Self {
            key: key.into(),
            old_value: Some(old_value),
            new_value: None,
        }
    }
}

/// Attestation DTO (serializable TEE attestation)
///
/// Contains cryptographic proof of TEE execution integrity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationDto {
    /// Unique enclave identifier
    pub enclave_id: String,
    /// Unix timestamp when attestation was generated
    pub timestamp: u64,
    /// Task ID that was executed (binding)
    pub task_id_binding: [u8; 32],
    /// Hash of all inputs to TEE execution
    pub input_hash: [u8; 32],
    /// State root before execution
    pub pre_state_root: [u8; 32],
    /// State root after execution
    pub post_state_root: [u8; 32],
    /// Cryptographic signature over all fields
    pub signature: Vec<u8>,
}

// ============================================
// Health/Info Types
// ============================================

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Health status: "healthy", "degraded", "unhealthy"
    pub status: String,
    /// Node identifier
    pub node_id: String,
    /// Protocol version
    pub version: String,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl HealthResponse {
    /// Create a healthy response
    pub fn healthy(node_id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            status: "healthy".to_string(),
            node_id: node_id.into(),
            version: version.into(),
            metadata: None,
        }
    }

    /// Add metadata to response
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Solver info response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverInfoResponse {
    /// Solver identifier
    pub solver_id: String,
    /// Enclave information
    pub enclave: EnclaveInfoDto,
    /// Operating mode
    pub mode: String,
}

/// Enclave info DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnclaveInfoDto {
    /// Enclave ID
    pub id: String,
    /// Enclave version
    pub version: String,
    /// Platform (e.g., "sgx", "sev", "mock")
    pub platform: String,
    /// Whether running in simulation mode
    pub is_simulated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::{Event, EventType, VLCSnapshot};
    use setu_types::task::ResolvedInputs;
    use setu_types::SubnetId;

    fn create_test_solver_task() -> SolverTask {
        let event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test-creator".to_string(),
        );

        SolverTask::new(
            [0u8; 32],
            event,
            ResolvedInputs::new(),
            [0u8; 32],
            SubnetId::default(),
        )
    }

    #[test]
    fn test_execute_task_request_serialization() {
        // Create a minimal request for testing
        let request = ExecuteTaskRequest {
            solver_task: create_test_solver_task(),
            validator_id: "validator-1".to_string(),
            request_id: "req-123".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: ExecuteTaskRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.validator_id, "validator-1");
        assert_eq!(deserialized.request_id, "req-123");
    }

    #[test]
    fn test_execute_task_response_success() {
        let response = ExecuteTaskResponse::success(
            TeeExecutionResultDto {
                task_id: [0u8; 32],
                subnet_id: "subnet-1".to_string(),
                post_state_root: [0u8; 32],
                state_changes: vec![],
                events_processed: 1,
                events_failed: 0,
                gas_used: 100,
                attestation: AttestationDto {
                    enclave_id: "enclave-1".to_string(),
                    timestamp: 12345,
                    task_id_binding: [0u8; 32],
                    input_hash: [0u8; 32],
                    pre_state_root: [0u8; 32],
                    post_state_root: [0u8; 32],
                    signature: vec![],
                },
                execution_time_us: 1000,
            },
            "Task executed successfully",
            1000,
        );

        assert!(response.success);
        assert!(response.result.is_some());
    }

    #[test]
    fn test_execute_task_response_error() {
        let response = ExecuteTaskResponse::error("Something went wrong", 500);

        assert!(!response.success);
        assert!(response.result.is_none());
        assert_eq!(response.message, "Something went wrong");
    }

    #[test]
    fn test_state_change_dto_variants() {
        let insert = StateChangeDto::insert("key1", vec![1, 2, 3]);
        assert!(insert.old_value.is_none());
        assert_eq!(insert.new_value, Some(vec![1, 2, 3]));

        let update = StateChangeDto::update("key2", vec![1], vec![2]);
        assert_eq!(update.old_value, Some(vec![1]));
        assert_eq!(update.new_value, Some(vec![2]));

        let delete = StateChangeDto::delete("key3", vec![1, 2, 3]);
        assert_eq!(delete.old_value, Some(vec![1, 2, 3]));
        assert!(delete.new_value.is_none());
    }
}
