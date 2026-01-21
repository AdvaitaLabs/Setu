//! Core ValidatorNetworkService implementation
//!
//! This is the main service handling network operations for the Validator.

use super::registration::ValidatorRegistrationHandler;
use super::types::*;
use crate::{RouterManager, TaskPreparer};
use axum::{
    routing::{get, post},
    Router,
};
use setu_types::{Transfer, TransferType, AssignedVlc};
use parking_lot::RwLock;
use setu_rpc::{
    GetTransferStatusResponse, ProcessingStep, RegisterSolverRequest,
    SubmitTransferRequest, SubmitTransferResponse, ValidatorListItem,
    RegistrationHandler, UserRpcHandler,
};
use setu_types::event::{Event, EventPayload};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// Import API handlers
use setu_api::{self, ValidatorService};

/// Validator network service
///
/// Core service handling:
/// - Solver/Validator registration
/// - Transfer submission and routing
/// - Event verification and DAG management
/// - State queries (Scheme B)
pub struct ValidatorNetworkService {
    /// Validator ID
    validator_id: String,

    /// Router manager for solver management
    router_manager: Arc<RouterManager>,

    /// Task preparer for solver-tee3 architecture
    task_preparer: Arc<TaskPreparer>,

    /// Registered validators
    validators: Arc<RwLock<HashMap<String, ValidatorInfo>>>,

    /// Solver channels for sending SolverTasks
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<setu_enclave::SolverTask>>>>,

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

        Self {
            validator_id,
            router_manager,
            task_preparer,
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn next_vlc(&self) -> u64 {
        self.vlc_counter.fetch_add(1, Ordering::SeqCst)
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

        // Step 2: VLC Assignment
        let vlc_time = self.vlc_counter.fetch_add(1, Ordering::SeqCst);
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

    /// Send SolverTask to Solver
    async fn send_solver_task_to_solver(
        &self,
        solver_id: &str,
        task: setu_enclave::SolverTask,
    ) -> Result<(), String> {
        debug!(
            solver_id = %solver_id,
            task_id = ?hex::encode(&task.task_id[..8]),
            "Sending SolverTask"
        );

        let channels = self.solver_channels.read();
        let channel = channels
            .get(solver_id)
            .ok_or_else(|| format!("Solver not found: {}", solver_id))?;

        channel
            .send(task)
            .map_err(|e| format!("Failed to send: {}", e))?;

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

        // VLC + 1
        let vlc_time = self.vlc_counter.fetch_add(1, Ordering::SeqCst) + 1;

        // Store event and add to DAG
        let event_id = event.id.clone();
        self.events.write().insert(event_id.clone(), event);
        self.pending_events.write().retain(|id| id != &event_id);
        self.dag_events.write().push(event_id.clone());

        // Apply side effects
        self.apply_event_side_effects(&event_id).await;

        info!(event_id = %&event_id[..20.min(event_id.len())], vlc_time = vlc_time, dag_size = self.dag_events.read().len(), "Event verified");

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
                        address: reg.address.clone(),
                        port: reg.port,
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
                address: v.address.clone(),
                port: v.port,
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
    ) -> mpsc::UnboundedSender<setu_enclave::SolverTask> {
        let (tx, rx) = mpsc::unbounded_channel();

        self.solver_channels
            .write()
            .insert(request.solver_id.clone(), tx.clone());

        // RouterManager still needs Transfer channel for routing decisions
        let (router_tx, _router_rx) = mpsc::unbounded_channel::<Transfer>();
        self.router_manager.register_solver_with_affinity(
            request.solver_id.clone(),
            format!("{}:{}", request.address, request.port),
            request.capacity,
            router_tx,
            request.shard_id.clone(),
            request.resources.clone(),
        );

        // TODO(Phase 6): Implement network forwarding to Solver
        tokio::spawn(async move {
            let mut rx = rx;
            let mut task_count = 0;
            while let Some(task) = rx.recv().await {
                task_count += 1;
                warn!(
                    task_id = ?hex::encode(&task.task_id[..8]),
                    dropped_count = task_count,
                    "⚠️ SolverTask dropped - Network forwarding not implemented"
                );
            }
        });

        warn!(
            solver_id = %request.solver_id,
            "⚠️ Solver registered but network forwarding not implemented"
        );

        tx
    }

    pub fn unregister_solver(&self, node_id: &str) {
        self.router_manager.unregister_solver(node_id);
        self.solver_channels.write().remove(node_id);
    }

    // ============================================
    // DAG Management
    // ============================================

    pub fn add_event_to_dag(&self, event: Event) {
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
            address: "127.0.0.1".to_string(),
            port: 9001,
            capacity: 100,
            shard_id: Some("shard-0".to_string()),
            resources: vec!["ETH".to_string()],
            public_key: None,
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
            address: "127.0.0.1".to_string(),
            port: 9002,
            public_key: None,
            stake: Some(1000),
        };

        let response = handler.register_validator(request).await;

        assert!(response.success);
        assert_eq!(service.validator_count(), 1);
    }
}
