//! Network service for Validator
//!
//! This module provides the network service for the Validator,
//! handling RPC requests for registration, transfer submission, and event processing.
//!
//! ## Flow
//! 
//! 1. CLI/Relay submits request (Register/Transfer/etc.)
//! 2. Validator converts request to Transfer and routes to Solver
//! 3. Solver executes and creates Event
//! 4. Solver sends Event back to Validator
//! 5. Validator verifies Event (Quick check → Sampling → VLC+1 → DAG)
//! 6. Event is finalized

use crate::RouterManager;
use axum::{
    routing::{get, post},
    Router, Json,
    extract::State,
};
use core_types::{Transfer, TransferType, Vlc, AssignedVlc};
use parking_lot::RwLock;
use setu_rpc::{
    RegisterSolverRequest, RegisterSolverResponse,
    RegisterValidatorRequest, RegisterValidatorResponse,
    UnregisterRequest, UnregisterResponse,
    HeartbeatRequest, HeartbeatResponse,
    GetSolverListRequest, GetSolverListResponse,
    GetValidatorListRequest, GetValidatorListResponse,
    GetNodeStatusRequest, GetNodeStatusResponse,
    SubmitTransferRequest, SubmitTransferResponse,
    GetTransferStatusRequest, GetTransferStatusResponse,
    ProcessingStep,
    SolverListItem, ValidatorListItem, NodeType,
    RegistrationHandler,
};
use setu_types::event::{Event, EventType, EventStatus, EventPayload, SolverRegistration, ValidatorRegistration, Unregistration};
use setu_vlc;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{info, warn, debug, error};

/// Validator info for registration
#[derive(Debug, Clone)]
pub struct ValidatorInfo {
    pub validator_id: String,
    pub address: String,
    pub port: u16,
    pub status: String,
    pub registered_at: u64,
}

/// Network service configuration
#[derive(Debug, Clone)]
pub struct NetworkServiceConfig {
    /// Listen address for HTTP API
    pub http_listen_addr: SocketAddr,
    /// Listen address for Anemo P2P
    pub p2p_listen_addr: SocketAddr,
}

impl Default for NetworkServiceConfig {
    fn default() -> Self {
        Self {
            http_listen_addr: "127.0.0.1:8080".parse().unwrap(),
            p2p_listen_addr: "127.0.0.1:9000".parse().unwrap(),
        }
    }
}

/// Validator network service
/// 
/// Handles incoming RPC requests for:
/// - Solver registration
/// - Validator registration  
/// - Heartbeat
/// - Status queries
/// - Transfer submission
/// - Event submission (from Solver)
pub struct ValidatorNetworkService {
    /// Validator ID
    validator_id: String,
    
    /// Router manager for solver management
    router_manager: Arc<RouterManager>,
    
    /// Registered validators (other validators in the network)
    validators: Arc<RwLock<HashMap<String, ValidatorInfo>>>,
    
    /// Solver channels for sending transfers
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<Transfer>>>>,
    
    /// Transfer tracking (transfer_id -> status)
    transfer_status: Arc<RwLock<HashMap<String, TransferTracker>>>,
    
    /// Event storage (event_id -> event)
    events: Arc<RwLock<HashMap<String, Event>>>,
    
    /// Event tracking (event_id -> tracker)
    event_trackers: Arc<RwLock<HashMap<String, EventTracker>>>,
    
    /// Pending event queue (events waiting for verification)
    pending_events: Arc<RwLock<Vec<String>>>,
    
    /// Verified events in DAG order
    dag_events: Arc<RwLock<Vec<String>>>,
    
    /// Configuration
    config: NetworkServiceConfig,
    
    /// Start time for uptime calculation
    start_time: u64,
    
    /// Transfer counter for generating IDs
    transfer_counter: AtomicU64,
    
    /// VLC counter (simulated)
    vlc_counter: AtomicU64,
    
    /// Event counter
    event_counter: AtomicU64,
}

/// Transfer tracking information
#[derive(Debug, Clone)]
pub struct TransferTracker {
    pub transfer_id: String,
    pub status: String,
    pub solver_id: Option<String>,
    pub event_id: Option<String>,
    pub processing_steps: Vec<ProcessingStep>,
    pub created_at: u64,
}

/// Event tracking information
#[derive(Debug, Clone)]
pub struct EventTracker {
    pub event_id: String,
    pub event_type: EventType,
    pub status: EventStatus,
    pub solver_id: String,
    pub verified: bool,
    pub vlc_time: u64,
    pub created_at: u64,
}

/// Submit Event Request (from Solver)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubmitEventRequest {
    pub event: Event,
}

/// Submit Event Response
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubmitEventResponse {
    pub success: bool,
    pub message: String,
    pub event_id: Option<String>,
    pub vlc_time: Option<u64>,
}

impl ValidatorNetworkService {
    /// Create a new validator network service
    pub fn new(
        validator_id: String,
        router_manager: Arc<RouterManager>,
        config: NetworkServiceConfig,
    ) -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        info!(
            validator_id = %validator_id,
            http_addr = %config.http_listen_addr,
            p2p_addr = %config.p2p_listen_addr,
            "Creating validator network service"
        );
        
        Self {
            validator_id,
            router_manager,
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            transfer_status: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            event_trackers: Arc::new(RwLock::new(HashMap::new())),
            pending_events: Arc::new(RwLock::new(Vec::new())),
            dag_events: Arc::new(RwLock::new(Vec::new())),
            config,
            start_time,
            transfer_counter: AtomicU64::new(0),
            vlc_counter: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
        }
    }
    
    /// Get the registration handler for RPC server
    pub fn registration_handler(self: &Arc<Self>) -> Arc<ValidatorRegistrationHandler> {
        Arc::new(ValidatorRegistrationHandler {
            service: self.clone(),
        })
    }
    
    /// Start the HTTP API server
    pub async fn start_http_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = self.clone();
        
        let app = Router::new()
            // Registration endpoints
            .route("/api/v1/register/solver", post(http_register_solver))
            .route("/api/v1/register/validator", post(http_register_validator))
            // Query endpoints
            .route("/api/v1/solvers", get(http_get_solvers))
            .route("/api/v1/validators", get(http_get_validators))
            .route("/api/v1/health", get(http_health))
            // Transfer endpoints
            .route("/api/v1/transfer", post(http_submit_transfer))
            .route("/api/v1/transfer/status", post(http_get_transfer_status))
            // Event endpoints (for Solver to submit events)
            .route("/api/v1/event", post(http_submit_event))
            .route("/api/v1/events", get(http_get_events))
            // Heartbeat
            .route("/api/v1/heartbeat", post(http_heartbeat))
            .with_state(service);
        
        let listener = tokio::net::TcpListener::bind(self.config.http_listen_addr).await?;
        
        info!(
            addr = %self.config.http_listen_addr,
            "HTTP API server started"
        );
        
        axum::serve(listener, app).await?;
        
        Ok(())
    }
    
    /// Submit a transfer (with simulated processing pipeline)
    pub async fn submit_transfer(&self, request: SubmitTransferRequest) -> SubmitTransferResponse {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        // Generate transfer ID
        let transfer_id = format!(
            "tx-{}-{}",
            now,
            self.transfer_counter.fetch_add(1, Ordering::SeqCst)
        );
        
        let mut steps = Vec::new();
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Processing Transfer: {}              ║", &transfer_id[..20.min(transfer_id.len())]);
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Receive and validate
        info!("[STEP 1/7] 📥 Receiving transfer request...");
        info!("           From: {} → To: {}", request.from, request.to);
        info!("           Amount: {}", request.amount);
        info!("           Type: {}", request.transfer_type);
        steps.push(ProcessingStep {
            step: "receive".to_string(),
            status: "completed".to_string(),
            details: Some(format!("Transfer {} received", transfer_id)),
            timestamp: now,
        });
        
        // Step 2: VLC Assignment - Validator assigns VLC to the transfer
        // This VLC will be passed to Solver and used in the resulting Event
        let vlc_time = self.vlc_counter.fetch_add(1, Ordering::SeqCst);
        let now_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        info!("[STEP 2/7] ⏰ [VLC] Assigning Vector Logical Clock...");
        info!("           └─ VLC Time: {}", vlc_time);
        info!("           └─ Validator: {}", self.validator_id);
        info!("           └─ Note: Solver will use this VLC, not generate its own");
        
        let assigned_vlc = AssignedVlc {
            logical_time: vlc_time,
            physical_time: now_millis,
            validator_id: self.validator_id.clone(),
        };
        
        steps.push(ProcessingStep {
            step: "vlc_assign".to_string(),
            status: "completed".to_string(),
            details: Some(format!("VLC time: {} assigned by Validator", vlc_time)),
            timestamp: now,
        });
        
        // Step 3: DAG Parent Resolution (Simulated)
        info!("[STEP 3/7] 🔗 [DAG] Resolving parent events...");
        info!("           └─ Checking resource dependencies: {:?}", request.resources);
        info!("           └─ Parent events: [] (genesis or no conflicts)");
        info!("           └─ DAG depth: 0 (simulated)");
        steps.push(ProcessingStep {
            step: "dag_resolve".to_string(),
            status: "completed".to_string(),
            details: Some("No parent conflicts (simulated)".to_string()),
            timestamp: now,
        });
        
        // Step 4: Router Selection
        info!("[STEP 4/7] 🔀 Selecting solver via router...");
        
        // Create internal transfer
        let mut vlc = Vlc::new();
        vlc.entries.insert(self.validator_id.clone(), vlc_time);
        
        let transfer_type = match request.transfer_type.to_lowercase().as_str() {
            "flux" | "fluxtransfer" => TransferType::FluxTransfer,
            _ => TransferType::FluxTransfer,
        };
        
        let transfer = Transfer {
            id: transfer_id.clone(),
            from: request.from.clone(),
            to: request.to.clone(),
            amount: request.amount,
            transfer_type,
            resources: if request.resources.is_empty() {
                vec![format!("account:{}", request.from), format!("account:{}", request.to)]
            } else {
                request.resources.clone()
            },
            vlc,
            power: 10,
            preferred_solver: request.preferred_solver.clone(),
            shard_id: request.shard_id.clone(),
            subnet_id: request.subnet_id.clone(),
            // VLC assigned by Validator - Solver must use this, not generate its own
            assigned_vlc: Some(assigned_vlc),
        };
        
        // Route to solver
        let solver_result = self.router_manager.route_transfer(&transfer);
        
        let solver_id = match solver_result {
            Ok(id) => {
                info!("           └─ Selected solver: {}", id);
                steps.push(ProcessingStep {
                    step: "route".to_string(),
                    status: "completed".to_string(),
                    details: Some(format!("Routed to solver: {}", id)),
                    timestamp: now,
                });
                Some(id)
            }
            Err(e) => {
                warn!("           └─ No solver available: {}", e);
                steps.push(ProcessingStep {
                    step: "route".to_string(),
                    status: "failed".to_string(),
                    details: Some(format!("No solver available: {}", e)),
                    timestamp: now,
                });
                
                // Store failed transfer status
                self.transfer_status.write().insert(transfer_id.clone(), TransferTracker {
                    transfer_id: transfer_id.clone(),
                    status: "failed".to_string(),
                    solver_id: None,
                    event_id: None,
                    processing_steps: steps.clone(),
                    created_at: now,
                });
                
                return SubmitTransferResponse {
                    success: false,
                    message: format!("No solver available: {}", e),
                    transfer_id: Some(transfer_id),
                    solver_id: None,
                    processing_steps: steps,
                };
            }
        };
        
        // Step 5: Send to Solver
        info!("[STEP 5/7] 📤 Dispatching to solver...");
        if let Some(ref sid) = solver_id {
            match self.router_manager.send_to_solver(sid, transfer).await {
                Ok(()) => {
                    info!("           └─ Transfer dispatched successfully");
                    steps.push(ProcessingStep {
                        step: "dispatch".to_string(),
                        status: "completed".to_string(),
                        details: Some("Transfer sent to solver".to_string()),
                        timestamp: now,
                    });
                }
                Err(e) => {
                    error!("           └─ Dispatch failed: {}", e);
                    steps.push(ProcessingStep {
                        step: "dispatch".to_string(),
                        status: "failed".to_string(),
                        details: Some(format!("Dispatch error: {}", e)),
                        timestamp: now,
                    });
                }
            }
        }
        
        // Step 6: Consensus Preparation (Simulated)
        info!("[STEP 6/7] 🤝 [CONSENSUS] Preparing consensus...");
        info!("           └─ Mode: Single validator (no consensus needed)");
        info!("           └─ Validators: 1 (self)");
        info!("           └─ Threshold: 1/1 (100%)");
        steps.push(ProcessingStep {
            step: "consensus_prepare".to_string(),
            status: "completed".to_string(),
            details: Some("Single validator mode - no consensus needed (simulated)".to_string()),
            timestamp: now,
        });
        
        // Step 7: FoldGraph Update (Simulated)
        info!("[STEP 7/7] 📊 [FOLDGRAPH] Updating fold graph...");
        info!("           └─ Current frame: 0");
        info!("           └─ Events in frame: 1");
        info!("           └─ Fold threshold: 100 events");
        info!("           └─ Status: Not folding (below threshold)");
        steps.push(ProcessingStep {
            step: "foldgraph_update".to_string(),
            status: "completed".to_string(),
            details: Some("FoldGraph updated (simulated)".to_string()),
            timestamp: now,
        });
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Transfer Submitted Successfully               ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Transfer ID: {:^44} ║", &transfer_id);
        info!("║  Solver:      {:^44} ║", solver_id.as_deref().unwrap_or("N/A"));
        info!("║  Status:      {:^44} ║", "pending_execution");
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Store transfer status
        self.transfer_status.write().insert(transfer_id.clone(), TransferTracker {
            transfer_id: transfer_id.clone(),
            status: "pending_execution".to_string(),
            solver_id: solver_id.clone(),
            event_id: None,
            processing_steps: steps.clone(),
            created_at: now,
        });
        
        SubmitTransferResponse {
            success: true,
            message: "Transfer submitted successfully".to_string(),
            transfer_id: Some(transfer_id),
            solver_id,
            processing_steps: steps,
        }
    }
    
    /// Get transfer status
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
    
    /// Submit an event from Solver - this is the core verification flow
    pub async fn submit_event(&self, request: SubmitEventRequest) -> SubmitEventResponse {
        let event = request.event;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Receiving Event from Solver                   ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Event ID:   {:^44} ║", &event.id[..20.min(event.id.len())]);
        info!("║  Type:       {:^44} ║", event.event_type.name());
        info!("║  Creator:    {:^44} ║", &event.creator);
        info!("║  Status:     {:^44} ║", format!("{:?}", event.status));
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Quick Check
        info!("[VERIFY 1/5] 🔍 Quick check...");
        if let Err(e) = self.quick_check(&event) {
            error!("           └─ Quick check failed: {}", e);
            return SubmitEventResponse {
                success: false,
                message: format!("Quick check failed: {}", e),
                event_id: Some(event.id),
                vlc_time: None,
            };
        }
        info!("           └─ Quick check passed");
        
        // Step 2: Add to Pending Queue
        info!("[VERIFY 2/5] 📋 Adding to pending queue...");
        self.pending_events.write().push(event.id.clone());
        info!("           └─ Pending queue size: {}", self.pending_events.read().len());
        
        // Step 3: Sampling Verification (simulated)
        info!("[VERIFY 3/5] 🎲 Sampling verification...");
        let should_sample = self.should_sample(&event);
        if should_sample {
            info!("           └─ Event selected for sampling");
            if let Err(e) = self.sampling_verify(&event).await {
                warn!("           └─ Sampling verification failed: {}", e);
                // In real implementation, this might trigger re-execution
            } else {
                info!("           └─ Sampling verification passed");
            }
        } else {
            info!("           └─ Event not selected for sampling (probabilistic skip)");
        }
        
        // Step 4: VLC + 1
        let vlc_time = self.vlc_counter.fetch_add(1, Ordering::SeqCst) + 1;
        info!("[VERIFY 4/5] ⏰ Updating VLC...");
        info!("           └─ Previous VLC: {}", vlc_time - 1);
        info!("           └─ New VLC: {}", vlc_time);
        info!("           └─ Delta VLC check: {} (threshold: 100)", vlc_time);
        
        // Check if we need to fold (delta VLC > M)
        if vlc_time % 100 == 0 {
            info!("           └─ [FOLDGRAPH] Triggering fold at VLC {}", vlc_time);
        }
        
        // Step 5: Add to DAG
        info!("[VERIFY 5/5] 🔗 Adding to DAG...");
        let parent_count = event.parent_ids.len();
        info!("           └─ Parents: {} events", parent_count);
        info!("           └─ Resources: {:?}", event.affected_resources());
        
        // Store the event
        let event_id = event.id.clone();
        let event_type = event.event_type;
        let creator = event.creator.clone();
        
        self.events.write().insert(event_id.clone(), event);
        
        // Remove from pending, add to DAG
        self.pending_events.write().retain(|id| id != &event_id);
        self.dag_events.write().push(event_id.clone());
        
        // Create tracker
        let tracker = EventTracker {
            event_id: event_id.clone(),
            event_type,
            status: EventStatus::Confirmed,
            solver_id: creator.clone(),
            verified: true,
            vlc_time,
            created_at: now,
        };
        self.event_trackers.write().insert(event_id.clone(), tracker);
        
        info!("           └─ DAG size: {} events", self.dag_events.read().len());
        
        // Handle specific event types (apply side effects)
        self.apply_event_side_effects(&event_id).await;
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Event Verified Successfully                   ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Event ID:   {:^44} ║", &event_id[..20.min(event_id.len())]);
        info!("║  VLC Time:   {:^44} ║", vlc_time);
        info!("║  DAG Size:   {:^44} ║", self.dag_events.read().len());
        info!("╚════════════════════════════════════════════════════════════╝");
        
        SubmitEventResponse {
            success: true,
            message: "Event verified and added to DAG".to_string(),
            event_id: Some(event_id),
            vlc_time: Some(vlc_time),
        }
    }
    
    /// Quick check for event validity
    fn quick_check(&self, event: &Event) -> Result<(), String> {
        // Check event has execution result
        if event.execution_result.is_none() {
            return Err("Event has no execution result".to_string());
        }
        
        // Check execution was successful
        if let Some(ref result) = event.execution_result {
            if !result.success {
                return Err(format!(
                    "Event execution failed: {}",
                    result.message.as_deref().unwrap_or("unknown error")
                ));
            }
        }
        
        // Check creator is not empty
        if event.creator.is_empty() {
            return Err("Event creator is empty".to_string());
        }
        
        // Check timestamp is not in the future
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        if event.timestamp > now + 60000 {
            return Err("Event timestamp is in the future".to_string());
        }
        
        Ok(())
    }
    
    /// Determine if event should be sampled for verification
    fn should_sample(&self, _event: &Event) -> bool {
        // Simulated: sample 10% of events
        let counter = self.event_counter.fetch_add(1, Ordering::SeqCst);
        counter % 10 == 0
    }
    
    /// Sampling verification (re-execute and compare)
    async fn sampling_verify(&self, event: &Event) -> Result<(), String> {
        info!("           └─ [SAMPLING] Re-executing event for verification...");
        
        // Simulated: always pass
        // In real implementation, we would:
        // 1. Re-execute the transfer/operation
        // 2. Compare state changes with event's execution result
        // 3. If mismatch, submit proof of fraud
        
        info!("           └─ [SAMPLING] State changes match (simulated)");
        
        // Check if this is an "evil" event (simulated)
        if event.id.contains("evil") {
            return Err("Fraud detected: state changes do not match".to_string());
        }
        
        Ok(())
    }
    
    /// Apply side effects for specific event types
    async fn apply_event_side_effects(&self, event_id: &str) {
        let event = match self.events.read().get(event_id).cloned() {
            Some(e) => e,
            None => return,
        };
        
        match &event.payload {
            EventPayload::SolverRegister(reg) => {
                info!("[SIDE_EFFECT] Applying SolverRegister: {}", reg.solver_id);
                // The solver is already registered via the channel mechanism
                // This event confirms the registration in the DAG
            }
            EventPayload::ValidatorRegister(reg) => {
                info!("[SIDE_EFFECT] Applying ValidatorRegister: {}", reg.validator_id);
                let validator_info = ValidatorInfo {
                    validator_id: reg.validator_id.clone(),
                    address: reg.address.clone(),
                    port: reg.port,
                    status: "online".to_string(),
                    registered_at: event.timestamp / 1000,
                };
                self.validators.write().insert(reg.validator_id.clone(), validator_info);
            }
            EventPayload::SolverUnregister(unreg) => {
                info!("[SIDE_EFFECT] Applying SolverUnregister: {}", unreg.node_id);
                self.router_manager.unregister_solver(&unreg.node_id);
                self.solver_channels.write().remove(&unreg.node_id);
            }
            EventPayload::ValidatorUnregister(unreg) => {
                info!("[SIDE_EFFECT] Applying ValidatorUnregister: {}", unreg.node_id);
                self.validators.write().remove(&unreg.node_id);
            }
            EventPayload::Transfer(_) => {
                info!("[SIDE_EFFECT] Transfer event - state already updated by Solver");
            }
            EventPayload::PowerConsume(p) => {
                info!("[SIDE_EFFECT] PowerConsume: user={}, amount={}", p.user_id, p.amount);
            }
            EventPayload::TaskSubmit(t) => {
                info!("[SIDE_EFFECT] TaskSubmit: task_id={}", t.task_id);
            }
            EventPayload::None => {}
        }
    }
    
    /// Get all events (for debugging)
    pub fn get_events(&self) -> Vec<Event> {
        self.events.read().values().cloned().collect()
    }
    
    /// Register a solver internally (creates channel)
    fn register_solver_internal(&self, request: &RegisterSolverRequest) -> mpsc::UnboundedSender<Transfer> {
        let (tx, rx) = mpsc::unbounded_channel();
        
        // Store the channel
        self.solver_channels.write().insert(request.solver_id.clone(), tx.clone());
        
        // Register with router manager
        self.router_manager.register_solver_with_affinity(
            request.solver_id.clone(),
            format!("{}:{}", request.address, request.port),
            request.capacity,
            tx.clone(),
            request.shard_id.clone(),
            request.resources.clone(),
        );
        
        // TODO: In a real implementation, we would spawn a task to forward
        // transfers from rx to the actual solver via network
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(transfer) = rx.recv().await {
                debug!(
                    transfer_id = %transfer.id,
                    "Transfer ready to send to solver (network forwarding not implemented)"
                );
                // TODO: Send via Anemo RPC to the actual solver
            }
        });
        
        tx
    }
}

/// Registration handler implementation for Validator
pub struct ValidatorRegistrationHandler {
    service: Arc<ValidatorNetworkService>,
}

#[async_trait::async_trait]
impl RegistrationHandler for ValidatorRegistrationHandler {
    async fn register_solver(&self, request: RegisterSolverRequest) -> RegisterSolverResponse {
        info!(
            solver_id = %request.solver_id,
            address = %request.address,
            port = request.port,
            capacity = request.capacity,
            shard_id = ?request.shard_id,
            "Processing solver registration request"
        );
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Solver Registration Flow                      ║");
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Check if already registered
        info!("[REG 1/4] 🔍 Checking existing registration...");
        if self.service.router_manager.get_solver(&request.solver_id).is_some() {
            warn!("           └─ Solver already registered, will update");
        } else {
            info!("           └─ New solver registration");
        }
        
        // Step 2: Create channel and register with router (for receiving transfers)
        info!("[REG 2/4] 📡 Creating communication channel...");
        let _channel = self.service.register_solver_internal(&request);
        info!("           └─ Channel created for solver: {}", request.solver_id);
        
        // Step 3: In a full implementation, we would:
        // - Convert this to a Transfer with SolverRegister payload
        // - Route to an existing Solver for execution
        // - Wait for the Event to come back
        // For now, we simulate this by creating the Event directly
        info!("[REG 3/4] 📝 Creating registration event (simulated)...");
        info!("           └─ In production: Transfer → Solver → Event → Validator");
        info!("           └─ Current: Direct registration (bootstrap mode)");
        
        // Create a simulated registration event
        let vlc_time = self.service.vlc_counter.fetch_add(1, Ordering::SeqCst);
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(&self.service.validator_id);
        let vlc_snapshot = setu_vlc::VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        
        let registration = SolverRegistration {
            solver_id: request.solver_id.clone(),
            address: request.address.clone(),
            port: request.port,
            capacity: request.capacity,
            shard_id: request.shard_id.clone(),
            resources: request.resources.clone(),
            public_key: request.public_key.clone(),
        };
        
        let mut event = Event::solver_register(
            registration,
            vec![], // No parents for bootstrap registration
            vlc_snapshot,
            request.solver_id.clone(), // Creator is the solver itself
        );
        
        // Set execution result (simulated successful execution)
        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("Solver registration executed successfully".to_string()),
            state_changes: vec![
                setu_types::event::StateChange {
                    key: format!("solver:{}", request.solver_id),
                    old_value: None,
                    new_value: Some(format!("registered:{}:{}", request.address, request.port).into_bytes()),
                }
            ],
        });
        
        // Step 4: Add event to DAG
        info!("[REG 4/4] 🔗 Adding registration event to DAG...");
        let event_id = event.id.clone();
        self.service.events.write().insert(event_id.clone(), event);
        self.service.dag_events.write().push(event_id.clone());
        
        let tracker = EventTracker {
            event_id: event_id.clone(),
            event_type: EventType::SolverRegister,
            status: EventStatus::Confirmed,
            solver_id: request.solver_id.clone(),
            verified: true,
            vlc_time,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        self.service.event_trackers.write().insert(event_id.clone(), tracker);
        
        info!("           └─ Event ID: {}", &event_id[..20.min(event_id.len())]);
        info!("           └─ VLC Time: {}", vlc_time);
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Solver Registered Successfully                ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Solver ID:  {:^44} ║", &request.solver_id);
        info!("║  Address:    {:^44} ║", format!("{}:{}", request.address, request.port));
        info!("║  Total:      {:^44} ║", self.service.router_manager.solver_count());
        info!("╚════════════════════════════════════════════════════════════╝");
        
        RegisterSolverResponse {
            success: true,
            message: "Solver registered successfully".to_string(),
            assigned_id: Some(request.solver_id),
        }
    }
    
    async fn register_validator(&self, request: RegisterValidatorRequest) -> RegisterValidatorResponse {
        info!(
            validator_id = %request.validator_id,
            address = %request.address,
            port = request.port,
            "Processing validator registration request"
        );
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Validator Registration Flow                   ║");
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Create registration event
        info!("[REG 1/3] 📝 Creating validator registration event...");
        
        let vlc_time = self.service.vlc_counter.fetch_add(1, Ordering::SeqCst);
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(&self.service.validator_id);
        let vlc_snapshot = setu_vlc::VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        
        let registration = ValidatorRegistration {
            validator_id: request.validator_id.clone(),
            address: request.address.clone(),
            port: request.port,
            public_key: request.public_key.clone(),
            stake: request.stake,
        };
        
        let mut event = Event::validator_register(
            registration,
            vec![],
            vlc_snapshot,
            request.validator_id.clone(),
        );
        
        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("Validator registration executed successfully".to_string()),
            state_changes: vec![
                setu_types::event::StateChange {
                    key: format!("validator:{}", request.validator_id),
                    old_value: None,
                    new_value: Some(format!("registered:{}:{}", request.address, request.port).into_bytes()),
                }
            ],
        });
        
        // Step 2: Add to validators map
        info!("[REG 2/3] 📋 Registering validator...");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let validator_info = ValidatorInfo {
            validator_id: request.validator_id.clone(),
            address: request.address.clone(),
            port: request.port,
            status: "online".to_string(),
            registered_at: now,
        };
        
        self.service.validators.write().insert(
            request.validator_id.clone(),
            validator_info,
        );
        
        // Step 3: Add event to DAG
        info!("[REG 3/3] 🔗 Adding registration event to DAG...");
        let event_id = event.id.clone();
        self.service.events.write().insert(event_id.clone(), event);
        self.service.dag_events.write().push(event_id.clone());
        
        let tracker = EventTracker {
            event_id: event_id.clone(),
            event_type: EventType::ValidatorRegister,
            status: EventStatus::Confirmed,
            solver_id: request.validator_id.clone(),
            verified: true,
            vlc_time,
            created_at: now,
        };
        self.service.event_trackers.write().insert(event_id.clone(), tracker);
        
        info!("           └─ Event ID: {}", &event_id[..20.min(event_id.len())]);
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Validator Registered Successfully             ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Validator:  {:^44} ║", &request.validator_id);
        info!("║  Total:      {:^44} ║", self.service.validators.read().len());
        info!("╚════════════════════════════════════════════════════════════╝");
        
        RegisterValidatorResponse {
            success: true,
            message: "Validator registered successfully".to_string(),
        }
    }
    
    async fn unregister(&self, request: UnregisterRequest) -> UnregisterResponse {
        info!(
            node_id = %request.node_id,
            node_type = %request.node_type,
            "Processing unregister request"
        );
        
        match request.node_type {
            NodeType::Solver => {
                self.service.router_manager.unregister_solver(&request.node_id);
                self.service.solver_channels.write().remove(&request.node_id);
                
                UnregisterResponse {
                    success: true,
                    message: "Solver unregistered successfully".to_string(),
                }
            }
            NodeType::Validator => {
                self.service.validators.write().remove(&request.node_id);
                
                UnregisterResponse {
                    success: true,
                    message: "Validator unregistered successfully".to_string(),
                }
            }
        }
    }
    
    async fn heartbeat(&self, request: HeartbeatRequest) -> HeartbeatResponse {
        debug!(
            node_id = %request.node_id,
            current_load = ?request.current_load,
            "Processing heartbeat"
        );
        
        // Update solver load if provided
        if let Some(load) = request.current_load {
            self.service.router_manager.update_solver_load(&request.node_id, load);
        }
        
        let server_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        HeartbeatResponse {
            acknowledged: true,
            server_timestamp,
        }
    }
    
    async fn get_solver_list(&self, request: GetSolverListRequest) -> GetSolverListResponse {
        let solvers = self.service.router_manager.get_all_solvers();
        
        let solver_list: Vec<SolverListItem> = solvers
            .into_iter()
            .filter(|s| {
                // Filter by shard if specified
                if let Some(ref shard_id) = request.shard_id {
                    s.shard_id.as_ref() == Some(shard_id)
                } else {
                    true
                }
            })
            .map(|s| SolverListItem {
                solver_id: s.id,
                address: s.address.split(':').next().unwrap_or("").to_string(),
                port: s.address.split(':').nth(1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0),
                capacity: s.capacity,
                current_load: s.current_load,
                status: format!("{:?}", s.status),
                shard_id: s.shard_id,
            })
            .collect();
        
        GetSolverListResponse { solvers: solver_list }
    }
    
    async fn get_validator_list(&self, _request: GetValidatorListRequest) -> GetValidatorListResponse {
        let validators = self.service.validators.read();
        
        let validator_list: Vec<ValidatorListItem> = validators
            .values()
            .map(|v| ValidatorListItem {
                validator_id: v.validator_id.clone(),
                address: v.address.clone(),
                port: v.port,
                status: v.status.clone(),
            })
            .collect();
        
        GetValidatorListResponse { validators: validator_list }
    }
    
    async fn get_node_status(&self, request: GetNodeStatusRequest) -> GetNodeStatusResponse {
        // Check if it's a solver
        if let Some(solver) = self.service.router_manager.get_solver(&request.node_id) {
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Solver),
                status: Some(format!("{:?}", solver.status)),
                address: Some(solver.address.split(':').next().unwrap_or("").to_string()),
                port: solver.address.split(':').nth(1).and_then(|p| p.parse().ok()),
                uptime_seconds: None, // Not tracked for solvers
            };
        }
        
        // Check if it's a validator
        if let Some(validator) = self.service.validators.read().get(&request.node_id) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Validator),
                status: Some(validator.status.clone()),
                address: Some(validator.address.clone()),
                port: Some(validator.port),
                uptime_seconds: Some(now - validator.registered_at),
            };
        }
        
        // Check if it's this validator
        if request.node_id == self.service.validator_id {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Validator),
                status: Some("online".to_string()),
                address: None,
                port: None,
                uptime_seconds: Some(now - self.service.start_time),
            };
        }
        
        GetNodeStatusResponse {
            found: false,
            node_id: request.node_id,
            node_type: None,
            status: None,
            address: None,
            port: None,
            uptime_seconds: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_register_solver() {
        let router_manager = Arc::new(RouterManager::new());
        let config = NetworkServiceConfig::default();
        let service = Arc::new(ValidatorNetworkService::new(
            "test-validator".to_string(),
            router_manager.clone(),
            config,
        ));
        
        let handler = service.registration_handler();
        
        let request = RegisterSolverRequest {
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
        assert_eq!(router_manager.solver_count(), 1);
    }
    
    #[tokio::test]
    async fn test_register_validator() {
        let router_manager = Arc::new(RouterManager::new());
        let config = NetworkServiceConfig::default();
        let service = Arc::new(ValidatorNetworkService::new(
            "test-validator".to_string(),
            router_manager,
            config,
        ));
        
        let handler = service.registration_handler();
        
        let request = RegisterValidatorRequest {
            validator_id: "validator-2".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9002,
            public_key: None,
            stake: Some(1000),
        };
        
        let response = handler.register_validator(request).await;
        
        assert!(response.success);
        assert_eq!(service.validators.read().len(), 1);
    }
    
    #[tokio::test]
    async fn test_get_solver_list() {
        let router_manager = Arc::new(RouterManager::new());
        let config = NetworkServiceConfig::default();
        let service = Arc::new(ValidatorNetworkService::new(
            "test-validator".to_string(),
            router_manager,
            config,
        ));
        
        let handler = service.registration_handler();
        
        // Register two solvers
        handler.register_solver(RegisterSolverRequest {
            solver_id: "solver-1".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9001,
            capacity: 100,
            shard_id: Some("shard-0".to_string()),
            resources: vec![],
            public_key: None,
        }).await;
        
        handler.register_solver(RegisterSolverRequest {
            solver_id: "solver-2".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9002,
            capacity: 200,
            shard_id: Some("shard-1".to_string()),
            resources: vec![],
            public_key: None,
        }).await;
        
        // Get all solvers
        let response = handler.get_solver_list(GetSolverListRequest {
            shard_id: None,
            status_filter: None,
        }).await;
        
        assert_eq!(response.solvers.len(), 2);
        
        // Filter by shard
        let response = handler.get_solver_list(GetSolverListRequest {
            shard_id: Some("shard-0".to_string()),
            status_filter: None,
        }).await;
        
        assert_eq!(response.solvers.len(), 1);
        assert_eq!(response.solvers[0].solver_id, "solver-1");
    }
}

// ============================================
// HTTP Handler Functions (standalone for axum)
// ============================================

async fn http_register_solver(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<RegisterSolverRequest>,
) -> Json<RegisterSolverResponse> {
    let handler = service.registration_handler();
    Json(handler.register_solver(request).await)
}

async fn http_register_validator(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<RegisterValidatorRequest>,
) -> Json<RegisterValidatorResponse> {
    let handler = service.registration_handler();
    Json(handler.register_validator(request).await)
}

async fn http_get_solvers(
    State(service): State<Arc<ValidatorNetworkService>>,
) -> Json<GetSolverListResponse> {
    let handler = service.registration_handler();
    Json(handler.get_solver_list(GetSolverListRequest {
        shard_id: None,
        status_filter: None,
    }).await)
}

async fn http_get_validators(
    State(service): State<Arc<ValidatorNetworkService>>,
) -> Json<GetValidatorListResponse> {
    let handler = service.registration_handler();
    Json(handler.get_validator_list(GetValidatorListRequest {
        status_filter: None,
    }).await)
}

async fn http_submit_transfer(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<SubmitTransferRequest>,
) -> Json<SubmitTransferResponse> {
    Json(service.submit_transfer(request).await)
}

async fn http_get_transfer_status(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetTransferStatusRequest>,
) -> Json<GetTransferStatusResponse> {
    Json(service.get_transfer_status(&request.transfer_id))
}

async fn http_heartbeat(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<HeartbeatRequest>,
) -> Json<HeartbeatResponse> {
    let handler = service.registration_handler();
    Json(handler.heartbeat(request).await)
}

async fn http_submit_event(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<SubmitEventRequest>,
) -> Json<SubmitEventResponse> {
    Json(service.submit_event(request).await)
}

async fn http_get_events(
    State(service): State<Arc<ValidatorNetworkService>>,
) -> Json<serde_json::Value> {
    let events = service.get_events();
    let dag_size = service.dag_events.read().len();
    let pending_size = service.pending_events.read().len();
    
    Json(serde_json::json!({
        "total_events": events.len(),
        "dag_size": dag_size,
        "pending_size": pending_size,
        "events": events.iter().map(|e| serde_json::json!({
            "id": e.id,
            "type": e.event_type.name(),
            "creator": e.creator,
            "status": format!("{:?}", e.status),
            "timestamp": e.timestamp,
            "parent_count": e.parent_ids.len(),
        })).collect::<Vec<_>>()
    }))
}

async fn http_health(
    State(service): State<Arc<ValidatorNetworkService>>,
) -> Json<serde_json::Value> {
    let uptime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() - service.start_time;
    
    Json(serde_json::json!({
        "status": "healthy",
        "validator_id": service.validator_id,
        "uptime_seconds": uptime,
        "solver_count": service.router_manager.solver_count(),
        "validator_count": service.validators.read().len(),
    }))
}

