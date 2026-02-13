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
    
    // Get anchor chain length (accurate count)
    let t1 = Instant::now();
    let anchor_chain = storage.get_anchor_chain().await;
    let anchor_count = anchor_chain.len();
    tracing::info!("get_anchor_chain took: {:?}", t1.elapsed());
    
    let t2 = Instant::now();
    let latest_anchor = storage.get_latest_anchor().await;
    tracing::info!("get_latest_anchor took: {:?}", t2.elapsed());
    
    // Get all events to count accurately
    let t3 = Instant::now();
    let all_events = storage.get_all_events().await;
    let event_count = all_events.len();
    tracing::info!("get_all_events took: {:?}", t3.elapsed());
    
    // Count validators and solvers from events
    let t4 = Instant::now();
    let (validator_count, solver_count) = count_nodes_from_events(&all_events);
    tracing::info!("count_nodes_from_events took: {:?}", t4.elapsed());
    
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

/// Count nodes from events list (fast - already in memory)
fn count_nodes_from_events(events: &[setu_types::Event]) -> (usize, usize) {
    use std::collections::HashSet;
    
    let mut validators = HashSet::new();
    let mut solvers = HashSet::new();
    
    for event in events {
        match &event.payload {
            setu_types::EventPayload::ValidatorRegister(reg) => {
                validators.insert(reg.validator_id.clone());
            }
            setu_types::EventPayload::SolverRegister(reg) => {
                solvers.insert(reg.solver_id.clone());
            }
            setu_types::EventPayload::ValidatorUnregister(unreg) => {
                validators.remove(&unreg.node_id);
            }
            setu_types::EventPayload::SolverUnregister(unreg) => {
                solvers.remove(&unreg.node_id);
            }
            _ => {}
        }
    }
    
    (validators.len(), solvers.len())
}


