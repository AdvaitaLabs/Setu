//! Setu Solver - Execution node (solver-tee3 Architecture)
//!
//! In solver-tee3 architecture, Solver acts as a **pass-through** layer:
//!
//! ## Responsibilities
//! - Receives fully-prepared `SolverTask` from Validator
//! - Passes task to TEE for execution
//! - Returns `TeeExecutionResult` (with attestation) to Validator
//!
//! ## What Solver does NOT do
//! - Coin selection (done by Validator's TaskPreparer)
//! - State reads (included in SolverTask's read_set)
//! - VLC management (handled by Validator)
//! - Event creation (Validator creates Event from result)
//!
//! ## Architecture
//!
//! ```text
//! Validator (TaskPreparer)
//!     │
//!     │ SolverTask
//!     ▼
//! Solver (TeeExecutor)  ← This crate
//!     │
//!     │ StfInput
//!     ▼
//! TEE (EnclaveRuntime)
//!     │
//!     │ StfOutput
//!     ▼
//! TeeExecutionResult → Validator
//! ```

mod tee;
mod network_client;

// Core exports for solver-tee3 architecture
pub use tee::{TeeExecutor, TeeExecutionResult};
pub use setu_enclave::{
    EnclaveInfo, Attestation, SolverTask,
    ResolvedInputs, ResolvedObject, GasBudget, GasUsage,
};

// Network client for communication with Validator
pub use network_client::{
    SolverNetworkClient, SolverNetworkConfig, 
    SubmitEventRequest, SubmitEventResponse,
};

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::{Event, EventType, VLCSnapshot};
    use setu_types::SubnetId;
    
    #[tokio::test]
    async fn test_solver_tee3_flow() {
        // Create TeeExecutor
        let executor = TeeExecutor::new("test-solver".to_string());
        
        // Create a test event
        let event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test-creator".to_string(),
        );
        
        // Create SolverTask (normally prepared by Validator's TaskPreparer)
        let task_id = SolverTask::generate_task_id(&event, &[0u8; 32]);
        let task = SolverTask::new(
            task_id,
            event,
            ResolvedInputs::new(),
            [0u8; 32],
            SubnetId::ROOT,
        ).with_gas_budget(GasBudget::default());
        
        // Execute via TeeExecutor (Solver's main entry point)
        let result = executor.execute_solver_task(task).await;
        
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_success());
        assert_eq!(result.events_processed, 1);
        
        // Attestation should be included
        assert!(result.attestation.is_mock());
    }
}
