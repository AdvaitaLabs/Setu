use setu_types::{Anchor, AnchorId, ConsensusFrame, CFId, CFStatus, SetuResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug)]
pub struct AnchorStore {
    anchors: Arc<RwLock<HashMap<AnchorId, Anchor>>>,
    chain: Arc<RwLock<Vec<AnchorId>>>,
}

impl AnchorStore {
    pub fn new() -> Self {
        Self {
            anchors: Arc::new(RwLock::new(HashMap::new())),
            chain: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn store(&self, anchor: Anchor) -> SetuResult<()> {
        let anchor_id = anchor.id.clone();

        let mut anchors = self.anchors.write().await;
        anchors.insert(anchor_id.clone(), anchor);

        let mut chain = self.chain.write().await;
        chain.push(anchor_id);

        Ok(())
    }

    pub async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor> {
        let anchors = self.anchors.read().await;
        anchors.get(anchor_id).cloned()
    }

    pub async fn get_latest(&self) -> Option<Anchor> {
        let chain = self.chain.read().await;
        let anchors = self.anchors.read().await;

        chain.last().and_then(|id| anchors.get(id).cloned())
    }

    pub async fn get_by_depth(&self, depth: u64) -> Option<Anchor> {
        let anchors = self.anchors.read().await;
        anchors.values().find(|a| a.depth == depth).cloned()
    }

    pub async fn count(&self) -> usize {
        self.anchors.read().await.len()
    }

    pub async fn get_chain(&self) -> Vec<AnchorId> {
        self.chain.read().await.clone()
    }
    
    /// Get the latest anchor_chain_root for recovery after restart
    /// Returns (anchor_chain_root, depth, total_count, last_fold_vlc)
    /// 
    /// Note: The stored `anchor.merkle_roots.anchor_chain_root` represents the chain root
    /// BEFORE this anchor was created. To get the chain root AFTER (including this anchor),
    /// we recompute: final_root = chain_hash(stored_root, anchor_hash)
    pub async fn get_recovery_state(&self) -> Option<([u8; 32], u64, u64, u64)> {
        let chain = self.chain.read().await;
        let anchors = self.anchors.read().await;
        
        chain.last().and_then(|id| {
            anchors.get(id).and_then(|anchor| {
                anchor.merkle_roots.as_ref().map(|roots| {
                    // The stored anchor_chain_root is the "before" state
                    // We need to compute the "after" state by including this anchor
                    let anchor_hash = anchor.compute_hash();
                    let final_chain_root = Self::chain_hash(&roots.anchor_chain_root, &anchor_hash);
                    (
                        final_chain_root,
                        anchor.depth,
                        chain.len() as u64,
                        anchor.vlc_snapshot.logical_time,
                    )
                })
            })
        })
    }
    
    /// Chain hash: combines previous chain root with new anchor hash
    /// new_root = SHA256(prev_root || anchor_hash)
    fn chain_hash(prev_root: &[u8; 32], anchor_hash: &[u8; 32]) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(prev_root);
        hasher.update(anchor_hash);
        let result = hasher.finalize();
        let mut output = [0u8; 32];
        output.copy_from_slice(&result);
        output
    }
    
    // =========================================================================
    // Warmup support methods (for DagManager cache warmup)
    // =========================================================================
    
    /// Get the N most recent finalized anchors (for cache warmup)
    /// 
    /// Returns anchors sorted by finalized_at descending (most recent first)
    pub async fn get_recent_anchors(&self, count: usize) -> Vec<Anchor> {
        let chain = self.chain.read().await;
        let anchors = self.anchors.read().await;
        
        // Get the last N anchor IDs from chain (most recent are at the end)
        let start = chain.len().saturating_sub(count);
        chain[start..]
            .iter()
            .rev() // Reverse to get most recent first
            .filter_map(|id| anchors.get(id).cloned())
            .collect()
    }
}

impl Clone for AnchorStore {
    fn clone(&self) -> Self {
        Self {
            anchors: Arc::clone(&self.anchors),
            chain: Arc::clone(&self.chain),
        }
    }
}

impl Default for AnchorStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct CFStore {
    frames: Arc<RwLock<HashMap<CFId, ConsensusFrame>>>,
    pending: Arc<RwLock<Vec<CFId>>>,
    finalized: Arc<RwLock<Vec<CFId>>>,
}

impl CFStore {
    pub fn new() -> Self {
        Self {
            frames: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(RwLock::new(Vec::new())),
            finalized: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
        let cf_id = cf.id.clone();
        let is_finalized = cf.status == CFStatus::Finalized;

        let mut frames = self.frames.write().await;
        frames.insert(cf_id.clone(), cf);

        if is_finalized {
            let mut finalized = self.finalized.write().await;
            finalized.push(cf_id);
        } else {
            let mut pending = self.pending.write().await;
            pending.push(cf_id);
        }

        Ok(())
    }

    pub async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
        let frames = self.frames.read().await;
        frames.get(cf_id).cloned()
    }

    pub async fn mark_finalized(&self, cf_id: &CFId) {
        {
            let mut frames = self.frames.write().await;
            if let Some(cf) = frames.get_mut(cf_id) {
                cf.finalize();
            }
        }

        let mut pending = self.pending.write().await;
        pending.retain(|id| id != cf_id);

        let mut finalized = self.finalized.write().await;
        finalized.push(cf_id.clone());
    }

    pub async fn get_pending(&self) -> Vec<ConsensusFrame> {
        let pending = self.pending.read().await;
        let frames = self.frames.read().await;

        pending
            .iter()
            .filter_map(|id| frames.get(id).cloned())
            .collect()
    }

    pub async fn get_finalized(&self) -> Vec<ConsensusFrame> {
        let finalized = self.finalized.read().await;
        let frames = self.frames.read().await;

        finalized
            .iter()
            .filter_map(|id| frames.get(id).cloned())
            .collect()
    }

    pub async fn latest_finalized(&self) -> Option<ConsensusFrame> {
        let finalized = self.finalized.read().await;
        let frames = self.frames.read().await;

        finalized.last().and_then(|id| frames.get(id).cloned())
    }

    pub async fn finalized_count(&self) -> usize {
        self.finalized.read().await.len()
    }

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
    use setu_types::{VectorClock, VLCSnapshot};

    fn create_anchor(depth: u64) -> Anchor {
        Anchor::new(
            vec!["event1".to_string()],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: depth * 10,
                physical_time: depth * 10000,
            },
            format!("state_root_{}", depth),
            None,
            depth,
        )
    }

    #[tokio::test]
    async fn test_anchor_store() {
        let store = AnchorStore::new();
        
        let anchor1 = create_anchor(0);
        let anchor2 = create_anchor(1);

        store.store(anchor1).await.unwrap();
        store.store(anchor2.clone()).await.unwrap();

        assert_eq!(store.count().await, 2);
        
        let latest = store.get_latest().await.unwrap();
        assert_eq!(latest.depth, 1);
    }

    #[tokio::test]
    async fn test_cf_store() {
        let store = CFStore::new();
        
        let anchor = create_anchor(0);
        let cf = ConsensusFrame::new(anchor, "validator1".to_string());
        let cf_id = cf.id.clone();

        store.store(cf).await.unwrap();
        assert_eq!(store.pending_count().await, 1);

        store.mark_finalized(&cf_id).await;
        assert_eq!(store.pending_count().await, 0);
        assert_eq!(store.finalized_count().await, 1);
    }
}
