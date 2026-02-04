//! RocksDB implementation of AnchorStore
//!
//! This provides persistent storage for Anchors and anchor chain information.
//! It uses the SetuDB wrapper with the Anchors ColumnFamily.
//!
//! ## Key Design Decisions
//!
//! 1. **Chain Ordering**: Uses `chain:{index}` keys for ordered chain storage
//! 2. **Depth Index**: Uses `depth:{depth}` keys for depth-based lookups
//! 3. **Latest Tracking**: Uses `meta:latest` for quick latest anchor access
//! 4. **Atomic Batch Writes**: Uses WriteBatch for atomic store operations
//! 5. **Async-safe I/O**: Uses `spawn_blocking` for disk operations
//!
//! ## Column Family Layout
//!
//! All data is stored in ColumnFamily::Anchors:
//! - `anchor:{anchor_id}` -> Anchor (main anchor data)
//! - `chain:{index}` -> AnchorId (ordered chain index, 0-based)
//! - `depth:{depth}` -> AnchorId (depth lookup index)
//! - `meta:latest` -> AnchorId (latest anchor ID)
//! - `meta:count` -> u64 (total anchor count)

use crate::rocks::core::{SetuDB, ColumnFamily, spawn_db_op};
use setu_types::{Anchor, AnchorId, SetuResult, SetuError};
use std::sync::Arc;
use tracing::{debug, warn};
use sha2::{Sha256, Digest};

/// Key prefixes for different data types in Anchors CF
mod key_prefix {
    pub const ANCHOR: &[u8] = b"anchor:";
    pub const CHAIN: &[u8] = b"chain:";
    pub const DEPTH: &[u8] = b"depth:";
}

/// Metadata keys
mod meta_key {
    pub const LATEST: &[u8] = b"meta:latest";
    pub const COUNT: &[u8] = b"meta:count";
}

/// RocksDB-backed AnchorStore implementation
pub struct RocksDBAnchorStore {
    db: Arc<SetuDB>,
}

impl RocksDBAnchorStore {
    /// Create a new RocksDBAnchorStore with an owned SetuDB
    pub fn new(db: SetuDB) -> Self {
        Self { db: Arc::new(db) }
    }
    
    /// Create from a shared SetuDB instance
    pub fn from_shared(db: Arc<SetuDB>) -> Self {
        Self { db }
    }
    
    /// Get the underlying database reference
    pub fn db(&self) -> &SetuDB {
        &self.db
    }
    
    // =========================================================================
    // Key Construction Helpers
    // =========================================================================
    
    fn anchor_key(anchor_id: &AnchorId) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::ANCHOR.len() + anchor_id.len());
        key.extend_from_slice(key_prefix::ANCHOR);
        key.extend_from_slice(anchor_id.as_bytes());
        key
    }
    
    fn chain_key(index: u64) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::CHAIN.len() + 8);
        key.extend_from_slice(key_prefix::CHAIN);
        key.extend_from_slice(&index.to_be_bytes()); // Big-endian for lexicographic ordering
        key
    }
    
    fn depth_key(depth: u64) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::DEPTH.len() + 8);
        key.extend_from_slice(key_prefix::DEPTH);
        key.extend_from_slice(&depth.to_be_bytes());
        key
    }

    // =========================================================================
    // Core Storage Operations
    // =========================================================================

    /// Store an anchor (adds to chain)
    /// 
    /// Uses spawn_blocking for the disk write to avoid blocking the async runtime.
    pub async fn store(&self, anchor: Anchor) -> SetuResult<()> {
        let db = self.db.clone();
        let anchor_id = anchor.id.clone();
        let depth = anchor.depth;
        
        // Get current count for chain index
        let count = self.count().await as u64;
        
        // Perform the blocking batch write on the blocking thread pool
        spawn_db_op(move || {
            let mut batch = db.batch();
            
            // Store anchor
            let anchor_key = Self::anchor_key(&anchor_id);
            db.batch_put_raw(&mut batch, ColumnFamily::Anchors, &anchor_key, &anchor)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Store chain index
            let chain_key = Self::chain_key(count);
            db.batch_put_raw(&mut batch, ColumnFamily::Anchors, &chain_key, &anchor_id)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Store depth index
            let depth_key = Self::depth_key(depth);
            db.batch_put_raw(&mut batch, ColumnFamily::Anchors, &depth_key, &anchor_id)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Update latest
            db.batch_put_raw(&mut batch, ColumnFamily::Anchors, meta_key::LATEST, &anchor_id)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Update count
            let new_count = count + 1;
            db.batch_put_raw(&mut batch, ColumnFamily::Anchors, meta_key::COUNT, &new_count)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            db.write_batch(batch)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            debug!(anchor_id = %anchor_id, depth = depth, index = count, "Persisted anchor to RocksDB");
            Ok(())
        }).await
    }
    
    // =========================================================================
    // Query Operations
    // =========================================================================
    
    /// Get an anchor by ID
    pub async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor> {
        let anchor_key = Self::anchor_key(anchor_id);
        self.db.get_raw(ColumnFamily::Anchors, &anchor_key)
            .ok()
            .flatten()
    }
    
    /// Get the latest anchor
    pub async fn get_latest(&self) -> Option<Anchor> {
        // Get latest anchor ID
        let latest_id: Option<AnchorId> = self.db
            .get_raw(ColumnFamily::Anchors, meta_key::LATEST)
            .ok()
            .flatten();
        
        if let Some(id) = latest_id {
            self.get(&id).await
        } else {
            None
        }
    }
    
    /// Get anchor by depth
    pub async fn get_by_depth(&self, depth: u64) -> Option<Anchor> {
        let depth_key = Self::depth_key(depth);
        let anchor_id: Option<AnchorId> = self.db
            .get_raw(ColumnFamily::Anchors, &depth_key)
            .ok()
            .flatten();
        
        if let Some(id) = anchor_id {
            self.get(&id).await
        } else {
            None
        }
    }
    
    /// Get total anchor count
    pub async fn count(&self) -> usize {
        self.db
            .get_raw::<u64>(ColumnFamily::Anchors, meta_key::COUNT)
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }
    
    /// Get the entire chain as anchor IDs
    pub async fn get_chain(&self) -> Vec<AnchorId> {
        let count = self.count().await;
        if count == 0 {
            return Vec::new();
        }
        
        let mut chain = Vec::with_capacity(count);
        for i in 0..count as u64 {
            let chain_key = Self::chain_key(i);
            if let Ok(Some(anchor_id)) = self.db.get_raw::<AnchorId>(ColumnFamily::Anchors, &chain_key) {
                chain.push(anchor_id);
            }
        }
        chain
    }
    
    /// Get recovery state for restart
    /// Returns (anchor_chain_root, depth, total_count, last_fold_vlc)
    pub async fn get_recovery_state(&self) -> Option<([u8; 32], u64, u64, u64)> {
        let latest = self.get_latest().await?;
        let count = self.count().await;
        
        if latest.merkle_roots.is_none() {
            warn!(
                anchor_id = %latest.id,
                depth = latest.depth,
                "Latest anchor has no merkle_roots, recovery state unavailable. \
                 This may indicate the anchor was created before merkle computation was enabled."
            );
        }
        
        latest.merkle_roots.as_ref().map(|roots| {
            // Compute final chain root (including this anchor)
            let anchor_hash = latest.compute_hash();
            let final_chain_root = Self::chain_hash(&roots.anchor_chain_root, &anchor_hash);
            (
                final_chain_root,
                latest.depth,
                count as u64,
                latest.vlc_snapshot.logical_time,
            )
        })
    }
    
    /// Chain hash: combines previous chain root with new anchor hash
    fn chain_hash(prev_root: &[u8; 32], anchor_hash: &[u8; 32]) -> [u8; 32] {
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
    /// Returns anchors sorted by finalized_at descending (most recent first)
    pub async fn get_recent_anchors(&self, count: usize) -> Vec<Anchor> {
        let total = self.count().await;
        if total == 0 {
            return Vec::new();
        }
        
        let start = total.saturating_sub(count) as u64;
        let mut anchors = Vec::with_capacity(count.min(total));
        
        // Iterate in reverse order (most recent first)
        for i in (start..total as u64).rev() {
            let chain_key = Self::chain_key(i);
            if let Ok(Some(anchor_id)) = self.db.get_raw::<AnchorId>(ColumnFamily::Anchors, &chain_key) {
                if let Some(anchor) = self.get(&anchor_id).await {
                    anchors.push(anchor);
                }
            }
        }
        
        anchors
    }
}

impl Clone for RocksDBAnchorStore {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

impl std::fmt::Debug for RocksDBAnchorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBAnchorStore")
            .field("db", &"<SetuDB>")
            .finish()
    }
}

// ============================================================================
// Implement AnchorStoreBackend trait
// ============================================================================

use crate::backends::anchor::AnchorStoreBackend;
use async_trait::async_trait;

#[async_trait]
impl AnchorStoreBackend for RocksDBAnchorStore {
    async fn store(&self, anchor: Anchor) -> SetuResult<()> {
        RocksDBAnchorStore::store(self, anchor).await
    }

    async fn get(&self, anchor_id: &AnchorId) -> Option<Anchor> {
        RocksDBAnchorStore::get(self, anchor_id).await
    }

    async fn get_latest(&self) -> Option<Anchor> {
        RocksDBAnchorStore::get_latest(self).await
    }

    async fn get_by_depth(&self, depth: u64) -> Option<Anchor> {
        RocksDBAnchorStore::get_by_depth(self, depth).await
    }

    async fn count(&self) -> usize {
        RocksDBAnchorStore::count(self).await
    }

    async fn get_chain(&self) -> Vec<AnchorId> {
        RocksDBAnchorStore::get_chain(self).await
    }

    async fn get_recovery_state(&self) -> Option<([u8; 32], u64, u64, u64)> {
        RocksDBAnchorStore::get_recovery_state(self).await
    }

    async fn get_recent_anchors(&self, count: usize) -> Vec<Anchor> {
        RocksDBAnchorStore::get_recent_anchors(self, count).await
    }

    async fn get_by_depth_range(&self, min_depth: u64, max_depth: u64) -> Vec<Anchor> {
        let mut anchors = Vec::new();
        for depth in min_depth..=max_depth {
            if let Some(anchor) = self.get_by_depth(depth).await {
                anchors.push(anchor);
            }
        }
        anchors
    }
}
