//! HTTP handlers for network service

use super::service::ValidatorNetworkService;
use super::types::{
    GetBalanceResponse, GetObjectResponse, SubmitEventRequest, SubmitEventResponse,
    current_timestamp_secs,
};
use axum::{extract::State, Json};
use setu_rpc::{
    GetSolverListRequest, GetSolverListResponse, GetTransferStatusRequest,
    GetTransferStatusResponse, GetValidatorListRequest, GetValidatorListResponse,
    HeartbeatRequest, HeartbeatResponse, RegisterSolverRequest, RegisterSolverResponse,
    RegisterValidatorRequest, RegisterValidatorResponse, RegistrationHandler,
    SubmitTransferRequest, SubmitTransferResponse,
    // User RPC imports
    UserRpcHandler, RegisterUserRequest, RegisterUserResponse,
    GetAccountRequest, GetAccountResponse, GetBalanceRequest, GetBalanceResponse as UserGetBalanceResponse,
    GetPowerRequest, GetPowerResponse, GetCreditRequest, GetCreditResponse,
    GetCredentialsRequest, GetCredentialsResponse, TransferRequest, TransferResponse,
};
use std::sync::Arc;

// ============================================
// Registration Handlers
// ============================================

pub async fn http_register_solver(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<RegisterSolverRequest>,
) -> Json<RegisterSolverResponse> {
    let handler = service.registration_handler();
    Json(handler.register_solver(request).await)
}

pub async fn http_register_validator(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<RegisterValidatorRequest>,
) -> Json<RegisterValidatorResponse> {
    let handler = service.registration_handler();
    Json(handler.register_validator(request).await)
}

// ============================================
// Query Handlers
// ============================================

pub async fn http_get_solvers(
    State(service): State<Arc<ValidatorNetworkService>>,
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

pub async fn http_get_validators(
    State(service): State<Arc<ValidatorNetworkService>>,
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

// ============================================
// Transfer Handlers
// ============================================

pub async fn http_submit_transfer(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<SubmitTransferRequest>,
) -> Json<SubmitTransferResponse> {
    Json(service.submit_transfer(request).await)
}

pub async fn http_get_transfer_status(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetTransferStatusRequest>,
) -> Json<GetTransferStatusResponse> {
    Json(service.get_transfer_status(&request.transfer_id))
}

// ============================================
// Event Handlers
// ============================================

pub async fn http_submit_event(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<SubmitEventRequest>,
) -> Json<SubmitEventResponse> {
    Json(service.submit_event(request).await)
}

pub async fn http_get_events(
    State(service): State<Arc<ValidatorNetworkService>>,
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

pub async fn http_heartbeat(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<HeartbeatRequest>,
) -> Json<HeartbeatResponse> {
    let handler = service.registration_handler();
    Json(handler.heartbeat(request).await)
}

pub async fn http_health(
    State(service): State<Arc<ValidatorNetworkService>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "validator_id": service.validator_id(),
        "uptime_seconds": current_timestamp_secs() - service.start_time(),
        "solver_count": service.solver_count(),
        "validator_count": service.validator_count(),
    }))
}

// ============================================
// State Query Handlers (Scheme B)
// ============================================

pub async fn http_get_balance(
    State(service): State<Arc<ValidatorNetworkService>>,
    axum::extract::Path(account): axum::extract::Path<String>,
) -> Json<GetBalanceResponse> {
    Json(service.get_balance(&account))
}

pub async fn http_get_object(
    State(service): State<Arc<ValidatorNetworkService>>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Json<GetObjectResponse> {
    Json(service.get_object(&key))
}

// ============================================
// User RPC Handlers
// ============================================

pub async fn http_register_user(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<RegisterUserRequest>,
) -> Json<RegisterUserResponse> {
    let handler = service.user_handler();
    Json(handler.register_user(request).await)
}

pub async fn http_get_account(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetAccountRequest>,
) -> Json<GetAccountResponse> {
    let handler = service.user_handler();
    Json(handler.get_account(request).await)
}

pub async fn http_get_user_balance(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetBalanceRequest>,
) -> Json<UserGetBalanceResponse> {
    let handler = service.user_handler();
    Json(handler.get_balance(request).await)
}

pub async fn http_get_power(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetPowerRequest>,
) -> Json<GetPowerResponse> {
    let handler = service.user_handler();
    Json(handler.get_power(request).await)
}

pub async fn http_get_credit(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetCreditRequest>,
) -> Json<GetCreditResponse> {
    let handler = service.user_handler();
    Json(handler.get_credit(request).await)
}

pub async fn http_get_credentials(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<GetCredentialsRequest>,
) -> Json<GetCredentialsResponse> {
    let handler = service.user_handler();
    Json(handler.get_credentials(request).await)
}

pub async fn http_user_transfer(
    State(service): State<Arc<ValidatorNetworkService>>,
    Json(request): Json<TransferRequest>,
) -> Json<TransferResponse> {
    let handler = service.user_handler();
    Json(handler.transfer(request).await)
}
