//! RocksDB implementation of CFStore
//!
//! This provides persistent storage for ConsensusFrames.
//! It uses the SetuDB wrapper with a dedicated ColumnFamily.
//!
//! ## Key Design Decisions
//!
//! 1. **Composite Keys**: Uses prefix keys for different indexes
//! 2. **Status Tracking**: Separate indexes for pending and finalized CFs
//! 3. **API Compatible**: Maintains the same async API as in-memory CFStore
//!
//! ## Key Layout
//!
//! All data is stored in ColumnFamily::ConsensusFrames:
//! - `cf:{cf_id}` -> ConsensusFrame (main CF data)
//! - `pending:{seq}:{cf_id}` -> () (pending index, seq is insertion order)
//! - `finalized:{seq}:{cf_id}` -> () (finalized index, seq is finalization order)
//! - `meta:pending_seq` -> u64 (next pending sequence number)
//! - `meta:finalized_seq` -> u64 (next finalized sequence number)

use crate::rocks::core::{SetuDB, ColumnFamily};
use setu_types::{ConsensusFrame, CFId, CFStatus, SetuResult, SetuError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Key prefixes for different data types in ConsensusFrames CF
mod key_prefix {
    pub const CF: &[u8] = b"cf:";
    pub const PENDING: &[u8] = b"pending:";
    pub const FINALIZED: &[u8] = b"finalized:";
    pub const META_PENDING_SEQ: &[u8] = b"meta:pending_seq";
    pub const META_FINALIZED_SEQ: &[u8] = b"meta:finalized_seq";
}

/// RocksDB-backed CFStore implementation
pub struct RocksDBCFStore {
    db: Arc<SetuDB>,
    /// Counter for pending CF insertion order
    pending_seq: AtomicU64,
    /// Counter for finalized CF order
    finalized_seq: AtomicU64,
}

impl RocksDBCFStore {
    /// Create a new RocksDBCFStore with an owned SetuDB
    pub fn new(db: SetuDB) -> Self {
        let db = Arc::new(db);
        Self::from_shared(db)
    }
    
    /// Create from a shared SetuDB instance
    pub fn from_shared(db: Arc<SetuDB>) -> Self {
        // Load sequence counters from storage
        let pending_seq = db.get_raw::<u64>(ColumnFamily::ConsensusFrames, key_prefix::META_PENDING_SEQ)
            .ok()
            .flatten()
            .unwrap_or(0);
            
        let finalized_seq = db.get_raw::<u64>(ColumnFamily::ConsensusFrames, key_prefix::META_FINALIZED_SEQ)
            .ok()
            .flatten()
            .unwrap_or(0);
        
        Self {
            db,
            pending_seq: AtomicU64::new(pending_seq),
            finalized_seq: AtomicU64::new(finalized_seq),
        }
    }
    
    /// Get the underlying database reference
    pub fn db(&self) -> &SetuDB {
        &self.db
    }
    
    // =========================================================================
    // Key Construction Helpers
    // =========================================================================
    
    fn cf_key(cf_id: &CFId) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::CF.len() + cf_id.len());
        key.extend_from_slice(key_prefix::CF);
        key.extend_from_slice(cf_id.as_bytes());
        key
    }
    
    fn pending_key(seq: u64, cf_id: &CFId) -> Vec<u8> {
        // Format: pending:{seq:016x}:{cf_id}
        let seq_str = format!("{:016x}", seq);
        let mut key = Vec::with_capacity(
            key_prefix::PENDING.len() + seq_str.len() + 1 + cf_id.len()
        );
        key.extend_from_slice(key_prefix::PENDING);
        key.extend_from_slice(seq_str.as_bytes());
        key.push(b':');
        key.extend_from_slice(cf_id.as_bytes());
        key
    }
    
    fn finalized_key(seq: u64, cf_id: &CFId) -> Vec<u8> {
        // Format: finalized:{seq:016x}:{cf_id}
        let seq_str = format!("{:016x}", seq);
        let mut key = Vec::with_capacity(
            key_prefix::FINALIZED.len() + seq_str.len() + 1 + cf_id.len()
        );
        key.extend_from_slice(key_prefix::FINALIZED);
        key.extend_from_slice(seq_str.as_bytes());
        key.push(b':');
        key.extend_from_slice(cf_id.as_bytes());
        key
    }
    
    /// Extract cf_id from a pending/finalized key
    fn extract_cf_id_from_index_key(key: &[u8], prefix: &[u8]) -> Option<CFId> {
        // Key format: {prefix}{seq:016x}:{cf_id}
        // seq is 16 hex chars, plus a colon
        let prefix_len = prefix.len();
        let seq_len = 16 + 1; // 16 hex chars + colon
        
        if key.len() > prefix_len + seq_len {
            String::from_utf8(key[prefix_len + seq_len..].to_vec()).ok()
        } else {
            None
        }
    }

    // =========================================================================
    // Core Storage Operations
    // =========================================================================

    /// Store a consensus frame
    pub async fn store(&self, cf: ConsensusFrame) -> SetuResult<()> {
        let cf_id = cf.id.clone();
        let is_finalized = cf.status == CFStatus::Finalized;
        
        let mut batch = self.db.batch();
        
        // Store CF
        let cf_key = Self::cf_key(&cf_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, &cf_key, &cf)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Add to appropriate index
        if is_finalized {
            let seq = self.finalized_seq.fetch_add(1, Ordering::SeqCst);
            let finalized_key = Self::finalized_key(seq, &cf_id);
            self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, &finalized_key, &())
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Update sequence counter
            self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, key_prefix::META_FINALIZED_SEQ, &(seq + 1))
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
        } else {
            let seq = self.pending_seq.fetch_add(1, Ordering::SeqCst);
            let pending_key = Self::pending_key(seq, &cf_id);
            self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, &pending_key, &())
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            
            // Update sequence counter
            self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, key_prefix::META_PENDING_SEQ, &(seq + 1))
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
        }
        
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        debug!(cf_id = %cf_id, finalized = is_finalized, "Stored CF to RocksDB");
        Ok(())
    }
    
    /// Get a consensus frame by ID
    pub async fn get(&self, cf_id: &CFId) -> Option<ConsensusFrame> {
        let cf_key = Self::cf_key(cf_id);
        self.db.get_raw(ColumnFamily::ConsensusFrames, &cf_key)
            .ok()
            .flatten()
    }
    
    /// Mark a pending CF as finalized
    pub async fn mark_finalized(&self, cf_id: &CFId) {
        // Get current CF
        let mut cf = match self.get(cf_id).await {
            Some(cf) => cf,
            None => return,
        };
        
        if cf.status == CFStatus::Finalized {
            return; // Already finalized
        }
        
        // Update status
        cf.finalize();
        
        let mut batch = self.db.batch();
        
        // Update CF
        let cf_key = Self::cf_key(cf_id);
        if self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, &cf_key, &cf).is_err() {
            warn!("Failed to update CF status");
            return;
        }
        
        // Remove from pending index (need to find the key)
        let pending_keys = match self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::PENDING) {
            Ok(keys) => keys,
            Err(_) => return,
        };
        
        for key in pending_keys {
            if let Some(id) = Self::extract_cf_id_from_index_key(&key, key_prefix::PENDING) {
                if &id == cf_id {
                    let _ = self.db.batch_delete_raw(&mut batch, ColumnFamily::ConsensusFrames, &key);
                    break;
                }
            }
        }
        
        // Add to finalized index
        let seq = self.finalized_seq.fetch_add(1, Ordering::SeqCst);
        let finalized_key = Self::finalized_key(seq, cf_id);
        let _ = self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, &finalized_key, &());
        
        // Update sequence counter
        let _ = self.db.batch_put_raw(&mut batch, ColumnFamily::ConsensusFrames, key_prefix::META_FINALIZED_SEQ, &(seq + 1));
        
        if let Err(e) = self.db.write_batch(batch) {
            warn!("Failed to write mark_finalized batch: {}", e);
        }
    }
    
    /// Get all pending consensus frames
    pub async fn get_pending(&self) -> Vec<ConsensusFrame> {
        let pending_keys = match self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::PENDING) {
            Ok(keys) => keys,
            Err(_) => return Vec::new(),
        };
        
        let mut cfs = Vec::new();
        for key in pending_keys {
            if let Some(cf_id) = Self::extract_cf_id_from_index_key(&key, key_prefix::PENDING) {
                if let Some(cf) = self.get(&cf_id).await {
                    cfs.push(cf);
                }
            }
        }
        cfs
    }
    
    /// Get all finalized consensus frames
    pub async fn get_finalized(&self) -> Vec<ConsensusFrame> {
        let finalized_keys = match self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::FINALIZED) {
            Ok(keys) => keys,
            Err(_) => return Vec::new(),
        };
        
        let mut cfs = Vec::new();
        for key in finalized_keys {
            if let Some(cf_id) = Self::extract_cf_id_from_index_key(&key, key_prefix::FINALIZED) {
                if let Some(cf) = self.get(&cf_id).await {
                    cfs.push(cf);
                }
            }
        }
        cfs
    }
    
    /// Get the latest finalized CF
    pub async fn latest_finalized(&self) -> Option<ConsensusFrame> {
        let finalized_keys = match self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::FINALIZED) {
            Ok(keys) => keys,
            Err(_) => return None,
        };
        
        // Keys are sorted, so the last one is the most recent
        finalized_keys.last().and_then(|key| {
            Self::extract_cf_id_from_index_key(key, key_prefix::FINALIZED)
                .and_then(|cf_id| {
                    // We need to block here since we're in a sync context
                    // This is a limitation - in production, this should be async all the way
                    let cf_key = Self::cf_key(&cf_id);
                    self.db.get_raw(ColumnFamily::ConsensusFrames, &cf_key)
                        .ok()
                        .flatten()
                })
        })
    }
    
    /// Count finalized CFs
    pub async fn finalized_count(&self) -> usize {
        self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::FINALIZED)
            .map(|keys| keys.len())
            .unwrap_or(0)
    }
    
    /// Count pending CFs
    pub async fn pending_count(&self) -> usize {
        self.db.prefix_scan_keys(ColumnFamily::ConsensusFrames, key_prefix::PENDING)
            .map(|keys| keys.len())
            .unwrap_or(0)
    }
}

impl Clone for RocksDBCFStore {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            pending_seq: AtomicU64::new(self.pending_seq.load(Ordering::SeqCst)),
            finalized_seq: AtomicU64::new(self.finalized_seq.load(Ordering::SeqCst)),
        }
    }
}

impl std::fmt::Debug for RocksDBCFStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBCFStore")
            .field("db", &"<SetuDB>")
            .field("pending_seq", &self.pending_seq.load(Ordering::SeqCst))
            .field("finalized_seq", &self.finalized_seq.load(Ordering::SeqCst))
            .finish()
    }
}
