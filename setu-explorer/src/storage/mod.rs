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
        self.db.get_raw::<Event>(ColumnFamily::Events, event_id.as_bytes()).ok().flatten()
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
        let _ = self.db.try_catch_up_with_primary();
        
        use setu_storage::ColumnFamily;
        let mut events = Vec::new();
        if let Ok(iter) = self.db.iter_values::<Event>(ColumnFamily::Events) {
            for event_result in iter {
                if let Ok(event) = event_result {
                    if event.status == status {
                        events.push(event);
                    }
                }
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
    
    /// Count events (query RocksDB directly for real-time count)
    pub async fn count_events(&self) -> usize {
        use setu_storage::ColumnFamily;
        let mut count = 0;
        if let Ok(iter) = self.db.iter_values::<Event>(ColumnFamily::Events) {
            for _ in iter {
                count += 1;
            }
        }
        count
    }
    
    /// Get anchor by ID (query RocksDB directly for real-time data)
    pub async fn get_anchor(&self, anchor_id: &str) -> Option<Anchor> {
        use setu_storage::ColumnFamily;
        self.db.get_raw::<Anchor>(ColumnFamily::Anchors, anchor_id.as_bytes()).ok().flatten()
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
        use setu_storage::ColumnFamily;
        let mut anchors = Vec::new();
        if let Ok(iter) = self.db.iter_values::<Anchor>(ColumnFamily::Anchors) {
            for anchor_result in iter {
                if let Ok(anchor) = anchor_result {
                    anchors.push(anchor);
                }
            }
        }
        // Sort by depth
        anchors.sort_by_key(|a| a.depth);
        anchors.into_iter().map(|a| a.id).collect()
    }
    
    /// Count anchors (query RocksDB directly for real-time count)
    pub async fn count_anchors(&self) -> usize {
        use setu_storage::ColumnFamily;
        let mut count = 0;
        if let Ok(iter) = self.db.iter_values::<Anchor>(ColumnFamily::Anchors) {
            for _ in iter {
                count += 1;
            }
        }
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

