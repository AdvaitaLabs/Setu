//! TEE parallel execution logic
//!
//! This module handles:
//! - Inline Solver execution with Semaphore-controlled concurrency
//! - HTTP communication with Solver nodes
//! - Background consensus submission (fire-and-forget)
//! - Transfer status tracking updates
//! - Graceful shutdown with pending task completion
//!
//! ## TPS Optimizations
//!
//! This is the performance-critical path. The architecture uses a **split execution model**:
//!
//! 1. **Inline phase** (`execute_solver_inline`): Solver HTTP call + coin release
//!    are awaited in the HTTP handler's async context. This ensures coins are
//!    released before returning to the event loop, preventing tokio starvation.
//!
//! 2. **Background phase** (`spawn_post_execution`): Consensus submission +
//!    local storage are spawned as fire-and-forget (no coin held).
//!
//! Previous design spawned the entire TEE pipeline as a background task, which
//! caused tokio runtime starvation under load: HTTP request handling consumed
//! all runtime capacity, starving spawned tasks that held coin reservations.
//!
//! ## Panic Safety
//!
//! Uses ReservationGuard (RAII) to ensure coin reservations are released
//! even if the execution panics.

use super::types::*;
use super::solver_client::{ExecuteTaskRequest, ExecuteTaskResponse};
use crate::ConsensusValidator;
use crate::coin_reservation::{CoinReservationManager, ReservationHandle};
use dashmap::DashMap;
use parking_lot::RwLock;
use setu_types::event::Event;
use setu_types::task::SolverTask;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

/// RAII guard for coin reservation release
/// 
/// Ensures the reservation is released even if the task panics.
/// Uses `Option::take()` pattern to allow explicit release or auto-release on drop.
struct ReservationGuard {
    manager: Option<Arc<CoinReservationManager>>,
    handle: Option<ReservationHandle>,
    transfer_id: String,
}

impl ReservationGuard {
    fn new(
        manager: Option<Arc<CoinReservationManager>>,
        handle: Option<ReservationHandle>,
        transfer_id: String,
    ) -> Self {
        Self { manager, handle, transfer_id }
    }

    /// Explicitly release the reservation (called on normal completion)
    fn release(&mut self) {
        if let (Some(mgr), Some(handle)) = (self.manager.take(), self.handle.take()) {
            mgr.release(&handle);
            debug!(
                transfer_id = %self.transfer_id,
                coin_id = %hex::encode(handle.coin_id.as_bytes()),
                "Released coin reservation after TEE task completion"
            );
        }
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        // Auto-release on drop (panic safety)
        if let (Some(mgr), Some(handle)) = (self.manager.take(), self.handle.take()) {
            mgr.release(&handle);
            warn!(
                transfer_id = %self.transfer_id,
                coin_id = %hex::encode(handle.coin_id.as_bytes()),
                "Reservation released via Drop (possible panic or early return)"
            );
        }
    }
}

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
    /// Coin reservation manager for preventing cross-batch double-spending
    coin_reservation_manager: Option<Arc<CoinReservationManager>>,
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
            coin_reservation_manager: None,
        }
    }

    /// Set the coin reservation manager for cross-batch double-spend prevention
    pub fn with_coin_reservation_manager(mut self, manager: Arc<CoinReservationManager>) -> Self {
        self.coin_reservation_manager = Some(manager);
        self
    }

    /// Get the coin reservation manager reference (if set)
    pub fn coin_reservation_manager(&self) -> Option<&Arc<CoinReservationManager>> {
        self.coin_reservation_manager.as_ref()
    }

    // ============================================
    // Inline Solver Execution (TPS-optimized)
    // ============================================

    /// Execute Solver HTTP call inline (awaited in caller's async context)
    ///
    /// **This is the primary TPS optimization.** By awaiting the Solver call in
    /// the HTTP handler instead of spawning a background task, we ensure:
    ///
    /// 1. Coins are released in the HTTP handler's context (~2ms after reservation)
    /// 2. No tokio runtime starvation (no background tasks competing for CPU)
    /// 3. Natural backpressure: each HTTP connection handles one transfer at a time
    ///
    /// Returns `(Event, execution_time_us, events_processed)` on success.
    /// The coin reservation is released before returning on success.
    /// On error, the RAII guard releases the reservation on drop.
    pub async fn execute_solver_inline(
        &self,
        transfer_id: &str,
        solver_id: &str,
        task: SolverTask,
        reservation: Option<ReservationHandle>,
    ) -> Result<(Event, u64, usize), String> {
        let task_id_hex = hex::encode(&task.task_id[..8]);

        // RAII guard for panic-safe reservation release
        let mut reservation_guard = ReservationGuard::new(
            self.coin_reservation_manager.clone(),
            reservation,
            transfer_id.to_string(),
        );

        // 1. Acquire semaphore permit (backpressure)
        let _permit = self.semaphore.acquire().await
            .map_err(|_| "Service shutting down".to_string())?;

        debug!(
            transfer_id = %transfer_id,
            solver_id = %solver_id,
            task_id = %task_id_hex,
            "Executing TEE task inline (permit acquired)"
        );

        // 2. Get Solver address
        let solver_url = {
            match self.solver_info.get(solver_id) {
                Some(info) => info.execute_task_url(),
                None => return Err(format!("Solver not found: {}", solver_id)),
            }
        };

        let mut event = task.event.clone();

        // 3. Create HTTP request
        let request = ExecuteTaskRequest {
            solver_task: task,
            validator_id: self.validator_id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };

        // 4. Execute HTTP call with timeout
        let result = self.http_client
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
                            // 5. Build ExecutionResult
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

                            // 6. Release coin reservation EARLY (before returning)
                            reservation_guard.release();

                            let exec_time = result_dto.execution_time_us;
                            let events_proc = result_dto.events_processed;

                            Ok((event, exec_time, events_proc))
                        } else {
                            Err("No result in response".to_string())
                        }
                    }
                    Ok(exec_resp) => Err(exec_resp.message),
                    Err(e) => Err(format!("JSON parse error: {}", e)),
                }
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(format!("HTTP {}: {}", status, body))
            }
            Err(e) => Err(format!("Network error: {}", e)),
        }
        // reservation_guard drops here, releasing if not already released
    }

    /// Spawn post-execution work (consensus + storage) as background task
    ///
    /// This is fire-and-forget: consensus submission and local storage
    /// don't hold any coin reservations, so starvation is not an issue.
    /// Used after `execute_solver_inline()` returns successfully.
    pub fn spawn_post_execution(
        &self,
        transfer_id: String,
        event: Event,
        execution_time_us: u64,
        events_processed: usize,
    ) {
        let consensus = self.consensus.clone();
        let events_store = Arc::clone(&self.events);
        let dag_events = Arc::clone(&self.dag_events);
        let transfer_status = Arc::clone(&self.transfer_status);
        let pending_count = Arc::clone(&self.pending_count);

        pending_count.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let event_id = event.id.clone();

            // Submit to consensus (if enabled)
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

            // Store locally
            events_store.insert(event_id.clone(), event);
            dag_events.write().push(event_id.clone());

            // Update tracker to success
            Self::update_tracker_success(
                &transfer_status,
                &transfer_id,
                &event_id,
                execution_time_us,
                events_processed,
            );

            info!(
                transfer_id = %transfer_id,
                event_id = %&event_id[..20.min(event_id.len())],
                "TEE task completed successfully"
            );

            pending_count.fetch_sub(1, Ordering::Relaxed);
        });
    }

    // ============================================
    // Legacy Spawn-based Execution (for batch mode)
    // ============================================

    /// Spawn an async TEE task (non-blocking)
    ///
    /// **Note**: For single transfers, prefer `execute_solver_inline()` +
    /// `spawn_post_execution()` to avoid tokio runtime starvation.
    /// This method is kept for batch transfer processing where multiple
    /// tasks are spawned together.
    pub fn spawn_tee_task(
        &self,
        transfer_id: String,
        solver_id: String,
        task: SolverTask,
    ) {
        self.spawn_tee_task_with_reservation(transfer_id, solver_id, task, None);
    }

    /// Spawn an async TEE task with coin reservation (non-blocking)
    ///
    /// **Note**: For single transfers, prefer `execute_solver_inline()` +
    /// `spawn_post_execution()` to avoid tokio runtime starvation.
    ///
    /// Similar to `spawn_tee_task`, but also releases the coin reservation
    /// after task completion (success or failure). Used by batch processing.
    ///
    /// ## Cross-batch Double-spend Prevention
    ///
    /// When using `BatchTaskPreparer`, coins are reserved via `CoinReservationManager`
    /// before task preparation. This method ensures reservations are released after
    /// TEE execution completes, regardless of success or failure.
    pub fn spawn_tee_task_with_reservation(
        &self,
        transfer_id: String,
        solver_id: String,
        task: SolverTask,
        reservation: Option<ReservationHandle>,
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
        let reservation_mgr = self.coin_reservation_manager.clone();

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
                reservation_mgr,
                reservation,
            )
            .await;
        });
    }

    /// Execute TEE task with concurrency control
    ///
    /// This is a static method to avoid lifetime issues with spawn.
    /// All shared state is passed explicitly as Arc-wrapped parameters.
    /// 
    /// ## Panic Safety
    /// 
    /// Uses ReservationGuard (RAII) to ensure coin reservations are released
    /// even if this function panics at any point.
    #[allow(clippy::too_many_arguments)]
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
        reservation_mgr: Option<Arc<CoinReservationManager>>,
        reservation: Option<ReservationHandle>,
    ) {
        let task_id_hex = hex::encode(&task.task_id[..8]);

        // Create RAII guard for panic-safe reservation release
        // The guard will automatically release the reservation on drop (including panic)
        let mut reservation_guard = ReservationGuard::new(
            reservation_mgr,
            reservation,
            transfer_id.clone(),
        );

        // 1. Acquire semaphore permit (backpressure control)
        let _permit = match semaphore.acquire().await {
            Ok(p) => {
                pending_count.fetch_add(1, Ordering::Relaxed);
                p
            }
            Err(_) => {
                // Semaphore closed - service shutting down
                // reservation_guard will release on drop
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

                            // 5b. Release coin reservation early (after Solver success)
                            // Safety: apply_committed_events() validates old_value against
                            // current SMT state, so conflicting events from the same coin
                            // will be rejected at CF commit time.
                            reservation_guard.release();

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

        // Fallback release: if Solver failed or response was invalid,
        // the early release in step 5b was never reached.
        // ReservationGuard::release() is idempotent (no-ops if already released).
        reservation_guard.release();

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
