//! AnchorStore backend trait for abstracting storage implementations
//!
//! This trait allows switching between in-memory (AnchorStore) and
//! persistent (RocksDBAnchorStore) implementations at runtime.

use async_trait::async_trait;
use setu_types::{Anchor, AnchorId, SetuResult};
use std::fmt::Debug;

/// Backend trait for AnchorStore implementations
///
/// This trait defines the common interface that both in-memory and RocksDB
/// implementations must satisfy.
#[async_trait]
pub trait AnchorStoreBackend: Send + Sync + Debug {
    // =========================================================================
    // Core CRUD operations
    // =========================================================================

    /// Store an anchor
    async fn store(&self, anchor: Anchor) -> SetuResult<()>;

    /// Get an anchor by ID
    async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor>;

    /// Get the latest anchor in the chain
    async fn get_latest(&self) -> Option<Anchor>;

    /// Get an anchor by depth
    async fn get_by_depth(&self, depth: u64) -> Option<Anchor>;

    /// Get total anchor count
    async fn count(&self) -> usize;

    /// Get the full anchor chain (list of anchor IDs in order)
    async fn get_chain(&self) -> Vec<AnchorId>;

    // =========================================================================
    // Recovery operations
    // =========================================================================

    /// Get the recovery state: (anchor_chain_root, depth, total_count, last_fold_vlc)
    ///
    /// This is used to restore VLC and AnchorBuilder state after restart.
    async fn get_recovery_state(&self) -> Option<([u8; 32], u64, u64, u64)>;

    /// Get the N most recent anchors (for cache warmup)
    async fn get_recent_anchors(&self, count: usize) -> Vec<Anchor>;

    // =========================================================================
    // Optional: range queries
    // =========================================================================

    /// Get anchors in a depth range (for historical queries)
    async fn get_by_depth_range(&self, _min_depth: u64, _max_depth: u64) -> Vec<Anchor> {
        vec![]
    }
}

// ============================================================================
// Implement trait for in-memory AnchorStore
// ============================================================================

use crate::memory::AnchorStore;

#[async_trait]
impl AnchorStoreBackend for AnchorStore {
    async fn store(&self, anchor: Anchor) -> SetuResult<()> {
        AnchorStore::store(self, anchor).await
    }

    async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor> {
        AnchorStore::get(self, anchor_id).await
    }

    async fn get_latest(&self) -> Option<Anchor> {
        AnchorStore::get_latest(self).await
    }

    async fn get_by_depth(&self, depth: u64) -> Option<Anchor> {
        AnchorStore::get_by_depth(self, depth).await
    }

    async fn count(&self) -> usize {
        AnchorStore::count(self).await
    }

    async fn get_chain(&self) -> Vec<AnchorId> {
        AnchorStore::get_chain(self).await
    }

    async fn get_recovery_state(&self) -> Option<([u8; 32], u64, u64, u64)> {
        AnchorStore::get_recovery_state(self).await
    }

    async fn get_recent_anchors(&self, count: usize) -> Vec<Anchor> {
        AnchorStore::get_recent_anchors(self, count).await
    }
}
