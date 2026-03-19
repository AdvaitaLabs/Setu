//! Solver HTTP Server for receiving SolverTasks from Validator
//!
//! This implements the **synchronous HTTP** approach for solver-tee3:
//! - Validator sends POST /api/v1/execute-task with SolverTask
//! - Solver executes in TEE
//! - Solver returns TeeExecutionResult in HTTP response
//!
//! Uses setu-transport for HTTP abstractions and DTO types.

use async_trait::async_trait;
use setu_transport::http::{
    ExecuteTaskRequest, ExecuteTaskResponse,
    ExecuteBatchRequest, ExecuteBatchResponse,
    TeeExecutionResultDto, StateChangeDto, AttestationDto,
    SolverHttpHandler, HealthResponse, SolverInfoResponse, EnclaveInfoDto,
};
use crate::tee::{TeeExecutor, TeeExecutionResult};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, error};

/// Solver HTTP Handler implementation
///
/// Implements the `SolverHttpHandler` trait from setu-transport.
pub struct SolverHandler {
    /// Solver ID
    pub solver_id: String,
    /// TEE Executor for processing tasks
    pub tee_executor: Arc<TeeExecutor>,
}

impl SolverHandler {
    /// Create a new SolverHandler
    pub fn new(solver_id: String, tee_executor: Arc<TeeExecutor>) -> Self {
        Self {
            solver_id,
            tee_executor,
        }
    }
}

#[async_trait]
impl SolverHttpHandler for SolverHandler {
    async fn execute_task(&self, request: ExecuteTaskRequest) -> ExecuteTaskResponse {
        let start = Instant::now();
        let task_id_hex = hex::encode(&request.solver_task.task_id[..8]);

        info!(
            task_id = %task_id_hex,
            event_id = %request.solver_task.event.id,
            validator_id = %request.validator_id,
            request_id = %request.request_id,
            "Received SolverTask from Validator"
        );

        // Execute in TEE
        match self.tee_executor.execute_solver_task(request.solver_task).await {
            Ok(result) => {
                let execution_time_us = start.elapsed().as_micros() as u64;

                info!(
                    task_id = %task_id_hex,
                    events_processed = result.events_processed,
                    events_failed = result.events_failed,
                    gas_used = result.gas_usage.gas_used,
                    execution_time_us = execution_time_us,
                    "TEE execution completed successfully"
                );

                // Convert to DTO
                let result_dto = convert_to_dto(&result);

                ExecuteTaskResponse::success(
                    result_dto,
                    format!(
                        "Task executed: {} events processed, {} failed",
                        result.events_processed, result.events_failed
                    ),
                    execution_time_us,
                )
            }
            Err(e) => {
                let execution_time_us = start.elapsed().as_micros() as u64;

                error!(
                    task_id = %task_id_hex,
                    error = %e,
                    "TEE execution failed"
                );

                ExecuteTaskResponse::error(
                    format!("TEE execution failed: {}", e),
                    execution_time_us,
                )
            }
        }
    }

    async fn execute_task_batch(&self, request: ExecuteBatchRequest) -> ExecuteBatchResponse {
        let total_start = Instant::now();
        let batch_id = request.batch_id.clone();
        let total = request.tasks.len();

        info!(
            batch_id = %batch_id,
            batch_size = total,
            "Processing batch task execution (parallel)"
        );

        // Phase 2: Parallel execution — each task builds an isolated local_runtime,
        // no shared mutable state between tasks during STF execution.
        let handles: Vec<_> = request.tasks.into_iter().enumerate().map(|(idx, task_req)| {
            let executor = Arc::clone(&self.tee_executor);
            tokio::spawn(async move {
                let start = Instant::now();
                let task_id_hex = hex::encode(&task_req.solver_task.task_id[..8]);
                let result = executor.execute_solver_task(task_req.solver_task).await;
                (idx, task_id_hex, start, result)
            })
        }).collect();

        let mut indexed_results: Vec<(usize, ExecuteTaskResponse)> = Vec::with_capacity(total);
        for handle in handles {
            match handle.await {
                Ok((idx, task_id_hex, start, Ok(result))) => {
                    let execution_time_us = start.elapsed().as_micros() as u64;
                    let result_dto = convert_to_dto(&result);
                    indexed_results.push((idx, ExecuteTaskResponse::success(
                        result_dto,
                        format!("Task {}: {} events processed", task_id_hex, result.events_processed),
                        execution_time_us,
                    )));
                }
                Ok((idx, task_id_hex, start, Err(e))) => {
                    let execution_time_us = start.elapsed().as_micros() as u64;
                    error!(task_id = %task_id_hex, error = %e, "Batch task failed");
                    indexed_results.push((idx, ExecuteTaskResponse::error(
                        format!("TEE execution failed: {}", e),
                        execution_time_us,
                    )));
                }
                Err(join_err) => {
                    error!(error = %join_err, "Batch task panicked");
                    indexed_results.push((indexed_results.len(), ExecuteTaskResponse::error(
                        format!("Task panicked: {}", join_err),
                        0,
                    )));
                }
            }
        }

        // Restore original order (index-aligned with Validator's BatchEntry vec)
        indexed_results.sort_by_key(|(idx, _)| *idx);
        let results: Vec<ExecuteTaskResponse> = indexed_results.into_iter().map(|(_, r)| r).collect();
        let success_count = results.iter().filter(|r| r.success).count();

        ExecuteBatchResponse {
            all_success: success_count == total,
            success_count,
            failure_count: total - success_count,
            results,
            batch_id,
            total_execution_time_us: total_start.elapsed().as_micros() as u64,
        }
    }

    async fn health(&self) -> HealthResponse {
        HealthResponse::healthy(&self.solver_id, "solver-tee3-sync-http")
            .with_metadata(serde_json::json!({
                "mode": "solver-tee3-sync-http"
            }))
    }

    async fn info(&self) -> SolverInfoResponse {
        let enclave_info = self.tee_executor.enclave_info();

        SolverInfoResponse {
            solver_id: self.solver_id.clone(),
            enclave: EnclaveInfoDto {
                id: enclave_info.enclave_id.clone(),
                version: enclave_info.version.clone(),
                platform: format!("{:?}", enclave_info.platform),
                is_simulated: enclave_info.is_simulated,
            },
            mode: "solver-tee3-sync-http".to_string(),
        }
    }
}

/// Convert TeeExecutionResult to DTO
fn convert_to_dto(result: &TeeExecutionResult) -> TeeExecutionResultDto {
    TeeExecutionResultDto {
        task_id: result.task_id,
        subnet_id: result.subnet_id.to_string(),
        post_state_root: result.post_state_root,
        state_changes: result.state_changes.iter().map(|sc| StateChangeDto {
            key: sc.key.clone(),
            old_value: sc.old_value.clone(),
            new_value: sc.new_value.clone(),
        }).collect(),
        events_processed: result.events_processed,
        events_failed: result.events_failed,
        gas_used: result.gas_usage.gas_used,
        attestation: AttestationDto {
            enclave_id: result.attestation.enclave_id.clone(),
            timestamp: result.attestation.timestamp,
            task_id_binding: result.attestation.task_id_binding,
            input_hash: result.attestation.input_hash,
            pre_state_root: result.attestation.pre_state_root,
            post_state_root: result.attestation.post_state_root,
            signature: result.attestation.signature.clone(),
        },
        execution_time_us: result.execution_time_us,
    }
}

/// Create a Solver handler with the given configuration
pub fn create_handler(solver_id: String, tee_executor: Arc<TeeExecutor>) -> Arc<SolverHandler> {
    Arc::new(SolverHandler::new(solver_id, tee_executor))
}

/// Start the Solver HTTP server
///
/// This is a convenience function that creates the handler and starts the server.
pub async fn start_server(
    address: &str,
    port: u16,
    solver_id: String,
    tee_executor: Arc<TeeExecutor>,
) -> anyhow::Result<()> {
    let handler = create_handler(solver_id, tee_executor);
    setu_transport::http::start_server(address, port, handler).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_handler() {
        let executor = Arc::new(TeeExecutor::new("test-solver".to_string()));
        let handler = create_handler("test-solver".to_string(), executor);
        // Handler creation should not panic
        assert_eq!(handler.solver_id, "test-solver");
    }
}
