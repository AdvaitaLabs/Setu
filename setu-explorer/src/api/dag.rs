//! DAG visualization and causal graph endpoints

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use setu_types::EventType;

/// GET /api/v1/explorer/dag/live
/// 
/// Get DAG visualization data for live causal graph
/// Returns nodes (events) and edges (parent-child relationships)
pub async fn get_dag_live(
    Query(params): Query<DagLiveParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<DagLiveResponse>, StatusCode> {
    // Get events to visualize
    let events = if let Some(ref anchor_id) = params.anchor_id {
        // Get events from specific anchor
        let anchor = storage
            .get_anchor(anchor_id)
            .await
            .ok_or(StatusCode::NOT_FOUND)?;
        storage.get_events(&anchor.event_ids).await
    } else {
        // Get recent events from latest anchor
        if let Some(latest_anchor) = storage.get_latest_anchor().await {
            let mut all_events = storage.get_events(&latest_anchor.event_ids).await;
            
            // Limit to requested size
            all_events.truncate(params.limit);
            all_events
        } else {
            vec![]
        }
    };
    
    if events.is_empty() {
        return Ok(Json(DagLiveResponse {
            nodes: vec![],
            edges: vec![],
            metadata: DagMetadata {
                total_nodes: 0,
                total_edges: 0,
                depth_range: (0, 0),
                latest_event_id: String::new(),
                anchor_id: params.anchor_id,
            },
        }));
    }
    
    // Build node ID mapping (full ID -> short ID)
    let mut event_id_map = HashMap::new();
    for (idx, event) in events.iter().enumerate() {
        let short_id = format!("ev_{}", idx);
        event_id_map.insert(event.id.clone(), short_id);
    }
    
    // Build nodes
    let mut nodes = Vec::new();
    let mut depths = Vec::new();
    
    for (idx, event) in events.iter().enumerate() {
        let short_id = format!("ev_{}", idx);
        let depth = storage.get_event_depth(&event.id).await.unwrap_or(0);
        depths.push(depth);
        
        nodes.push(DagNode {
            id: short_id.clone(),
            event_id: event.id.to_string(),
            event_type: format!("{:?}", event.event_type),
            status: format!("{:?}", event.status),
            depth,
            timestamp: event.timestamp,
            creator: event.creator.clone(),
            vlc_time: event.vlc_snapshot.logical_time,
            label: short_id.clone(),
            size: calculate_node_size(&event.event_type),
        });
    }
    
    // Build edges
    let mut edges = Vec::new();
    for event in &events {
        if let Some(to_id) = event_id_map.get(&event.id) {
            for parent_id in &event.parent_ids {
                if let Some(from_id) = event_id_map.get(parent_id) {
                    edges.push(DagEdge {
                        from: from_id.clone(),
                        to: to_id.clone(),
                        edge_type: "parent".to_string(),
                    });
                }
            }
        }
    }
    
    // Calculate metadata
    let depth_range = if depths.is_empty() {
        (0, 0)
    } else {
        (*depths.iter().min().unwrap(), *depths.iter().max().unwrap())
    };
    
    let latest_event_id = events.last().map(|e| e.id.to_string()).unwrap_or_default();
    let total_edges = edges.len();
    let total_nodes = events.len();
    
    Ok(Json(DagLiveResponse {
        nodes,
        edges,
        metadata: DagMetadata {
            total_nodes,
            total_edges,
            depth_range,
            latest_event_id,
            anchor_id: params.anchor_id,
        },
    }))
}

/// GET /api/v1/explorer/dag/path/:event_id
/// 
/// Get causal path for a specific event (ancestors and descendants)
pub async fn get_causal_path(
    Path(event_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<CausalPathResponse>, StatusCode> {
    // Check if event exists
    let event = storage
        .get_event(&event_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    
    // Trace ancestors (BFS upward)
    let ancestors = trace_ancestors(&storage, &event.id).await;
    
    // Trace descendants (BFS downward)
    // Note: This requires a children index in EventStore
    let descendants = vec![]; // TODO: Implement when children index is available
    
    // Build path edges
    let mut path_edges = Vec::new();
    
    // Add edges between ancestors
    for ancestor in &ancestors {
        let ancestor_event = storage.get_event(&ancestor.event_id).await;
        if let Some(ae) = ancestor_event {
            for parent_id in &ae.parent_ids {
                let parent_id_str = parent_id.to_string();
                if let Some(parent) = ancestors.iter().find(|a| a.event_id == parent_id_str) {
                    path_edges.push(DagEdge {
                        from: parent.id.clone(),
                        to: ancestor.id.clone(),
                        edge_type: "parent".to_string(),
                    });
                }
            }
        }
    }
    
    Ok(Json(CausalPathResponse {
        event_id: event.id.to_string(),
        ancestors,
        descendants,
        path_edges,
    }))
}

/// Trace ancestor events using BFS
async fn trace_ancestors(
    storage: &ExplorerStorage,
    event_id: &str,
) -> Vec<DagNode> {
    let mut ancestors = Vec::new();
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    
    queue.push_back(event_id.to_string());
    
    while let Some(current_id) = queue.pop_front() {
        if visited.contains(&current_id) {
            continue;
        }
        visited.insert(current_id.clone());
        
        if let Some(event) = storage.get_event(&current_id).await {
            let depth = storage.get_event_depth(&event.id).await.unwrap_or(0);
            
            ancestors.push(DagNode {
                id: format!("ev_{}", ancestors.len()),
                event_id: event.id.to_string(),
                event_type: format!("{:?}", event.event_type),
                status: format!("{:?}", event.status),
                depth,
                timestamp: event.timestamp,
                creator: event.creator.clone(),
                vlc_time: event.vlc_snapshot.logical_time,
                label: format!("ev_{}", ancestors.len()),
                size: calculate_node_size(&event.event_type),
            });
            
            // Add parents to queue
            for parent_id in &event.parent_ids {
                queue.push_back(parent_id.to_string());
            }
        }
    }
    
    // Sort by depth (root first)
    ancestors.sort_by_key(|n| n.depth);
    ancestors
}

/// Calculate node size based on event type
fn calculate_node_size(event_type: &EventType) -> usize {
    match event_type {
        EventType::Genesis => 20,
        EventType::System => 15,
        EventType::Transfer => 10,
        EventType::TaskSubmit => 12,
        EventType::ValidatorRegister | EventType::SolverRegister => 14,
        _ => 8,
    }
}

