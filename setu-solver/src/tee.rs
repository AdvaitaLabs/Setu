//! TEE (Trusted Execution Environment) integration for Solver
//!
//! This module provides a wrapper around `setu-enclave` for Solver-specific use cases.
//! In solver-tee3 architecture, Solver acts as a **pass-through** layer:
//!
//! - Validator prepares `SolverTask` with all necessary data (coin selection, proofs)
//! - Solver receives `SolverTask` and passes it to TEE
//! - TEE validates proofs and executes the state transition
//! - Solver returns the result to Validator
//!
//! ## Architecture (solver-tee3)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        Validator                                    │
//! │  TaskPreparer: coin selection, read_set, Merkle proofs             │
//! └─────────────────────────────────────────────────────────────────────┘
//!                              │
//!                        SolverTask
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                    Solver (Pass-through)                            │
//! │  ┌───────────────────────────────────────────────────────────────┐  │
//! │  │               TeeExecutor (this module)                        │  │
//! │  │   • Receives SolverTask from Validator                         │  │
//! │  │   • Converts to StfInput                                       │  │
//! │  │   • Passes to EnclaveRuntime                                   │  │
//! │  │   • Returns TeeExecutionResult                                 │  │
//! │  └───────────────────────────────────────────────────────────────┘  │
//! │                              │                                      │
//! │                              ▼                                      │
//! │  ┌───────────────────────────────────────────────────────────────┐  │
//! │  │                setu-enclave (EnclaveRuntime)                   │  │
//! │  │   • MockEnclave (dev/test)                                     │  │
//! │  │   • NitroEnclave (production)                                  │  │
//! │  └───────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────┘
//!                              │
//!                     TeeExecutionResult
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        Validator                                    │
//! │  Verifies attestation, applies state changes                       │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Principle
//!
//! **Solver does NOT**:
//! - Read state from anywhere
//! - Perform coin selection
//! - Compute state changes
//! - Maintain local state
//!
//! **Solver ONLY**:
//! - Receives fully-prepared SolverTask
//! - Passes it to TEE
//! - Returns the result

use setu_enclave::{
    Attestation, EnclaveInfo, EnclaveRuntime,
    MockEnclave, StfInput, StfOutput,
    SolverTask, GasUsage,
};
use setu_types::event::{ExecutionResult, StateChange};
use setu_types::SubnetId;
use std::sync::Arc;
use tracing::{info, debug, warn};

/// TEE Executor for Solver nodes
///
/// Wraps an `EnclaveRuntime` implementation and provides the pass-through
/// interface for executing `SolverTask` from Validator.
///
/// In solver-tee3 architecture, TeeExecutor is intentionally simple:
/// it only converts `SolverTask` to `StfInput` and forwards to the enclave.
pub struct TeeExecutor {
    solver_id: String,
    enclave: Arc<dyn EnclaveRuntime>,
}

impl TeeExecutor {
    /// Create a new TEE Executor with the default MockEnclave
    pub fn new(solver_id: String) -> Self {
        let enclave = MockEnclave::default_with_solver_id(solver_id.clone());
        
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            "Initializing TEE Executor"
        );
        
        Self {
            solver_id,
            enclave: Arc::new(enclave),
        }
    }
    
    /// Create with a custom enclave implementation
    pub fn with_enclave(solver_id: String, enclave: Arc<dyn EnclaveRuntime>) -> Self {
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            simulated = enclave.is_simulated(),
            "Initializing TEE Executor with custom enclave"
        );
        
        Self {
            solver_id,
            enclave,
        }
    }

    /// Execute a SolverTask prepared by Validator (solver-tee3 architecture)
    ///
    /// This is the **primary entry point** for Solver TEE execution.
    /// The method follows the pass-through pattern:
    ///
    /// 1. Receives `SolverTask` from Validator (contains everything needed)
    /// 2. Converts to `StfInput` (direct mapping, no additional processing)
    /// 3. Passes to TEE enclave for execution
    /// 4. Returns `TeeExecutionResult` with attestation
    ///
    /// # Arguments
    /// * `task` - Fully prepared SolverTask from Validator's TaskPreparer
    ///
    /// # Returns
    /// * `TeeExecutionResult` containing post_state_root, state_diff, and attestation
    pub async fn execute_solver_task(&self, task: SolverTask) -> anyhow::Result<TeeExecutionResult> {
        debug!(
            solver_id = %self.solver_id,
            task_id = ?hex::encode(&task.task_id[..8]),
            subnet_id = ?task.subnet_id,
            event_id = %task.event.id,
            read_set_len = task.read_set.len(),
            resolved_objects = task.resolved_inputs.input_objects.len(),
            "Executing SolverTask in TEE (pass-through)"
        );
        
        // Convert SolverTask to StfInput (direct pass-through, no modification)
        let input = StfInput::new(
            task.task_id,
            task.subnet_id.clone(),
            task.pre_state_root,
            task.resolved_inputs,
            task.gas_budget,
        )
        .with_events(vec![task.event])
        .with_read_set(task.read_set);
        
        // Execute STF in TEE
        let output = self.enclave.execute_stf(input).await
            .map_err(|e| anyhow::anyhow!("STF execution failed: {}", e))?;
        
        info!(
            solver_id = %self.solver_id,
            task_id = ?hex::encode(&output.task_id[..8]),
            events_processed = output.events_processed.len(),
            events_failed = output.events_failed.len(),
            gas_used = output.gas_usage.gas_used,
            attestation_type = ?output.attestation.attestation_type,
            "SolverTask TEE execution completed"
        );
        
        Ok(TeeExecutionResult::from_stf_output(output))
    }
    
    /// Get enclave information
    pub fn enclave_info(&self) -> EnclaveInfo {
        self.enclave.info()
    }
    
    /// Check if running in simulated mode
    pub fn is_simulated(&self) -> bool {
        self.enclave.is_simulated()
    }
    
    /// Get enclave measurement (PCR0 for Nitro, MRENCLAVE for SGX)
    pub fn measurement(&self) -> [u8; 32] {
        self.enclave.measurement()
    }
    
    /// Get solver ID
    pub fn solver_id(&self) -> &str {
        &self.solver_id
    }
}

/// Result of TEE execution
///
/// Contains all information needed by Validator to:
/// 1. Verify the attestation
/// 2. Apply state changes to the Merkle store
/// 3. Update the DAG with execution result
#[derive(Debug, Clone)]
pub struct TeeExecutionResult {
    /// Task ID for this execution (matches input for verification)
    pub task_id: [u8; 32],
    
    /// Subnet that was executed
    pub subnet_id: SubnetId,
    
    /// Post-execution state root (commitment to new state)
    pub post_state_root: [u8; 32],
    
    /// State changes to apply (key-value pairs)
    pub state_changes: Vec<StateChange>,
    
    /// Number of events successfully processed
    pub events_processed: usize,
    
    /// Number of events that failed
    pub events_failed: usize,
    
    /// Gas usage for this execution
    pub gas_usage: GasUsage,
    
    /// TEE attestation (proof of execution in enclave)
    /// Contains: task_id binding, input_hash, state roots
    pub attestation: Attestation,
    
    /// Execution time in microseconds
    pub execution_time_us: u64,
}

impl TeeExecutionResult {
    /// Convert from StfOutput (internal enclave output)
    pub fn from_stf_output(output: StfOutput) -> Self {
        // Convert StateDiff to Vec<StateChange>
        let state_changes: Vec<StateChange> = output.state_diff.writes
            .into_iter()
            .map(|w| StateChange {
                key: w.key,
                old_value: w.old_value,
                new_value: Some(w.new_value),
            })
            .chain(
                output.state_diff.deletes.into_iter().map(|k| StateChange {
                    key: k,
                    old_value: None,
                    new_value: None,
                })
            )
            .collect();
        
        Self {
            task_id: output.task_id,
            subnet_id: output.subnet_id,
            post_state_root: output.post_state_root,
            state_changes,
            events_processed: output.events_processed.len(),
            events_failed: output.events_failed.len(),
            gas_usage: output.gas_usage,
            attestation: output.attestation,
            execution_time_us: output.stats.execution_time_us,
        }
    }
    
    /// Convert to ExecutionResult (for backward compatibility with existing APIs)
    pub fn to_execution_result(&self) -> ExecutionResult {
        ExecutionResult {
            success: self.events_failed == 0,
            message: if self.events_failed == 0 {
                Some(format!("Processed {} events in {}μs", self.events_processed, self.execution_time_us))
            } else {
                Some(format!("{} events failed out of {}", self.events_failed, self.events_processed + self.events_failed))
            },
            state_changes: self.state_changes.clone(),
        }
    }
    
    /// Check if execution was successful (no failed events)
    pub fn is_success(&self) -> bool {
        self.events_failed == 0
    }
    
    /// Get task_id as hex string (for logging)
    pub fn task_id_hex(&self) -> String {
        hex::encode(&self.task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_enclave::{EnclavePlatform, ResolvedInputs, GasBudget};
    use setu_types::event::{Event, EventType, VLCSnapshot};
    use setu_types::SubnetId;
    
    fn create_test_event(_id: &str) -> Event {
        Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test-creator".to_string(),
        )
    }
    
    fn create_test_solver_task() -> SolverTask {
        let event = create_test_event("test-event");
        let task_id = SolverTask::generate_task_id(&event, &[0u8; 32]);
        
        SolverTask::new(
            task_id,
            event,
            ResolvedInputs::new(),
            [0u8; 32],
            SubnetId::ROOT,
        ).with_gas_budget(GasBudget::default())
    }
    
    #[tokio::test]
    async fn test_tee_executor_creation() {
        let executor = TeeExecutor::new("test-solver".to_string());
        let info = executor.enclave_info();
        
        assert_eq!(info.platform, EnclavePlatform::Mock);
        assert!(executor.is_simulated());
        assert_eq!(executor.solver_id(), "test-solver");
    }
    
    #[tokio::test]
    async fn test_execute_solver_task() {
        let executor = TeeExecutor::new("test-solver".to_string());
        let task = create_test_solver_task();
        let task_id = task.task_id;
        
        let result = executor.execute_solver_task(task).await;
        
        assert!(result.is_ok());
        let result = result.unwrap();
        
        assert_eq!(result.task_id, task_id);
        assert_eq!(result.events_processed, 1);
        assert_eq!(result.events_failed, 0);
        assert!(result.is_success());
        assert!(result.attestation.is_mock());
    }
    
    #[tokio::test]
    async fn test_execution_result_conversion() {
        let executor = TeeExecutor::new("test-solver".to_string());
        let task = create_test_solver_task();
        
        let result = executor.execute_solver_task(task).await.unwrap();
        let exec_result = result.to_execution_result();
        
        assert!(exec_result.success);
        assert!(exec_result.message.is_some());
    }
    
    #[test]
    fn test_task_id_hex() {
        let result = TeeExecutionResult {
            task_id: [0xab; 32],
            subnet_id: SubnetId::ROOT,
            post_state_root: [0u8; 32],
            state_changes: vec![],
            events_processed: 1,
            events_failed: 0,
            gas_usage: GasUsage::default(),
            attestation: Attestation::mock([0u8; 32]),
            execution_time_us: 100,
        };
        
        assert_eq!(result.task_id_hex(), "ab".repeat(32));
    }
}
