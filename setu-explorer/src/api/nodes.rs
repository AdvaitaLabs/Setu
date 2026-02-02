//! Validator and Solver node query endpoints

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

/// GET /api/v1/explorer/validators
/// 
/// Get list of registered validators
pub async fn get_validators(
    Query(params): Query<PaginationParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<ValidatorListResponse> {
    // TODO: Implement when validator registry is available in storage
    // For now, return empty list
    Json(ValidatorListResponse {
        validators: vec![],
        pagination: PaginationInfo {
            page: params.page,
            limit: params.limit,
            total: 0,
            total_pages: 0,
        },
    })
}

/// GET /api/v1/explorer/validator/:id
/// 
/// Get validator details
pub async fn get_validator(
    Path(validator_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<ValidatorDetail>, StatusCode> {
    // TODO: Implement when validator registry is available
    Err(StatusCode::NOT_IMPLEMENTED)
}

/// GET /api/v1/explorer/solvers
/// 
/// Get list of registered solvers
pub async fn get_solvers(
    Query(params): Query<PaginationParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<SolverListResponse> {
    // TODO: Implement when solver registry is available in storage
    // For now, return empty list
    Json(SolverListResponse {
        solvers: vec![],
        pagination: PaginationInfo {
            page: params.page,
            limit: params.limit,
            total: 0,
            total_pages: 0,
        },
    })
}

/// GET /api/v1/explorer/solver/:id
/// 
/// Get solver details
pub async fn get_solver(
    Path(solver_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<SolverDetail>, StatusCode> {
    // TODO: Implement when solver registry is available
    Err(StatusCode::NOT_IMPLEMENTED)
}

