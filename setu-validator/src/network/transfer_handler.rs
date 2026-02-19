//! Transfer submission and processing logic
//!
//! This module handles:
//! - Transfer request validation
//! - VLC assignment
//! - Solver routing
//! - Transfer status tracking

use super::tee_executor::TeeExecutor;
use super::types::*;
use crate::{RouterManager, TaskPreparer};
use dashmap::DashMap;
use setu_rpc::{
    GetTransferStatusResponse, ProcessingStep, SubmitTransferRequest, SubmitTransferResponse,
};
use setu_types::{AssignedVlc, Transfer, TransferType};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Transfer handler for processing transfer submissions
pub struct TransferHandler;

impl TransferHandler {
    /// Process a transfer submission request
    ///
    /// This is the main entry point for transfer processing:
    /// 1. Assign VLC time
    /// 2. Create Transfer object
    /// 3. Prepare SolverTask
    /// 4. Route to solver
    /// 5. Spawn async TEE execution
    pub async fn submit_transfer(
        validator_id: &str,
        router_manager: &RouterManager,
        task_preparer: &TaskPreparer,
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

        let transfer = Transfer::new(&transfer_id, &request.from, &request.to, request.amount)
            .with_type(transfer_type)
            .with_resources(resources)
            .with_power(10)
            .with_preferred_solver_opt(request.preferred_solver.clone())
            .with_shard_id(request.shard_id.clone())
            .with_subnet_id(request.subnet_id.clone())
            .with_assigned_vlc(assigned_vlc);

        // Step 4a: Prepare SolverTask
        let subnet_id = match &transfer.subnet_id {
            Some(subnet_str) if subnet_str != "subnet-0" => {
                warn!(subnet = %subnet_str, "Custom subnet not supported, using ROOT");
                setu_types::SubnetId::ROOT
            }
            _ => setu_types::SubnetId::ROOT,
        };

        let solver_task = match task_preparer.prepare_transfer_task(&transfer, subnet_id) {
            Ok(task) => {
                steps.push(ProcessingStep {
                    step: "prepare_task".to_string(),
                    status: "completed".to_string(),
                    details: Some(format!(
                        "SolverTask prepared: {} inputs, {} read_set",
                        task.resolved_inputs.input_objects.len(),
                        task.read_set.len()
                    )),
                    timestamp: now,
                });
                task
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

        // Step 6: Spawn async TEE task (non-blocking!)
        if let Some(ref sid) = solver_id {
            tee_executor.spawn_tee_task(transfer_id.clone(), sid.clone(), solver_task);
        }

        info!(transfer_id = %transfer_id, solver_id = ?solver_id, "Transfer submitted (TEE execution spawned)");

        SubmitTransferResponse {
            success: true,
            message: "Transfer submitted, awaiting TEE execution".to_string(),
            transfer_id: Some(transfer_id),
            solver_id,
            processing_steps: steps,
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
}
