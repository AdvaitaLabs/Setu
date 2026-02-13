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
    // Get anchor store stats (fast)
    let anchor_count = storage.count_anchors().await;
    let latest_anchor = storage.get_latest_anchor().await;
    
    // Get event store stats (fast - uses metadata or prefix count)
    let event_count = storage.count_events().await;
    
    // Count validators and solvers by scanning only registration events (fast)
    let (validator_count, solver_count) = count_registered_nodes(&storage).await;
    
    // Calculate TPS (placeholder - need time-series data)
    let tps = 0.0;
    
    // Calculate average anchor time (placeholder)
    let avg_anchor_time = if anchor_count > 1 {
        5.0
    } else {
        0.0
    };
    
    // Build latest anchor info
    let latest_anchor_info = latest_anchor.as_ref().map(|anchor| LatestAnchorInfo {
        id: anchor.id.to_string(),
        depth: anchor.depth,
        event_count: anchor.event_ids.len(),
        timestamp: anchor.timestamp,
        vlc_time: anchor.vlc_snapshot.logical_time,
    });
    
    // Calculate recent activity from latest anchor only (fast approximation)
    let (last_24h_events, last_24h_transfers, last_24h_registrations) = 
        if let Some(anchor) = latest_anchor {
            calculate_recent_activity_from_anchor(&storage, &anchor).await
        } else {
            (0, 0, 0)
        };
    
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

/// Count registered nodes by scanning only registration events (fast)
async fn count_registered_nodes(storage: &ExplorerStorage) -> (usize, usize) {
    use setu_storage::ColumnFamily;
    
    let mut registered_validators = std::collections::HashSet::new();
    let mut registered_solvers = std::collections::HashSet::new();
    
    // Catch up with primary
    let _ = storage.db().try_catch_up_with_primary();
    
    // Get CF handle
    let cf_handle = match storage.db().inner().cf_handle("events") {
        Some(cf) => cf,
        None => return (0, 0),
    };
    
    // Scan only "evt:" prefix (skip index keys)
    let prefix = b"evt:";
    for result in storage.db().inner().prefix_iterator_cf(cf_handle, prefix) {
        if let Ok((_key, value_bytes)) = result {
            // Deserialize event
            if let Ok(event) = bcs::from_bytes::<setu_types::Event>(&value_bytes) {
                // Only process registration events
                match &event.payload {
                    setu_types::EventPayload::ValidatorRegister(reg) => {
                        registered_validators.insert(reg.validator_id.clone());
                    }
                    setu_types::EventPayload::SolverRegister(reg) => {
                        registered_solvers.insert(reg.solver_id.clone());
                    }
                    setu_types::EventPayload::ValidatorUnregister(unreg) => {
                        registered_validators.remove(&unreg.node_id);
                    }
                    setu_types::EventPayload::SolverUnregister(unreg) => {
                        registered_solvers.remove(&unreg.node_id);
                    }
                    _ => {}
                }
            }
        }
    }
    
    (registered_validators.len(), registered_solvers.len())
}

/// Calculate recent activity from anchor events (fast approximation)
async fn calculate_recent_activity_from_anchor(
    storage: &ExplorerStorage,
    anchor: &setu_types::Anchor,
) -> (u64, u64, u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let day_ago = now.saturating_sub(24 * 60 * 60 * 1000);
    
    let events = storage.get_events(&anchor.event_ids).await;
    
    let mut last_24h_events = 0;
    let mut last_24h_transfers = 0;
    let mut last_24h_registrations = 0;
    
    for event in &events {
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
    
    (last_24h_events, last_24h_transfers, last_24h_registrations)
}


