//! Storage access layer for Explorer
//!
//! Provides unified interface for accessing blockchain data,
//! supporting both direct RocksDB access and RPC mode.

use setu_storage::{SetuDB, EventStore, AnchorStore};
use setu_types::{Event, EventId, Anchor, AnchorId, EventStatus};
use std::sync::Arc;
use std::path::Path;

/// Storage interface for Explorer
#[derive(Clone)]
pub struct ExplorerStorage {
    db: Arc<SetuDB>,
    event_store: EventStore,
    anchor_store: AnchorStore,
}

impl ExplorerStorage {
    /// Open storage in read-only mode
    pub fn open_readonly<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        // Open RocksDB in read-only mode
        let db = Arc::new(SetuDB::open_default(path)?);
        
        // Create stores (they will use the read-only DB)
        let event_store = EventStore::new();
        let anchor_store = AnchorStore::new();
        
        Ok(Self {
            db,
            event_store,
            anchor_store,
        })
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
    
    /// Get event by ID
    pub async fn get_event(&self, event_id: &str) -> Option<Event> {
        self.event_store.get(&event_id.to_string()).await
    }
    
    /// Get multiple events
    pub async fn get_events(&self, event_ids: &[EventId]) -> Vec<Event> {
        self.event_store.get_many(event_ids).await
    }
    
    /// Get events by status
    pub async fn get_events_by_status(&self, status: EventStatus) -> Vec<Event> {
        self.event_store.get_by_status(status).await
    }
    
    /// Get events by creator
    pub async fn get_events_by_creator(&self, creator: &str) -> Vec<Event> {
        self.event_store.get_by_creator(creator).await
    }
    
    /// Get event depth
    pub async fn get_event_depth(&self, event_id: &EventId) -> Option<u64> {
        self.event_store.get_depth(event_id).await
    }
    
    /// Count events
    pub async fn count_events(&self) -> usize {
        self.event_store.count().await
    }
    
    /// Get anchor by ID
    pub async fn get_anchor(&self, anchor_id: &str) -> Option<Anchor> {
        self.anchor_store.get(&anchor_id.to_string()).await
    }
    
    /// Get latest anchor
    pub async fn get_latest_anchor(&self) -> Option<Anchor> {
        self.anchor_store.get_latest().await
    }
    
    /// Get anchor by depth
    pub async fn get_anchor_by_depth(&self, depth: u64) -> Option<Anchor> {
        self.anchor_store.get_by_depth(depth).await
    }
    
    /// Get anchor chain
    pub async fn get_anchor_chain(&self) -> Vec<AnchorId> {
        self.anchor_store.get_chain().await
    }
    
    /// Count anchors
    pub async fn count_anchors(&self) -> usize {
        self.anchor_store.count().await
    }
}

