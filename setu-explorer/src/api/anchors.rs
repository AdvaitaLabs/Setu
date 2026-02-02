//! Anchor (block) query endpoints

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

/// GET /api/v1/explorer/anchors
/// 
/// List anchors with pagination
pub async fn list_anchors(
    Query(params): Query<PaginationParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<AnchorListResponse>, StatusCode> {
    // Get all anchor IDs from chain
    let chain = storage.get_anchor_chain().await;
    let total = chain.len();
    
    // Calculate pagination
    let page = params.page.max(1);
    let limit = params.limit.min(100); // Max 100 per page
    let total_pages = (total + limit - 1) / limit;
    let start = (page - 1) * limit;
    let end = (start + limit).min(total);
    
    // Get anchors for current page (reverse order - newest first)
    let mut anchors = Vec::new();
    for anchor_id in chain.iter().rev().skip(start).take(end - start) {
        if let Some(anchor) = storage.get_anchor(anchor_id).await {
            anchors.push(AnchorListItem {
                id: anchor.id.to_string(),
                depth: anchor.depth,
                event_count: anchor.event_ids.len(),
                timestamp: anchor.timestamp,
                vlc_time: anchor.vlc_snapshot.logical_time,
                proposer: "validator-1".to_string(), // TODO: Get from CF
                status: "finalized".to_string(),
                state_root: anchor.state_root.clone(),
            });
        }
    }
    
    Ok(Json(AnchorListResponse {
        anchors,
        pagination: Pagination {
            page,
            limit,
            total,
            total_pages,
        },
    }))
}

/// GET /api/v1/explorer/anchor/:id
/// 
/// Get detailed information about a specific anchor
pub async fn get_anchor_detail(
    Path(anchor_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<AnchorDetailResponse>, StatusCode> {
    // Get anchor
    let anchor = storage
        .get_anchor(&anchor_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    
    // Get chain to find previous/next anchors
    let chain = storage.get_anchor_chain().await;
    let current_pos = chain.iter().position(|id| id == &anchor_id);
    
    let previous_anchor = current_pos
        .and_then(|pos| pos.checked_sub(1))
        .and_then(|pos| chain.get(pos))
        .map(|id| id.to_string());
    
    let next_anchor = current_pos
        .and_then(|pos| chain.get(pos + 1))
        .map(|id| id.to_string());
    
    // Build merkle roots info
    let merkle_roots = anchor.merkle_roots.as_ref().map(|roots| {
        let mut subnet_roots = std::collections::HashMap::new();
        for (subnet_id, root) in &roots.subnet_roots {
            subnet_roots.insert(subnet_id.to_string(), hex::encode(root));
        }
        
        MerkleRootsInfo {
            global_state_root: hex::encode(&roots.global_state_root),
            events_root: hex::encode(&roots.events_root),
            anchor_chain_root: hex::encode(&roots.anchor_chain_root),
            subnet_roots,
        }
    });
    
    // Calculate statistics
    let events = storage.get_events(&anchor.event_ids).await;
    
    let mut transfer_count = 0;
    let mut registration_count = 0;
    let mut system_event_count = 0;
    
    for event in &events {
        match event.event_type {
            setu_types::EventType::Transfer => transfer_count += 1,
            setu_types::EventType::ValidatorRegister
            | setu_types::EventType::SolverRegister
            | setu_types::EventType::UserRegister => registration_count += 1,
            setu_types::EventType::System | setu_types::EventType::Genesis => {
                system_event_count += 1
            }
            _ => {}
        }
    }
    
    Ok(Json(AnchorDetailResponse {
        id: anchor.id.to_string(),
        depth: anchor.depth,
        timestamp: anchor.timestamp,
        vlc_snapshot: VLCSnapshotInfo {
            logical_time: anchor.vlc_snapshot.logical_time,
            physical_time: anchor.vlc_snapshot.physical_time,
        },
        previous_anchor,
        next_anchor,
        event_ids: anchor.event_ids.iter().map(|id| id.to_string()).collect(),
        event_count: anchor.event_ids.len(),
        merkle_roots,
        statistics: AnchorStatistics {
            transfer_count,
            registration_count,
            system_event_count,
        },
    }))
}

