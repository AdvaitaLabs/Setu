//! CFStore backend trait for abstracting storage implementations
//!
//! This trait allows switching between in-memory (CFStore) and
//! persistent (RocksDBCFStore) implementations at runtime.

use async_trait::async_trait;
use setu_types::{ConsensusFrame, CFId, SetuResult};
use std::fmt::Debug;

/// Backend trait for CFStore implementations
///
/// This trait defines the common interface that both in-memory and RocksDB
/// implementations must satisfy.
#[async_trait]
pub trait CFStoreBackend: Send + Sync + Debug {
    // =========================================================================
    // Core CRUD operations
    // =========================================================================

    /// Store a consensus frame
    async fn store(&self, cf: ConsensusFrame) -> SetuResult<()>;

    /// Get a consensus frame by ID
    async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame>;

    /// Mark a pending CF as finalized
    async fn mark_finalized(&self, cf_id: &CFId);

    // =========================================================================
    // Query operations
    // =========================================================================

    /// Get all pending consensus frames
    async fn get_pending(&self) -> Vec<ConsensusFrame>;

    /// Get all finalized consensus frames
    async fn get_finalized(&self) -> Vec<ConsensusFrame>;

    /// Get the latest finalized CF
    async fn latest_finalized(&self) -> Option<ConsensusFrame>;

    /// Count finalized CFs
    async fn finalized_count(&self) -> usize;

    /// Count pending CFs
    async fn pending_count(&self) -> usize;
}

// ============================================================================
// Implement trait for in-memory CFStore
// ============================================================================

use crate::CFStore;

#[async_trait]
impl CFStoreBackend for CFStore {
    async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
        CFStore::store(self, cf).await
    }

    async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
        CFStore::get(self, cf_id).await
    }

    async fn mark_finalized(&self, cf_id: &CFId) {
        CFStore::mark_finalized(self, cf_id).await
    }

    async fn get_pending(&self) -> Vec<ConsensusFrame> {
        CFStore::get_pending(self).await
    }

    async fn get_finalized(&self) -> Vec<ConsensusFrame> {
        CFStore::get_finalized(self).await
    }

    async fn latest_finalized(&self) -> Option<ConsensusFrame> {
        CFStore::latest_finalized(self).await
    }

    async fn finalized_count(&self) -> usize {
        CFStore::finalized_count(self).await
    }

    async fn pending_count(&self) -> usize {
        CFStore::pending_count(self).await
    }
}

// ============================================================================
// Implement trait for RocksDBCFStore
// ============================================================================

use crate::rocks_cf_store::RocksDBCFStore;

#[async_trait]
impl CFStoreBackend for RocksDBCFStore {
    async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
        RocksDBCFStore::store(self, cf).await
    }

    async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
        RocksDBCFStore::get(self, cf_id).await
    }

    async fn mark_finalized(&self, cf_id: &CFId) {
        RocksDBCFStore::mark_finalized(self, cf_id).await
    }

    async fn get_pending(&self) -> Vec<ConsensusFrame> {
        RocksDBCFStore::get_pending(self).await
    }

    async fn get_finalized(&self) -> Vec<ConsensusFrame> {
        RocksDBCFStore::get_finalized(self).await
    }

    async fn latest_finalized(&self) -> Option<ConsensusFrame> {
        RocksDBCFStore::latest_finalized(self).await
    }

    async fn finalized_count(&self) -> usize {
        RocksDBCFStore::finalized_count(self).await
    }

    async fn pending_count(&self) -> usize {
        RocksDBCFStore::pending_count(self).await
    }
}
