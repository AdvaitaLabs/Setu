use dashmap::DashMap;
use setu_types::{Anchor, AnchorId, SetuResult};
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory storage for Anchors with concurrent access
///
/// AnchorStore uses DashMap for the main anchor storage, providing
/// lock-free concurrent access. The chain (ordered list of anchor IDs)
/// uses RwLock since it requires ordered operations.
#[derive(Debug)]
pub struct AnchorStore {
    anchors: Arc<DashMap<AnchorId, Anchor>>,
    /// Chain maintains insertion order, requires RwLock for ordered access
    chain: Arc<RwLock<Vec<AnchorId>>>,
}

impl AnchorStore {
    pub fn new() -> Self {
        Self {
            anchors: Arc::new(DashMap::new()),
            chain: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn store(&self, anchor: Anchor) -> SetuResult<()> {
        let anchor_id = anchor.id.clone();

        // Insert into DashMap (lock-free)
        self.anchors.insert(anchor_id.clone(), anchor);

        // Update chain (requires lock for ordering)
        let mut chain = self.chain.write().await;
        chain.push(anchor_id);

        Ok(())
    }

    pub async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor> {
        self.anchors.get(anchor_id).map(|r| r.value().clone())
    }

    pub async fn get_latest(&self) -> Option<Anchor> {
        let chain = self.chain.read().await;
        chain
            .last()
            .and_then(|id| self.anchors.get(id).map(|r| r.value().clone()))
    }

    pub async fn get_by_depth(&self, depth: u64) -> Option<Anchor> {
        // Iterate over all anchors to find by depth
        self.anchors
            .iter()
            .find(|r| r.value().depth == depth)
            .map(|r| r.value().clone())
    }

    pub async fn count(&self) -> usize {
        self.anchors.len()
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

        chain.last().and_then(|id| {
            self.anchors.get(id).and_then(|anchor_ref| {
                let anchor = anchor_ref.value();
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

        // Get the last N anchor IDs from chain (most recent are at the end)
        let start = chain.len().saturating_sub(count);
        chain[start..]
            .iter()
            .rev() // Reverse to get most recent first
            .filter_map(|id| self.anchors.get(id).map(|r| r.value().clone()))
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
}
