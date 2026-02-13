//! Storage access layer for Explorer
//!
//! Provides unified interface for accessing blockchain data,
//! supporting both direct RocksDB access and RPC mode.

use setu_storage::{SetuDB, EventStore, AnchorStore, RocksObjectStore, ObjectStore};
use setu_types::{Event, EventId, Anchor, AnchorId, EventStatus, Address, Coin, CoinType};
use std::sync::Arc;
use std::path::Path;

/// Storage interface for Explorer
#[derive(Clone)]
pub struct ExplorerStorage {
    db: Arc<SetuDB>,
    event_store: EventStore,
    anchor_store: AnchorStore,
    object_store: Arc<RocksObjectStore>,
}

impl ExplorerStorage {
    /// Open storage in read-only mode
    pub fn open_readonly<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        // Open RocksDB as secondary instance (can see validator's writes)
        // Secondary path is in a subdirectory to avoid conflicts
        let primary_path = path.as_ref();
        let secondary_path = primary_path.join("secondary");
        std::fs::create_dir_all(&secondary_path)?;
        
        let db = Arc::new(SetuDB::open_secondary(primary_path, &secondary_path)?);
        
        // Catch up with primary to see latest data
        db.try_catch_up_with_primary()?;
        
        // Create stores (not used anymore, kept for compatibility)
        let event_store = EventStore::new();
        let anchor_store = AnchorStore::new();
        
        // Create object store (also read-only)
        let object_store = Arc::new(RocksObjectStore::open_readonly(path)?);
        
        let storage = Self {
            db,
            event_store,
            anchor_store,
            object_store,
        };
        
        // No need to load data into memory - we query RocksDB directly!
        println!("✓ Explorer storage initialized (secondary instance, real-time sync)");
        
        Ok(storage)
    }
    
    /// Load events and anchors from RocksDB into memory caches (DEPRECATED - not used)
    fn load_from_db(&self) -> anyhow::Result<()> {
        // This method is no longer used - we query RocksDB directly for real-time data
        Ok(())
    }
    
    /// Get event store
    pub fn event_store(&self) -> &EventStore {
        &self.event_store
    }
    
    /// Get anchor store
    pub fn anchor_store(&self) -> &AnchorStore {
        &self.anchor_store
    }
    
    /// Get database handle
    pub fn db(&self) -> &Arc<SetuDB> {
        &self.db
    }
    
    // Convenience methods
    
    /// Get event by ID (query RocksDB directly for real-time data)
    pub async fn get_event(&self, event_id: &str) -> Option<Event> {
        // Catch up with primary to see latest writes
        let _ = self.db.try_catch_up_with_primary();
        
        use setu_storage::ColumnFamily;
        // Event keys are stored as "evt:{event_id}"
        let key = format!("evt:{}", event_id);
        self.db.get_raw::<Event>(ColumnFamily::Events, key.as_bytes()).ok().flatten()
    }
    
    /// Get multiple events
    pub async fn get_events(&self, event_ids: &[EventId]) -> Vec<Event> {
        // Catch up with primary to see latest writes
        let _ = self.db.try_catch_up_with_primary();
        
        let mut events = Vec::new();
        for event_id in event_ids {
            if let Some(event) = self.get_event(event_id).await {
                events.push(event);
            }
        }
        events
    }
    
    /// Get events by status (query RocksDB directly for real-time data)
    pub async fn get_events_by_status(&self, status: EventStatus) -> Vec<Event> {
        // Catch up with primary to see latest writes
        if let Err(e) = self.db.try_catch_up_with_primary() {
            tracing::warn!("Failed to catch up with primary: {}", e);
        }
        
        use setu_storage::ColumnFamily;
        
        // Build status prefix: "status:{status_byte}:"
        let status_byte = status as u8;
        let mut prefix = Vec::with_capacity(7 + 2); // "status:" + byte + ":"
        prefix.extend_from_slice(b"status:");
        prefix.push(status_byte);
        prefix.push(b':');
        
        // Scan prefix to get event IDs
        let event_ids: Vec<String> = match self.db.prefix_scan_keys(ColumnFamily::Events, &prefix) {
            Ok(keys) => {
                keys.into_iter()
                    .filter_map(|key| {
                        // Key format: status:{status}:{event_id}
                        // Extract event_id from end of key
                        if key.len() > prefix.len() {
                            String::from_utf8(key[prefix.len()..].to_vec()).ok()
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            Err(e) => {
                tracing::warn!("Failed to scan status prefix: {}", e);
                return Vec::new();
            }
        };
        
        tracing::info!("Found {} events with status {:?}", event_ids.len(), status);
        
        // Get events by IDs
        let mut events = Vec::new();
        for event_id in event_ids {
            if let Some(event) = self.get_event(&event_id).await {
                events.push(event);
            }
        }
        
        events
    }
    
    /// Get events by creator
    pub async fn get_events_by_creator(&self, creator: &str) -> Vec<Event> {
        use setu_storage::ColumnFamily;
        let mut events = Vec::new();
        if let Ok(iter) = self.db.iter_values::<Event>(ColumnFamily::Events) {
            for event_result in iter {
                if let Ok(event) = event_result {
                    if event.creator == creator {
                        events.push(event);
                    }
                }
            }
        }
        events
    }
    
    /// Get event depth
    pub async fn get_event_depth(&self, event_id: &EventId) -> Option<u64> {
        // TODO: Store depth in RocksDB
        None
    }
    
    /// Get all events (query RocksDB directly for real-time data)
    pub async fn get_all_events(&self) -> Vec<Event> {
        // Catch up with primary to see latest writes
        if let Err(e) = self.db.try_catch_up_with_primary() {
            tracing::warn!("Failed to catch up with primary: {}", e);
        }
        
        // Use prefix_iterator to scan only "evt:" keys efficiently
        let cf_handle = match self.db.inner().cf_handle("events") {
            Some(cf) => cf,
            None => {
                tracing::error!("Events column family not found");
                return Vec::new();
            }
        };
        
        let mut events = Vec::new();
        let prefix = b"evt:";
        
        for result in self.db.inner().prefix_iterator_cf(cf_handle, prefix) {
            match result {
                Ok((_key, value_bytes)) => {
                    // Deserialize value using BCS (same as SetuDB)
                    match bcs::from_bytes::<Event>(&value_bytes) {
                        Ok(event) => events.push(event),
                        Err(e) => {
                            tracing::debug!("Failed to deserialize event: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Iterator error: {}", e);
                    break;
                }
            }
        }
        
        tracing::info!("Found {} total events", events.len());
        events
    }
    
    /// Count events (query RocksDB directly for real-time count)
    pub async fn count_events(&self) -> usize {
        // Catch up with primary to see latest writes
        if let Err(e) = self.db.try_catch_up_with_primary() {
            tracing::warn!("Failed to catch up with primary: {}", e);
        }
        
        use setu_storage::ColumnFamily;
        
        // Try to get count from metadata first (much faster)
        let count_key = b"meta:event_count";
        if let Ok(Some(count)) = self.db.get_raw::<u64>(ColumnFamily::Events, count_key) {
            return count as usize;
        }
        
        // Fallback: count by iterating (slower)
        let mut count = 0;
        let cf_handle = match self.db.inner().cf_handle("events") {
            Some(cf) => cf,
            None => return 0,
        };
        
        let prefix = b"evt:";
        for result in self.db.inner().prefix_iterator_cf(cf_handle, prefix) {
            if result.is_ok() {
                count += 1;
            } else {
                break;
            }
        }
        
        count
    }
    
    /// Get anchor by ID (query RocksDB directly for real-time data)
    pub async fn get_anchor(&self, anchor_id: &str) -> Option<Anchor> {
        // Catch up with primary to see latest writes
        let _ = self.db.try_catch_up_with_primary();
        
        use setu_storage::ColumnFamily;
        // Anchor keys are stored as "anchor:{anchor_id}"
        let key = format!("anchor:{}", anchor_id);
        self.db.get_raw::<Anchor>(ColumnFamily::Anchors, key.as_bytes()).ok().flatten()
    }
    
    /// Get latest anchor
    pub async fn get_latest_anchor(&self) -> Option<Anchor> {
        use setu_storage::ColumnFamily;
        let mut latest: Option<Anchor> = None;
        if let Ok(iter) = self.db.iter_values::<Anchor>(ColumnFamily::Anchors) {
            for anchor_result in iter {
                if let Ok(anchor) = anchor_result {
                    if latest.is_none() || anchor.depth > latest.as_ref().unwrap().depth {
                        latest = Some(anchor);
                    }
                }
            }
        }
        latest
    }
    
    /// Get anchor by depth
    pub async fn get_anchor_by_depth(&self, depth: u64) -> Option<Anchor> {
        use setu_storage::ColumnFamily;
        if let Ok(iter) = self.db.iter_values::<Anchor>(ColumnFamily::Anchors) {
            for anchor_result in iter {
                if let Ok(anchor) = anchor_result {
                    if anchor.depth == depth {
                        return Some(anchor);
                    }
                }
            }
        }
        None
    }
    
    /// Get anchor chain
    pub async fn get_anchor_chain(&self) -> Vec<AnchorId> {
        // Catch up with primary to see latest writes
        let _ = self.db.try_catch_up_with_primary();
        
        use setu_storage::ColumnFamily;
        
        // First, try to get count from metadata
        let count_key = b"meta:count";
        let count: usize = self.db.get_raw::<u64>(ColumnFamily::Anchors, count_key)
            .ok()
            .flatten()
            .unwrap_or(0) as usize;
        
        if count == 0 {
            return Vec::new();
        }
        
        // Read chain indices: "chain:{index}" -> AnchorId
        // Key format: b"chain:" + index.to_be_bytes()
        let mut chain = Vec::with_capacity(count);
        for i in 0..count as u64 {
            let mut chain_key = Vec::with_capacity(6 + 8); // "chain:" + 8 bytes
            chain_key.extend_from_slice(b"chain:");
            chain_key.extend_from_slice(&i.to_be_bytes());
            
            if let Ok(Some(anchor_id)) = self.db.get_raw::<AnchorId>(ColumnFamily::Anchors, &chain_key) {
                chain.push(anchor_id);
            }
        }
        
        chain
    }
    
    /// Count anchors (query RocksDB directly for real-time count)
    pub async fn count_anchors(&self) -> usize {
        // Catch up with primary to see latest writes
        if let Err(e) = self.db.try_catch_up_with_primary() {
            tracing::warn!("Failed to catch up with primary: {}", e);
        }
        
        use setu_storage::ColumnFamily;
        let mut count = 0;
        if let Ok(iter) = self.db.iter_values::<Anchor>(ColumnFamily::Anchors) {
            for _ in iter {
                count += 1;
            }
        }
        tracing::info!("Found {} anchors in RocksDB", count);
        count
    }
    
    // Object store methods
    
    /// Get coins by owner address
    pub fn get_coins_by_owner(&self, owner: &Address) -> anyhow::Result<Vec<Coin>> {
        self.object_store.get_coins_by_owner(owner)
            .map_err(|e| anyhow::anyhow!("Failed to get coins: {}", e))
    }
    
    /// Get coins by owner and coin type
    pub fn get_coins_by_owner_and_type(&self, owner: &Address, coin_type: &CoinType) -> anyhow::Result<Vec<Coin>> {
        self.object_store.get_coins_by_owner_and_type(owner, coin_type)
            .map_err(|e| anyhow::anyhow!("Failed to get coins: {}", e))
    }
}

