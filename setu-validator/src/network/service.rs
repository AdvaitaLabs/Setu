//! Core ValidatorNetworkService implementation
//!
//! This is the main service handling network operations for the Validator.
//! Now with integrated consensus support.

use super::registration::ValidatorRegistrationHandler;
use super::types::*;
use super::solver_client::{ExecuteTaskRequest, ExecuteTaskResponse};
use crate::{RouterManager, TaskPreparer, ConsensusValidator};
use axum::{
    routing::{get, post},
    Router,
};
use setu_types::{Transfer, TransferType, AssignedVlc};
use parking_lot::RwLock;
use setu_rpc::{
    GetTransferStatusResponse, ProcessingStep, RegisterSolverRequest,
    SubmitTransferRequest, SubmitTransferResponse, ValidatorListItem,
    RegistrationHandler,
};
use setu_types::event::{Event, EventPayload};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// Import API handlers
use setu_api;

/// Validator network service
///
/// Core service handling:
/// - Solver/Validator registration
/// - Transfer submission and routing
/// - Consensus integration (CF proposal and voting)
/// - Event verification and DAG management
/// - State queries (Scheme B)
pub struct ValidatorNetworkService {
    /// Validator ID
    validator_id: String,

    /// Router manager for solver management
    router_manager: Arc<RouterManager>,

    /// Task preparer for solver-tee3 architecture
    task_preparer: Arc<TaskPreparer>,

    /// Consensus validator (optional)
    consensus_validator: Option<Arc<ConsensusValidator>>,

    /// Registered validators
    validators: Arc<RwLock<HashMap<String, ValidatorInfo>>>,

    /// Registered solver information (for sync HTTP calls)
    solver_info: Arc<RwLock<HashMap<String, SolverInfo>>>,

    /// Solver channels for sending SolverTasks (legacy, kept for compatibility)
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<setu_types::task::SolverTask>>>>,

    /// HTTP client for sync Solver calls
    http_client: reqwest::Client,

    /// Transfer tracking
    transfer_status: Arc<RwLock<HashMap<String, TransferTracker>>>,

    /// Event storage
    events: Arc<RwLock<HashMap<String, Event>>>,

    /// Pending event queue
    pending_events: Arc<RwLock<Vec<String>>>,

    /// Verified events in DAG order
    dag_events: Arc<RwLock<Vec<String>>>,

    /// Configuration
    config: NetworkServiceConfig,

    /// Start time
    start_time: u64,

    /// Counters
    transfer_counter: AtomicU64,
    vlc_counter: AtomicU64,
    event_counter: AtomicU64,
}

impl ValidatorNetworkService {
    /// Create a new validator network service
    pub fn new(
        validator_id: String,
        router_manager: Arc<RouterManager>,
        task_preparer: Arc<TaskPreparer>,
        config: NetworkServiceConfig,
    ) -> Self {
        let start_time = current_timestamp_secs();

        info!(
            validator_id = %validator_id,
            http_addr = %config.http_listen_addr,
            p2p_addr = %config.p2p_listen_addr,
            "Creating validator network service"
        );

        // Create HTTP client for sync Solver calls
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))  // TEE execution may take time
            .build()
            .expect("Failed to create HTTP client");

        Self {
            validator_id,
            router_manager,
            task_preparer,
            consensus_validator: None,
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_info: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            http_client,
            transfer_status: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            pending_events: Arc::new(RwLock::new(Vec::new())),
            dag_events: Arc::new(RwLock::new(Vec::new())),
            config,
            start_time,
            transfer_counter: AtomicU64::new(0),
            vlc_counter: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
        }
    }
    
    /// Create with consensus enabled
    pub fn with_consensus(
        validator_id: String,
        router_manager: Arc<RouterManager>,
        task_preparer: Arc<TaskPreparer>,
        consensus_validator: Arc<ConsensusValidator>,
        config: NetworkServiceConfig,
    ) -> Self {
        let start_time = current_timestamp_secs();

        info!(
            validator_id = %validator_id,
            http_addr = %config.http_listen_addr,
            p2p_addr = %config.p2p_listen_addr,
            consensus_enabled = true,
            "Creating validator network service with consensus"
        );

        // Create HTTP client for sync Solver calls
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            validator_id,
            router_manager,
            task_preparer,
            consensus_validator: Some(consensus_validator),
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_info: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            http_client,
            transfer_status: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            pending_events: Arc::new(RwLock::new(Vec::new())),
            dag_events: Arc::new(RwLock::new(Vec::new())),
            config,
            start_time,
            transfer_counter: AtomicU64::new(0),
            vlc_counter: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
        }
    }

    // ============================================
    // Accessors
    // ============================================

    pub fn validator_id(&self) -> &str {
        &self.validator_id
    }

    pub fn router_manager(&self) -> &RouterManager {
        &self.router_manager
    }
    
    pub fn consensus_validator(&self) -> Option<&Arc<ConsensusValidator>> {
        self.consensus_validator.as_ref()
    }
    
    /// Check if consensus is enabled
    pub fn consensus_enabled(&self) -> bool {
        self.consensus_validator.is_some()
    }

    pub fn start_time(&self) -> u64 {
        self.start_time
    }

    pub fn solver_count(&self) -> usize {
        self.router_manager.solver_count()
    }

    pub fn validator_count(&self) -> usize {
        self.validators.read().len()
    }

    pub fn dag_events_count(&self) -> usize {
        self.dag_events.read().len()
    }

    pub fn pending_events_count(&self) -> usize {
        self.pending_events.read().len()
    }

    /// Get the next VLC time
    /// 
    /// If consensus is enabled, returns the current VLC logical time from ConsensusValidator.
    /// Otherwise, uses the local vlc_counter (legacy mode).
    pub async fn get_vlc_time(&self) -> u64 {
        if let Some(ref consensus) = self.consensus_validator {
            consensus.vlc_snapshot().await.logical_time
        } else {
            // Legacy mode: use local counter
        self.vlc_counter.fetch_add(1, Ordering::SeqCst)
        }
    }

    // ============================================
    // Registration Handler
    // ============================================

    pub fn registration_handler(self: &Arc<Self>) -> Arc<ValidatorRegistrationHandler> {
        Arc::new(ValidatorRegistrationHandler {
            service: self.clone(),
        })
    }

    // ============================================
    // User Handler
    // ============================================

    pub fn user_handler(self: &Arc<Self>) -> Arc<crate::ValidatorUserHandler> {
        Arc::new(crate::ValidatorUserHandler::new(self.clone()))
    }

    // ============================================
    // HTTP Server
    // ============================================

    pub async fn start_http_server(
        self: Arc<Self>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = self.clone();

        let app = Router::new()
            // Registration endpoints
            .route("/api/v1/register/solver", post(setu_api::http_register_solver::<ValidatorNetworkService>))
            .route("/api/v1/register/validator", post(setu_api::http_register_validator::<ValidatorNetworkService>))
            // Query endpoints
            .route("/api/v1/solvers", get(setu_api::http_get_solvers::<ValidatorNetworkService>))
            .route("/api/v1/validators", get(setu_api::http_get_validators::<ValidatorNetworkService>))
            .route("/api/v1/health", get(setu_api::http_health::<ValidatorNetworkService>))
            // State query endpoints (Scheme B)
            .route("/api/v1/state/balance/:account", get(setu_api::http_get_balance::<ValidatorNetworkService>))
            .route("/api/v1/state/object/:key", get(setu_api::http_get_object::<ValidatorNetworkService>))
            // Transfer endpoints
            .route("/api/v1/transfer", post(setu_api::http_submit_transfer::<ValidatorNetworkService>))
            .route("/api/v1/transfer/status", post(setu_api::http_get_transfer_status::<ValidatorNetworkService>))
            // Event endpoints
            .route("/api/v1/event", post(setu_api::http_submit_event::<ValidatorNetworkService>))
            .route("/api/v1/events", get(setu_api::http_get_events::<ValidatorNetworkService>))
            // Heartbeat
            .route("/api/v1/heartbeat", post(setu_api::http_heartbeat::<ValidatorNetworkService>))
            // User RPC endpoints
            .route("/api/v1/user/register", post(setu_api::http_register_user::<ValidatorNetworkService>))
            .route("/api/v1/user/account", post(setu_api::http_get_account::<ValidatorNetworkService>))
            .route("/api/v1/user/balance", post(setu_api::http_get_user_balance::<ValidatorNetworkService>))
            .route("/api/v1/user/power", post(setu_api::http_get_power::<ValidatorNetworkService>))
            .route("/api/v1/user/credit", post(setu_api::http_get_credit::<ValidatorNetworkService>))
            .route("/api/v1/user/credentials", post(setu_api::http_get_credentials::<ValidatorNetworkService>))
            .route("/api/v1/user/transfer", post(setu_api::http_user_transfer::<ValidatorNetworkService>))
            .with_state(service);

        let listener = tokio::net::TcpListener::bind(self.config.http_listen_addr).await?;

        info!(addr = %self.config.http_listen_addr, "HTTP API server started");

        axum::serve(listener, app).await?;

        Ok(())
    }

    // ============================================
    // Transfer Processing
    // ============================================

    pub async fn submit_transfer(&self, request: SubmitTransferRequest) -> SubmitTransferResponse {
        let now = current_timestamp_secs();
        let transfer_id = format!(
            "tx-{}-{}",
            now,
            self.transfer_counter.fetch_add(1, Ordering::SeqCst)
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

        // Step 2: VLC Assignment (from consensus if enabled, otherwise local counter)
        let vlc_time = self.get_vlc_time().await;
        let now_millis = current_timestamp_millis();

        let assigned_vlc = AssignedVlc {
            logical_time: vlc_time,
            physical_time: now_millis,
            validator_id: self.validator_id.clone(),
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

        // Step 4a: Prepare SolverTask
        let subnet_id = match &transfer.subnet_id {
            Some(subnet_str) if subnet_str != "subnet-0" => {
                warn!(subnet = %subnet_str, "Custom subnet not supported, using ROOT");
                setu_types::SubnetId::ROOT
            }
            _ => setu_types::SubnetId::ROOT,
        };

        let solver_task = match self.task_preparer.prepare_transfer_task(&transfer, subnet_id) {
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
                return self.fail_transfer(transfer_id, &format!("Task preparation failed: {}", e), steps, now);
            }
        };

        // Step 4b: Route to solver
        let solver_id = match self.router_manager.route_transfer(&transfer) {
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
                return self.fail_transfer(transfer_id, &format!("No solver available: {}", e), steps, now);
            }
        };

        // Step 5: Send SolverTask
        if let Some(ref sid) = solver_id {
            match self.send_solver_task_to_solver(sid, solver_task).await {
                Ok(()) => {
                    steps.push(ProcessingStep {
                        step: "dispatch".to_string(),
                        status: "completed".to_string(),
                        details: Some("SolverTask dispatched".to_string()),
                        timestamp: now,
                    });
                }
                Err(e) => {
                    steps.push(ProcessingStep {
                        step: "dispatch".to_string(),
                        status: "failed".to_string(),
                        details: Some(format!("Dispatch error: {}", e)),
                        timestamp: now,
                    });
                }
            }
        }

        // Step 6-7: Simulated steps
        steps.push(ProcessingStep {
            step: "consensus_prepare".to_string(),
            status: "completed".to_string(),
            details: Some("Single validator mode".to_string()),
            timestamp: now,
        });

        steps.push(ProcessingStep {
            step: "awaiting_tee_result".to_string(),
            status: "pending".to_string(),
            details: Some("Waiting for TEE execution".to_string()),
            timestamp: now,
        });

        // Store status
        self.transfer_status.write().insert(
            transfer_id.clone(),
            TransferTracker {
                transfer_id: transfer_id.clone(),
                status: "pending_execution".to_string(),
                solver_id: solver_id.clone(),
                event_id: None,
                processing_steps: steps.clone(),
                created_at: now,
            },
        );

        info!(transfer_id = %transfer_id, solver_id = ?solver_id, "Transfer submitted");

        SubmitTransferResponse {
            success: true,
            message: "Transfer submitted successfully".to_string(),
            transfer_id: Some(transfer_id),
            solver_id,
            processing_steps: steps,
        }
    }

    /// Helper to create a failed transfer response
    fn fail_transfer(
        &self,
        transfer_id: String,
        message: &str,
        mut steps: Vec<ProcessingStep>,
        now: u64,
    ) -> SubmitTransferResponse {
        error!(transfer_id = %transfer_id, error = %message, "Transfer failed");

        steps.push(ProcessingStep {
            step: "error".to_string(),
            status: "failed".to_string(),
            details: Some(message.to_string()),
            timestamp: now,
        });

        self.transfer_status.write().insert(
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

    /// Send SolverTask to Solver via synchronous HTTP
    ///
    /// This is the **sync HTTP** approach for solver-tee3:
    /// 1. POST SolverTask to Solver
    /// 2. Wait for TEE execution result in response
    /// 3. Set execution_result on Event (kept in scope)
    /// 4. Add Event to DAG
    ///
    /// Key benefit: No pending_tasks mapping needed!
    async fn send_solver_task_to_solver(
        &self,
        solver_id: &str,
        task: setu_types::task::SolverTask,
    ) -> Result<(), String> {
        let task_id_hex = hex::encode(&task.task_id[..8]);
        
        debug!(
            solver_id = %solver_id,
            task_id = %task_id_hex,
            event_id = %task.event.id,
            "Sending SolverTask via sync HTTP"
        );

        // Get Solver address
        let solver_url = {
            let solvers = self.solver_info.read();
            match solvers.get(solver_id) {
                Some(info) => info.execute_task_url(),
                None => {
                    error!(solver_id = %solver_id, "Solver not found in registry");
                    return Err(format!("Solver not found: {}", solver_id));
                }
            }
        };

        // Keep Event for later use (the key insight of sync approach!)
        let mut event = task.event.clone();
        let event_id = event.id.clone();

        // Create request
        let request = ExecuteTaskRequest {
            solver_task: task,
            validator_id: self.validator_id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };

        // Send HTTP request and wait for response
        info!(
            solver_id = %solver_id,
            url = %solver_url,
            task_id = %task_id_hex,
            "Sending sync HTTP request to Solver"
        );

        let response = self.http_client
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
        let result_dto = exec_response.result.ok_or_else(|| {
            "Solver returned success but no result".to_string()
        })?;

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
                result_dto.events_processed,
                result_dto.execution_time_us
            )),
            state_changes: result_dto.state_changes.iter().map(|sc| {
                setu_types::event::StateChange {
                    key: sc.key.clone(),
                    old_value: sc.old_value.clone(),
                    new_value: sc.new_value.clone(),
                }
            }).collect(),
        };

        event.set_execution_result(execution_result);
        event.status = setu_types::event::EventStatus::Executed;
        
        // Store TEE attestation on event (if field exists)
        // Note: May need to add tee_attestation field to Event type

        // Add Event to DAG (submits to consensus if enabled)
        self.add_event_to_dag(event).await;

        // Update transfer status
        if let Some(tracker) = self.transfer_status.write().values_mut().find(|t| {
            t.solver_id.as_ref() == Some(&solver_id.to_string()) && t.event_id.is_none()
        }) {
            tracker.status = "executed".to_string();
            tracker.event_id = Some(event_id.clone());
            tracker.processing_steps.push(setu_rpc::ProcessingStep {
                step: "tee_execution".to_string(),
                status: "completed".to_string(),
                details: Some(format!(
                    "TEE executed in {}μs, {} events processed",
                    result_dto.execution_time_us,
                    result_dto.events_processed
                )),
                timestamp: current_timestamp_secs(),
            });
        }

        info!(
            task_id = %task_id_hex,
            event_id = %event_id,
            "SolverTask completed, Event added to DAG"
        );

        Ok(())
    }

    pub fn get_transfer_status(&self, transfer_id: &str) -> GetTransferStatusResponse {
        if let Some(tracker) = self.transfer_status.read().get(transfer_id) {
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
    // Event Processing
    // ============================================

    pub async fn submit_event(&self, request: SubmitEventRequest) -> SubmitEventResponse {
        let event = request.event;

        info!(
            event_id = %&event.id[..20.min(event.id.len())],
            event_type = %event.event_type.name(),
            creator = %event.creator,
            consensus_enabled = self.consensus_enabled(),
            "Receiving event from solver"
        );

        // Quick check
        if let Err(e) = self.quick_check(&event) {
            return SubmitEventResponse {
                success: false,
                message: format!("Quick check failed: {}", e),
                event_id: Some(event.id),
                vlc_time: None,
            };
        }

        // Add to pending
        self.pending_events.write().push(event.id.clone());

        // Sampling (10% of events)
        let counter = self.event_counter.fetch_add(1, Ordering::SeqCst);
        if counter % 10 == 0 {
            if let Err(e) = self.sampling_verify(&event).await {
                warn!(event_id = %event.id, error = %e, "Sampling verification failed");
            }
        }

        let event_id = event.id.clone();
        
        // If consensus is enabled, submit to consensus engine
        if let Some(ref consensus) = self.consensus_validator {
            match consensus.submit_event(event.clone()).await {
                Ok(_) => {
                    // Get VLC from consensus
                    let vlc = consensus.vlc_snapshot().await;
                    let vlc_time = vlc.logical_time;
                    
                    // Store event locally
                    self.events.write().insert(event_id.clone(), event);
                    self.pending_events.write().retain(|id| id != &event_id);
                    self.dag_events.write().push(event_id.clone());
                    
                    // Apply side effects
                    self.apply_event_side_effects(&event_id).await;
                    
                    // Log consensus stats - evaluate all async values BEFORE the info! macro
                    let stats = consensus.dag_stats().await;
                    let is_leader = consensus.is_leader().await;
                    info!(
                        event_id = %&event_id[..20.min(event_id.len())],
                        vlc_time = vlc_time,
                        consensus_dag_size = stats.node_count,
                        is_leader = is_leader,
                        "Event added to consensus DAG"
                    );
                    
                    return SubmitEventResponse {
                        success: true,
                        message: "Event verified and added to consensus DAG".to_string(),
                        event_id: Some(event_id),
                        vlc_time: Some(vlc_time),
                    };
                }
                Err(e) => {
                    error!(event_id = %event_id, error = %e, "Failed to submit event to consensus");
                    return SubmitEventResponse {
                        success: false,
                        message: format!("Consensus submission failed: {}", e),
                        event_id: Some(event_id),
                        vlc_time: None,
                    };
                }
            }
        }
        
        // Legacy path: local VLC and DAG only
        let vlc_time = self.vlc_counter.fetch_add(1, Ordering::SeqCst) + 1;

        // Store event and add to DAG
        self.events.write().insert(event_id.clone(), event);
        self.pending_events.write().retain(|id| id != &event_id);
        self.dag_events.write().push(event_id.clone());

        // Apply side effects
        self.apply_event_side_effects(&event_id).await;

        info!(event_id = %&event_id[..20.min(event_id.len())], vlc_time = vlc_time, dag_size = self.dag_events.read().len(), "Event verified (legacy mode)");

        SubmitEventResponse {
            success: true,
            message: "Event verified and added to DAG".to_string(),
            event_id: Some(event_id),
            vlc_time: Some(vlc_time),
        }
    }

    fn quick_check(&self, event: &Event) -> Result<(), String> {
        if event.execution_result.is_none() {
            return Err("Event has no execution result".to_string());
        }

        if let Some(ref result) = event.execution_result {
            if !result.success {
                return Err(format!(
                    "Event execution failed: {}",
                    result.message.as_deref().unwrap_or("unknown error")
                ));
            }
        }

        if event.creator.is_empty() {
            return Err("Event creator is empty".to_string());
        }

        let now = current_timestamp_millis();
        if event.timestamp > now + 60000 {
            return Err("Event timestamp is in the future".to_string());
        }

        Ok(())
    }

    async fn sampling_verify(&self, event: &Event) -> Result<(), String> {
        // Simulated: always pass unless "evil" in ID
        if event.id.contains("evil") {
            return Err("Fraud detected".to_string());
        }
        Ok(())
    }

    pub async fn apply_event_side_effects(&self, event_id: &str) {
        let event = match self.events.read().get(event_id).cloned() {
            Some(e) => e,
            None => return,
        };

        match &event.payload {
            EventPayload::ValidatorRegister(reg) => {
                self.validators.write().insert(
                    reg.validator_id.clone(),
                    ValidatorInfo {
                        validator_id: reg.validator_id.clone(),
                        address: reg.network_address.clone(),
                        port: reg.network_port,
                        status: "online".to_string(),
                        registered_at: event.timestamp / 1000,
                    },
                );
            }
            EventPayload::SolverUnregister(unreg) => self.unregister_solver(&unreg.node_id),
            EventPayload::ValidatorUnregister(unreg) => {
                self.validators.write().remove(&unreg.node_id);
            }
            _ => {} // Other payloads: no side effects needed
        }
    }

    pub fn get_events(&self) -> Vec<Event> {
        self.events.read().values().cloned().collect()
    }

    // ============================================
    // State Query (Scheme B)
    // ============================================

    pub fn get_balance(&self, account: &str) -> GetBalanceResponse {
        debug!(account = %account, "Getting balance (mock)");
        GetBalanceResponse {
            account: account.to_string(),
            balance: 1_000_000,
            exists: true,
        }
    }

    pub fn get_object(&self, key: &str) -> GetObjectResponse {
        debug!(key = %key, "Getting object (mock)");
        GetObjectResponse {
            key: key.to_string(),
            value: None,
            exists: false,
        }
    }

    // ============================================
    // Validator Management
    // ============================================

    pub fn add_validator(&self, info: ValidatorInfo) {
        self.validators
            .write()
            .insert(info.validator_id.clone(), info);
    }

    pub fn unregister_validator(&self, node_id: &str) {
        self.validators.write().remove(node_id);
    }

    pub fn get_validator_info(&self, node_id: &str) -> Option<ValidatorInfo> {
        self.validators.read().get(node_id).cloned()
    }

    pub fn get_validator_uptime(&self, node_id: &str) -> Option<u64> {
        self.validators.read().get(node_id).map(|v| {
            current_timestamp_secs() - v.registered_at
        })
    }

    pub fn get_validator_list(&self) -> Vec<ValidatorListItem> {
        self.validators
            .read()
            .values()
            .map(|v| ValidatorListItem {
                validator_id: v.validator_id.clone(),
                network_address: v.address.clone(),
                network_port: v.port,
                account_address: None,  // TODO: 从 ValidatorInfo 中获取
                status: v.status.clone(),
            })
            .collect()
    }

    // ============================================
    // Solver Management
    // ============================================

    pub fn register_solver_internal(
        &self,
        request: &RegisterSolverRequest,
    ) -> mpsc::UnboundedSender<setu_types::task::SolverTask> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Store Solver info for sync HTTP calls
        let solver_info = SolverInfo {
            solver_id: request.solver_id.clone(),
            address: request.network_address.clone(),
            port: request.network_port,
            capacity: request.capacity,
            shard_id: request.shard_id.clone(),
            resources: request.resources.clone(),
            status: "active".to_string(),
            registered_at: current_timestamp_secs(),
        };

        self.solver_info.write().insert(
            request.solver_id.clone(),
            solver_info,
        );

        self.solver_channels
            .write()
            .insert(request.solver_id.clone(), tx.clone());

        // RouterManager still needs Transfer channel for routing decisions
        let (router_tx, _router_rx) = mpsc::unbounded_channel::<Transfer>();
        self.router_manager.register_solver_with_affinity(
            request.solver_id.clone(),
            format!("{}:{}", request.network_address, request.network_port),
            request.capacity,
            router_tx,
            request.shard_id.clone(),
            request.resources.clone(),
        );

        // Consume channel to avoid memory leak (sync HTTP doesn't use it)
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(_task) = rx.recv().await {
                // Channel consumed but not used - sync HTTP is primary
            }
        });

        info!(
            solver_id = %request.solver_id,
            address = %request.network_address,
            port = request.network_port,
            "Solver registered for sync HTTP communication"
        );

        tx
    }

    pub fn unregister_solver(&self, node_id: &str) {
        self.router_manager.unregister_solver(node_id);
        self.solver_channels.write().remove(node_id);
        self.solver_info.write().remove(node_id);
        info!(solver_id = %node_id, "Solver unregistered");
    }

    // ============================================
    // DAG Management
    // ============================================

    /// Add event to DAG and submit to consensus engine if enabled
    /// 
    /// This is the unified entry point for adding events to the DAG.
    /// It ensures events are:
    /// 1. Stored locally for queries
    /// 2. Submitted to consensus engine (if enabled) for:
    ///    - Broadcasting to other validators
    ///    - Inclusion in ConsensusFrames
    ///    - Finalization into Anchor Chain
    pub async fn add_event_to_dag(&self, event: Event) {
        let event_id = event.id.clone();
        
        // If consensus is enabled, submit to consensus engine
        if let Some(ref consensus) = self.consensus_validator {
            match consensus.submit_event(event.clone()).await {
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
                        "Failed to submit event to consensus, storing locally only"
                    );
                }
            }
        }
        
        // Always store locally for queries
        self.events.write().insert(event_id.clone(), event);
        self.dag_events.write().push(event_id);
    }
    
    /// Synchronous version for backward compatibility (legacy mode only)
    /// 
    /// WARNING: This does NOT submit to consensus. Use `add_event_to_dag` instead.
    #[allow(dead_code)]
    pub fn add_event_to_dag_sync(&self, event: Event) {
        let event_id = event.id.clone();
        self.events.write().insert(event_id.clone(), event);
        self.dag_events.write().push(event_id);
    }
}

// ============================================
// Implement ValidatorService trait for API layer
// ============================================

impl setu_api::ValidatorService for ValidatorNetworkService {
    fn validator_id(&self) -> &str {
        &self.validator_id
    }
    
    fn start_time(&self) -> u64 {
        self.start_time
    }
    
    fn solver_count(&self) -> usize {
        self.router_manager.solver_count()
    }
    
    fn validator_count(&self) -> usize {
        self.validators.read().len()
    }
    
    fn dag_events_count(&self) -> usize {
        self.dag_events.read().len()
    }
    
    fn pending_events_count(&self) -> usize {
        self.pending_events.read().len()
    }
    
    fn registration_handler(self: &Arc<Self>) -> Arc<dyn setu_rpc::RegistrationHandler> {
        Arc::new(ValidatorRegistrationHandler {
            service: self.clone(),
        })
    }
    
    fn user_handler(self: &Arc<Self>) -> Arc<dyn setu_rpc::UserRpcHandler> {
        Arc::new(crate::ValidatorUserHandler::new(self.clone()))
    }
    
    async fn submit_transfer(&self, request: SubmitTransferRequest) -> SubmitTransferResponse {
        self.submit_transfer(request).await
    }
    
    fn get_transfer_status(&self, transfer_id: &str) -> GetTransferStatusResponse {
        self.get_transfer_status(transfer_id)
    }
    
    async fn submit_event(&self, request: setu_api::SubmitEventRequest) -> setu_api::SubmitEventResponse {
        self.submit_event(request).await
    }
    
    fn get_events(&self) -> Vec<Event> {
        self.get_events()
    }
    
    fn get_balance(&self, account: &str) -> setu_api::GetBalanceResponse {
        self.get_balance(account)
    }
    
    fn get_object(&self, key: &str) -> setu_api::GetObjectResponse {
        self.get_object(key)
    }
}

// ============================================
// Tests
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    use setu_rpc::RegistrationHandler;

    fn create_test_service() -> Arc<ValidatorNetworkService> {
        let router_manager = Arc::new(RouterManager::new());
        let task_preparer = Arc::new(TaskPreparer::new_for_testing("test-validator".to_string()));
        let config = NetworkServiceConfig::default();

        Arc::new(ValidatorNetworkService::new(
            "test-validator".to_string(),
            router_manager,
            task_preparer,
            config,
        ))
    }

    #[tokio::test]
    async fn test_register_solver() {
        let service = create_test_service();
        let handler = service.registration_handler();

        let request = setu_rpc::RegisterSolverRequest {
            solver_id: "solver-1".to_string(),
            network_address: "127.0.0.1".to_string(),
            network_port: 9001,
            account_address: "0xtest".to_string(),
            public_key: vec![],
            signature: vec![],
            capacity: 100,
            shard_id: Some("shard-0".to_string()),
            resources: vec!["ETH".to_string()],
        };

        let response = handler.register_solver(request).await;

        assert!(response.success);
        assert_eq!(service.solver_count(), 1);
    }

    #[tokio::test]
    async fn test_register_validator() {
        let service = create_test_service();
        let handler = service.registration_handler();

        let request = setu_rpc::RegisterValidatorRequest {
            validator_id: "validator-2".to_string(),
            network_address: "127.0.0.1".to_string(),
            network_port: 9002,
            account_address: "0xtest".to_string(),
            public_key: vec![],
            signature: vec![],
            stake_amount: 1000,
            commission_rate: 10,
        };

        let response = handler.register_validator(request).await;

        assert!(response.success);
        assert_eq!(service.validator_count(), 1);
    }
}
