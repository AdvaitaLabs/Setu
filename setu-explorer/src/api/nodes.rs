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
    Query(_params): Query<PaginationParams>,
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
                        validator_id: reg.validator_id.clone(),
                        address: reg.validator_id.clone(),
                        network_address: "127.0.0.1:9000".to_string(), // TODO: Get from registration
                        status: "online".to_string(),
                        stake_amount: 10000, // TODO: Track stake from state
                        commission_rate: 10,
                        statistics: ValidatorStatistics {
                            proposed_cfs: 0,
                            approved_votes: 0,
                            rejected_votes: 0,
                            uptime_percentage: 99.8,
                        },
                        registered_at: event.timestamp,
                    },
                );
            }
            EventPayload::ValidatorUnregister(unreg) => {
                if let Some(validator) = validators_map.get_mut(&unreg.node_id) {
                    validator.status = "offline".to_string();
                }
            }
            _ => {}
        }
    }
    
    // Convert to sorted list
    let mut validators: Vec<ValidatorListItem> = validators_map.into_values().collect();
    validators.sort_by(|a, b| b.registered_at.cmp(&a.registered_at));
    
    Json(ValidatorListResponse {
        validators,
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
                    validator_id: reg.validator_id.clone(),
                    address: reg.validator_id.clone(),
                    network_address: "127.0.0.1:9000".to_string(),
                    status: "online".to_string(),
                    stake_amount: 10000,
                    commission_rate: 10,
                    statistics: ValidatorStatistics {
                        proposed_cfs: 0,
                        approved_votes: 0,
                        rejected_votes: 0,
                        uptime_percentage: 99.8,
                    },
                    registered_at: event.timestamp,
                });
            }
            EventPayload::ValidatorUnregister(unreg) if unreg.node_id == validator_id => {
                if let Some(ref mut v) = validator {
                    v.status = "offline".to_string();
                }
            }
            _ => {
                if event.creator == validator_id {
                    total_events_validated += 1;
                }
            }
        }
    }
    
    if let Some(mut v) = validator {
        v.statistics.approved_votes = total_events_validated;
        Ok(Json(v))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// GET /api/v1/explorer/solvers
/// 
/// Get list of registered solvers
pub async fn get_solvers(
    Query(_params): Query<PaginationParams>,
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
                        solver_id: reg.solver_id.clone(),
                        address: reg.solver_id.clone(),
                        network_address: "127.0.0.1:9001".to_string(),
                        status: "active".to_string(),
                        capacity: 100,
                        current_load: 45,
                        shard_id: "shard-0".to_string(),
                        resources: vec!["ETH".to_string(), "BTC".to_string()],
                        statistics: SolverStatistics {
                            total_events_processed: 0,
                            success_rate: 99.5,
                            avg_execution_time_us: 1234,
                        },
                        registered_at: event.timestamp,
                    },
                );
            }
            EventPayload::SolverUnregister(unreg) => {
                if let Some(solver) = solvers_map.get_mut(&unreg.node_id) {
                    solver.status = "inactive".to_string();
                }
            }
            _ => {
                // Count events created by this solver
                if solvers_map.contains_key(&event.creator) {
                    *event_counts.entry(event.creator.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    
    // Update event counts
    for (solver_id, count) in event_counts {
        if let Some(solver) = solvers_map.get_mut(&solver_id) {
            solver.statistics.total_events_processed = count;
            solver.current_load = (count % 100) as u64; // Mock current load
        }
    }
    
    // Convert to sorted list
    let mut solvers: Vec<SolverListItem> = solvers_map.into_values().collect();
    solvers.sort_by(|a, b| b.registered_at.cmp(&a.registered_at));
    
    Json(SolverListResponse {
        solvers,
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
                    solver_id: reg.solver_id.clone(),
                    address: reg.solver_id.clone(),
                    network_address: "127.0.0.1:9001".to_string(),
                    status: "active".to_string(),
                    capacity: 100,
                    current_load: 45,
                    shard_id: "shard-0".to_string(),
                    resources: vec!["ETH".to_string(), "BTC".to_string()],
                    statistics: SolverStatistics {
                        total_events_processed: 0,
                        success_rate: 99.5,
                        avg_execution_time_us: 1234,
                    },
                    registered_at: event.timestamp,
                });
            }
            EventPayload::SolverUnregister(unreg) if unreg.node_id == solver_id => {
                if let Some(ref mut s) = solver {
                    s.status = "inactive".to_string();
                }
            }
            _ => {
                if event.creator == solver_id {
                    total_events_created += 1;
                    
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
        s.statistics.total_events_processed = total_events_created;
        s.current_load = (total_events_created % 100) as u64;
        Ok(Json(s))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

