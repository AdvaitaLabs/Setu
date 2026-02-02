//! Network statistics endpoint

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{extract::State, Json};
use std::sync::Arc;

/// GET /api/v1/explorer/stats
/// 
/// Returns network-wide statistics including:
/// - Total anchors and events
/// - Active validators and solvers
/// - Current TPS
/// - Latest anchor information
/// - Recent activity metrics
pub async fn get_stats(
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<StatsResponse> {
    // Get anchor store stats
    let anchor_count = storage.count_anchors().await;
    let latest_anchor = storage.get_latest_anchor().await;
    
    // Get event store stats
    let event_count = storage.count_events().await;
    
    // Calculate TPS (placeholder - need time-series data)
    let tps = 0.0;
    
    // Get validator and solver counts (placeholder - need registry)
    let validator_count = 1;
    let solver_count = 0;
    
    // Calculate average anchor time (placeholder)
    let avg_anchor_time = if anchor_count > 1 {
        5.0
    } else {
        0.0
    };
    
    // Build latest anchor info
    let latest_anchor_info = latest_anchor.map(|anchor| LatestAnchorInfo {
        id: anchor.id.to_string(),
        depth: anchor.depth,
        event_count: anchor.event_ids.len(),
        timestamp: anchor.timestamp,
        vlc_time: anchor.vlc_snapshot.logical_time,
    });
    
    // Calculate recent activity (placeholder)
    let recent_activity = RecentActivity {
        last_24h_events: 0,
        last_24h_transfers: 0,
        last_24h_registrations: 0,
    };
    
    Json(StatsResponse {
        network: NetworkStats {
            total_anchors: anchor_count as u64,
            total_events: event_count as u64,
            total_validators: validator_count,
            total_solvers: solver_count,
            tps,
            avg_anchor_time,
        },
        latest_anchor: latest_anchor_info,
        recent_activity,
    })
}

