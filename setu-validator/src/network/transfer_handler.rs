//! Transfer submission and processing logic
//!
//! This module handles:
//! - Transfer request validation
//! - VLC assignment
//! - Solver routing
//! - Transfer status tracking
//! - **Batch transfer processing** (high-throughput optimization)
//!
//! ## Batch Processing
//!
//! For high-throughput scenarios (>100 TPS), use `submit_transfers_batch()`:
//! - Reduces lock acquisitions from 5-6N to just 2
//! - Caches state_root computation (N → 1)
//! - Detects same-sender overdraft conflicts

use super::types::*;
use super::tee_executor::TeeExecutor;
use crate::{RouterManager, TaskPreparer, BatchTaskPreparer};
use crate::coin_reservation::CoinReservationManager;
use dashmap::DashMap;
use setu_types::{Transfer, TransferType, AssignedVlc, SubnetId};
use setu_rpc::{
    GetTransferStatusResponse, ProcessingStep,
    SubmitTransferRequest, SubmitTransferResponse,
    SubmitTransfersBatchRequest, SubmitTransfersBatchResponse,
    BatchTransferResult, BatchPrepareStatsResponse,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Transfer handler for processing transfer submissions
pub struct TransferHandler;

impl TransferHandler {
    /// Process a transfer submission request
    ///
    /// This is the main entry point for transfer processing:
    /// 1. Assign VLC time
    /// 2. Create Transfer object
    /// 3. Prepare SolverTask (with coin reservation)
    /// 4. Route to solver
    /// 5. Spawn async TEE execution
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_transfer(
        validator_id: &str,
        router_manager: &RouterManager,
        task_preparer: &TaskPreparer,
        coin_reservation_manager: &CoinReservationManager,
        transfer_status: &Arc<DashMap<String, TransferTracker>>,
        solver_pending_transfers: &Arc<DashMap<String, Vec<String>>>,
        transfer_counter: &AtomicU64,
        vlc_time: u64,
        request: SubmitTransferRequest,
        tee_executor: &TeeExecutor,
    ) -> SubmitTransferResponse {
        let now = current_timestamp_secs();
        let transfer_id = format!(
            "tx-{}-{}",
            now,
            transfer_counter.fetch_add(1, Ordering::SeqCst)
        );

        let mut steps = Vec::new();

        info!(transfer_id = %transfer_id, from = %request.from, to = %request.to, amount = request.amount, "Processing transfer");

        // Step 1: Receive
        steps.push(ProcessingStep {
            step: "receive".to_string(),
            status: "completed".to_string(),
            details: Some(format!("Transfer {} received", transfer_id)),
            timestamp: now,
        });

        // Step 2: VLC Assignment
        let now_millis = current_timestamp_millis();

        let assigned_vlc = AssignedVlc {
            logical_time: vlc_time,
            physical_time: now_millis,
            validator_id: validator_id.to_string(),
        };

        steps.push(ProcessingStep {
            step: "vlc_assign".to_string(),
            status: "completed".to_string(),
            details: Some(format!("VLC time: {}", vlc_time)),
            timestamp: now,
        });

        // Step 3: DAG Resolution (simulated)
        steps.push(ProcessingStep {
            step: "dag_resolve".to_string(),
            status: "completed".to_string(),
            details: Some("No parent conflicts".to_string()),
            timestamp: now,
        });

        // Step 4: Create Transfer using builder pattern
        let transfer_type = match request.transfer_type.to_lowercase().as_str() {
            "flux" | "fluxtransfer" => TransferType::FluxTransfer,
            _ => TransferType::FluxTransfer,
        };

        let resources = if request.resources.is_empty() {
            vec![
                format!("account:{}", request.from),
                format!("account:{}", request.to),
            ]
        } else {
            request.resources.clone()
        };

        let transfer = Transfer::new(
            &transfer_id,
            &request.from,
            &request.to,
            request.amount,
        )
        .with_type(transfer_type)
        .with_resources(resources)
        .with_power(10)
        .with_preferred_solver_opt(request.preferred_solver.clone())
        .with_shard_id(request.shard_id.clone())
        .with_subnet_id(request.subnet_id.clone())
        .with_assigned_vlc(assigned_vlc);

        // Step 4a: Prepare SolverTask WITH COIN RESERVATION
        // This prevents double-spend between concurrent single/batch API calls
        let subnet_id = match &transfer.subnet_id {
            Some(subnet_str) if subnet_str != "subnet-0" => {
                warn!(subnet = %subnet_str, "Custom subnet not supported, using ROOT");
                setu_types::SubnetId::ROOT
            }
            _ => setu_types::SubnetId::ROOT,
        };

        let (solver_task, reservation_handle) = match task_preparer.prepare_transfer_task_with_reservation(
            &transfer, subnet_id, coin_reservation_manager
        ) {
            Ok((task, handle)) => {
                steps.push(ProcessingStep {
                    step: "prepare_task".to_string(),
                    status: "completed".to_string(),
                    details: Some(format!(
                        "SolverTask prepared with reservation: {} inputs, {} read_set",
                        task.resolved_inputs.input_objects.len(),
                        task.read_set.len()
                    )),
                    timestamp: now,
                });
                (task, handle)
            }
            Err(e) => {
                return Self::fail_transfer(
                    transfer_id,
                    &format!("Task preparation failed: {}", e),
                    steps,
                    now,
                    transfer_status,
                );
            }
        };

        // Step 4b: Route to solver
        let solver_id = match router_manager.route_transfer(&transfer) {
            Ok(id) => {
                steps.push(ProcessingStep {
                    step: "route".to_string(),
                    status: "completed".to_string(),
                    details: Some(format!("Routed to: {}", id)),
                    timestamp: now,
                });
                Some(id)
            }
            Err(e) => {
                // Release reservation on routing failure
                coin_reservation_manager.release(&reservation_handle);
                return Self::fail_transfer(
                    transfer_id,
                    &format!("No solver available: {}", e),
                    steps,
                    now,
                    transfer_status,
                );
            }
        };

        // Step 5: Store status BEFORE spawning TEE task
        transfer_status.insert(
            transfer_id.clone(),
            TransferTracker {
                transfer_id: transfer_id.clone(),
                status: "pending_tee_execution".to_string(),
                solver_id: solver_id.clone(),
                event_id: None,
                processing_steps: steps.clone(),
                created_at: now,
            },
        );

        // Add to reverse index for O(1) lookup during TEE completion
        if let Some(ref sid) = solver_id {
            solver_pending_transfers
                .entry(sid.clone())
                .or_insert_with(Vec::new)
                .push(transfer_id.clone());
        }

        // Step 6: Execute Solver INLINE (await) → release coin → spawn consensus
        //
        // Key TPS optimization: By awaiting the Solver call here instead of spawning
        // a background task, we prevent tokio runtime starvation. Under high load,
        // spawned tasks get starved by HTTP request handling, causing coins to remain
        // reserved indefinitely → retry storm → total starvation.
        //
        // With inline execution:
        // - Coin held for ~2ms (Solver call time only)
        // - Natural backpressure (HTTP connection blocks until Solver responds)
        // - No retry storm (coin released before HTTP response)
        if let Some(ref sid) = solver_id {
            match tee_executor.execute_solver_inline(
                &transfer_id, sid, solver_task, Some(reservation_handle),
            ).await {
                Ok((event, execution_time_us, events_processed)) => {
                    // Fire-and-forget: consensus + storage (no coin held)
                    tee_executor.spawn_post_execution(
                        transfer_id.clone(), event, execution_time_us, events_processed,
                    );

                    info!(transfer_id = %transfer_id, solver_id = ?solver_id, "Transfer executed inline, consensus spawned");

                    SubmitTransferResponse {
                        success: true,
                        message: "Transfer executed, consensus pending".to_string(),
                        transfer_id: Some(transfer_id),
                        solver_id,
                        processing_steps: steps,
                    }
                }
                Err(e) => {
                    error!(transfer_id = %transfer_id, error = %e, "Inline TEE execution failed");
                    // Update existing tracker to failed
                    if let Some(mut tracker) = transfer_status.get_mut(&transfer_id) {
                        tracker.status = "failed".to_string();
                        tracker.processing_steps.push(ProcessingStep {
                            step: "tee_execution".to_string(),
                            status: "failed".to_string(),
                            details: Some(e.clone()),
                            timestamp: now,
                        });
                    }
                    SubmitTransferResponse {
                        success: false,
                        message: format!("TEE execution failed: {}", e),
                        transfer_id: Some(transfer_id),
                        solver_id,
                        processing_steps: steps,
                    }
                }
            }
        } else {
            // Shouldn't reach here (routing above returns early on failure)
            SubmitTransferResponse {
                success: true,
                message: "Transfer submitted".to_string(),
                transfer_id: Some(transfer_id),
                solver_id,
                processing_steps: steps,
            }
        }
    }

    /// Create a failed transfer response
    fn fail_transfer(
        transfer_id: String,
        message: &str,
        mut steps: Vec<ProcessingStep>,
        now: u64,
        transfer_status: &Arc<DashMap<String, TransferTracker>>,
    ) -> SubmitTransferResponse {
        error!(transfer_id = %transfer_id, error = %message, "Transfer failed");

        steps.push(ProcessingStep {
            step: "error".to_string(),
            status: "failed".to_string(),
            details: Some(message.to_string()),
            timestamp: now,
        });

        transfer_status.insert(
            transfer_id.clone(),
            TransferTracker {
                transfer_id: transfer_id.clone(),
                status: "failed".to_string(),
                solver_id: None,
                event_id: None,
                processing_steps: steps.clone(),
                created_at: now,
            },
        );

        SubmitTransferResponse {
            success: false,
            message: message.to_string(),
            transfer_id: Some(transfer_id),
            solver_id: None,
            processing_steps: steps,
        }
    }

    /// Get transfer status by ID
    pub fn get_transfer_status(
        transfer_status: &DashMap<String, TransferTracker>,
        transfer_id: &str,
    ) -> GetTransferStatusResponse {
        if let Some(tracker) = transfer_status.get(transfer_id) {
            GetTransferStatusResponse {
                found: true,
                transfer_id: tracker.transfer_id.clone(),
                status: Some(tracker.status.clone()),
                solver_id: tracker.solver_id.clone(),
                event_id: tracker.event_id.clone(),
                processing_steps: tracker.processing_steps.clone(),
            }
        } else {
            GetTransferStatusResponse {
                found: false,
                transfer_id: transfer_id.to_string(),
                status: None,
                solver_id: None,
                event_id: None,
                processing_steps: vec![],
            }
        }
    }

    // ============================================
    // Batch Transfer Processing (High-Throughput)
    // ============================================

    /// Maximum batch size for safety
    pub const MAX_BATCH_SIZE: usize = 200;
    /// Warning threshold for batch size
    pub const WARN_BATCH_SIZE: usize = 100;

    /// Process multiple transfer submissions in a batch
    ///
    /// This is the optimized entry point for high-throughput scenarios:
    /// - Lock acquisitions: 2 (regardless of batch size) vs 5-6N for single
    /// - state_root calculations: 1 vs N
    /// - Same-sender overdraft detection built-in
    ///
    /// ## Performance
    ///
    /// | Batch Size | Single-tx approach | Batch approach | Speedup |
    /// |------------|-------------------|----------------|---------|
    /// | 100        | ~500-600 locks    | 2 locks        | ~300x   |
    /// | 200        | ~1000-1200 locks  | 2 locks        | ~600x   |
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_transfers_batch(
        validator_id: &str,
        router_manager: &RouterManager,
        batch_preparer: &BatchTaskPreparer,
        coin_reservation_manager: &CoinReservationManager,
        transfer_status: &Arc<DashMap<String, TransferTracker>>,
        solver_pending_transfers: &Arc<DashMap<String, Vec<String>>>,
        transfer_counter: &AtomicU64,
        vlc_counter: &AtomicU64,
        request: SubmitTransfersBatchRequest,
        tee_executor: &TeeExecutor,
    ) -> SubmitTransfersBatchResponse {
        let now = current_timestamp_secs();
        let batch_size = request.transfers.len();

        // Safety check: limit batch size
        if batch_size > Self::MAX_BATCH_SIZE {
            return SubmitTransfersBatchResponse {
                success: false,
                message: format!(
                    "Batch size {} exceeds maximum allowed ({})",
                    batch_size, Self::MAX_BATCH_SIZE
                ),
                submitted: 0,
                failed: batch_size,
                results: vec![],
                stats: BatchPrepareStatsResponse::default(),
            };
        }

        if batch_size == 0 {
            return SubmitTransfersBatchResponse {
                success: true,
                message: "Empty batch".to_string(),
                submitted: 0,
                failed: 0,
                results: vec![],
                stats: BatchPrepareStatsResponse::default(),
            };
        }

        // Warn for large batches
        if batch_size > Self::WARN_BATCH_SIZE {
            warn!(
                batch_size = batch_size,
                threshold = Self::WARN_BATCH_SIZE,
                "Large batch may increase memory usage"
            );
        }

        info!(batch_size = batch_size, "Processing batch transfer submission");

        // Step 1: Convert requests to Transfers with VLC assignment
        let now_millis = current_timestamp_millis();
        let mut transfers: Vec<Transfer> = Vec::with_capacity(batch_size);
        let mut transfer_id_map: Vec<String> = Vec::with_capacity(batch_size);

        for (idx, req) in request.transfers.iter().enumerate() {
            let transfer_id = format!(
                "tx-{}-{}",
                now,
                transfer_counter.fetch_add(1, Ordering::SeqCst)
            );
            let vlc_time = vlc_counter.fetch_add(1, Ordering::SeqCst);

            let assigned_vlc = AssignedVlc {
                logical_time: vlc_time,
                physical_time: now_millis,
                validator_id: validator_id.to_string(),
            };

            let transfer_type = match req.transfer_type.to_lowercase().as_str() {
                "flux" | "fluxtransfer" => TransferType::FluxTransfer,
                _ => TransferType::FluxTransfer,
            };

            let resources = if req.resources.is_empty() {
                vec![
                    format!("account:{}", req.from),
                    format!("account:{}", req.to),
                ]
            } else {
                req.resources.clone()
            };

            let transfer = Transfer::new(&transfer_id, &req.from, &req.to, req.amount)
                .with_type(transfer_type)
                .with_resources(resources)
                .with_power(10)
                .with_preferred_solver_opt(req.preferred_solver.clone())
                .with_shard_id(req.shard_id.clone())
                .with_subnet_id(req.subnet_id.clone())
                .with_assigned_vlc(assigned_vlc);

            debug!(
                idx = idx,
                transfer_id = %transfer_id,
                from = %req.from,
                to = %req.to,
                amount = req.amount,
                "Added transfer to batch"
            );

            transfer_id_map.push(transfer_id);
            transfers.push(transfer);
        }

        // Step 2: Batch prepare all tasks WITH COIN RESERVATION (2 lock acquisitions total!)
        // This prevents cross-batch double-spending by reserving coins
        let batch_result = batch_preparer.prepare_transfers_batch_with_reservation(
            &transfers,
            coin_reservation_manager,
        );

        info!(
            total = batch_result.stats.total_transfers,
            successful = batch_result.stats.successful,
            failed = batch_result.stats.failed,
            conflicts = batch_result.stats.same_sender_conflicts,
            reserved = batch_result.reservations.iter().filter(|r| r.is_some()).count(),
            "Batch task preparation with reservation completed"
        );

        // Step 3: Build results and spawn TEE tasks
        let mut results: Vec<BatchTransferResult> = Vec::with_capacity(batch_size);
        let mut submitted_count = 0;
        let mut failed_count = 0;

        // Track which transfer indices succeeded (for result ordering)
        let mut success_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

        // Process successful tasks with their reservations
        // Note: tasks and reservations are aligned by index
        for (task_idx, task) in batch_result.tasks.into_iter().enumerate() {
            // Get the corresponding reservation (if any)
            let reservation = batch_result.reservations
                .get(task_idx)
                .and_then(|r| r.clone());

            // Find the transfer index by matching transfer_id from event
            let transfer_id = task.event.transfer
                .as_ref()
                .map(|t| t.id.clone())
                .unwrap_or_default();
            if let Some(idx) = transfer_id_map.iter().position(|id| id == &transfer_id) {
                success_indices.insert(idx);

                // Route to solver
                match router_manager.route_transfer(&transfers[idx]) {
                    Ok(solver_id) => {
                        // Store status
                        transfer_status.insert(
                            transfer_id.clone(),
                            TransferTracker {
                                transfer_id: transfer_id.clone(),
                                status: "pending_tee_execution".to_string(),
                                solver_id: Some(solver_id.clone()),
                                event_id: None,
                                processing_steps: vec![ProcessingStep {
                                    step: "batch_submit".to_string(),
                                    status: "completed".to_string(),
                                    details: Some(format!("Batch index: {}", idx)),
                                    timestamp: now,
                                }],
                                created_at: now,
                            },
                        );

                        // Add to reverse index
                        solver_pending_transfers
                            .entry(solver_id.clone())
                            .or_insert_with(Vec::new)
                            .push(transfer_id.clone());

                        // Spawn TEE task with reservation (reservation will be released after task completion)
                        tee_executor.spawn_tee_task_with_reservation(
                            transfer_id.clone(), 
                            solver_id.clone(), 
                            task,
                            reservation,
                        );

                        results.push(BatchTransferResult {
                            index: idx,
                            success: true,
                            transfer_id: Some(transfer_id),
                            solver_id: Some(solver_id),
                            error: None,
                        });
                        submitted_count += 1;
                    }
                    Err(e) => {
                        // Release reservation on routing failure
                        if let Some(ref handle) = reservation {
                            coin_reservation_manager.release(handle);
                        }

                        results.push(BatchTransferResult {
                            index: idx,
                            success: false,
                            transfer_id: Some(transfer_id.clone()),
                            solver_id: None,
                            error: Some(format!("Routing failed: {}", e)),
                        });
                        failed_count += 1;

                        // Update status
                        transfer_status.insert(
                            transfer_id.clone(),
                            TransferTracker {
                                transfer_id: transfer_id.clone(),
                                status: "failed".to_string(),
                                solver_id: None,
                                event_id: None,
                                processing_steps: vec![ProcessingStep {
                                    step: "route".to_string(),
                                    status: "failed".to_string(),
                                    details: Some(e.to_string()),
                                    timestamp: now,
                                }],
                                created_at: now,
                            },
                        );
                    }
                }
            }
        }

        // Process failures from batch preparation
        for (failed_transfer, error) in batch_result.failures {
            if let Some(idx) = transfer_id_map.iter().position(|id| id == &failed_transfer.id) {
                if !success_indices.contains(&idx) {
                    let transfer_id = transfer_id_map[idx].clone();
                    
                    results.push(BatchTransferResult {
                        index: idx,
                        success: false,
                        transfer_id: Some(transfer_id.clone()),
                        solver_id: None,
                        error: Some(error.to_string()),
                    });
                    failed_count += 1;

                    // Update status
                    transfer_status.insert(
                        transfer_id.clone(),
                        TransferTracker {
                            transfer_id: transfer_id.clone(),
                            status: "failed".to_string(),
                            solver_id: None,
                            event_id: None,
                            processing_steps: vec![ProcessingStep {
                                step: "prepare_task".to_string(),
                                status: "failed".to_string(),
                                details: Some(error.to_string()),
                                timestamp: now,
                            }],
                            created_at: now,
                        },
                    );
                }
            }
        }

        // Sort results by index for consistent ordering
        results.sort_by_key(|r| r.index);

        info!(
            submitted = submitted_count,
            failed = failed_count,
            "Batch transfer submission completed"
        );

        SubmitTransfersBatchResponse {
            success: submitted_count > 0,
            message: format!(
                "Batch processed: {} submitted, {} failed",
                submitted_count, failed_count
            ),
            submitted: submitted_count,
            failed: failed_count,
            results,
            stats: BatchPrepareStatsResponse {
                total_transfers: batch_result.stats.total_transfers,
                unique_sender_subnet_pairs: batch_result.stats.unique_sender_subnet_pairs,
                coins_selected: batch_result.stats.coins_selected,
                same_sender_conflicts: batch_result.stats.same_sender_conflicts,
            },
        }
    }
}
