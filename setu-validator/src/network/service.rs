//! Core ValidatorNetworkService implementation
//!
//! This is the main service handling network operations for the Validator.
//! Now with integrated consensus support.
//!
//! ## Module Structure
//!
//! The service is split across multiple modules for maintainability:
//! - `service.rs` - Core struct, constructors, accessors, management
//! - `transfer_handler.rs` - Transfer submission and routing
//! - `tee_executor.rs` - Parallel TEE execution (performance critical)
//! - `event_handler.rs` - Event processing, verification, DAG, state queries
//!
//! ## TPS Optimizations
//!
//! This module implements several optimizations for high throughput:
//! - DashMap for lock-free concurrent access to transfer_status, events, solver_info
//! - Reverse index (solver_pending_transfers) to avoid O(n) scans
//! - Lock-free VLC allocation via atomic counter

use super::registration::ValidatorRegistrationHandler;
use super::types::*;
use super::transfer_handler::TransferHandler;
use super::tee_executor::TeeExecutor;
use super::event_handler::EventHandler;
use crate::{RouterManager, TaskPreparer, BatchTaskPreparer, ConsensusValidator};
use crate::coin_reservation::CoinReservationManager;
use axum::{
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use setu_types::Transfer;
use parking_lot::RwLock;
use setu_rpc::{
    GetTransferStatusResponse, RegisterSolverRequest,
    SubmitTransferRequest, SubmitTransferResponse, ValidatorListItem,
    SubmitTransfersBatchRequest, SubmitTransfersBatchResponse,
};
use setu_types::event::{Event, EventPayload};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

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

    /// Batch task preparer for high-throughput scenarios
    batch_task_preparer: Arc<BatchTaskPreparer>,

    /// Consensus validator (optional)
    consensus_validator: Option<Arc<ConsensusValidator>>,

    /// Registered validators
    validators: Arc<RwLock<HashMap<String, ValidatorInfo>>>,

    /// Registered solver information (for sync HTTP calls)
    /// Uses DashMap for lock-free concurrent access
    solver_info: Arc<DashMap<String, SolverInfo>>,

    /// Solver channels for sending SolverTasks (legacy, kept for compatibility)
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<setu_types::task::SolverTask>>>>,

    /// HTTP client for sync Solver calls (used by TeeExecutor)
    #[allow(dead_code)]
    http_client: reqwest::Client,

    /// Transfer tracking - uses DashMap for lock-free concurrent access
    transfer_status: Arc<DashMap<String, TransferTracker>>,

    /// Reverse index: solver_id -> pending transfer_ids (for O(1) lookup)
    solver_pending_transfers: Arc<DashMap<String, Vec<String>>>,

    /// Event storage - uses DashMap for lock-free concurrent access
    events: Arc<DashMap<String, Event>>,

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

    /// Coin reservation manager for cross-batch double-spend prevention
    coin_reservation_manager: Arc<CoinReservationManager>,

    /// TEE executor for parallel task execution
    tee_executor: TeeExecutor,
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
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        // Shared state
        let solver_info = Arc::new(DashMap::new());
        let transfer_status = Arc::new(DashMap::new());
        let events = Arc::new(DashMap::new());
        let dag_events = Arc::new(RwLock::new(Vec::new()));

        // Create CoinReservationManager for cross-batch double-spend prevention
        let coin_reservation_manager = Arc::new(CoinReservationManager::default());

        // Create TEE executor with reservation manager
        let tee_executor = TeeExecutor::new(
            http_client.clone(),
            Arc::clone(&solver_info),
            Arc::clone(&transfer_status),
            Arc::clone(&events),
            Arc::clone(&dag_events),
            None, // No consensus
            validator_id.clone(),
            100, // Max concurrent TEE calls
        ).with_coin_reservation_manager(Arc::clone(&coin_reservation_manager));

        // Create BatchTaskPreparer from TaskPreparer's state
        // Note: In production, both should share the same MerkleStateProvider
        let batch_task_preparer = Arc::new(
            BatchTaskPreparer::new_for_testing(validator_id.clone())
        );

        Self {
            validator_id,
            router_manager,
            task_preparer,
            batch_task_preparer,
            consensus_validator: None,
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_info,
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            http_client,
            transfer_status,
            solver_pending_transfers: Arc::new(DashMap::new()),
            events,
            pending_events: Arc::new(RwLock::new(Vec::new())),
            dag_events,
            config,
            start_time,
            transfer_counter: AtomicU64::new(0),
            vlc_counter: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
            coin_reservation_manager,
            tee_executor,
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

        // Shared state
        let solver_info = Arc::new(DashMap::new());
        let transfer_status = Arc::new(DashMap::new());
        let events = Arc::new(DashMap::new());
        let dag_events = Arc::new(RwLock::new(Vec::new()));

        // Create CoinReservationManager for cross-batch double-spend prevention
        let coin_reservation_manager = Arc::new(CoinReservationManager::default());

        // Create TEE executor with consensus and reservation manager
        let tee_executor = TeeExecutor::new(
            http_client.clone(),
            Arc::clone(&solver_info),
            Arc::clone(&transfer_status),
            Arc::clone(&events),
            Arc::clone(&dag_events),
            Some(Arc::clone(&consensus_validator)),
            validator_id.clone(),
            100, // Max concurrent TEE calls
        ).with_coin_reservation_manager(Arc::clone(&coin_reservation_manager));

        // Create BatchTaskPreparer from TaskPreparer's state
        // Note: In production, both should share the same MerkleStateProvider
        let batch_task_preparer = Arc::new(
            BatchTaskPreparer::new_for_testing(validator_id.clone())
        );

        Self {
            validator_id,
            router_manager,
            task_preparer,
            batch_task_preparer,
            consensus_validator: Some(consensus_validator),
            validators: Arc::new(RwLock::new(HashMap::new())),
            solver_info,
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            http_client,
            transfer_status,
            solver_pending_transfers: Arc::new(DashMap::new()),
            events,
            pending_events: Arc::new(RwLock::new(Vec::new())),
            dag_events,
            config,
            start_time,
            transfer_counter: AtomicU64::new(0),
            vlc_counter: AtomicU64::new(0),
            event_counter: AtomicU64::new(0),
            coin_reservation_manager,
            tee_executor,
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

    /// Get the next VLC time (FAST PATH - lock-free)
    ///
    /// If consensus is enabled, uses atomic counter for O(1) performance.
    /// Otherwise, uses the local vlc_counter (legacy mode).
    #[inline]
    pub fn get_vlc_time(&self) -> u64 {
        if let Some(ref consensus) = self.consensus_validator {
            consensus.allocate_logical_time()
        } else {
            self.vlc_counter.fetch_add(1, Ordering::SeqCst)
        }
    }

    /// Get count of pending TEE tasks
    pub fn pending_tee_count(&self) -> u64 {
        self.tee_executor.pending_count()
    }

    /// Wait for all pending TEE tasks to complete (for graceful shutdown)
    pub async fn wait_for_pending_tee_tasks(&self, timeout: Duration) -> Result<(), &'static str> {
        self.tee_executor.wait_for_pending_tasks(timeout).await
    }

    /// Start background cleanup task for expired coin reservations
    /// 
    /// This spawns a background task that periodically cleans up expired reservations
    /// to prevent memory accumulation. The task runs every 60 seconds.
    ///
    /// Returns a JoinHandle that can be used to cancel the task on shutdown.
    pub fn start_reservation_cleanup_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let reservation_mgr = Arc::clone(&self.coin_reservation_manager);
        let validator_id = self.validator_id.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            
            loop {
                interval.tick().await;
                let removed = reservation_mgr.cleanup_expired();
                if removed > 0 {
                    tracing::debug!(
                        validator_id = %validator_id,
                        removed = removed,
                        "Cleaned up expired coin reservations"
                    );
                }
            }
        })
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
            .route("/api/v1/transfers/batch", post(setu_api::http_submit_transfers_batch::<ValidatorNetworkService>))
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
    // Transfer Processing (delegates to TransferHandler)
    // ============================================

    pub async fn submit_transfer(&self, request: SubmitTransferRequest) -> SubmitTransferResponse {
        let vlc_time = self.get_vlc_time();

        TransferHandler::submit_transfer(
            &self.validator_id,
            &self.router_manager,
            &self.task_preparer,
            &self.coin_reservation_manager,
            &self.transfer_status,
            &self.solver_pending_transfers,
            &self.transfer_counter,
            vlc_time,
            request,
            &self.tee_executor,
        )
        .await
    }

    pub fn get_transfer_status(&self, transfer_id: &str) -> GetTransferStatusResponse {
        TransferHandler::get_transfer_status(&self.transfer_status, transfer_id)
    }

    /// Submit a batch of transfers for optimized processing.
    ///
    /// This method leverages BatchTaskPreparer to reduce lock acquisitions from 5-6N to 2,
    /// providing significant performance improvement for high-throughput scenarios.
    ///
    /// ## Performance
    /// - Single transfer: ~5-6 lock acquisitions
    /// - Batch of 100: ~2 lock acquisitions (99.6% reduction)
    ///
    /// ## Cross-batch Double-spend Prevention
    /// - Uses CoinReservationManager to reserve coins during batch processing
    /// - Reservations are automatically released after TEE task completion
    pub async fn submit_transfers_batch(
        &self,
        request: SubmitTransfersBatchRequest,
    ) -> SubmitTransfersBatchResponse {
        // Generate VLC time for all transfers in batch
        let _vlc_time = self.vlc_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        TransferHandler::submit_transfers_batch(
            &self.validator_id,
            &self.router_manager,
            &self.batch_task_preparer,
            &self.coin_reservation_manager,
            &self.transfer_status,
            &self.solver_pending_transfers,
            &self.transfer_counter,
            &self.vlc_counter,
            request,
            &self.tee_executor,
        )
        .await
    }

    // ============================================
    // Event Processing (delegates to EventHandler)
    // ============================================

    pub async fn submit_event(&self, request: SubmitEventRequest) -> SubmitEventResponse {
        EventHandler::submit_event(
            &self.events,
            &self.pending_events,
            &self.dag_events,
            &self.validators,
            self.consensus_validator.as_ref(),
            &self.event_counter,
            &self.vlc_counter,
            request,
        )
        .await
    }

    pub fn get_events(&self) -> Vec<Event> {
        EventHandler::get_events(&self.events)
    }

    pub async fn add_event_to_dag(&self, event: Event) {
        EventHandler::add_event_to_dag(
            &self.events,
            &self.dag_events,
            self.consensus_validator.as_ref(),
            event,
        )
        .await
    }

    // ============================================
    // State Query (Scheme B)
    // ============================================

    pub fn get_balance(&self, account: &str) -> GetBalanceResponse {
        // Query real balance from MerkleStateProvider
        let coins = self.task_preparer.state_provider().get_coins_for_address(account);
        
        if coins.is_empty() {
            GetBalanceResponse {
                account: account.to_string(),
                balance: 0,
                exists: false,
            }
        } else {
            // Sum all coin balances for this account (across all coin types)
            let total_balance: u64 = coins.iter().map(|c| c.balance).sum();
            GetBalanceResponse {
                account: account.to_string(),
                balance: total_balance as u128,
                exists: true,
            }
        }
    }

    pub fn get_object(&self, key: &str) -> GetObjectResponse {
        EventHandler::get_object(key)
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
        self.validators
            .read()
            .get(node_id)
            .map(|v| current_timestamp_secs() - v.registered_at)
    }

    pub fn get_validator_list(&self) -> Vec<ValidatorListItem> {
        self.validators
            .read()
            .values()
            .map(|v| ValidatorListItem {
                validator_id: v.validator_id.clone(),
                address: v.address.clone(),
                port: v.port,
                account_address: None,
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
            address: request.address.clone(),
            port: request.port,
            capacity: request.capacity,
            shard_id: request.shard_id.clone(),
            resources: request.resources.clone(),
            status: "active".to_string(),
            registered_at: current_timestamp_secs(),
        };

        self.solver_info.insert(request.solver_id.clone(), solver_info);

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

        // Consume channel to avoid memory leak (sync HTTP doesn't use it)
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(_task) = rx.recv().await {
                // Channel consumed but not used - sync HTTP is primary
            }
        });

        info!(
            solver_id = %request.solver_id,
            address = %request.address,
            port = request.port,
            "Solver registered for sync HTTP communication"
        );

        tx
    }

    pub fn unregister_solver(&self, node_id: &str) {
        self.router_manager.unregister_solver(node_id);
        self.solver_channels.write().remove(node_id);
        self.solver_info.remove(node_id);
        info!(solver_id = %node_id, "Solver unregistered");
    }

    /// Apply event side effects (called from registration handler)
    pub async fn apply_event_side_effects(&self, event_id: &str) {
        let event = match self.events.get(event_id).map(|e| e.clone()) {
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
            _ => {}
        }
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

    async fn submit_transfers_batch(&self, request: SubmitTransfersBatchRequest) -> SubmitTransfersBatchResponse {
        self.submit_transfers_batch(request).await
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
            address: "127.0.0.1".to_string(),
            port: 9002,
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
