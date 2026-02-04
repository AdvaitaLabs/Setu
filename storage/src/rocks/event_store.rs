//! RocksDB implementation of EventStore
//!
//! This provides persistent storage for Events and their depth information.
//! It uses the SetuDB wrapper with the Events ColumnFamily.
//!
//! ## Key Design Decisions
//!
//! 1. **Composite Keys for Depth**: Uses `depth:{event_id}` prefix keys for depth storage
//! 2. **Atomic Batch Writes**: Uses WriteBatch for atomic store_with_depth operations
//! 3. **API Compatible**: Maintains the same async API as in-memory EventStore
//! 4. **Index Tables**: Stores by_creator and by_status as separate key prefixes
//!
//! ## Column Family Layout
//!
//! All data is stored in ColumnFamily::Events:
//! - `evt:{event_id}` -> Event (main event data)
//! - `depth:{event_id}` -> u64 (depth information)
//! - `creator:{creator}:{event_id}` -> () (creator index)
//! - `status:{status}:{event_id}` -> () (status index)

use crate::types::BatchStoreResult;
use crate::rocks::core::{SetuDB, ColumnFamily};
use setu_types::{Event, EventId, EventStatus, SetuResult, SetuError};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn, error};

/// Key prefixes for different data types in Events CF
mod key_prefix {
    pub const EVENT: &[u8] = b"evt:";
    pub const DEPTH: &[u8] = b"depth:";
    pub const CREATOR: &[u8] = b"creator:";
    pub const STATUS: &[u8] = b"status:";
}

/// RocksDB-backed EventStore implementation
pub struct RocksDBEventStore {
    db: Arc<SetuDB>,
}

impl RocksDBEventStore {
    /// Create a new RocksDBEventStore with an owned SetuDB
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
    
    fn event_key(event_id: &EventId) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::EVENT.len() + event_id.len());
        key.extend_from_slice(key_prefix::EVENT);
        key.extend_from_slice(event_id.as_bytes());
        key
    }
    
    fn depth_key(event_id: &EventId) -> Vec<u8> {
        let mut key = Vec::with_capacity(key_prefix::DEPTH.len() + event_id.len());
        key.extend_from_slice(key_prefix::DEPTH);
        key.extend_from_slice(event_id.as_bytes());
        key
    }
    
    fn creator_key(creator: &str, event_id: &EventId) -> Vec<u8> {
        let mut key = Vec::with_capacity(
            key_prefix::CREATOR.len() + creator.len() + 1 + event_id.len()
        );
        key.extend_from_slice(key_prefix::CREATOR);
        key.extend_from_slice(creator.as_bytes());
        key.push(b':');
        key.extend_from_slice(event_id.as_bytes());
        key
    }
    
    fn status_key(status: EventStatus, event_id: &EventId) -> Vec<u8> {
        let status_byte = status as u8;
        let mut key = Vec::with_capacity(
            key_prefix::STATUS.len() + 1 + 1 + event_id.len()
        );
        key.extend_from_slice(key_prefix::STATUS);
        key.push(status_byte);
        key.push(b':');
        key.extend_from_slice(event_id.as_bytes());
        key
    }
    
    fn creator_prefix(creator: &str) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(key_prefix::CREATOR.len() + creator.len() + 1);
        prefix.extend_from_slice(key_prefix::CREATOR);
        prefix.extend_from_slice(creator.as_bytes());
        prefix.push(b':');
        prefix
    }
    
    fn status_prefix(status: EventStatus) -> Vec<u8> {
        let status_byte = status as u8;
        let mut prefix = Vec::with_capacity(key_prefix::STATUS.len() + 2);
        prefix.extend_from_slice(key_prefix::STATUS);
        prefix.push(status_byte);
        prefix.push(b':');
        prefix
    }

    // =========================================================================
    // Core Storage Operations
    // =========================================================================

    /// Store an event (without depth)
    pub async fn store(&self, event: Event) -> SetuResult<()> {
        let event_id = event.id.clone();
        let creator = event.creator.clone();
        let status = event.status;
        
        let mut batch = self.db.batch();
        
        // Store event
        let event_key = Self::event_key(&event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &event_key, &event)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Store creator index
        let creator_key = Self::creator_key(&creator, &event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &creator_key, &())
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Store status index
        let status_key = Self::status_key(status, &event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &status_key, &())
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Store an event with its depth (atomic operation)
    /// 
    /// This is the primary method used during anchor finalization.
    pub async fn store_with_depth(&self, event: Event, depth: u64) -> SetuResult<()> {
        let event_id = event.id.clone();
        let creator = event.creator.clone();
        let status = event.status;
        
        let mut batch = self.db.batch();
        
        // Store event
        let event_key = Self::event_key(&event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &event_key, &event)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Store depth
        let depth_key = Self::depth_key(&event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &depth_key, &depth)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Store creator index
        let creator_key = Self::creator_key(&creator, &event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &creator_key, &())
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Store status index
        let status_key = Self::status_key(status, &event_id);
        self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &status_key, &())
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        debug!(event_id = %event_id, depth = depth, "Persisted event with depth to RocksDB");
        Ok(())
    }
    
    /// Batch store events with depths (optimized for finalization)
    pub async fn store_batch_with_depth(
        &self,
        events_with_depths: Vec<(Event, u64)>,
    ) -> BatchStoreResult {
        let mut result = BatchStoreResult::default();
        
        if events_with_depths.is_empty() {
            return result;
        }
        
        let mut batch = self.db.batch();
        
        for (event, depth) in events_with_depths {
            let event_id = event.id.clone();
            
            // Check for duplicates
            if self.exists(&event_id).await {
                result.skipped += 1;
                result.skipped_ids.push(event_id);
                continue;
            }
            
            let creator = event.creator.clone();
            let status = event.status;
            
            // Store event
            let event_key = Self::event_key(&event_id);
            if let Err(e) = self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &event_key, &event) {
                result.failed += 1;
                result.failed_errors.push((event_id.clone(), e.to_string()));
                continue;
            }
            
            // Store depth
            let depth_key = Self::depth_key(&event_id);
            if let Err(e) = self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &depth_key, &depth) {
                result.failed += 1;
                result.failed_errors.push((event_id.clone(), e.to_string()));
                continue;
            }
            
            // Store creator index
            let creator_key = Self::creator_key(&creator, &event_id);
            if let Err(e) = self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &creator_key, &()) {
                result.failed += 1;
                result.failed_errors.push((event_id.clone(), e.to_string()));
                continue;
            }
            
            // Store status index
            let status_key = Self::status_key(status, &event_id);
            if let Err(e) = self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &status_key, &()) {
                result.failed += 1;
                result.failed_errors.push((event_id, e.to_string()));
                continue;
            }
            
            result.stored += 1;
        }
        
        // Atomic write
        if let Err(e) = self.db.write_batch(batch) {
            error!("Batch write failed: {}", e);
            result.failed += result.stored;
            result.stored = 0;
        }
        
        result
    }
    
    // =========================================================================
    // Query Operations
    // =========================================================================
    
    /// Get an event by ID
    pub async fn get(&self, event_id: &EventId) -> Option<Event> {
        let event_key = Self::event_key(event_id);
        self.db.get_raw(ColumnFamily::Events, &event_key)
            .ok()
            .flatten()
    }
    
    /// Get multiple events by ID
    pub async fn get_many(&self, event_ids: &[EventId]) -> Vec<Event> {
        event_ids
            .iter()
            .filter_map(|id| {
                let event_key = Self::event_key(id);
                self.db.get_raw::<Event>(ColumnFamily::Events, &event_key)
                    .ok()
                    .flatten()
            })
            .collect()
    }
    
    /// Get event depth
    pub async fn get_depth(&self, event_id: &EventId) -> Option<u64> {
        let depth_key = Self::depth_key(event_id);
        self.db.get_raw(ColumnFamily::Events, &depth_key)
            .ok()
            .flatten()
    }
    
    /// Batch get depths
    pub async fn get_depths_batch(&self, event_ids: &[EventId]) -> HashMap<EventId, u64> {
        event_ids
            .iter()
            .filter_map(|id| {
                let depth_key = Self::depth_key(id);
                self.db.get_raw::<u64>(ColumnFamily::Events, &depth_key)
                    .ok()
                    .flatten()
                    .map(|d| (id.clone(), d))
            })
            .collect()
    }
    
    /// Check if an event exists
    pub async fn exists(&self, event_id: &EventId) -> bool {
        let event_key = Self::event_key(event_id);
        self.db.exists_raw(ColumnFamily::Events, &event_key)
            .unwrap_or(false)
    }
    
    /// Check if multiple events exist
    pub async fn exists_many(&self, event_ids: &[EventId]) -> Vec<bool> {
        event_ids
            .iter()
            .map(|id| {
                let event_key = Self::event_key(id);
                self.db.exists_raw(ColumnFamily::Events, &event_key)
                    .unwrap_or(false)
            })
            .collect()
    }
    
    /// Get event's parent_ids (for cache warmup)
    pub async fn get_parent_ids(&self, event_id: &EventId) -> Option<Vec<EventId>> {
        self.get(event_id).await.map(|e| e.parent_ids)
    }
    
    /// Update event status
    pub async fn update_status(&self, event_id: &EventId, new_status: EventStatus) {
        // Get current event
        let event = match self.get(event_id).await {
            Some(e) => e,
            None => return,
        };
        
        let old_status = event.status;
        if old_status == new_status {
            return;
        }
        
        // Create updated event
        let mut updated_event = event;
        updated_event.status = new_status;
        
        let mut batch = self.db.batch();
        
        // Update event
        let event_key = Self::event_key(event_id);
        if self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &event_key, &updated_event).is_err() {
            warn!("Failed to update event status");
            return;
        }
        
        // Remove old status index
        let old_status_key = Self::status_key(old_status, event_id);
        let _ = self.db.batch_delete_raw(&mut batch, ColumnFamily::Events, &old_status_key);
        
        // Add new status index
        let new_status_key = Self::status_key(new_status, event_id);
        let _ = self.db.batch_put_raw(&mut batch, ColumnFamily::Events, &new_status_key, &());
        
        if let Err(e) = self.db.write_batch(batch) {
            warn!("Failed to write status update batch: {}", e);
        }
    }
    
    /// Get events by creator (uses prefix scan)
    pub async fn get_by_creator(&self, creator: &str) -> Vec<Event> {
        let prefix = Self::creator_prefix(creator);
        
        // Scan prefix and collect event IDs
        let event_ids: Vec<EventId> = match self.db.prefix_scan_keys(ColumnFamily::Events, &prefix) {
            Ok(keys) => {
                keys.into_iter()
                    .filter_map(|key| {
                        // Key format: creator:{creator}:{event_id}
                        // Extract event_id from end of key
                        let prefix_len = prefix.len();
                        if key.len() > prefix_len {
                            String::from_utf8(key[prefix_len..].to_vec()).ok()
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            Err(_) => return Vec::new(),
        };
        
        self.get_many(&event_ids).await
    }
    
    /// Get events by status (uses prefix scan)
    pub async fn get_by_status(&self, status: EventStatus) -> Vec<Event> {
        let prefix = Self::status_prefix(status);
        
        // Scan prefix and collect event IDs
        let event_ids: Vec<EventId> = match self.db.prefix_scan_keys(ColumnFamily::Events, &prefix) {
            Ok(keys) => {
                keys.into_iter()
                    .filter_map(|key| {
                        // Key format: status:{status}:{event_id}
                        let prefix_len = prefix.len();
                        if key.len() > prefix_len {
                            String::from_utf8(key[prefix_len..].to_vec()).ok()
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            Err(_) => return Vec::new(),
        };
        
        self.get_many(&event_ids).await
    }
    
    /// Count all events
    pub async fn count(&self) -> usize {
        // Count events by scanning the event prefix
        match self.db.prefix_scan_keys(ColumnFamily::Events, key_prefix::EVENT) {
            Ok(keys) => keys.len(),
            Err(_) => 0,
        }
    }
    
    /// Count events by status
    pub async fn count_by_status(&self, status: EventStatus) -> usize {
        let prefix = Self::status_prefix(status);
        match self.db.prefix_scan_keys(ColumnFamily::Events, &prefix) {
            Ok(keys) => keys.len(),
            Err(_) => 0,
        }
    }
    
    /// Get batch of events (for network sync)
    pub async fn get_events_batch(&self, event_ids: &[EventId]) -> Vec<Event> {
        self.get_many(event_ids).await
    }
}

impl Clone for RocksDBEventStore {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

impl std::fmt::Debug for RocksDBEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBEventStore")
            .field("db", &"<SetuDB>")
            .finish()
    }
}

// ============================================================================
// Implement EventStoreBackend trait
// ============================================================================

use crate::backends::event::EventStoreBackend;
use async_trait::async_trait;

#[async_trait]
impl EventStoreBackend for RocksDBEventStore {
    async fn store(&self, event: Event) -> SetuResult<()> {
        RocksDBEventStore::store(self, event).await
    }

    async fn get(&self, event_id: &EventId) -> Option<Event> {
        RocksDBEventStore::get(self, event_id).await
    }

    async fn get_many(&self, event_ids: &[EventId]) -> Vec<Event> {
        RocksDBEventStore::get_many(self, event_ids).await
    }

    async fn exists(&self, event_id: &EventId) -> bool {
        RocksDBEventStore::exists(self, event_id).await
    }

    async fn exists_many(&self, event_ids: &[EventId]) -> Vec<bool> {
        RocksDBEventStore::exists_many(self, event_ids).await
    }

    async fn count(&self) -> usize {
        RocksDBEventStore::count(self).await
    }

    async fn store_with_depth(&self, event: Event, depth: u64) -> SetuResult<()> {
        RocksDBEventStore::store_with_depth(self, event, depth).await
    }

    async fn store_batch_with_depth(
        &self,
        events_with_depths: Vec<(Event, u64)>,
    ) -> BatchStoreResult {
        RocksDBEventStore::store_batch_with_depth(self, events_with_depths).await
    }

    async fn get_depth(&self, event_id: &EventId) -> Option<u64> {
        RocksDBEventStore::get_depth(self, event_id).await
    }

    async fn get_depths_batch(&self, event_ids: &[EventId]) -> HashMap<EventId, u64> {
        RocksDBEventStore::get_depths_batch(self, event_ids).await
    }

    async fn get_parent_ids(&self, event_id: &EventId) -> Option<Vec<EventId>> {
        RocksDBEventStore::get_parent_ids(self, event_id).await
    }

    async fn get_by_creator(&self, creator: &str) -> Vec<Event> {
        RocksDBEventStore::get_by_creator(self, creator).await
    }

    async fn get_by_status(&self, status: EventStatus) -> Vec<Event> {
        RocksDBEventStore::get_by_status(self, status).await
    }

    async fn count_by_status(&self, status: EventStatus) -> usize {
        RocksDBEventStore::count_by_status(self, status).await
    }

    async fn update_status(&self, event_id: &EventId, new_status: EventStatus) {
        RocksDBEventStore::update_status(self, event_id, new_status).await
    }

    async fn get_events_batch(&self, event_ids: &[EventId]) -> Vec<Event> {
        RocksDBEventStore::get_events_batch(self, event_ids).await
    }

    async fn get_events_by_depth_range(
        &self,
        min_depth: u64,
        max_depth: u64,
    ) -> SetuResult<Vec<(Event, u64)>> {
        // Scan all depth keys and filter by range
        let prefix = key_prefix::DEPTH;
        let depth_keys = self.db.prefix_scan_keys(ColumnFamily::Events, prefix)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        let mut results = Vec::new();
        
        for key in depth_keys {
            // Extract event_id from key
            if key.len() <= prefix.len() {
                continue;
            }
            let event_id = match String::from_utf8(key[prefix.len()..].to_vec()) {
                Ok(id) => id,
                Err(_) => continue,
            };
            
            // Get depth
            if let Some(depth) = self.get_depth(&event_id).await {
                if depth >= min_depth && depth <= max_depth {
                    if let Some(event) = self.get(&event_id).await {
                        results.push((event, depth));
                    }
                }
            }
        }
        
        // Sort by depth
        results.sort_by_key(|(_, d)| *d);
        
        Ok(results)
    }

    async fn get_max_depth(&self) -> Option<u64> {
        // Scan all depth keys and find max
        let prefix = key_prefix::DEPTH;
        let depth_keys = self.db.prefix_scan_keys(ColumnFamily::Events, prefix).ok()?;
        
        let mut max_depth: Option<u64> = None;
        
        for key in depth_keys {
            if key.len() <= prefix.len() {
                continue;
            }
            let event_id = match String::from_utf8(key[prefix.len()..].to_vec()) {
                Ok(id) => id,
                Err(_) => continue,
            };
            
            if let Some(depth) = self.get_depth(&event_id).await {
                max_depth = Some(max_depth.map_or(depth, |m| m.max(depth)));
            }
        }
        
        max_depth
    }
}
