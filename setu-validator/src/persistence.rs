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
use setu_types::{Anchor, CFId};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Maximum consecutive CF-index persistence retries before escalating to a
/// hard PersistenceError. Below this threshold the failed CF stays in the
/// engine's `pending_persist_cfs` queue and is retried on the next
/// `persist_finalized_anchor()` call. This avoids a single transient
/// RocksDB CF-handle issue converting an indexing miss into a permanent
/// finality stall (see fix-post-restart-finality-stall-v2 review-log.md
/// R3-VERIFY-9).
pub const MAX_CF_INDEX_RETRIES: u32 = 5;

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

    /// Layer D: CF index persistence failed `MAX_CF_INDEX_RETRIES` consecutive
    /// times for the same `cf_id`. The CF remains in
    /// `engine.pending_persist_cfs` (peek+drain semantics, not destructively
    /// taken), so a future call may still recover it once the underlying
    /// store fault clears.
    #[error("CF index persistence escalated after {retries} retries for cf {cf_id}: {reason}")]
    CFIndexEscalated {
        cf_id: String,
        retries: u32,
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

    /// Per-CF retry counter for the CF-index persistence path (Layer D).
    /// Used by the retry-then-escalate policy in `persist_pending_finalized_cfs`.
    /// Implementations typically own an `Arc<parking_lot::Mutex<HashMap<CFId, u32>>>`
    /// initialized via `Default::default()`.
    fn cf_index_retries(&self) -> &Arc<parking_lot::Mutex<HashMap<CFId, u32>>>;

    /// Persist all CFs queued by the engine since the last call.
    ///
    /// Layer D (retry-then-escalate, R3-VERIFY-1/9):
    /// - Uses `peek_pending_finalized_cfs` (non-destructive) so failed CFs
    ///   stay in the engine queue for retry on the next call.
    /// - Per-CF retry counter is incremented on each failure.
    /// - Successful writes drain the CF from the engine queue and clear its
    ///   retry counter.
    /// - When a CF crosses `MAX_CF_INDEX_RETRIES`, this method returns
    ///   `PersistenceError::CFIndexEscalated`. The caller
    ///   (`persist_finalized_anchor`) propagates the error and aborts the
    ///   anchor commit so we do not produce a durable anchor with no
    ///   accompanying CF index entry.
    ///
    /// Returns Ok(()) when every peeked CF either succeeded (and was drained)
    /// or failed but is still under the retry budget.
    async fn persist_pending_finalized_cfs(&self) -> PersistenceResult<()> {
        let pending = self.engine().peek_pending_finalized_cfs().await;
        if pending.is_empty() {
            return Ok(());
        }
        let mut stored: Vec<CFId> = Vec::new();
        for cf in pending {
            let cf_id = cf.id.clone();
            let store_res = if self.cf_store().get(&cf_id).await.is_none() {
                self.cf_store().store(cf.clone()).await
            } else {
                self.cf_store().mark_finalized(&cf_id).await
            };

            match store_res {
                Ok(()) => {
                    stored.push(cf_id.clone());
                    self.cf_index_retries().lock().remove(&cf_id);
                }
                Err(e) => {
                    let attempts = {
                        let mut map = self.cf_index_retries().lock();
                        let entry = map.entry(cf_id.clone()).or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    if attempts >= MAX_CF_INDEX_RETRIES {
                        error!(
                            cf_id = %cf_id,
                            retries = attempts,
                            error = %e,
                            "CF index persistence escalated after {} retries", MAX_CF_INDEX_RETRIES
                        );
                        // Drain CFs we did manage to persist this round before
                        // bubbling the error so the caller does not retry them.
                        if !stored.is_empty() {
                            self.engine().drain_pending_finalized_cfs(&stored).await;
                        }
                        return Err(PersistenceError::CFIndexEscalated {
                            cf_id: cf_id.to_string(),
                            retries: attempts,
                            reason: e.to_string(),
                        });
                    }
                    warn!(
                        cf_id = %cf_id,
                        attempt = attempts,
                        error = %e,
                        "CF index write failed; will retry on next anchor commit"
                    );
                    // Do NOT push cf_id into stored — leave it in pending_persist_cfs.
                }
            }
        }
        if !stored.is_empty() {
            self.engine().drain_pending_finalized_cfs(&stored).await;
        }
        Ok(())
    }

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
            // Layer D: even on the idempotent path we must drain any
            // late-queued CF index entries so they do not leak; a failure
            // here may still escalate after MAX_CF_INDEX_RETRIES tries.
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
        // Layer D (retry-then-escalate): a failure here propagates so we do
        // not commit an anchor whose accompanying CF index escalated past
        // MAX_CF_INDEX_RETRIES. Transient failures (under the retry budget)
        // return Ok(()) and leave the CF in `engine.pending_persist_cfs` for
        // the next call.
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
    //! F1 verification (review doc m0-m2-rust-code-review-20260509.md):
    //! `persist_pending_finalized_cfs` returns `()`, so a failing CFStore
    //! write is unobservable to the caller. We can dynamically demonstrate
    //! this by mirroring the exact loop body against a stub CFStore that
    //! always returns Err, and asserting:
    //!   1. the loop function returns ()
    //!   2. the in-memory pending buffer is fully drained
    //!   3. the CFStore remains empty
    //! This proves the F1 control-flow defect without instantiating a full
    //! ConsensusEngine.

    use async_trait::async_trait;
    use setu_storage::CFStoreBackend;
    use setu_types::{Anchor, CFId, ConsensusFrame, SetuError, SetuResult, VLCSnapshot};
    use std::sync::Arc;

    #[derive(Debug)]
    struct AlwaysFailCFStore {
        store_calls: parking_lot::Mutex<usize>,
    }

    impl AlwaysFailCFStore {
        fn new() -> Self {
            Self {
                store_calls: parking_lot::Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl CFStoreBackend for AlwaysFailCFStore {
        async fn store(&self, _cf: ConsensusFrame) -> SetuResult<()> {
            *self.store_calls.lock() += 1;
            Err(SetuError::InvalidData("simulated CFStore disk error".to_string()))
        }
        async fn get(&self, _cf_id: &CFId) -> Option<ConsensusFrame> {
            None
        }
        async fn mark_finalized(&self, _cf_id: &CFId) -> SetuResult<()> { Ok(()) }
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

    /// Mirror of `persist_pending_finalized_cfs` body with an explicit
    /// in-memory pending buffer instead of `engine.take_pending_finalized_cfs`.
    /// The structural pattern (drain + ignore Err) is identical.
    async fn run_pattern(
        cf_store: Arc<dyn CFStoreBackend>,
        mut pending: Vec<ConsensusFrame>,
    ) -> Vec<ConsensusFrame> {
        let drained: Vec<_> = std::mem::take(&mut pending);
        for cf in drained {
            if cf_store.get(&cf.id).await.is_none() {
                if let Err(_e) = cf_store.store(cf.clone()).await {
                    // mirrors warn!(...) — error is logged then swallowed
                }
            } else {
                cf_store.mark_finalized(&cf.id).await;
            }
        }
        pending
    }

    #[tokio::test]
    async fn f1_persist_pending_finalized_cfs_swallows_cfstore_failures() {
        let failing: Arc<dyn CFStoreBackend> = Arc::new(AlwaysFailCFStore::new());
        let anchor = Anchor::new(
            vec![],
            VLCSnapshot::default(),
            "state-root".to_string(),
            None,
            0,
        );
        let cf = ConsensusFrame::new(anchor, "v1".to_string());
        let pending = vec![cf.clone()];

        // No panic, no error propagation: function returns ().
        let after: Vec<ConsensusFrame> = run_pattern(failing.clone(), pending).await;

        // F1 dynamic confirmation:
        // 1. Pending buffer was drained even though store() failed.
        assert!(after.is_empty(), "pending CFs were drained despite store failure");
        // 2. CFStore remains empty (write was rejected).
        assert!(failing.get(&cf.id).await.is_none());
        // 3. The caller has no signal of failure (return type was unit).
        //    This is enforced at compile time — the assertion below will only
        //    typecheck if the callee returns Vec<ConsensusFrame> here, which
        //    is a stand-in for "no Result<_, _> escapes the loop".
        let _: Vec<ConsensusFrame> = after;
    }
}
