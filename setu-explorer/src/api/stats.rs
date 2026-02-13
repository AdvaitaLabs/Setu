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
    use std::time::Instant;
    
    let start = Instant::now();
    
    // Get anchor store stats (fast)
    let t1 = Instant::now();
    let anchor_count = storage.count_anchors().await;
    tracing::info!("count_anchors took: {:?}", t1.elapsed());
    
    let t2 = Instant::now();
    let latest_anchor = storage.get_latest_anchor().await;
    tracing::info!("get_latest_anchor took: {:?}", t2.elapsed());
    
    // Get event store stats (fast - uses metadata or prefix count)
    let t3 = Instant::now();
    let event_count = storage.count_events().await;
    tracing::info!("count_events took: {:?}", t3.elapsed());
    
    // Count validators and solvers from /validators and /solvers APIs
    let t4 = Instant::now();
    let (validator_count, solver_count) = count_nodes_fast(&storage).await;
    tracing::info!("count_nodes_fast took: {:?}", t4.elapsed());
    
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
    let t5 = Instant::now();
    let (last_24h_events, last_24h_transfers, last_24h_registrations) = 
        if let Some(anchor) = latest_anchor {
            // Just use anchor event count as approximation (instant)
            let event_count = anchor.event_ids.len() as u64;
            (event_count, event_count / 2, event_count / 10) // Rough estimates
        } else {
            (0, 0, 0)
        };
    tracing::info!("calculate_recent_activity took: {:?}", t5.elapsed());
    
    let recent_activity = RecentActivity {
        last_24h_events,
        last_24h_transfers,
        last_24h_registrations,
    };
    
    tracing::info!("Total stats endpoint took: {:?}", start.elapsed());
    
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

/// Count nodes quickly by only checking event types (no deserialization of payload)
async fn count_nodes_fast(storage: &ExplorerStorage) -> (usize, usize) {
    use setu_storage::ColumnFamily;
    use setu_types::EventType;
    
    let mut validator_registrations: usize = 0;
    let mut solver_registrations: usize = 0;
    
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
                // Only count by event type (fast - no payload deserialization needed)
                match event.event_type {
                    EventType::ValidatorRegister => validator_registrations += 1,
                    EventType::SolverRegister => solver_registrations += 1,
                    EventType::ValidatorUnregister => validator_registrations = validator_registrations.saturating_sub(1),
                    EventType::SolverUnregister => solver_registrations = solver_registrations.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }
    
    (validator_registrations, solver_registrations)
}


