//! CFStore - In-memory Consensus Frame storage
//!
//! This module provides an in-memory implementation of consensus frame storage,
//! supporting pending and finalized frame tracking with concurrent access.

use dashmap::DashMap;
use setu_types::{CFId, CFStatus, ConsensusFrame, SetuResult};
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory storage for Consensus Frames with concurrent access
///
/// CFStore uses DashMap for the main frame storage (lock-free access),
/// while pending/finalized lists use RwLock for ordered operations.
///
/// - `frames`: DashMap storing all consensus frames by ID (lock-free)
/// - `pending`: Vector of pending (not yet finalized) frame IDs (ordered)
/// - `finalized`: Vector of finalized frame IDs (ordered)
#[derive(Debug)]
pub struct CFStore {
    frames: Arc<DashMap<CFId, ConsensusFrame>>,
    pending: Arc<RwLock<Vec<CFId>>>,
    finalized: Arc<RwLock<Vec<CFId>>>,
}

impl CFStore {
    /// Create a new empty CFStore
    pub fn new() -> Self {
        Self {
            frames: Arc::new(DashMap::new()),
            pending: Arc::new(RwLock::new(Vec::new())),
            finalized: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Store a consensus frame
    ///
    /// The frame is automatically added to either the pending or finalized
    /// list based on its current status.
    pub async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
        let cf_id = cf.id.clone();
        let is_finalized = cf.status == CFStatus::Finalized;

        // Insert into DashMap (lock-free)
        self.frames.insert(cf_id.clone(), cf);

        // Update ordered lists
        if is_finalized {
            let mut finalized = self.finalized.write().await;
            finalized.push(cf_id);
        } else {
            let mut pending = self.pending.write().await;
            pending.push(cf_id);
        }

        Ok(())
    }

    /// Get a consensus frame by ID
    pub async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
        self.frames.get(cf_id).map(|r| r.value().clone())
    }

    /// Mark a pending CF as finalized
    ///
    /// This updates the frame's status and moves it from the pending
    /// list to the finalized list.
    pub async fn mark_finalized(&self, cf_id: &CFId) {
        // Update frame status in DashMap
        if let Some(mut cf) = self.frames.get_mut(cf_id) {
            cf.finalize();
        }

        // Move from pending to finalized
        let mut pending = self.pending.write().await;
        pending.retain(|id| id != cf_id);

        let mut finalized = self.finalized.write().await;
        finalized.push(cf_id.clone());
    }

    /// Get all pending consensus frames
    pub async fn get_pending(&self) -> Vec<ConsensusFrame> {
        let pending = self.pending.read().await;

        pending
            .iter()
            .filter_map(|id| self.frames.get(id).map(|r| r.value().clone()))
            .collect()
    }

    /// Get all finalized consensus frames
    pub async fn get_finalized(&self) -> Vec<ConsensusFrame> {
        let finalized = self.finalized.read().await;

        finalized
            .iter()
            .filter_map(|id| self.frames.get(id).map(|r| r.value().clone()))
            .collect()
    }

    /// Get the latest finalized CF
    pub async fn latest_finalized(&self) -> Option<ConsensusFrame> {
        let finalized = self.finalized.read().await;
        finalized
            .last()
            .and_then(|id| self.frames.get(id).map(|r| r.value().clone()))
    }

    /// Count finalized CFs
    pub async fn finalized_count(&self) -> usize {
        self.finalized.read().await.len()
    }

    /// Count pending CFs
    pub async fn pending_count(&self) -> usize {
        self.pending.read().await.len()
    }
}

impl Clone for CFStore {
    fn clone(&self) -> Self {
        Self {
            frames: Arc::clone(&self.frames),
            pending: Arc::clone(&self.pending),
            finalized: Arc::clone(&self.finalized),
        }
    }
}

impl Default for CFStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Anchor, VLCSnapshot, VectorClock};

    fn create_test_cf(depth: u64, validator: &str) -> ConsensusFrame {
        let anchor = Anchor::new(
            vec!["event1".to_string()],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: depth * 10,
                physical_time: depth * 10000,
            },
            format!("state_root_{}", depth),
            None,
            depth,
        );
        ConsensusFrame::new(anchor, validator.to_string())
    }

    #[tokio::test]
    async fn test_cf_store_basic() {
        let store = CFStore::new();

        let cf = create_test_cf(0, "validator1");
        let cf_id = cf.id.clone();

        store.store(cf).await.unwrap();
        assert_eq!(store.pending_count().await, 1);
        assert_eq!(store.finalized_count().await, 0);

        let retrieved = store.get(&cf_id).await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_cf_store_finalization() {
        let store = CFStore::new();

        let cf = create_test_cf(0, "validator1");
        let cf_id = cf.id.clone();

        store.store(cf).await.unwrap();
        assert_eq!(store.pending_count().await, 1);

        store.mark_finalized(&cf_id).await;
        assert_eq!(store.pending_count().await, 0);
        assert_eq!(store.finalized_count().await, 1);

        let latest = store.latest_finalized().await;
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().id, cf_id);
    }

    #[tokio::test]
    async fn test_cf_store_multiple_frames() {
        let store = CFStore::new();

        let cf1 = create_test_cf(0, "validator1");
        let cf2 = create_test_cf(1, "validator2");
        let cf1_id = cf1.id.clone();
        let cf2_id = cf2.id.clone();

        store.store(cf1).await.unwrap();
        store.store(cf2).await.unwrap();
        assert_eq!(store.pending_count().await, 2);

        store.mark_finalized(&cf1_id).await;
        assert_eq!(store.pending_count().await, 1);
        assert_eq!(store.finalized_count().await, 1);

        let pending = store.get_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, cf2_id);

        let finalized = store.get_finalized().await;
        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].id, cf1_id);
    }
}
