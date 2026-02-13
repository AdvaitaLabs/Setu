//! Event (transaction) query endpoints

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use setu_types::{EventType, EventStatus};

/// GET /api/v1/explorer/events
/// 
/// List events with pagination and filtering
pub async fn list_events(
    Query(params): Query<EventListParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<EventListResponse>, StatusCode> {
    // Get events based on filters
    let mut all_events = if let Some(ref status_str) = params.status {
        // Filter by status
        let status = parse_event_status(status_str).ok_or(StatusCode::BAD_REQUEST)?;
        storage.get_events_by_status(status).await
    } else if let Some(ref creator) = params.creator {
        // Filter by creator
        storage.get_events_by_creator(creator).await
    } else {
        // Get all finalized events by default
        storage.get_events_by_status(EventStatus::Finalized).await
    };
    
    // Filter by event type if specified
    if let Some(ref type_str) = params.event_type {
        if let Some(event_type) = parse_event_type(type_str) {
            all_events.retain(|e| e.event_type == event_type);
        }
    }
    
    let total = all_events.len();
    
    // Sort by timestamp (newest first)
    all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    
    // Paginate
    let page = params.pagination.page.max(1);
    let limit = params.pagination.limit.min(100);
    let total_pages = (total + limit - 1) / limit;
    let start = (page - 1) * limit;
    let end = (start + limit).min(total);
    
    let page_events = &all_events[start..end];
    
    // Build response items
    let mut events = Vec::new();
    for event in page_events {
        let summary = generate_event_summary(event);
        
        events.push(EventListItem {
            id: event.id.to_string(),
            event_type: format!("{:?}", event.event_type),
            status: format!("{:?}", event.status),
            creator: event.creator.clone(),
            timestamp: event.timestamp,
            vlc_time: event.vlc_snapshot.logical_time,
            anchor_id: None, // TODO: Track event->anchor mapping
            anchor_depth: None,
            parent_count: event.parent_ids.len(),
            summary,
        });
    }
    
    Ok(Json(EventListResponse {
        events,
        pagination: Pagination {
            page,
            limit,
            total,
            total_pages,
        },
    }))
}

/// GET /api/v1/explorer/event/:id
/// 
/// Get detailed information about a specific event
pub async fn get_event_detail(
    Path(event_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<EventDetailResponse>, StatusCode> {
    // Get event
    let event = storage
        .get_event(&event_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    
    // Get depth
    let depth = storage.get_event_depth(&event.id).await.unwrap_or(0);
    
    // Get parent depths
    let mut parent_depths = Vec::new();
    for parent_id in &event.parent_ids {
        if let Some(parent_depth) = storage.get_event_depth(parent_id).await {
            parent_depths.push(parent_depth);
        }
    }
    
    // Get children (TODO: need children index in EventStore)
    let children_ids = Vec::new();
    
    // Serialize payload
    let payload = serde_json::to_value(&event.payload).unwrap_or(serde_json::json!({}));
    
    // Build execution result
    let execution_result = event.execution_result.as_ref().map(|result| {
        ExecutionResultInfo {
            success: result.success,
            message: result.message.clone().unwrap_or_default(),
            state_changes: result
                .state_changes
                .iter()
                .map(|change| StateChange {
                    key: change.key.clone(),
                    old_value: change.old_value.as_ref().map(|v| String::from_utf8_lossy(v).to_string()),
                    new_value: change.new_value.as_ref().map(|v| String::from_utf8_lossy(v).to_string()),
                })
                .collect(),
        }
    });
    
    Ok(Json(EventDetailResponse {
        id: event.id.to_string(),
        event_type: format!("{:?}", event.event_type),
        status: format!("{:?}", event.status),
        creator: event.creator.clone(),
        timestamp: event.timestamp,
        vlc_snapshot: VLCSnapshotInfo {
            logical_time: event.vlc_snapshot.logical_time,
            physical_time: event.vlc_snapshot.physical_time,
        },
        parent_ids: event.parent_ids.iter().map(|id| id.to_string()).collect(),
        children_ids,
        subnet_id: event.subnet_id.as_ref().map(|id| id.to_string()),
        anchor_id: None, // TODO: Track event->anchor mapping
        anchor_depth: None,
        payload,
        execution_result,
        dag_visualization: DagVisualizationInfo {
            depth,
            parent_depths,
            children_count: 0, // TODO: Get from children index
        },
    }))
}

/// Parse event status from string
fn parse_event_status(s: &str) -> Option<EventStatus> {
    match s.to_lowercase().as_str() {
        "pending" => Some(EventStatus::Pending),
        "inworkqueue" => Some(EventStatus::InWorkQueue),
        "executed" => Some(EventStatus::Executed),
        "confirmed" => Some(EventStatus::Confirmed),
        "finalized" => Some(EventStatus::Finalized),
        "failed" => Some(EventStatus::Failed),
        _ => None,
    }
}

/// Parse event type from string
fn parse_event_type(s: &str) -> Option<EventType> {
    match s.to_lowercase().as_str() {
        "genesis" => Some(EventType::Genesis),
        "system" => Some(EventType::System),
        "transfer" => Some(EventType::Transfer),
        "validatorregister" => Some(EventType::ValidatorRegister),
        "validatorunregister" => Some(EventType::ValidatorUnregister),
        "solverregister" => Some(EventType::SolverRegister),
        "solverunregister" => Some(EventType::SolverUnregister),
        "subnetregister" => Some(EventType::SubnetRegister),
        "userregister" => Some(EventType::UserRegister),
        "powerconsume" => Some(EventType::PowerConsume),
        "tasksubmit" => Some(EventType::TaskSubmit),
        _ => None,
    }
}

/// Generate human-readable summary for an event
fn generate_event_summary(event: &setu_types::Event) -> String {
    match &event.payload {
        setu_types::EventPayload::Transfer(transfer) => {
            format!(
                "Transfer {} from {} to {}",
                transfer.amount, transfer.from, transfer.to
            )
        }
        setu_types::EventPayload::ValidatorRegister(reg) => {
            format!("Validator {} registered", reg.validator_id)
        }
        setu_types::EventPayload::SolverRegister(reg) => {
            format!("Solver {} registered", reg.solver_id)
        }
        setu_types::EventPayload::UserRegister(reg) => {
            format!("User {} registered", reg.address)
        }
        _ => format!("{:?} event", event.event_type),
    }
}

