//! HTTP API Handlers
//!
//! This module contains all HTTP endpoint handlers for the Setu Validator API.
//! These handlers are designed to work with Axum web framework.

use crate::types::*;
use axum::{extract::State, Json};
use serde::Deserialize;
use setu_rpc::{
    GetSolverListRequest, GetSolverListResponse, GetTransferStatusRequest,
    GetTransferStatusResponse, GetValidatorListRequest, GetValidatorListResponse,
    HeartbeatRequest, HeartbeatResponse, RegisterSolverRequest, RegisterSolverResponse,
    RegisterValidatorRequest, RegisterValidatorResponse, RegistrationHandler,
    RegisterSubnetRequest, RegisterSubnetResponse, GetSubnetListRequest, GetSubnetListResponse,
    SubmitTransferRequest, SubmitTransferResponse,
    // Batch transfer imports
    SubmitTransfersBatchRequest, SubmitTransfersBatchResponse,
    // User RPC imports
    UserRpcHandler, RegisterUserRequest, RegisterUserResponse,
    GetAccountRequest, GetAccountResponse, GetBalanceRequest, 
    GetBalanceResponse as UserGetBalanceResponse,
    GetPowerRequest, GetPowerResponse, GetFluxRequest, GetFluxResponse,
    GetCredentialsRequest, GetCredentialsResponse, TransferRequest, TransferResponse,
    // Phase 3: Profile & Subnet Membership
    UpdateProfileRequest, UpdateProfileResponse,
    GetProfileResponse, JoinSubnetRequest, JoinSubnetResponse,
    LeaveSubnetRequest, LeaveSubnetResponse,
    CheckMembershipResponse, GetUserSubnetsResponse,
};
use std::sync::Arc;

/// Trait that the validator service must implement to work with these handlers
pub trait ValidatorService: Send + Sync {
    /// Get validator ID
    fn validator_id(&self) -> &str;
    
    /// Get start time
    fn start_time(&self) -> u64;
    
    /// Get solver count
    fn solver_count(&self) -> usize;
    
    /// Get validator count
    fn validator_count(&self) -> usize;
    
    /// Get DAG events count
    fn dag_events_count(&self) -> usize;
    
    /// Get pending events count
    fn pending_events_count(&self) -> usize;
    
    /// Get registration handler
    fn registration_handler(self: &Arc<Self>) -> Arc<dyn RegistrationHandler>;
    
    /// Get user handler
    fn user_handler(self: &Arc<Self>) -> Arc<dyn UserRpcHandler>;
    
    /// Submit transfer
    fn submit_transfer(&self, request: SubmitTransferRequest) -> impl std::future::Future<Output = SubmitTransferResponse> + Send;
    
    /// Submit batch of transfers (optimized: 2 locks instead of 5-6N)
    fn submit_transfers_batch(&self, request: SubmitTransfersBatchRequest) -> impl std::future::Future<Output = SubmitTransfersBatchResponse> + Send;
    
    /// Get transfer status
    fn get_transfer_status(&self, transfer_id: &str) -> GetTransferStatusResponse;
    
    /// Submit event
    fn submit_event(&self, request: SubmitEventRequest) -> impl std::future::Future<Output = SubmitEventResponse> + Send;
    
    /// Get events
    fn get_events(&self) -> Vec<setu_types::event::Event>;

    /// R5 · Get a single event's full status (execution + on-chain verdict).
    /// Returns `None` if the event is not known to this validator.
    fn get_event_by_id(&self, event_id: &str) -> Option<GetEventResponse>;
    
    /// Get balance (state query)
    fn get_balance(&self, account: &str) -> GetBalanceResponse;
    
    /// Get object (state query)
    fn get_object(&self, key: &str) -> GetObjectResponse;

    /// Submit a Move function call
    fn submit_move_call(&self, request: MoveCallRequest) -> impl std::future::Future<Output = MoveCallResponse> + Send;

    /// Submit a Move module publish
    fn submit_move_publish(&self, request: MovePublishRequest) -> impl std::future::Future<Output = MovePublishResponse> + Send;

    /// Submit a Move package upgrade (B5).
    fn submit_move_upgrade(&self, request: MoveUpgradeRequest) -> impl std::future::Future<Output = MoveUpgradeResponse> + Send;

    /// Submit a Programmable Transaction Block (PTB).
    ///
    /// `Ok(resp)` → HTTP 200 with execution result;
    /// `Err(resp)` → HTTP 400 with the wire/domain error explained in
    /// `resp.error`. See `docs/feat/move-vm-phase9-ptb-event-wire/design.md`.
    fn submit_move_ptb(
        &self,
        request: MovePtbRequest,
    ) -> impl std::future::Future<Output = Result<MovePtbResponse, MovePtbResponse>> + Send;

    /// Query a Move object by object ID (hex).
    ///
    /// When `finalized` is true, bypass the speculative overlay and read the
    /// committed SMT only. Required for cross-validator state comparisons
    /// (see docs/bugs/20260424-state-get-overlay-leak-cross-node.md).
    fn get_move_object(&self, object_id: &str, finalized: bool) -> GetMoveObjectResponse;

    /// B1 · Long-poll variant of [`get_move_object`](Self::get_move_object).
    ///
    /// Blocks until the object's `version` field is `>= min_version`, the
    /// caller-specified `timeout_ms` elapses, or the per-object / global
    /// waiter cap is exceeded. See
    /// [`docs/feat/pwoo-r6-wait-api/design.md`](../../docs/feat/pwoo-r6-wait-api/design.md).
    fn wait_move_object_min_version(
        &self,
        object_id: &str,
        finalized: bool,
        min_version: u64,
        timeout_ms: u64,
    ) -> impl std::future::Future<Output = WaitMoveObjectOutcome> + Send;

    /// Query module ABI (function list)
    fn get_module_abi(&self, address: &str, name: &str) -> GetModuleAbiResponse;

    /// List all modules at an address
    fn list_modules(&self, address: &str) -> ListModulesResponse;
}

// ============================================
// Registration Handlers
// ============================================

/// Register a solver node
pub async fn http_register_solver<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<RegisterSolverRequest>,
) -> Json<RegisterSolverResponse> {
    let handler = service.registration_handler();
    Json(handler.register_solver(request).await)
}

/// Register a validator node
pub async fn http_register_validator<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<RegisterValidatorRequest>,
) -> Json<RegisterValidatorResponse> {
    let handler = service.registration_handler();
    Json(handler.register_validator(request).await)
}

/// Register a subnet
pub async fn http_register_subnet<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<RegisterSubnetRequest>,
) -> Json<RegisterSubnetResponse> {
    let handler = service.registration_handler();
    Json(handler.register_subnet(request).await)
}

// ============================================
// Query Handlers
// ============================================

/// Get list of registered solvers
pub async fn http_get_solvers<S: ValidatorService>(
    State(service): State<Arc<S>>,
) -> Json<GetSolverListResponse> {
    let handler = service.registration_handler();
    Json(
        handler
            .get_solver_list(GetSolverListRequest {
                shard_id: None,
                status_filter: None,
            })
            .await,
    )
}

/// Get list of registered validators
pub async fn http_get_validators<S: ValidatorService>(
    State(service): State<Arc<S>>,
) -> Json<GetValidatorListResponse> {
    let handler = service.registration_handler();
    Json(
        handler
            .get_validator_list(GetValidatorListRequest {
                status_filter: None,
            })
            .await,
    )
}

/// Query parameters for subnet list filtering
#[derive(Debug, Deserialize, Default)]
pub struct SubnetListQuery {
    pub subnet_type: Option<String>,
    pub owner: Option<String>,
}

/// Get list of registered subnets
pub async fn http_get_subnets<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Query(params): axum::extract::Query<SubnetListQuery>,
) -> Json<GetSubnetListResponse> {
    let handler = service.registration_handler();
    Json(
        handler
            .get_subnet_list(GetSubnetListRequest {
                type_filter: params.subnet_type,
                owner_filter: params.owner,
            })
            .await,
    )
}

// ============================================
// Transfer Handlers
// ============================================

/// Submit a transfer
pub async fn http_submit_transfer<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<SubmitTransferRequest>,
) -> Json<SubmitTransferResponse> {
    Json(service.submit_transfer(request).await)
}

/// Submit a batch of transfers
///
/// This endpoint is optimized for high-throughput scenarios.
/// It reduces lock acquisitions from 5-6N (N transfers) to just 2.
///
/// ## Request
/// ```json
/// {
///   "transfers": [
///     { "sender": "alice", "receiver": "bob", "amount": 100 },
///     { "sender": "alice", "receiver": "charlie", "amount": 50 }
///   ]
/// }
/// ```
///
/// ## Limits
/// - Maximum batch size: 200 transfers
/// - Warning threshold: 100 transfers
pub async fn http_submit_transfers_batch<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<SubmitTransfersBatchRequest>,
) -> Json<SubmitTransfersBatchResponse> {
    Json(service.submit_transfers_batch(request).await)
}

/// Get transfer status
pub async fn http_get_transfer_status<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetTransferStatusRequest>,
) -> Json<GetTransferStatusResponse> {
    Json(service.get_transfer_status(&request.transfer_id))
}

// ============================================
// Event Handlers
// ============================================

/// Submit an event
pub async fn http_submit_event<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<SubmitEventRequest>,
) -> Json<SubmitEventResponse> {
    Json(service.submit_event(request).await)
}

/// Get all events
pub async fn http_get_events<S: ValidatorService>(
    State(service): State<Arc<S>>,
) -> Json<serde_json::Value> {
    let events = service.get_events();
    let dag_size = service.dag_events_count();
    let pending_size = service.pending_events_count();

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

/// R5 · Get a single event by id.
///
/// Returns 404 when the validator has never seen the event.  When the event
/// exists but has not yet been applied in a finalized CF, `on_chain` is
/// `null` — callers should poll.
pub async fn http_get_event_by_id<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(event_id): axum::extract::Path<String>,
) -> Result<Json<GetEventResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    match service.get_event_by_id(&event_id) {
        Some(resp) => Ok(Json(resp)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "event not found",
                "event_id": event_id,
            })),
        )),
    }
}

// ============================================
// Heartbeat & Health
// ============================================

/// Handle heartbeat from nodes
pub async fn http_heartbeat<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<HeartbeatRequest>,
) -> Json<HeartbeatResponse> {
    let handler = service.registration_handler();
    Json(handler.heartbeat(request).await)
}

/// Health check endpoint
pub async fn http_health<S: ValidatorService>(
    State(service): State<Arc<S>>,
) -> Json<serde_json::Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    Json(serde_json::json!({
        "status": "healthy",
        "validator_id": service.validator_id(),
        "uptime_seconds": now - service.start_time(),
        "solver_count": service.solver_count(),
        "validator_count": service.validator_count(),
        "dag_events_count": service.dag_events_count(),
    }))
}

// ============================================
// State Query Handlers (Scheme B)
// ============================================

/// Query account balance
pub async fn http_get_balance<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(account): axum::extract::Path<String>,
) -> Json<GetBalanceResponse> {
    Json(service.get_balance(&account))
}

/// Query object by key
pub async fn http_get_object<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Json<GetObjectResponse> {
    Json(service.get_object(&key))
}

// ============================================
// Move VM Handlers (Phase 4)
// ============================================

/// Submit a Move function call
pub async fn http_submit_move_call<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<MoveCallRequest>,
) -> Json<MoveCallResponse> {
    Json(service.submit_move_call(request).await)
}

/// Submit Move module publish
pub async fn http_submit_move_publish<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<MovePublishRequest>,
) -> Json<MovePublishResponse> {
    Json(service.submit_move_publish(request).await)
}

/// Submit Move package upgrade (B5)
pub async fn http_submit_move_upgrade<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<MoveUpgradeRequest>,
) -> Json<MoveUpgradeResponse> {
    Json(service.submit_move_upgrade(request).await)
}

/// Submit a Programmable Transaction Block (PTB).
///
/// HTTP status mapping (post move-vm-phase9-ptb-event-wire):
/// - **200 OK** — PTB executed successfully (`success = true`).
/// - **400 Bad Request** — wire-format failure (hex / BCS / `validate_wire`)
///   OR domain-level execution failure (NotOwnedBySender, ObjectNotFound,
///   StaleObjectVersion, ObjectDigestMismatch, SharedObjectsNotYetSupported,
///   …). The response body's `error` / `code` fields describe which.
///
/// See `docs/feat/move-vm-phase9-ptb-event-wire/design.md` §10.
pub async fn http_submit_move_ptb<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<MovePtbRequest>,
) -> (axum::http::StatusCode, Json<MovePtbResponse>) {
    match service.submit_move_ptb(request).await {
        Ok(resp) => (axum::http::StatusCode::OK, Json(resp)),
        Err(resp) => (axum::http::StatusCode::BAD_REQUEST, Json(resp)),
    }
}

/// Query parameters for Move object lookup.
#[derive(Debug, Deserialize, Default)]
pub struct GetMoveObjectQuery {
    /// If true, read committed SMT only (bypass speculative overlay).
    /// Default false preserves read-your-writes semantics for clients on the
    /// same node that submitted a write.
    pub finalized: Option<bool>,
    /// B1: long-poll until the object's `version` reaches at least this value.
    /// Treated as 0 if absent (non-blocking GET, the legacy semantics).
    pub wait_min_version: Option<u64>,
    /// B1: max wait time in milliseconds. Defaults to
    /// [`DEFAULT_WAIT_TIMEOUT_MS`] when `wait_min_version` is set.
    pub timeout_ms: Option<u64>,
}

/// B1 default long-poll timeout. Picked under the assumption that most
/// reverse proxies idle-cut at 60s; clients can shorten via `?timeout_ms=`.
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// B1 outcome returned by
/// [`ValidatorService::wait_move_object_min_version`]. The HTTP handler maps
/// each variant to an appropriate status code.
#[derive(Debug)]
pub enum WaitMoveObjectOutcome {
    /// Object materialized at `version >= min_version`. → HTTP 200.
    Resolved(GetMoveObjectResponse),
    /// Timeout elapsed before the version target was reached. → HTTP 408.
    /// Carries the most recently observed state for diagnostics.
    Timeout(GetMoveObjectResponse),
    /// Per-object or global waiter cap exceeded. → HTTP 429.
    CapExceeded { reason: String },
    /// Validator service has no watcher attached (mis-configured node). → HTTP 503.
    Unavailable,
}

/// Query a Move object by object ID (hex)
pub async fn http_get_move_object<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(object_id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<GetMoveObjectQuery>,
) -> (axum::http::StatusCode, Json<GetMoveObjectResponse>) {
    let finalized = q.finalized.unwrap_or(false);
    match q.wait_min_version {
        // No wait requested → preserve legacy non-blocking 200 path.
        None | Some(0) => (
            axum::http::StatusCode::OK,
            Json(service.get_move_object(&object_id, finalized)),
        ),
        Some(min_version) => {
            let timeout_ms = q.timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
            match service
                .wait_move_object_min_version(&object_id, finalized, min_version, timeout_ms)
                .await
            {
                WaitMoveObjectOutcome::Resolved(resp) => (axum::http::StatusCode::OK, Json(resp)),
                WaitMoveObjectOutcome::Timeout(resp) => {
                    (axum::http::StatusCode::REQUEST_TIMEOUT, Json(resp))
                }
                WaitMoveObjectOutcome::CapExceeded { reason } => (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    Json(GetMoveObjectResponse {
                        key: format!("oid:{}", object_id.strip_prefix("0x").unwrap_or(&object_id)),
                        object_id,
                        owner: String::new(),
                        ownership: String::new(),
                        type_tag: String::new(),
                        version: 0,
                        data_hex: String::new(),
                        exists: false,
                        digest_hex: String::new(),
                        // v1 contract: object query failures that are NOT a
                        // normal not-found must carry a stable class marker.
                        // Backpressure is not directly listed in the v1 marker
                        // table; classify under SOLVER_UNAVAILABLE because
                        // both surface to clients as "validator cannot serve
                        // this request right now, retry later".
                        error: Some(stable_error(ERROR_SOLVER_UNAVAILABLE, reason)),
                    }),
                ),
                WaitMoveObjectOutcome::Unavailable => (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(GetMoveObjectResponse {
                        key: format!("oid:{}", object_id.strip_prefix("0x").unwrap_or(&object_id)),
                        object_id,
                        owner: String::new(),
                        ownership: String::new(),
                        type_tag: String::new(),
                        version: 0,
                        data_hex: String::new(),
                        exists: false,
                        digest_hex: String::new(),
                        // v1 contract: a watcher-less validator is an
                        // infrastructure/wiring failure, classify as
                        // CONSENSUS_STORAGE so cross-validator scripts can
                        // assert the marker rather than the raw string.
                        error: Some(stable_error(
                            ERROR_CONSENSUS_STORAGE,
                            "wait_min_version not supported on this validator",
                        )),
                    }),
                ),
            }
        }
    }
}

/// Query a module's ABI (function list)
pub async fn http_get_module_abi<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path((address, name)): axum::extract::Path<(String, String)>,
) -> Json<GetModuleAbiResponse> {
    Json(service.get_module_abi(&address, &name))
}

/// List all modules published at an address
pub async fn http_list_modules<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Json<ListModulesResponse> {
    Json(service.list_modules(&address))
}

// ============================================
// User RPC Handlers
// ============================================

/// Register a new user
pub async fn http_register_user<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<RegisterUserRequest>,
) -> Json<RegisterUserResponse> {
    let handler = service.user_handler();
    Json(handler.register_user(request).await)
}

/// Get user account information
pub async fn http_get_account<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetAccountRequest>,
) -> Json<GetAccountResponse> {
    let handler = service.user_handler();
    Json(handler.get_account(request).await)
}

/// Get user balance
pub async fn http_get_user_balance<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetBalanceRequest>,
) -> Json<UserGetBalanceResponse> {
    let handler = service.user_handler();
    Json(handler.get_balance(request).await)
}

/// Get user power
pub async fn http_get_power<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetPowerRequest>,
) -> Json<GetPowerResponse> {
    let handler = service.user_handler();
    Json(handler.get_power(request).await)
}

/// Get user credit
pub async fn http_get_flux<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetFluxRequest>,
) -> Json<GetFluxResponse> {
    let handler = service.user_handler();
    Json(handler.get_flux(request).await)
}

/// Get user credentials
pub async fn http_get_credentials<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetCredentialsRequest>,
) -> Json<GetCredentialsResponse> {
    let handler = service.user_handler();
    Json(handler.get_credentials(request).await)
}

/// User transfer
pub async fn http_user_transfer<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<TransferRequest>,
) -> Json<TransferResponse> {
    let handler = service.user_handler();
    Json(handler.transfer(request).await)
}

// ============================================
// Phase 3: Profile & Subnet Membership
// ============================================

/// Update user profile
pub async fn http_update_profile<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<UpdateProfileRequest>,
) -> Json<UpdateProfileResponse> {
    let handler = service.user_handler();
    Json(handler.update_profile(request).await)
}

/// Get user profile
pub async fn http_get_profile<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Json<GetProfileResponse> {
    let handler = service.user_handler();
    Json(handler.get_profile(&address).await)
}

/// Join subnet
pub async fn http_join_subnet<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<JoinSubnetRequest>,
) -> Json<JoinSubnetResponse> {
    let handler = service.user_handler();
    Json(handler.join_subnet(request).await)
}

/// Leave subnet
pub async fn http_leave_subnet<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<LeaveSubnetRequest>,
) -> Json<LeaveSubnetResponse> {
    let handler = service.user_handler();
    Json(handler.leave_subnet(request).await)
}

/// Check subnet membership
pub async fn http_check_membership<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path((address, subnet_id)): axum::extract::Path<(String, String)>,
) -> Json<CheckMembershipResponse> {
    let handler = service.user_handler();
    Json(handler.check_membership(&address, &subnet_id).await)
}

/// Get user's subnet list
pub async fn http_get_user_subnets<S: ValidatorService>(
    State(service): State<Arc<S>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Json<GetUserSubnetsResponse> {
    let handler = service.user_handler();
    Json(handler.get_user_subnets(&address).await)
}
