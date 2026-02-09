//! TEE parallel execution logic
//!
//! This module handles:
//! - Parallel TEE task execution with Semaphore-controlled concurrency
//! - HTTP communication with Solver nodes
//! - Transfer status tracking updates
//! - Graceful shutdown with pending task completion
//!
//! ## TPS Optimizations
//!
//! This is the performance-critical path:
//! - Non-blocking spawn for immediate transfer response
//! - Semaphore limits concurrent TEE calls (default: 100)
//! - Direct DashMap updates avoid mutex contention

use super::types::*;
use super::solver_client::{ExecuteTaskRequest, ExecuteTaskResponse};
use crate::ConsensusValidator;
use dashmap::DashMap;
use parking_lot::RwLock;
use setu_types::event::Event;
use setu_types::task::SolverTask;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

/// TEE Executor handles parallel TEE task execution
pub struct TeeExecutor {
    /// HTTP client for Solver communication
    http_client: reqwest::Client,
    /// Solver information registry
    solver_info: Arc<DashMap<String, SolverInfo>>,
    /// Transfer status tracking
    transfer_status: Arc<DashMap<String, TransferTracker>>,
    /// Event storage
    events: Arc<DashMap<String, Event>>,
    /// DAG events list
    dag_events: Arc<RwLock<Vec<String>>>,
    /// Consensus validator (optional)
    consensus: Option<Arc<ConsensusValidator>>,
    /// Validator ID
    validator_id: String,
    /// Concurrency limiter (default: 100 concurrent TEE calls)
    semaphore: Arc<Semaphore>,
    /// Count of pending TEE tasks
    pending_count: Arc<AtomicU64>,
}

impl TeeExecutor {
    /// Create a new TEE executor
    pub fn new(
        http_client: reqwest::Client,
        solver_info: Arc<DashMap<String, SolverInfo>>,
        transfer_status: Arc<DashMap<String, TransferTracker>>,
        events: Arc<DashMap<String, Event>>,
        dag_events: Arc<RwLock<Vec<String>>>,
        consensus: Option<Arc<ConsensusValidator>>,
        validator_id: String,
        max_concurrent: usize,
    ) -> Self {
        Self {
            http_client,
            solver_info,
            transfer_status,
            events,
            dag_events,
            consensus,
            validator_id,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            pending_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Spawn an async TEE task (non-blocking)
    ///
    /// This is the key optimization for TPS:
    /// - submit_transfer returns immediately after spawning
    /// - TEE execution happens in background with Semaphore-controlled concurrency
    /// - Status is updated via transfer_id lookup (not find())
    pub fn spawn_tee_task(
        &self,
        transfer_id: String,
        solver_id: String,
        task: SolverTask,
    ) {
        // Clone Arc-wrapped fields for the spawned task
        let semaphore = Arc::clone(&self.semaphore);
        let pending_count = Arc::clone(&self.pending_count);
        let http_client = self.http_client.clone();
        let solver_info = Arc::clone(&self.solver_info);
        let transfer_status = Arc::clone(&self.transfer_status);
        let events = Arc::clone(&self.events);
        let dag_events = Arc::clone(&self.dag_events);
        let consensus = self.consensus.clone();
        let validator_id = self.validator_id.clone();

        tokio::spawn(async move {
            Self::execute_tee_task_internal(
                transfer_id,
                solver_id,
                task,
                semaphore,
                pending_count,
                http_client,
                solver_info,
                transfer_status,
                events,
                dag_events,
                consensus,
                validator_id,
            )
            .await;
        });
    }

    /// Execute TEE task with concurrency control
    ///
    /// This is a static method to avoid lifetime issues with spawn.
    /// All shared state is passed explicitly as Arc-wrapped parameters.
    async fn execute_tee_task_internal(
        transfer_id: String,
        solver_id: String,
        task: SolverTask,
        semaphore: Arc<Semaphore>,
        pending_count: Arc<AtomicU64>,
        http_client: reqwest::Client,
        solver_info: Arc<DashMap<String, SolverInfo>>,
        transfer_status: Arc<DashMap<String, TransferTracker>>,
        events: Arc<DashMap<String, Event>>,
        dag_events: Arc<RwLock<Vec<String>>>,
        consensus: Option<Arc<ConsensusValidator>>,
        validator_id: String,
    ) {
        let task_id_hex = hex::encode(&task.task_id[..8]);

        // 1. Acquire semaphore permit (backpressure control)
        let _permit = match semaphore.acquire().await {
            Ok(p) => {
                pending_count.fetch_add(1, Ordering::Relaxed);
                p
            }
            Err(_) => {
                // Semaphore closed - service shutting down
                Self::update_tracker_failed(&transfer_status, &transfer_id, "Service shutting down");
                return;
            }
        };

        debug!(
            transfer_id = %transfer_id,
            solver_id = %solver_id,
            task_id = %task_id_hex,
            "Executing TEE task (permit acquired)"
        );

        // 2. Get Solver address
        let solver_url = {
            match solver_info.get(&solver_id) {
                Some(info) => info.execute_task_url(),
                None => {
                    Self::update_tracker_failed(
                        &transfer_status,
                        &transfer_id,
                        &format!("Solver not found: {}", solver_id),
                    );
                    pending_count.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
        };

        // Keep Event for later use
        let mut event = task.event.clone();
        let event_id = event.id.clone();

        // 3. Create HTTP request
        let request = ExecuteTaskRequest {
            solver_task: task,
            validator_id: validator_id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };

        // 4. Execute HTTP call with timeout
        let result = http_client
            .post(&solver_url)
            .json(&request)
            .timeout(Duration::from_secs(30))
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                match response.json::<ExecuteTaskResponse>().await {
                    Ok(exec_resp) if exec_resp.success => {
                        if let Some(result_dto) = exec_resp.result {
                            // 5a. Success: Build ExecutionResult and set on Event
                            let execution_result = setu_types::event::ExecutionResult {
                                success: result_dto.events_failed == 0,
                                message: Some(format!(
                                    "TEE executed: {} events in {}μs",
                                    result_dto.events_processed, result_dto.execution_time_us
                                )),
                                state_changes: result_dto
                                    .state_changes
                                    .iter()
                                    .map(|sc| setu_types::event::StateChange {
                                        key: sc.key.clone(),
                                        old_value: sc.old_value.clone(),
                                        new_value: sc.new_value.clone(),
                                    })
                                    .collect(),
                            };

                            event.set_execution_result(execution_result);
                            event.status = setu_types::event::EventStatus::Executed;

                            // 6. Submit to consensus (if enabled)
                            if let Some(ref consensus_validator) = consensus {
                                match consensus_validator.submit_event(event.clone()).await {
                                    Ok(_) => {
                                        info!(
                                            event_id = %&event_id[..20.min(event_id.len())],
                                            "Event submitted to consensus DAG"
                                        );
                                    }
                                    Err(e) => {
                                        error!(
                                            event_id = %&event_id[..20.min(event_id.len())],
                                            error = %e,
                                            "Failed to submit event to consensus"
                                        );
                                    }
                                }
                            }

                            // 7. Store locally
                            events.insert(event_id.clone(), event);
                            dag_events.write().push(event_id.clone());

                            // 8. Update tracker to success
                            Self::update_tracker_success(
                                &transfer_status,
                                &transfer_id,
                                &event_id,
                                result_dto.execution_time_us,
                                result_dto.events_processed,
                            );

                            info!(
                                transfer_id = %transfer_id,
                                task_id = %task_id_hex,
                                event_id = %&event_id[..20.min(event_id.len())],
                                "TEE task completed successfully"
                            );
                        } else {
                            Self::update_tracker_failed(
                                &transfer_status,
                                &transfer_id,
                                "No result in response",
                            );
                        }
                    }
                    Ok(exec_resp) => {
                        Self::update_tracker_failed(
                            &transfer_status,
                            &transfer_id,
                            &exec_resp.message,
                        );
                    }
                    Err(e) => {
                        Self::update_tracker_failed(
                            &transfer_status,
                            &transfer_id,
                            &format!("JSON parse error: {}", e),
                        );
                    }
                }
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Self::update_tracker_failed(
                    &transfer_status,
                    &transfer_id,
                    &format!("HTTP {}: {}", status, body),
                );
            }
            Err(e) => {
                Self::update_tracker_failed(
                    &transfer_status,
                    &transfer_id,
                    &format!("Network error: {}", e),
                );
            }
        }

        pending_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Update tracker to success status
    fn update_tracker_success(
        transfer_status: &Arc<DashMap<String, TransferTracker>>,
        transfer_id: &str,
        event_id: &str,
        execution_time_us: u64,
        events_processed: usize,
    ) {
        if let Some(mut tracker) = transfer_status.get_mut(transfer_id) {
            tracker.status = "executed".to_string();
            tracker.event_id = Some(event_id.to_string());
            tracker.processing_steps.push(setu_rpc::ProcessingStep {
                step: "tee_execution".to_string(),
                status: "completed".to_string(),
                details: Some(format!(
                    "TEE executed in {}μs, {} events processed",
                    execution_time_us, events_processed
                )),
                timestamp: super::types::current_timestamp_secs(),
            });
        }
    }

    /// Update tracker to failed status
    fn update_tracker_failed(
        transfer_status: &Arc<DashMap<String, TransferTracker>>,
        transfer_id: &str,
        error: &str,
    ) {
        error!(transfer_id = %transfer_id, error = %error, "TEE task failed");
        if let Some(mut tracker) = transfer_status.get_mut(transfer_id) {
            tracker.status = "failed".to_string();
            tracker.processing_steps.push(setu_rpc::ProcessingStep {
                step: "tee_execution".to_string(),
                status: "failed".to_string(),
                details: Some(error.to_string()),
                timestamp: super::types::current_timestamp_secs(),
            });
        }
    }

    /// Get count of pending TEE tasks
    pub fn pending_count(&self) -> u64 {
        self.pending_count.load(Ordering::Relaxed)
    }

    /// Wait for all pending TEE tasks to complete (for graceful shutdown)
    pub async fn wait_for_pending_tasks(&self, timeout: Duration) -> Result<(), &'static str> {
        let start = std::time::Instant::now();

        while self.pending_count.load(Ordering::Relaxed) > 0 {
            if start.elapsed() > timeout {
                let remaining = self.pending_count.load(Ordering::Relaxed);
                warn!("Shutdown timeout, {} TEE tasks still pending", remaining);
                return Err("Shutdown timeout");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        info!("All TEE tasks completed");
        Ok(())
    }
}

// ============================================
// Legacy Sync TEE Execution (kept for reference)
// ============================================

/// Legacy synchronous TEE execution
/// 
/// **DEPRECATED**: Use TeeExecutor::spawn_tee_task for parallel execution.
/// This is kept for backward compatibility and testing.
#[allow(dead_code)]
pub async fn send_solver_task_sync(
    http_client: &reqwest::Client,
    solver_info: &DashMap<String, SolverInfo>,
    solver_id: &str,
    task: SolverTask,
    validator_id: &str,
) -> Result<(Event, ExecuteTaskResponse), String> {
    let task_id_hex = hex::encode(&task.task_id[..8]);

    debug!(
        solver_id = %solver_id,
        task_id = %task_id_hex,
        event_id = %task.event.id,
        "Sending SolverTask via sync HTTP"
    );

    // Get Solver address
    let solver_url = {
        match solver_info.get(solver_id) {
            Some(info) => info.execute_task_url(),
            None => {
                error!(solver_id = %solver_id, "Solver not found in registry");
                return Err(format!("Solver not found: {}", solver_id));
            }
        }
    };

    // Keep Event for later use
    let mut event = task.event.clone();

    // Create request
    let request = ExecuteTaskRequest {
        solver_task: task,
        validator_id: validator_id.to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
    };

    // Send HTTP request and wait for response
    info!(
        solver_id = %solver_id,
        url = %solver_url,
        task_id = %task_id_hex,
        "Sending sync HTTP request to Solver"
    );

    let response = http_client
        .post(&solver_url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        error!(
            solver_id = %solver_id,
            status = %status,
            body = %body,
            "Solver returned error"
        );
        return Err(format!("Solver HTTP error: {} - {}", status, body));
    }

    // Parse response
    let exec_response: ExecuteTaskResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Solver response: {}", e))?;

    if !exec_response.success {
        error!(
            task_id = %task_id_hex,
            message = %exec_response.message,
            "Solver execution failed"
        );
        return Err(format!("Solver execution failed: {}", exec_response.message));
    }

    // Get result from response
    let result_dto = exec_response
        .result
        .as_ref()
        .ok_or_else(|| "Solver returned success but no result".to_string())?;

    info!(
        task_id = %task_id_hex,
        events_processed = result_dto.events_processed,
        events_failed = result_dto.events_failed,
        execution_time_us = result_dto.execution_time_us,
        "Received TEE execution result from Solver"
    );

    // Convert DTO to ExecutionResult and set on Event
    let execution_result = setu_types::event::ExecutionResult {
        success: result_dto.events_failed == 0,
        message: Some(format!(
            "TEE executed: {} events in {}μs",
            result_dto.events_processed, result_dto.execution_time_us
        )),
        state_changes: result_dto
            .state_changes
            .iter()
            .map(|sc| setu_types::event::StateChange {
                key: sc.key.clone(),
                old_value: sc.old_value.clone(),
                new_value: sc.new_value.clone(),
            })
            .collect(),
    };

    event.set_execution_result(execution_result);
    event.status = setu_types::event::EventStatus::Executed;

    Ok((event, exec_response))
}
