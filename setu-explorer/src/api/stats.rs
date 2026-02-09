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
    
    // Count validators and solvers
    // Strategy: Try RPC first, fallback to events
    let (validator_count, solver_count) = match get_node_counts_from_rpc().await {
        Some(counts) => counts,
        None => get_node_counts_from_events(&storage).await,
    };
    
    // Get all events for activity stats
    let all_events = storage.get_events_by_status(setu_types::EventStatus::Finalized).await;
    
    // Calculate TPS (placeholder - need time-series data)
    let tps = 0.0;
    
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
    
    // Calculate recent activity (last 24 hours)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let day_ago = now.saturating_sub(24 * 60 * 60 * 1000);
    
    let mut last_24h_events = 0;
    let mut last_24h_transfers = 0;
    let mut last_24h_registrations = 0;
    
    for event in &all_events {
        if event.timestamp >= day_ago {
            last_24h_events += 1;
            match event.event_type {
                setu_types::EventType::Transfer => last_24h_transfers += 1,
                setu_types::EventType::ValidatorRegister
                | setu_types::EventType::SolverRegister
                | setu_types::EventType::UserRegister => last_24h_registrations += 1,
                _ => {}
            }
        }
    }
    
    let recent_activity = RecentActivity {
        last_24h_events,
        last_24h_transfers,
        last_24h_registrations,
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

/// Try to get node counts from RPC
async fn get_node_counts_from_rpc() -> Option<(usize, usize)> {
    // Get RPC address from environment or use default
    let rpc_addr = std::env::var("VALIDATOR_RPC_ADDR")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    
    // Try to get validators
    let validators_url = format!("{}/validators", rpc_addr);
    let validator_count = reqwest::get(&validators_url)
        .await
        .ok()?
        .json::<Vec<serde_json::Value>>()
        .await
        .ok()?
        .len();
    
    // Try to get solvers
    let solvers_url = format!("{}/solvers", rpc_addr);
    let solver_count = reqwest::get(&solvers_url)
        .await
        .ok()?
        .json::<Vec<serde_json::Value>>()
        .await
        .ok()?
        .len();
    
    Some((validator_count, solver_count))
}

/// Fallback: count nodes from events
async fn get_node_counts_from_events(storage: &ExplorerStorage) -> (usize, usize) {
    let mut registered_validators = std::collections::HashSet::new();
    let mut registered_solvers = std::collections::HashSet::new();
    
    let all_events = storage.get_events_by_status(setu_types::EventStatus::Finalized).await;
    
    for event in &all_events {
        match &event.payload {
            setu_types::EventPayload::ValidatorRegister(reg) => {
                registered_validators.insert(reg.validator_id.clone());
            }
            setu_types::EventPayload::SolverRegister(reg) => {
                registered_solvers.insert(reg.solver_id.clone());
            }
            _ => {}
        }
    }
    
    (registered_validators.len(), registered_solvers.len())
}


