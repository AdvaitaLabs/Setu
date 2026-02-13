//! Validator and Solver node query endpoints

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use std::collections::HashMap;
use setu_types::{EventType, EventPayload};

/// GET /api/v1/explorer/validators
/// 
/// Get list of registered validators
pub async fn get_validators(
    Query(params): Query<PaginationParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<ValidatorListResponse> {
    // Get all events
    let all_events = storage.get_all_events().await;
    
    // Extract validators from ValidatorRegister events
    let mut validators_map: HashMap<String, ValidatorListItem> = HashMap::new();
    
    for event in all_events {
        match &event.payload {
            EventPayload::ValidatorRegister(reg) => {
                validators_map.insert(
                    reg.validator_id.clone(),
                    ValidatorListItem {
                        id: reg.validator_id.clone(),
                        address: reg.validator_id.clone(),
                        status: "Active".to_string(),
                        stake: 0, // TODO: Track stake from state
                        registered_at: event.timestamp,
                        last_active: event.timestamp,
                    },
                );
            }
            EventPayload::ValidatorUnregister(unreg) => {
                if let Some(validator) = validators_map.get_mut(&unreg.node_id) {
                    validator.status = "Inactive".to_string();
                    validator.last_active = event.timestamp;
                }
            }
            _ => {}
        }
    }
    
    // Convert to sorted list
    let mut validators: Vec<ValidatorListItem> = validators_map.into_values().collect();
    validators.sort_by(|a, b| b.registered_at.cmp(&a.registered_at));
    
    let total = validators.len();
    
    // Paginate
    let page = params.page.max(1);
    let limit = params.limit.min(100);
    let total_pages = if total == 0 { 0 } else { (total + limit - 1) / limit };
    let start = (page - 1) * limit;
    let end = (start + limit).min(total);
    
    let page_validators = validators[start..end].to_vec();
    
    Json(ValidatorListResponse {
        validators: page_validators,
        pagination: PaginationInfo {
            page,
            limit,
            total,
            total_pages,
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
    let all_events = storage.get_all_events().await;
    
    let mut validator: Option<ValidatorDetail> = None;
    let mut total_events_validated = 0u64;
    
    for event in all_events {
        match &event.payload {
            EventPayload::ValidatorRegister(reg) if reg.validator_id == validator_id => {
                validator = Some(ValidatorDetail {
                    id: reg.validator_id.clone(),
                    address: reg.validator_id.clone(),
                    status: "Active".to_string(),
                    stake: 0,
                    registered_at: event.timestamp,
                    last_active: event.timestamp,
                    total_anchors_proposed: 0,
                    total_events_validated: 0,
                });
            }
            EventPayload::ValidatorUnregister(unreg) if unreg.node_id == validator_id => {
                if let Some(ref mut v) = validator {
                    v.status = "Inactive".to_string();
                    v.last_active = event.timestamp;
                }
            }
            _ => {
                if event.creator == validator_id {
                    total_events_validated += 1;
                    if let Some(ref mut v) = validator {
                        v.last_active = event.timestamp;
                    }
                }
            }
        }
    }
    
    if let Some(mut v) = validator {
        v.total_events_validated = total_events_validated;
        Ok(Json(v))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// GET /api/v1/explorer/solvers
/// 
/// Get list of registered solvers
pub async fn get_solvers(
    Query(params): Query<PaginationParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<SolverListResponse> {
    // Get all events
    let all_events = storage.get_all_events().await;
    
    // Extract solvers from SolverRegister events
    let mut solvers_map: HashMap<String, SolverListItem> = HashMap::new();
    let mut event_counts: HashMap<String, u64> = HashMap::new();
    
    for event in all_events {
        match &event.payload {
            EventPayload::SolverRegister(reg) => {
                solvers_map.insert(
                    reg.solver_id.clone(),
                    SolverListItem {
                        id: reg.solver_id.clone(),
                        address: reg.solver_id.clone(),
                        status: "Active".to_string(),
                        registered_at: event.timestamp,
                        last_active: event.timestamp,
                        total_events_created: 0,
                    },
                );
            }
            EventPayload::SolverUnregister(unreg) => {
                if let Some(solver) = solvers_map.get_mut(&unreg.node_id) {
                    solver.status = "Inactive".to_string();
                    solver.last_active = event.timestamp;
                }
            }
            _ => {
                // Count events created by this solver
                if solvers_map.contains_key(&event.creator) {
                    *event_counts.entry(event.creator.clone()).or_insert(0) += 1;
                    if let Some(solver) = solvers_map.get_mut(&event.creator) {
                        solver.last_active = event.timestamp;
                    }
                }
            }
        }
    }
    
    // Update event counts
    for (solver_id, count) in event_counts {
        if let Some(solver) = solvers_map.get_mut(&solver_id) {
            solver.total_events_created = count;
        }
    }
    
    // Convert to sorted list
    let mut solvers: Vec<SolverListItem> = solvers_map.into_values().collect();
    solvers.sort_by(|a, b| b.registered_at.cmp(&a.registered_at));
    
    let total = solvers.len();
    
    // Paginate
    let page = params.page.max(1);
    let limit = params.limit.min(100);
    let total_pages = if total == 0 { 0 } else { (total + limit - 1) / limit };
    let start = (page - 1) * limit;
    let end = (start + limit).min(total);
    
    let page_solvers = solvers[start..end].to_vec();
    
    Json(SolverListResponse {
        solvers: page_solvers,
        pagination: PaginationInfo {
            page,
            limit,
            total,
            total_pages,
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
    let all_events = storage.get_all_events().await;
    
    let mut solver: Option<SolverDetail> = None;
    let mut total_events_created = 0u64;
    let mut total_transfers = 0u64;
    let mut total_tasks = 0u64;
    
    for event in all_events {
        match &event.payload {
            EventPayload::SolverRegister(reg) if reg.solver_id == solver_id => {
                solver = Some(SolverDetail {
                    id: reg.solver_id.clone(),
                    address: reg.solver_id.clone(),
                    status: "Active".to_string(),
                    registered_at: event.timestamp,
                    last_active: event.timestamp,
                    total_events_created: 0,
                    total_transfers: 0,
                    total_tasks: 0,
                });
            }
            EventPayload::SolverUnregister(unreg) if unreg.node_id == solver_id => {
                if let Some(ref mut s) = solver {
                    s.status = "Inactive".to_string();
                    s.last_active = event.timestamp;
                }
            }
            _ => {
                if event.creator == solver_id {
                    total_events_created += 1;
                    if let Some(ref mut s) = solver {
                        s.last_active = event.timestamp;
                    }
                    
                    // Count specific event types
                    match event.event_type {
                        EventType::Transfer => total_transfers += 1,
                        EventType::TaskSubmit => total_tasks += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    
    if let Some(mut s) = solver {
        s.total_events_created = total_events_created;
        s.total_transfers = total_transfers;
        s.total_tasks = total_tasks;
        Ok(Json(s))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

