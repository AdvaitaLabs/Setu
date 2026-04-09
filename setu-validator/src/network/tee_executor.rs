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
use super::solver_client::{
    ExecuteTaskRequest, ExecuteTaskResponse,
    ExecuteBatchRequest, ExecuteBatchResponse,
};
use crate::ConsensusValidator;
use crate::coin_reservation::{CoinReservationManager, ReservationHandle};
use dashmap::DashMap;
use parking_lot::RwLock;
use setu_types::event::Event;
use setu_types::task::SolverTask;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tracing::{debug, error, info, warn};

/// RAII guard for coin reservation release
/// 
/// Ensures the reservation is released even if the task panics.
/// Uses `Option::take()` pattern to allow explicit release or auto-release on drop.
/// Supports both single and multi-coin (NeedMerge) reservations.
struct ReservationGuard {
    manager: Option<Arc<CoinReservationManager>>,
    handles: Vec<ReservationHandle>,
    transfer_id: String,
}

impl ReservationGuard {
    fn new(
        manager: Option<Arc<CoinReservationManager>>,
        handle: Option<ReservationHandle>,
        transfer_id: String,
    ) -> Self {
        let handles = handle.into_iter().collect();
        Self { manager, handles, transfer_id }
    }

    fn new_batch(
        manager: Option<Arc<CoinReservationManager>>,
        handles: Vec<ReservationHandle>,
        transfer_id: String,
    ) -> Self {
        Self { manager, handles, transfer_id }
    }

    /// Explicitly release all reservations (called on normal completion)
    fn release(&mut self) {
        if let Some(mgr) = self.manager.take() {
            for handle in self.handles.drain(..) {
                debug!(
                    transfer_id = %self.transfer_id,
                    coin_id = %hex::encode(handle.coin_id.as_bytes()),
                    "Released coin reservation after TEE task completion"
                );
                mgr.release(&handle);
            }
        }
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        // Auto-release on drop (panic safety)
        if let Some(mgr) = self.manager.take() {
            for handle in self.handles.drain(..) {
                warn!(
                    transfer_id = %self.transfer_id,
                    coin_id = %hex::encode(handle.coin_id.as_bytes()),
                    "Reservation released via Drop (possible panic or early return)"
                );
                mgr.release(&handle);
            }
        }
    }
}

// ============================================
// Batch Collection Types
// ============================================

/// Entry sent from inline execution to batch collector
struct BatchEntry {
    /// Transfer ID for tracking
    transfer_id: String,
    /// Target Solver ID (for grouping)
    solver_id: String,
    /// Solver base URL (e.g. "http://addr:port")
    solver_base_url: String,
    /// The task request to execute
    request: ExecuteTaskRequest,
    /// The original event (for building result)
    event: Event,
    /// RAII-guarded reservation handles — auto-release on drop
    reservations: ReservationGuard,
    /// Channel to send result back to caller
    result_tx: oneshot::Sender<Result<(Event, u64, usize), String>>,
}

/// Configuration for batch collection
struct BatchConfig {
    /// Maximum tasks per batch
    max_batch_size: usize,
    /// Maximum wait time for batch collection
    batch_window: Duration,
    /// mpsc channel buffer size
    channel_buffer: usize,
    /// HTTP timeout for batch requests
    http_timeout: Duration,
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

    // ── Batch collection fields ──
    /// Channel sender for batch collection (None = batch disabled)
    batch_tx: Option<mpsc::Sender<BatchEntry>>,
    /// Whether batch collector is alive (for auto-degradation)
    batch_collector_alive: Arc<AtomicBool>,
    /// Shutdown signal sender for batch collector
    batch_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// JoinHandle for the batch collector supervisor task
    batch_collector_handle: Option<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Per-solver batch support tracking (false after 404/405)
    solver_batch_support: Arc<DashMap<String, bool>>,
}

impl TeeExecutor {
    /// Create a new TEE executor
    ///
    /// Reads batch configuration from environment variables:
    /// - `SETU_BATCH_ENABLED`: "true" to enable batch collection (default: false)
    /// - `SETU_BATCH_MAX_SIZE`: max tasks per batch (default: 20)
    /// - `SETU_BATCH_WINDOW_MS`: batch collection window in ms (default: 2)
    /// - `SETU_BATCH_HTTP_TIMEOUT_SECS`: HTTP timeout for batch requests (default: 10)
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
        let batch_enabled = std::env::var("SETU_BATCH_ENABLED")
            .map(|v| v == "true")
            .unwrap_or(false);

        let batch_collector_alive = Arc::new(AtomicBool::new(false));
        let solver_batch_support = Arc::new(DashMap::new());
        let semaphore = Arc::new(Semaphore::new(max_concurrent));

        let (batch_tx, batch_shutdown_tx, batch_collector_handle) = if batch_enabled {
            let config = BatchConfig {
                max_batch_size: std::env::var("SETU_BATCH_MAX_SIZE")
                    .ok().and_then(|v| v.parse().ok()).unwrap_or(20),
                batch_window: Duration::from_millis(
                    std::env::var("SETU_BATCH_WINDOW_MS")
                        .ok().and_then(|v| v.parse().ok()).unwrap_or(2),
                ),
                channel_buffer: 1_000,
                http_timeout: Duration::from_secs(
                    std::env::var("SETU_BATCH_HTTP_TIMEOUT_SECS")
                        .ok().and_then(|v| v.parse().ok()).unwrap_or(10),
                ),
            };

            let (tx, rx) = mpsc::channel(config.channel_buffer);
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

            let alive = batch_collector_alive.clone();
            let handle = Self::spawn_batch_collector(
                rx,
                http_client.clone(),
                semaphore.clone(),
                solver_batch_support.clone(),
                config,
                alive,
                shutdown_rx,
            );

            info!(
                "Batch collection enabled (max_size={}, window={}ms)",
                std::env::var("SETU_BATCH_MAX_SIZE").unwrap_or_else(|_| "20".into()),
                std::env::var("SETU_BATCH_WINDOW_MS").unwrap_or_else(|_| "2".into()),
            );

            (
                Some(tx),
                Some(shutdown_tx),
                Some(tokio::sync::Mutex::new(Some(handle))),
            )
        } else {
            (None, None, None)
        };

        Self {
            http_client,
            solver_info,
            transfer_status,
            events,
            dag_events,
            consensus,
            validator_id,
            semaphore,
            pending_count: Arc::new(AtomicU64::new(0)),
            coin_reservation_manager: None,
            batch_tx,
            batch_collector_alive,
            batch_shutdown_tx,
            batch_collector_handle,
            solver_batch_support,
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
        self.execute_solver_inline_batch(
            transfer_id,
            solver_id,
            task,
            reservation.into_iter().collect(),
        )
        .await
    }

    /// Execute a solver task inline with automatic batching.
    ///
    /// If batch collection is enabled and the collector is alive, routes through
    /// the batch channel. Otherwise falls back to the direct single-task path.
    ///
    /// Accepts multiple reservation handles for NeedMerge (multi-coin) scenarios.
    pub async fn execute_solver_inline_batch(
        &self,
        transfer_id: &str,
        solver_id: &str,
        task: SolverTask,
        reservations: Vec<ReservationHandle>,
    ) -> Result<(Event, u64, usize), String> {
        // Check if batch path is available
        let use_batch = self.batch_tx.is_some()
            && self.batch_collector_alive.load(Ordering::Acquire);

        if use_batch {
            let batch_tx = self.batch_tx.as_ref().unwrap();

            let solver_base_url = match self.solver_info.get(solver_id) {
                Some(info) => info.http_url(),
                None => return Err(format!("Solver not found: {}", solver_id)),
            };

            let event = task.event.clone();
            let request = ExecuteTaskRequest {
                solver_task: task,
                validator_id: self.validator_id.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
            };

            let (result_tx, result_rx) = oneshot::channel();

            let entry = BatchEntry {
                transfer_id: transfer_id.to_string(),
                solver_id: solver_id.to_string(),
                solver_base_url,
                request,
                event,
                reservations: ReservationGuard::new_batch(
                    self.coin_reservation_manager.clone(),
                    reservations,
                    transfer_id.to_string(),
                ),
                result_tx,
            };

            // Timeout-wrapped send to prevent infinite blocking on full channel
            const SEND_TIMEOUT: Duration = Duration::from_secs(5);
            match tokio::time::timeout(SEND_TIMEOUT, batch_tx.send(entry)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err("Batch channel closed".to_string()),
                Err(_elapsed) => {
                    warn!(
                        transfer_id = %transfer_id,
                        "batch_tx.send timeout after {:?}, system under heavy load",
                        SEND_TIMEOUT,
                    );
                    return Err("Batch channel send timeout".to_string());
                }
            }

            // Await batch result (blocks caller, same as direct path)
            result_rx.await
                .map_err(|_| "Batch result channel dropped".to_string())?
        } else {
            // Batch disabled or collector dead: use direct single-task path
            self.execute_solver_direct(transfer_id, solver_id, task, reservations).await
        }
    }

    /// Execute a solver task inline with batch reservation support (direct path).
    ///
    /// This is the original single-task HTTP execution logic, renamed from
    /// `execute_solver_inline_batch` to allow the new batch-aware entry point
    /// to reuse this as the fallback/direct path.
    ///
    /// Same as `execute_solver_inline` but accepts multiple reservation handles
    /// for NeedMerge (multi-coin) scenarios.
    async fn execute_solver_direct(
        &self,
        transfer_id: &str,
        solver_id: &str,
        task: SolverTask,
        reservations: Vec<ReservationHandle>,
    ) -> Result<(Event, u64, usize), String> {
        let task_id_hex = hex::encode(&task.task_id[..8]);

        // RAII guard for panic-safe reservation release
        let mut reservation_guard = ReservationGuard::new_batch(
            self.coin_reservation_manager.clone(),
            reservations,
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

        // 4. Execute HTTP call with timeout (bincode)
        let body = bincode::serialize(&request)
            .map_err(|e| format!("bincode serialize error: {}", e))?;

        let result = self.http_client
            .post(&solver_url)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .timeout(Duration::from_secs(30))
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes().await
                    .map_err(|e| format!("Failed to read response bytes: {}", e))?;
                match bincode::deserialize::<ExecuteTaskResponse>(&bytes) {
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
                                        target_subnet: None,
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
                    Err(e) => Err(format!("bincode parse error: {}", e)),
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

        // 4. Execute HTTP call with timeout (bincode)
        let body = match bincode::serialize(&request) {
            Ok(b) => b,
            Err(e) => {
                Self::update_tracker_failed(
                    &transfer_status,
                    &transfer_id,
                    &format!("bincode serialize error: {}", e),
                );
                pending_count.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        let result = http_client
            .post(&solver_url)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .timeout(Duration::from_secs(30))
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                let bytes = match response.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        Self::update_tracker_failed(
                            &transfer_status,
                            &transfer_id,
                            &format!("Failed to read response bytes: {}", e),
                        );
                        pending_count.fetch_sub(1, Ordering::Relaxed);
                        return;
                    }
                };
                match bincode::deserialize::<ExecuteTaskResponse>(&bytes) {
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
                                        target_subnet: None,
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
                            &format!("bincode parse error: {}", e),
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

    // ============================================
    // Batch Collection (Phase 1)
    // ============================================

    /// Spawn the background batch collector supervisor task.
    ///
    /// The supervisor wraps the inner collector loop with panic recovery.
    /// If the inner loop panics more than MAX_RESTARTS times, the supervisor
    /// exits and sets `alive_flag` to false, causing automatic degradation
    /// to the direct (non-batch) path.
    fn spawn_batch_collector(
        rx: mpsc::Receiver<BatchEntry>,
        http_client: reqwest::Client,
        semaphore: Arc<Semaphore>,
        solver_batch_support: Arc<DashMap<String, bool>>,
        config: BatchConfig,
        alive_flag: Arc<AtomicBool>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        // Wrap receiver in Arc<Mutex> for panic recovery re-acquisition
        let rx = Arc::new(tokio::sync::Mutex::new(rx));

        alive_flag.store(true, Ordering::Release);

        tokio::spawn(async move {
            let mut restart_count: u32 = 0;
            const MAX_RESTARTS: u32 = 10;

            loop {
                let rx_clone = rx.clone();
                let client = http_client.clone();
                let sem = semaphore.clone();
                let sbs = solver_batch_support.clone();
                let cfg_window = config.batch_window;
                let cfg_max = config.max_batch_size;
                let cfg_timeout = config.http_timeout;
                let mut shutdown = shutdown_rx.clone();

                let result = tokio::spawn(async move {
                    Self::batch_collector_loop(
                        rx_clone, client, sem, sbs,
                        cfg_window, cfg_max, cfg_timeout,
                        &mut shutdown,
                    ).await
                }).await;

                match result {
                    Ok(()) => {
                        info!("BatchCollector: channel closed, shutting down gracefully");
                        alive_flag.store(false, Ordering::Release);
                        break;
                    }
                    Err(e) => {
                        restart_count += 1;
                        error!(
                            error = %e,
                            restart_count,
                            "BatchCollector panicked! Restarting..."
                        );
                        if restart_count >= MAX_RESTARTS {
                            error!(
                                "BatchCollector exceeded max restarts ({}), \
                                 degrading to non-batch path",
                                MAX_RESTARTS,
                            );
                            alive_flag.store(false, Ordering::Release);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        })
    }

    /// Core batch collection loop (extracted for panic isolation).
    ///
    /// Normal exit = channel closed or shutdown signal (returns Ok(())).
    /// Abnormal exit = panic (captured by JoinError in supervisor).
    async fn batch_collector_loop(
        rx: Arc<tokio::sync::Mutex<mpsc::Receiver<BatchEntry>>>,
        http_client: reqwest::Client,
        semaphore: Arc<Semaphore>,
        solver_batch_support: Arc<DashMap<String, bool>>,
        batch_window: Duration,
        max_batch_size: usize,
        http_timeout: Duration,
        shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            // Phase 0: Acquire Mutex (released before group execution)
            let mut rx_guard = rx.lock().await;

            // Phase 1: Block waiting for first task, with shutdown check
            let first = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    // Drain remaining entries, signal error to callers
                    while let Ok(entry) = rx_guard.try_recv() {
                        let _ = entry.result_tx.send(Err("Service shutting down".into()));
                    }
                    return;
                }
                entry = rx_guard.recv() => {
                    match entry {
                        Some(e) => e,
                        None => return, // channel closed
                    }
                }
            };
            let mut pending: Vec<BatchEntry> = vec![first];

            // Phase 2: Non-blocking drain — grab all already-queued tasks
            while pending.len() < max_batch_size {
                match rx_guard.try_recv() {
                    Ok(e) => pending.push(e),
                    Err(_) => break,
                }
            }

            // Phase 3: Adaptive window — only wait if multiple tasks present
            if pending.len() > 1 && pending.len() < max_batch_size {
                let deadline = tokio::time::Instant::now() + batch_window;
                loop {
                    let remaining = deadline.saturating_duration_since(
                        tokio::time::Instant::now(),
                    );
                    if remaining.is_zero() || pending.len() >= max_batch_size {
                        break;
                    }
                    tokio::select! {
                        entry = rx_guard.recv() => {
                            match entry {
                                Some(e) => pending.push(e),
                                None => break, // channel closed
                            }
                        }
                        _ = tokio::time::sleep(remaining) => break,
                    }
                }
            }

            // Phase 4: Release Mutex BEFORE executing groups
            drop(rx_guard);

            if pending.is_empty() {
                continue;
            }

            // Phase 5: Group by solver_id, separating solvers that don't support batch
            let mut batch_groups: HashMap<String, Vec<BatchEntry>> = HashMap::new();
            let mut direct_entries: Vec<BatchEntry> = Vec::new();

            for entry in pending.drain(..) {
                let supports_batch = solver_batch_support
                    .get(&entry.solver_id)
                    .map(|v| *v)
                    .unwrap_or(true); // assume supported until proven otherwise

                if supports_batch {
                    batch_groups
                        .entry(entry.solver_id.clone())
                        .or_default()
                        .push(entry);
                } else {
                    direct_entries.push(entry);
                }
            }

            // Phase 6: Execute groups concurrently, AWAIT ALL (backpressure)
            let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

            // Batch groups
            for (solver_id, entries) in batch_groups {
                let client = http_client.clone();
                let sem = semaphore.clone();
                let sbs = solver_batch_support.clone();
                let timeout = http_timeout;
                handles.push(tokio::spawn(async move {
                    Self::execute_batch_group(
                        solver_id, entries, client, sem, sbs, timeout,
                    ).await;
                }));
            }

            // Direct entries (solvers marked as not supporting batch)
            for entry in direct_entries {
                let client = http_client.clone();
                handles.push(tokio::spawn(async move {
                    Self::execute_single_entry_direct(entry, client).await;
                }));
            }

            for h in handles {
                let _ = h.await;
            }
        }
    }

    /// Execute a batch of tasks for a single solver.
    ///
    /// Acquires N semaphore permits (1 per task), sends a single batch HTTP
    /// request, and scatters results back to individual callers via oneshot.
    /// On 404/405, falls back to concurrent single requests and marks
    /// the solver as not supporting batch.
    async fn execute_batch_group(
        solver_id: String,
        entries: Vec<BatchEntry>,
        http_client: reqwest::Client,
        semaphore: Arc<Semaphore>,
        solver_batch_support: Arc<DashMap<String, bool>>,
        http_timeout: Duration,
    ) {
        let batch_size = entries.len();
        let batch_url = format!("{}/api/v1/execute-task-batch", entries[0].solver_base_url);

        // Acquire N permits (1 per task)
        let _permit = match semaphore.acquire_many(batch_size as u32).await {
            Ok(p) => p,
            Err(_) => {
                for entry in entries {
                    let _ = entry.result_tx.send(Err("Service shutting down".into()));
                }
                return;
            }
        };

        // Build batch request
        let batch_id = uuid::Uuid::new_v4().to_string();
        let requests: Vec<ExecuteTaskRequest> = entries
            .iter()
            .map(|e| e.request.clone())
            .collect();

        let batch_request = ExecuteBatchRequest {
            tasks: requests,
            batch_id: batch_id.clone(),
        };

        let body = match bincode::serialize(&batch_request) {
            Ok(b) => b,
            Err(e) => {
                let err = format!("batch serialize error: {}", e);
                for entry in entries {
                    let _ = entry.result_tx.send(Err(err.clone()));
                }
                return;
            }
        };

        let result = http_client
            .post(&batch_url)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .timeout(http_timeout)
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                let bytes = match response.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        let err = format!("Failed to read batch response: {}", e);
                        for entry in entries {
                            let _ = entry.result_tx.send(Err(err.clone()));
                        }
                        return;
                    }
                };

                match bincode::deserialize::<ExecuteBatchResponse>(&bytes) {
                    Ok(batch_resp) => {
                        Self::scatter_batch_results(entries, batch_resp);
                    }
                    Err(e) => {
                        let err = format!("batch bincode parse error: {}", e);
                        for entry in entries {
                            let _ = entry.result_tx.send(Err(err.clone()));
                        }
                    }
                }
            }
            // 404/405: Solver doesn't support batch — fallback + mark
            Ok(response)
                if response.status().as_u16() == 404
                    || response.status().as_u16() == 405 =>
            {
                warn!(
                    solver_id = %solver_id,
                    status = response.status().as_u16(),
                    "Solver does not support batch endpoint, falling back to concurrent single requests"
                );

                solver_batch_support.insert(solver_id.clone(), false);

                // Concurrent fallback (not serial)
                let fallback_handles: Vec<_> = entries
                    .into_iter()
                    .map(|entry| {
                        let client = http_client.clone();
                        tokio::spawn(async move {
                            Self::execute_single_entry_direct(entry, client).await;
                        })
                    })
                    .collect();

                for h in fallback_handles {
                    let _ = h.await;
                }
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let err = format!("Batch HTTP {}: {}", status, body);
                for entry in entries {
                    let _ = entry.result_tx.send(Err(err.clone()));
                }
            }
            Err(e) => {
                let err = format!("Batch network error: {}", e);
                for entry in entries {
                    let _ = entry.result_tx.send(Err(err.clone()));
                }
            }
        }
    }

    /// Scatter batch results back to individual callers.
    fn scatter_batch_results(
        entries: Vec<BatchEntry>,
        batch_resp: ExecuteBatchResponse,
    ) {
        for (idx, mut entry) in entries.into_iter().enumerate() {
            let result = if let Some(resp) = batch_resp.results.get(idx) {
                if resp.success {
                    if let Some(ref result_dto) = resp.result {
                        let mut event = entry.event;
                        let execution_result = setu_types::event::ExecutionResult {
                            success: result_dto.events_failed == 0,
                            message: Some(format!(
                                "TEE executed: {} events in {}μs (batch)",
                                result_dto.events_processed,
                                result_dto.execution_time_us,
                            )),
                            state_changes: result_dto
                                .state_changes
                                .iter()
                                .map(|sc| setu_types::event::StateChange {
                                    key: sc.key.clone(),
                                    old_value: sc.old_value.clone(),
                                    new_value: sc.new_value.clone(),
                                        target_subnet: None,
                                })
                                .collect(),
                        };

                        event.set_execution_result(execution_result);
                        event.status = setu_types::event::EventStatus::Executed;

                        entry.reservations.release();

                        Ok((
                            event,
                            result_dto.execution_time_us,
                            result_dto.events_processed,
                        ))
                    } else {
                        Err("No result in batch response".to_string())
                    }
                } else {
                    Err(resp.message.clone())
                }
            } else {
                Err(format!("Missing result at index {}", idx))
            };

            let _ = entry.result_tx.send(result);
        }
    }

    /// Execute a single entry via the direct (non-batch) HTTP path.
    ///
    /// Used by the collector for:
    /// - Solvers marked as not supporting batch (solver_batch_support == false)
    /// - 404/405 fallback within execute_batch_group
    async fn execute_single_entry_direct(mut entry: BatchEntry, http_client: reqwest::Client) {
        let single_url = format!("{}/api/v1/execute-task", entry.solver_base_url);
        let single_body = match bincode::serialize(&entry.request) {
            Ok(b) => b,
            Err(e) => {
                let _ = entry
                    .result_tx
                    .send(Err(format!("serialize error: {}", e)));
                return;
            }
        };

        let result = http_client
            .post(&single_url)
            .header("Content-Type", "application/octet-stream")
            .body(single_body)
            .timeout(Duration::from_secs(30))
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await.unwrap_or_default();
                match bincode::deserialize::<ExecuteTaskResponse>(&bytes) {
                    Ok(exec_resp) if exec_resp.success => {
                        if let Some(result_dto) = exec_resp.result {
                            let mut event = entry.event;
                            let execution_result = setu_types::event::ExecutionResult {
                                success: result_dto.events_failed == 0,
                                message: Some(format!(
                                    "TEE executed: {} events in {}μs (fallback-single)",
                                    result_dto.events_processed, result_dto.execution_time_us,
                                )),
                                state_changes: result_dto.state_changes.iter()
                                    .map(|sc| setu_types::event::StateChange {
                                        key: sc.key.clone(),
                                        old_value: sc.old_value.clone(),
                                        new_value: sc.new_value.clone(),
                                        target_subnet: None,
                                    })
                                    .collect(),
                            };
                            event.set_execution_result(execution_result);
                            event.status = setu_types::event::EventStatus::Executed;
                            entry.reservations.release();
                            let _ = entry.result_tx.send(
                                Ok((event, result_dto.execution_time_us, result_dto.events_processed))
                            );
                        } else {
                            let _ = entry.result_tx.send(Err("No result in response".to_string()));
                        }
                    }
                    Ok(exec_resp) => {
                        let _ = entry.result_tx.send(Err(exec_resp.message));
                    }
                    Err(e) => {
                        let _ = entry
                            .result_tx
                            .send(Err(format!("bincode parse error: {}", e)));
                    }
                }
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let _ = entry
                    .result_tx
                    .send(Err(format!("HTTP {}: {}", status, body)));
            }
            Err(e) => {
                let _ = entry
                    .result_tx
                    .send(Err(format!("Network error: {}", e)));
            }
        }
    }

    /// Gracefully shutdown the batch collector.
    ///
    /// Sends shutdown signal via watch channel, then waits for the
    /// collector supervisor task to drain pending entries and exit.
    pub async fn shutdown_batch_collector(&self) {
        if let Some(ref tx) = self.batch_shutdown_tx {
            let _ = tx.send(true);
        }
        if let Some(ref handle_mutex) = self.batch_collector_handle {
            if let Some(handle) = handle_mutex.lock().await.take() {
                match tokio::time::timeout(Duration::from_secs(15), handle).await {
                    Ok(Ok(())) => info!("BatchCollector shut down cleanly"),
                    Ok(Err(e)) => warn!("BatchCollector panicked on shutdown: {}", e),
                    Err(_) => warn!("BatchCollector shutdown timed out after 15s"),
                }
            }
        }
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

    // Send HTTP request and wait for response (bincode)
    info!(
        solver_id = %solver_id,
        url = %solver_url,
        task_id = %task_id_hex,
        "Sending sync HTTP request to Solver (bincode)"
    );

    let body = bincode::serialize(&request)
        .map_err(|e| format!("bincode serialize error: {}", e))?;

    let response = http_client
        .post(&solver_url)
        .header("Content-Type", "application/octet-stream")
        .body(body)
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

    // Parse response (bincode)
    let resp_bytes = response.bytes().await
        .map_err(|e| format!("Failed to read response bytes: {}", e))?;
    let exec_response: ExecuteTaskResponse = bincode::deserialize(&resp_bytes)
        .map_err(|e| format!("Failed to parse Solver response (bincode): {}", e))?;

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
                                        target_subnet: None,
            })
            .collect(),
    };

    event.set_execution_result(execution_result);
    event.status = setu_types::event::EventStatus::Executed;

    Ok((event, exec_response))
}
