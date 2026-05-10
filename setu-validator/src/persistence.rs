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
use setu_storage::{AnchorStoreBackend, CFStoreBackend, EventStoreBackend};
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

    #[error("Failed to persist finalized CF index {cf_id}: {reason}")]
    CFIndexPersistenceFailed {
        cf_id: String,
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

    /// Get the CF store (for indexing finalized CFs used by restart projection)
    fn cf_store(&self) -> &Arc<dyn CFStoreBackend>;

    async fn persist_pending_finalized_cfs(&self) -> PersistenceResult<()> {
        let pending_cfs = self.engine().peek_pending_finalized_cfs().await;
        let mut persisted_cf_ids = Vec::new();

        for cf in pending_cfs {
            if self.cf_store().get(&cf.id).await.is_none() {
                self.cf_store().store(cf.clone()).await.map_err(|e| {
                    error!(
                        cf_id = %cf.id,
                        error = %e,
                        "Failed to persist finalized CF index"
                    );
                    PersistenceError::CFIndexPersistenceFailed {
                        cf_id: cf.id.clone(),
                        reason: e.to_string(),
                    }
                })?;
            } else {
                self.cf_store().mark_finalized(&cf.id).await.map_err(|e| {
                    error!(
                        cf_id = %cf.id,
                        error = %e,
                        "Failed to mark finalized CF index"
                    );
                    PersistenceError::CFIndexPersistenceFailed {
                        cf_id: cf.id.clone(),
                        reason: e.to_string(),
                    }
                })?;
            }

            persisted_cf_ids.push(cf.id.clone());
        }

        self.engine()
            .drain_pending_finalized_cfs(&persisted_cf_ids)
            .await;
        Ok(())
    }

    /// Persist a finalized anchor and its events to storage
    ///
    /// F7 note: `ConsensusEngine::handle_finalization` may already have emitted
    /// local and network finalization notifications before this method is called.
    /// If the originator crashes in that window, restart relies on peer state-sync
    /// to recover the finalized CF/anchor relationship. This method preserves the
    /// existing event -> CF index -> anchor commit ordering once invoked.
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
            self.persist_pending_finalized_cfs().await?;
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
        
        // 4. Persist finalized CFs before the anchor commit marker. Recovery
        // only trusts CFs whose anchors exist, so CF-before-anchor is safe
        // across crashes and avoids losing the CF id needed for event queries.
        self.persist_pending_finalized_cfs().await?;

        // 5. Persist anchor to AnchorStore (commit marker)
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
        
        // 6. Mark the anchor as persisted in engine (allows GC of in-memory data)
        self.engine().mark_anchor_persisted(&anchor.id).await;
        
        // 7. Trigger GC via DagManager.on_anchor_finalized()
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use consensus::{ConsensusEngine, ValidatorSet};
    use setu_storage::{AnchorStore, AnchorStoreBackend, CFStoreBackend, EventStore, EventStoreBackend};
    use setu_types::{Anchor, CFId, ConsensusConfig, ConsensusFrame, NodeInfo, SetuError, SetuResult, ValidatorInfo, Vote, VLCSnapshot};
    use std::sync::Arc;

    #[derive(Debug)]
    enum FailureMode {
        Store,
        Mark,
    }

    #[derive(Debug)]
    struct AlwaysFailCFStore {
        mode: FailureMode,
        existing: parking_lot::Mutex<Option<ConsensusFrame>>,
        store_calls: parking_lot::Mutex<usize>,
        mark_calls: parking_lot::Mutex<usize>,
    }

    impl AlwaysFailCFStore {
        fn fail_store() -> Self {
            Self {
                mode: FailureMode::Store,
                existing: parking_lot::Mutex::new(None),
                store_calls: parking_lot::Mutex::new(0),
                mark_calls: parking_lot::Mutex::new(0),
            }
        }

        fn fail_mark(existing: ConsensusFrame) -> Self {
            Self {
                mode: FailureMode::Mark,
                existing: parking_lot::Mutex::new(Some(existing)),
                store_calls: parking_lot::Mutex::new(0),
                mark_calls: parking_lot::Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl CFStoreBackend for AlwaysFailCFStore {
        async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
            *self.store_calls.lock() += 1;
            match self.mode {
                FailureMode::Store => Err(SetuError::InvalidData("simulated CFStore disk error".to_string())),
                FailureMode::Mark => {
                    *self.existing.lock() = Some(cf);
                    Ok(())
                }
            }
        }
        async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
            self.existing
                .lock()
                .as_ref()
                .filter(|cf| &cf.id == cf_id)
                .cloned()
        }
        async fn mark_finalized(&self, _cf_id: &CFId) -> SetuResult<()> {
            *self.mark_calls.lock() += 1;
            match self.mode {
                FailureMode::Store => Ok(()),
                FailureMode::Mark => Err(SetuError::InvalidData("simulated mark_finalized error".to_string())),
            }
        }
        async fn get_pending(&self) -> Vec<ConsensusFrame> {
            Vec::new()
        }
        async fn get_finalized(&self) -> Vec<ConsensusFrame> {
            Vec::new()
        }
        async fn latest_finalized(&self) -> Option<ConsensusFrame> {
            None
        }
        async fn finalized_count(&self) -> usize {
            0
        }
        async fn pending_count(&self) -> usize {
            0
        }
    }

    struct TestPersister {
        engine: Arc<ConsensusEngine>,
        event_store: Arc<dyn EventStoreBackend>,
        anchor_store: Arc<dyn AnchorStoreBackend>,
        cf_store: Arc<dyn CFStoreBackend>,
    }

    impl FinalizationPersister for TestPersister {
        fn engine(&self) -> &Arc<ConsensusEngine> {
            &self.engine
        }

        fn event_store(&self) -> &Arc<dyn EventStoreBackend> {
            &self.event_store
        }

        fn anchor_store(&self) -> &Arc<dyn AnchorStoreBackend> {
            &self.anchor_store
        }

        fn cf_store(&self) -> &Arc<dyn CFStoreBackend> {
            &self.cf_store
        }
    }

    fn create_validator_set() -> ValidatorSet {
        let mut set = ValidatorSet::new();
        for i in 1..=3 {
            let node = NodeInfo::new_validator(
                format!("v{}", i),
                "127.0.0.1".to_string(),
                8000 + i as u16,
            );
            set.add_validator(ValidatorInfo::new(node, false));
        }
        set
    }

    async fn create_engine_with_pending_finalized_cf() -> (Arc<ConsensusEngine>, Anchor) {
        let config = ConsensusConfig {
            vlc_delta_threshold: 1,
            min_events_per_cf: 1,
            max_events_per_cf: 1000,
            cf_timeout_ms: 5000,
            validator_count: 3,
        };
        let engine = Arc::new(ConsensusEngine::new(config, "v1".to_string(), create_validator_set()));
        let anchor = Anchor::new(
            vec![],
            VLCSnapshot::default(),
            "state-root".to_string(),
            None,
            0,
        );
        let mut cf = ConsensusFrame::new(anchor, "v1".to_string());
        cf.add_vote(Vote::new("v1".to_string(), cf.id.clone(), true));
        cf.add_vote(Vote::new("v2".to_string(), cf.id.clone(), true));
        cf.add_vote(Vote::new("v3".to_string(), cf.id.clone(), true));
        cf.finalize();

        let (_finalized, finalized_anchor) = engine
            .receive_finalized_cf(cf)
            .await
            .expect("test finalized CF should be accepted");
        let finalized_anchor = finalized_anchor.expect("finalized CF should return anchor");
        assert_eq!(engine.peek_pending_finalized_cfs().await.len(), 1);
        (engine, finalized_anchor)
    }

    #[tokio::test]
    async fn test_f1_persist_pending_finalized_cfs_aborts_anchor_on_cf_store_failure() {
        let (engine, anchor) = create_engine_with_pending_finalized_cf().await;
        let anchor_store = Arc::new(AnchorStore::new());
        let persister = TestPersister {
            engine: Arc::clone(&engine),
            event_store: Arc::new(EventStore::new()),
            anchor_store: anchor_store.clone(),
            cf_store: Arc::new(AlwaysFailCFStore::fail_store()),
        };

        let result = persister.persist_finalized_anchor(&anchor).await;

        assert!(matches!(result, Err(PersistenceError::CFIndexPersistenceFailed { .. })));
        assert!(anchor_store.get(&anchor.id).await.is_none());
        assert_eq!(engine.peek_pending_finalized_cfs().await.len(), 1);
    }

    #[tokio::test]
    async fn test_f1_persist_pending_finalized_cfs_propagates_mark_finalized_failure() {
        let (engine, anchor) = create_engine_with_pending_finalized_cf().await;
        let pending_cf = engine
            .peek_pending_finalized_cfs()
            .await
            .into_iter()
            .next()
            .expect("pending finalized CF exists");
        let anchor_store = Arc::new(AnchorStore::new());
        let persister = TestPersister {
            engine: Arc::clone(&engine),
            event_store: Arc::new(EventStore::new()),
            anchor_store: anchor_store.clone(),
            cf_store: Arc::new(AlwaysFailCFStore::fail_mark(pending_cf)),
        };

        let result = persister.persist_finalized_anchor(&anchor).await;

        assert!(matches!(result, Err(PersistenceError::CFIndexPersistenceFailed { .. })));
        assert!(anchor_store.get(&anchor.id).await.is_none());
        assert_eq!(engine.peek_pending_finalized_cfs().await.len(), 1);
    }
}
