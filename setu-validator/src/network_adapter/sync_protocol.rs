//! Sync Protocol
//!
//! Provides event and consensus frame synchronization between validators.
//! This module implements the sync store trait and protocol logic that
//! was previously embedded in the network layer.

use async_trait::async_trait;
use crate::protocol::{SerializedConsensusFrame, SerializedEvent};
use setu_types::{ConsensusFrame, Event, EventId};
use setu_storage::{EventStore, CFStore};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn, error};

/// Sync store trait for state synchronization
///
/// This trait abstracts the storage operations needed for state sync,
/// allowing the protocol to work with different storage backends.
/// Unlike the network layer's StateSyncStore, this trait works with
/// business types directly.
#[async_trait]
pub trait SyncStore: Send + Sync + 'static {
    /// Get events by their IDs
    async fn get_events_by_ids(&self, event_ids: &[EventId]) -> Vec<Event>;
    
    /// Get events starting from a sequence number
    async fn get_events_from_seq(&self, start_seq: u64, limit: u32) -> Vec<Event>;
    
    /// Store events received from peers
    async fn store_events(&self, events: Vec<Event>) -> (u32, Vec<String>);
    
    /// Get consensus frames starting from a sequence number
    async fn get_cfs_from_seq(&self, start_seq: u64, limit: u32) -> Vec<ConsensusFrame>;
    
    /// Store a consensus frame
    async fn store_cf(&self, cf: ConsensusFrame) -> bool;
    
    /// Get the highest event sequence
    async fn highest_event_seq(&self) -> u64;
    
    /// Get the highest finalized CF sequence
    async fn highest_cf_seq(&self) -> u64;
}

/// Persistent implementation of SyncStore using RocksDB
#[allow(dead_code)]
pub struct PersistentSyncStore {
    event_store: Arc<EventStore>,
    cf_store: Arc<CFStore>,
}

#[allow(dead_code)]
impl PersistentSyncStore {
    pub fn new(event_store: Arc<EventStore>, cf_store: Arc<CFStore>) -> Self {
        Self {
            event_store,
            cf_store,
        }
    }
}

#[async_trait]
impl SyncStore for PersistentSyncStore {
    async fn get_events_by_ids(&self, event_ids: &[EventId]) -> Vec<Event> {
        self.event_store.get_many(event_ids).await
    }
    
    async fn get_events_from_seq(&self, _start_seq: u64, _limit: u32) -> Vec<Event> {
        // TODO: Implement sequence-based retrieval in EventStore
        // For now, return empty - this is used for state sync protocol
        Vec::new()
    }
    
    async fn store_events(&self, events: Vec<Event>) -> (u32, Vec<String>) {
        let mut accepted = 0;
        let mut rejected = Vec::new();
        
        for event in events {
            if let Err(e) = self.event_store.store(event.clone()).await {
                warn!(event_id = %event.id, error = %e, "Failed to store synced event");
                rejected.push(event.id);
            } else {
                accepted += 1;
            }
        }
        
        (accepted, rejected)
    }
    
    async fn get_cfs_from_seq(&self, _start_seq: u64, _limit: u32) -> Vec<ConsensusFrame> {
        // TODO: Implement sequence-based retrieval in CFStore
        Vec::new()
    }
    
    async fn store_cf(&self, cf: ConsensusFrame) -> bool {
        match self.cf_store.store(cf).await {
            Ok(_) => true,
            Err(e) => {
                error!(error = %e, "Failed to store synced CF");
                false
            }
        }
    }
    
    async fn highest_event_seq(&self) -> u64 {
        self.event_store.count().await as u64
    }
    
    async fn highest_cf_seq(&self) -> u64 {
        self.cf_store.pending_count().await as u64 // Approximation
    }
}

/// In-memory implementation of SyncStore for testing
#[derive(Default)]
pub struct InMemorySyncStore {
    events: RwLock<HashMap<EventId, (u64, Event)>>,
    cfs: RwLock<HashMap<String, (u64, ConsensusFrame)>>,
    event_seq_counter: RwLock<u64>,
    cf_seq_counter: RwLock<u64>,
}

impl InMemorySyncStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SyncStore for InMemorySyncStore {
    async fn get_events_by_ids(&self, event_ids: &[EventId]) -> Vec<Event> {
        let events = self.events.read().await;
        event_ids
            .iter()
            .filter_map(|id| events.get(id).map(|(_, e)| e.clone()))
            .collect()
    }
    
    async fn get_events_from_seq(&self, start_seq: u64, limit: u32) -> Vec<Event> {
        let events = self.events.read().await;
        let mut result: Vec<(u64, Event)> = events
            .values()
            .filter(|(seq, _)| *seq > start_seq)
            .cloned()
            .collect();
        result.sort_by_key(|(seq, _)| *seq);
        result.into_iter()
            .take(limit as usize)
            .map(|(_, e)| e)
            .collect()
    }
    
    async fn store_events(&self, events: Vec<Event>) -> (u32, Vec<String>) {
        let mut storage = self.events.write().await;
        let mut seq_counter = self.event_seq_counter.write().await;
        let mut accepted = 0u32;
        let mut rejected = Vec::new();
        
        for event in events {
            if storage.contains_key(&event.id) {
                rejected.push(event.id.clone());
                continue;
            }
            *seq_counter += 1;
            storage.insert(event.id.clone(), (*seq_counter, event));
            accepted += 1;
        }
        
        (accepted, rejected)
    }
    
    async fn get_cfs_from_seq(&self, start_seq: u64, limit: u32) -> Vec<ConsensusFrame> {
        let cfs = self.cfs.read().await;
        let mut result: Vec<(u64, ConsensusFrame)> = cfs
            .values()
            .filter(|(seq, _)| *seq > start_seq)
            .cloned()
            .collect();
        result.sort_by_key(|(seq, _)| *seq);
        result.into_iter()
            .take(limit as usize)
            .map(|(_, cf)| cf)
            .collect()
    }
    
    async fn store_cf(&self, cf: ConsensusFrame) -> bool {
        let mut storage = self.cfs.write().await;
        if storage.contains_key(&cf.id) {
            return false;
        }
        let mut seq_counter = self.cf_seq_counter.write().await;
        *seq_counter += 1;
        storage.insert(cf.id.clone(), (*seq_counter, cf));
        true
    }
    
    async fn highest_event_seq(&self) -> u64 {
        *self.event_seq_counter.read().await
    }
    
    async fn highest_cf_seq(&self) -> u64 {
        *self.cf_seq_counter.read().await
    }
}

/// Sync protocol implementation
///
/// This handles the actual synchronization logic, converting between
/// business types and serialized network types.
pub struct SyncProtocol<S: SyncStore> {
    store: Arc<S>,
}

impl<S: SyncStore> SyncProtocol<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
        }
    }
    
    /// Handle a request for events by ID
    pub async fn handle_request_events(&self, event_ids: &[String]) -> Vec<Event> {
        debug!(
            count = event_ids.len(),
            "Processing request for events"
        );
        
        self.store.get_events_by_ids(event_ids).await
    }
    
    /// Store events received from network
    pub async fn store_received_events(&self, events: Vec<Event>) -> (u32, Vec<String>) {
        debug!(
            count = events.len(),
            "Storing events received from network"
        );
        
        self.store.store_events(events).await
    }
    
    /// Convert Event to SerializedEvent for network transmission
    pub fn serialize_event(event: &Event, seq: u64) -> SerializedEvent {
        SerializedEvent {
            seq,
            id: event.id.clone(),
            data: bincode::serialize(event).unwrap_or_default(),
        }
    }
    
    /// Convert SerializedEvent back to Event
    pub fn deserialize_event(serialized: &SerializedEvent) -> Option<Event> {
        bincode::deserialize(&serialized.data).ok()
    }
    
    /// Convert ConsensusFrame to SerializedConsensusFrame
    pub fn serialize_cf(cf: &ConsensusFrame, seq: u64) -> SerializedConsensusFrame {
        SerializedConsensusFrame {
            seq,
            id: cf.id.clone(),
            data: bincode::serialize(cf).unwrap_or_default(),
            vlc: None, // VLC is embedded in the CF
        }
    }
    
    /// Convert SerializedConsensusFrame back to ConsensusFrame
    pub fn deserialize_cf(serialized: &SerializedConsensusFrame) -> Option<ConsensusFrame> {
        bincode::deserialize(&serialized.data).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::VLCSnapshot;
    
    #[tokio::test]
    async fn test_in_memory_store() {
        let store = InMemorySyncStore::default();
        
        // Create test event
        let event = Event::genesis("test_creator".to_string(), VLCSnapshot::default());
        let event_id = event.id.clone();
        
        // Store event
        let (accepted, rejected) = store.store_events(vec![event.clone()]).await;
        assert_eq!(accepted, 1);
        assert!(rejected.is_empty());
        
        // Retrieve by ID
        let events = store.get_events_by_ids(&[event_id.clone()]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event_id);
        
        // Check sequence
        assert_eq!(store.highest_event_seq().await, 1);
        
        // Try to store duplicate
        let (accepted, rejected) = store.store_events(vec![events[0].clone()]).await;
        assert_eq!(accepted, 0);
        assert_eq!(rejected.len(), 1);
    }
    
    #[tokio::test]
    async fn test_sync_protocol_serialization() {
        let event = Event::genesis("creator".to_string(), VLCSnapshot::default());
        
        let serialized = SyncProtocol::<InMemorySyncStore>::serialize_event(&event, 1);
        assert_eq!(serialized.seq, 1);
        assert_eq!(serialized.id, event.id);
        
        let deserialized = SyncProtocol::<InMemorySyncStore>::deserialize_event(&serialized);
        assert!(deserialized.is_some());
        assert_eq!(deserialized.unwrap().id, event.id);
    }
}
