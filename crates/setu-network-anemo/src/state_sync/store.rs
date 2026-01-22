// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! In-memory implementation of StateSyncStore for testing

use super::{StateSyncStore, SerializedConsensusFrame, SerializedEvent, SerializedVote};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Error type for in-memory store operations
#[derive(Debug, thiserror::Error)]
pub enum InMemoryStoreError {
    #[error("Event not found: {0}")]
    EventNotFound(String),
    #[error("CF not found: {0}")]
    CFNotFound(String),
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}

/// In-memory implementation of StateSyncStore
/// 
/// Useful for testing and as a reference implementation.
#[derive(Clone)]
pub struct InMemoryStateSyncStore {
    /// Events indexed by sequence number
    events_by_seq: Arc<RwLock<BTreeMap<u64, SerializedEvent>>>,
    /// Events indexed by ID
    events_by_id: Arc<RwLock<std::collections::HashMap<String, u64>>>,
    /// Consensus frames indexed by sequence number
    cfs_by_seq: Arc<RwLock<BTreeMap<u64, SerializedConsensusFrame>>>,
    /// Votes indexed by CF ID
    votes_by_cf: Arc<RwLock<std::collections::HashMap<String, Vec<SerializedVote>>>>,
    /// Highest event sequence
    highest_event: Arc<RwLock<u64>>,
    /// Highest finalized CF sequence
    highest_cf: Arc<RwLock<u64>>,
}

impl InMemoryStateSyncStore {
    pub fn new() -> Self {
        Self {
            events_by_seq: Arc::new(RwLock::new(BTreeMap::new())),
            events_by_id: Arc::new(RwLock::new(std::collections::HashMap::new())),
            cfs_by_seq: Arc::new(RwLock::new(BTreeMap::new())),
            votes_by_cf: Arc::new(RwLock::new(std::collections::HashMap::new())),
            highest_event: Arc::new(RwLock::new(0)),
            highest_cf: Arc::new(RwLock::new(0)),
        }
    }

    /// Add an event (for testing)
    pub async fn add_event(&self, event: SerializedEvent) {
        let seq = event.seq;
        let id = event.id.clone();
        
        self.events_by_seq.write().await.insert(seq, event);
        self.events_by_id.write().await.insert(id, seq);
        
        let mut highest = self.highest_event.write().await;
        *highest = (*highest).max(seq);
    }

    /// Add a consensus frame (for testing)
    pub async fn add_cf(&self, cf: SerializedConsensusFrame, votes: Vec<SerializedVote>) {
        let seq = cf.seq;
        let id = cf.id.clone();
        
        self.cfs_by_seq.write().await.insert(seq, cf);
        self.votes_by_cf.write().await.insert(id, votes);
        
        let mut highest = self.highest_cf.write().await;
        *highest = (*highest).max(seq);
    }
}

impl Default for InMemoryStateSyncStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateSyncStore for InMemoryStateSyncStore {
    type Error = InMemoryStoreError;

    async fn get_events_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedEvent>, bool, u64), Self::Error> {
        let events = self.events_by_seq.read().await;
        
        let result: Vec<SerializedEvent> = events
            .range((start_seq + 1)..)
            .take(limit as usize)
            .map(|(_, e)| e.clone())
            .collect();
        
        let highest_seq = result.last().map(|e| e.seq).unwrap_or(start_seq);
        let has_more = events.range((highest_seq + 1)..).next().is_some();
        
        Ok((result, has_more, highest_seq))
    }

    async fn store_events(
        &self,
        events: Vec<SerializedEvent>,
    ) -> Result<(u32, Vec<String>), Self::Error> {
        let mut accepted = 0u32;
        let mut rejected = Vec::new();
        
        for mut event in events {
            // Check if already exists
            if self.events_by_id.read().await.contains_key(&event.id) {
                rejected.push(event.id);
                continue;
            }
            
            // Auto-assign seq if not set (seq=0 means not assigned)
            if event.seq == 0 {
                let mut highest = self.highest_event.write().await;
                *highest += 1;
                event.seq = *highest;
            }
            
            // Store the event
            self.add_event(event).await;
            accepted += 1;
        }
        
        Ok((accepted, rejected))
    }

    async fn get_events_by_ids(
        &self,
        event_ids: &[String],
    ) -> Result<Vec<SerializedEvent>, Self::Error> {
        let events_by_id = self.events_by_id.read().await;
        let events_by_seq = self.events_by_seq.read().await;
        
        let result = event_ids
            .iter()
            .filter_map(|id| {
                events_by_id.get(id).and_then(|seq| events_by_seq.get(seq).cloned())
            })
            .collect();
        
        Ok(result)
    }

    async fn highest_event_seq(&self) -> u64 {
        *self.highest_event.read().await
    }

    async fn get_cfs_from_seq(
        &self,
        start_seq: u64,
        limit: u32,
    ) -> Result<(Vec<SerializedConsensusFrame>, bool, u64), Self::Error> {
        let cfs = self.cfs_by_seq.read().await;
        
        let result: Vec<SerializedConsensusFrame> = cfs
            .range((start_seq + 1)..)
            .take(limit as usize)
            .map(|(_, cf)| cf.clone())
            .collect();
        
        let highest_seq = result.last().map(|cf| cf.seq).unwrap_or(start_seq);
        let has_more = cfs.range((highest_seq + 1)..).next().is_some();
        
        Ok((result, has_more, highest_seq))
    }

    async fn store_cf(
        &self,
        frame: SerializedConsensusFrame,
        votes: Vec<SerializedVote>,
    ) -> Result<(bool, Option<String>), Self::Error> {
        // Check if already exists
        if self.cfs_by_seq.read().await.contains_key(&frame.seq) {
            return Ok((false, Some("CF already exists".to_string())));
        }
        
        // TODO: Add vote validation logic here
        // For now, just store it
        self.add_cf(frame, votes).await;
        
        Ok((true, None))
    }

    async fn highest_finalized_cf_seq(&self) -> u64 {
        *self.highest_cf.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_storage() {
        let store = InMemoryStateSyncStore::new();
        
        // Add some events
        for i in 1..=10 {
            store.add_event(SerializedEvent {
                seq: i,
                id: format!("event_{}", i),
                data: vec![i as u8],
            }).await;
        }
        
        // Get events from seq 0
        let (events, has_more, highest) = store.get_events_from_seq(0, 5).await.unwrap();
        assert_eq!(events.len(), 5);
        assert!(has_more);
        assert_eq!(highest, 5);
        
        // Get events from seq 5
        let (events, has_more, highest) = store.get_events_from_seq(5, 5).await.unwrap();
        assert_eq!(events.len(), 5);
        assert!(!has_more);
        assert_eq!(highest, 10);
    }

    #[tokio::test]
    async fn test_event_by_id() {
        let store = InMemoryStateSyncStore::new();
        
        store.add_event(SerializedEvent {
            seq: 1,
            id: "event_1".to_string(),
            data: vec![1],
        }).await;
        
        store.add_event(SerializedEvent {
            seq: 2,
            id: "event_2".to_string(),
            data: vec![2],
        }).await;
        
        let events = store.get_events_by_ids(&["event_1".to_string(), "event_3".to_string()]).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "event_1");
    }
}
