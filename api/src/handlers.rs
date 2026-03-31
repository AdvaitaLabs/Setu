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
    GetPowerRequest, GetPowerResponse, GetCreditRequest, GetCreditResponse,
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
    
    /// Get balance (state query)
    fn get_balance(&self, account: &str) -> GetBalanceResponse;
    
    /// Get object (state query)
    fn get_object(&self, key: &str) -> GetObjectResponse;
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
pub async fn http_get_credit<S: ValidatorService>(
    State(service): State<Arc<S>>,
    Json(request): Json<GetCreditRequest>,
) -> Json<GetCreditResponse> {
    let handler = service.user_handler();
    Json(handler.get_credit(request).await)
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
