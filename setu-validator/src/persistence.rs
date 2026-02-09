//! Finalization Persistence
//!
//! Shared trait for persisting finalized consensus data (Anchor + Events).
//! This eliminates code duplication between MessageRouter and ConsensusValidator.
//!
//! ## GC Integration
//! 
//! After persistence completes, this module triggers GC via `DagManager.on_anchor_finalized()`.
//! This ensures:
//! 1. Events are safely persisted before being removed from DAG
//! 2. Depth information is preserved in EventStore for three-layer queries
//! 3. RecentCache is populated for efficient cross-CF parent resolution
//!
//! ## Crash Consistency
//!
//! This module guarantees crash consistency:
//! - Events are written BEFORE the anchor (anchor serves as commit marker)
//! - If ANY event write fails critically, anchor is NOT written
//! - On recovery, missing anchor indicates incomplete persistence → retry

use consensus::ConsensusEngine;
use setu_storage::{AnchorStoreBackend, EventStoreBackend};
use setu_types::Anchor;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Error type for finalization persistence
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("Failed to persist {failed} of {total} events for anchor {anchor_id}")]
    EventPersistenceFailed {
        anchor_id: String,
        failed: usize,
        total: usize,
    },
    
    #[error("Failed to persist anchor {anchor_id}: {reason}")]
    AnchorPersistenceFailed {
        anchor_id: String,
        reason: String,
    },
}

/// Result type for persistence operations
pub type PersistenceResult<T> = Result<T, PersistenceError>;

/// Trait for components that can persist finalized anchors
///
/// Both `MessageRouter` and `ConsensusValidator` need to persist finalized data
/// when a CF reaches quorum. This trait provides a shared implementation.
///
/// ## Persistence Order
/// 
/// Events are persisted first, then Anchor (as commit marker).
/// This ensures crash recovery can detect incomplete persistence.
///
/// ## Crash Consistency
///
/// If any event fails to persist (critical error), the anchor is NOT written.
/// Duplicate events are considered non-critical and don't block anchor write.
///
/// ## GC Trigger
/// 
/// After persistence, `on_anchor_finalized()` is called to:
/// 1. Move finalized events from DAG to RecentCache
/// 2. GC events without active children
#[async_trait::async_trait]
pub trait FinalizationPersister: Send + Sync {
    /// Get the consensus engine (for fetching events by ID)
    fn engine(&self) -> &Arc<ConsensusEngine>;
    
    /// Get the event store (for persisting events)
    fn event_store(&self) -> &Arc<dyn EventStoreBackend>;
    
    /// Get the anchor store (for persisting anchors)
    fn anchor_store(&self) -> &Arc<dyn AnchorStoreBackend>;

    /// Persist a finalized anchor and its events to storage
    /// 
    /// ## Crash Consistency Guarantee
    /// 
    /// - Events are written BEFORE the anchor
    /// - If ANY event fails critically, anchor is NOT written (returns error)
    /// - Duplicate events are non-critical (skipped, don't block anchor)
    /// 
    /// ## Returns
    /// 
    /// - `Ok(())` if persistence succeeded (all events + anchor written)
    /// - `Err(PersistenceError)` if critical failure occurred
    async fn persist_finalized_anchor(&self, anchor: &Anchor) -> PersistenceResult<()> {
        // 0. Idempotency check: skip if anchor already persisted
        // This handles retries and prevents false "state corruption" errors
        if self.anchor_store().get(&anchor.id).await.is_some() {
            debug!(anchor_id = %anchor.id, "Anchor already persisted, skipping (idempotent)");
            return Ok(());
        }
        
        // 1. Get all events included in this anchor from the DAG
        let dag = self.engine().dag_manager().dag().read().await;
        
        // Collect events and track any missing ones (indicates state corruption)
        let mut missing_ids = Vec::new();
        let events_with_depths: Vec<_> = anchor.event_ids
            .iter()
            .filter_map(|id| {
                match dag.get_event(id) {
                    Some(event) => {
                        let depth = dag.get_depth(id)
                            .expect("depth must exist for event in DAG");
                        Some((event.clone(), depth))
                    }
                    None => {
                        missing_ids.push(id.clone());
                        None
                    }
                }
            })
            .collect();
        drop(dag);
        
        // CRITICAL: Events in anchor but missing from DAG indicates state corruption
        if !missing_ids.is_empty() {
            error!(
                anchor_id = %anchor.id,
                missing_count = missing_ids.len(),
                missing_ids = ?missing_ids,
                "CRITICAL: Events in anchor but missing from DAG - possible state corruption!"
            );
            return Err(PersistenceError::EventPersistenceFailed {
                anchor_id: anchor.id.clone(),
                failed: missing_ids.len(),
                total: anchor.event_ids.len(),
            });
        }
        
        // 2. Batch persist events with depth to EventStore (before anchor)
        // Uses optimized batch operation with single lock acquisition
        let total_events = events_with_depths.len();
        let batch_result = self.event_store()
            .store_batch_with_depth(events_with_depths)
            .await;
        
        // Log batch result
        if batch_result.stored > 0 || batch_result.skipped > 0 {
            debug!(
                anchor_id = %anchor.id,
                stored = batch_result.stored,
                skipped = batch_result.skipped,
                failed = batch_result.failed,
                total = total_events,
                "Batch persisted finalized events with depth"
            );
        }
        
        // 3. Check for critical failures - DO NOT write anchor if events failed
        if batch_result.has_critical_failures() {
            error!(
                anchor_id = %anchor.id,
                failed = batch_result.failed,
                total = total_events,
                errors = ?batch_result.failed_errors,
                "Critical event persistence failure - anchor NOT written (crash consistency)"
            );
            return Err(PersistenceError::EventPersistenceFailed {
                anchor_id: anchor.id.clone(),
                failed: batch_result.failed,
                total: total_events,
            });
        }
        
        // Log skipped duplicates (non-critical, just informational)
        if batch_result.skipped > 0 {
            debug!(
                anchor_id = %anchor.id,
                skipped = batch_result.skipped,
                skipped_ids = ?batch_result.skipped_ids,
                "Some events were duplicates (already persisted)"
            );
        }
        
        // 4. Persist anchor to AnchorStore (commit marker)
        // Only reached if all events persisted successfully
        if let Err(e) = self.anchor_store().store(anchor.clone()).await {
            error!(
                anchor_id = %anchor.id,
                error = %e,
                "Failed to persist finalized anchor"
            );
            return Err(PersistenceError::AnchorPersistenceFailed {
                anchor_id: anchor.id.clone(),
                reason: e.to_string(),
            });
        }
        
        info!(
            anchor_id = %anchor.id,
            events_stored = batch_result.stored,
            "Persisted finalized anchor with all events"
        );
        
        // 5. Mark the anchor as persisted in engine (allows GC of in-memory data)
        self.engine().mark_anchor_persisted(&anchor.id).await;
        
        // 6. Trigger GC via DagManager.on_anchor_finalized()
        // This moves events to RecentCache and removes those without active children
        match self.engine().dag_manager().on_anchor_finalized(anchor).await {
            Ok(gc_stats) => {
                debug!(
                    anchor_id = %anchor.id,
                    removed = gc_stats.removed,
                    retained = gc_stats.retained,
                    "GC completed after finalization"
                );
            }
            Err(e) => {
                warn!(
                    anchor_id = %anchor.id,
                    error = %e,
                    "GC failed after finalization (non-fatal, will retry on next finalization)"
                );
            }
        }
        
        Ok(())
    }
}
