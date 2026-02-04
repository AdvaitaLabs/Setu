//! EventStore - In-memory Event storage with concurrent access
//!
//! This module provides a high-performance in-memory implementation of event storage
//! using DashMap for lock-free concurrent access.

use crate::types::BatchStoreResult;
use dashmap::DashMap;
use setu_types::{Event, EventId, EventStatus, SetuResult};
use std::collections::HashMap;
use std::sync::Arc;

/// In-memory storage for Events with lock-free concurrent access
///
/// EventStore uses DashMap internally, providing:
/// - Lock-free reads and writes for most operations
/// - Better performance under high concurrency
/// - Automatic sharding for reduced contention
///
/// ## Index Tables
/// - `events`: Primary storage (EventId -> Event)
/// - `by_creator`: Creator index (Creator -> Vec<EventId>)
/// - `by_status`: Status index (EventStatus -> Vec<EventId>)
/// - `depths`: Depth index (EventId -> u64)
#[derive(Debug)]
pub struct EventStore {
    events: Arc<DashMap<EventId, Event>>,
    by_creator: Arc<DashMap<String, Vec<EventId>>>,
    by_status: Arc<DashMap<EventStatus, Vec<EventId>>>,
    /// Depth index table - stores event depths separately from Event struct
    /// Design note: depth is a DAG topological property, not an intrinsic event property
    depths: Arc<DashMap<EventId, u64>>,
}

impl EventStore {
    /// Create a new empty EventStore
    pub fn new() -> Self {
        Self {
            events: Arc::new(DashMap::new()),
            by_creator: Arc::new(DashMap::new()),
            by_status: Arc::new(DashMap::new()),
            depths: Arc::new(DashMap::new()),
        }
    }

    /// Store an event
    pub async fn store(&self, event: Event) -> SetuResult<()> {
        let event_id = event.id.clone();
        let creator = event.creator.clone();
        let status = event.status;

        // Insert into main store
        self.events.insert(event_id.clone(), event);

        // Update creator index
        self.by_creator
            .entry(creator)
            .or_insert_with(Vec::new)
            .push(event_id.clone());

        // Update status index
        self.by_status
            .entry(status)
            .or_insert_with(Vec::new)
            .push(event_id);

        Ok(())
    }

    /// Get an event by ID
    pub async fn get(&self, event_id: &EventId) -> Option<Event> {
        self.events.get(event_id).map(|r| r.value().clone())
    }

    /// Get multiple events by IDs
    pub async fn get_many(&self, event_ids: &[EventId]) -> Vec<Event> {
        event_ids
            .iter()
            .filter_map(|id| self.events.get(id).map(|r| r.value().clone()))
            .collect()
    }

    /// Update event status
    pub async fn update_status(&self, event_id: &EventId, new_status: EventStatus) {
        let old_status = {
            if let Some(mut event) = self.events.get_mut(event_id) {
                let old = event.status;
                event.status = new_status;
                Some(old)
            } else {
                None
            }
        };

        if let Some(old_status) = old_status {
            // Remove from old status index
            if let Some(mut ids) = self.by_status.get_mut(&old_status) {
                ids.retain(|id| id != event_id);
            }

            // Add to new status index
            self.by_status
                .entry(new_status)
                .or_insert_with(Vec::new)
                .push(event_id.clone());
        }
    }

    /// Get events by creator
    pub async fn get_by_creator(&self, creator: &str) -> Vec<Event> {
        self.by_creator
            .get(creator)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.events.get(id).map(|r| r.value().clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get events by status
    pub async fn get_by_status(&self, status: EventStatus) -> Vec<Event> {
        self.by_status
            .get(&status)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.events.get(id).map(|r| r.value().clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get total event count
    pub async fn count(&self) -> usize {
        self.events.len()
    }

    /// Count events by status
    pub async fn count_by_status(&self, status: EventStatus) -> usize {
        self.by_status.get(&status).map(|v| v.len()).unwrap_or(0)
    }

    /// Check if an event exists
    pub async fn exists(&self, event_id: &EventId) -> bool {
        self.events.contains_key(event_id)
    }

    // =========================================================================
    // Depth-related methods (for DagManager integration)
    // =========================================================================

    /// Check if multiple events exist
    pub async fn exists_many(&self, event_ids: &[EventId]) -> Vec<bool> {
        event_ids
            .iter()
            .map(|id| self.events.contains_key(id))
            .collect()
    }

    /// Get event depth from the independent depths index
    /// Used by resolve_parents() Phase 1c fallback query
    pub async fn get_depth(&self, event_id: &EventId) -> Option<u64> {
        self.depths.get(event_id).map(|r| *r.value())
    }

    /// Batch get event depths (for cache warmup optimization)
    pub async fn get_depths_batch(&self, event_ids: &[EventId]) -> HashMap<EventId, u64> {
        event_ids
            .iter()
            .filter_map(|id| self.depths.get(id).map(|r| (id.clone(), *r.value())))
            .collect()
    }

    /// Get event's parent_ids (for cache warmup)
    pub async fn get_parent_ids(&self, event_id: &EventId) -> Option<Vec<EventId>> {
        self.events.get(event_id).map(|e| e.parent_ids.clone())
    }

    /// Persist event with its depth (called during persist_finalized_anchor)
    ///
    /// # Atomicity Note
    ///
    /// With DashMap, each insert is atomic at the shard level. While individual
    /// operations are lock-free, the overall store operation is not strictly atomic
    /// across all indexes. This is acceptable for in-memory testing scenarios.
    /// For production use with RocksDB, WriteBatch ensures full atomicity.
    pub async fn store_with_depth(&self, event: Event, depth: u64) -> SetuResult<()> {
        let event_id = event.id.clone();
        let creator = event.creator.clone();
        let status = event.status;

        // Store event
        self.events.insert(event_id.clone(), event);

        // Update creator index
        self.by_creator
            .entry(creator)
            .or_insert_with(Vec::new)
            .push(event_id.clone());

        // Update status index
        self.by_status
            .entry(status)
            .or_insert_with(Vec::new)
            .push(event_id.clone());

        // Store depth
        self.depths.insert(event_id, depth);

        Ok(())
    }

    /// Batch persist events with depth (optimized for finalization)
    ///
    /// Returns a result indicating success/failure counts.
    ///
    /// # Error Handling
    /// - Duplicate events are counted as `skipped` (non-critical)
    /// - Other errors are counted as `failed` (critical)
    /// - Caller should check `result.failed > 0` before proceeding with anchor write
    pub async fn store_batch_with_depth(
        &self,
        events_with_depths: Vec<(Event, u64)>,
    ) -> BatchStoreResult {
        let mut result = BatchStoreResult::default();

        if events_with_depths.is_empty() {
            return result;
        }

        for (event, depth) in events_with_depths {
            let event_id = event.id.clone();

            // Check for duplicates (non-critical, skip)
            if self.events.contains_key(&event_id) {
                result.skipped += 1;
                result.skipped_ids.push(event_id);
                continue;
            }

            // Store event
            let creator = event.creator.clone();
            let status = event.status;

            self.events.insert(event_id.clone(), event);

            self.by_creator
                .entry(creator)
                .or_insert_with(Vec::new)
                .push(event_id.clone());

            self.by_status
                .entry(status)
                .or_insert_with(Vec::new)
                .push(event_id.clone());

            // Store depth
            self.depths.insert(event_id, depth);

            result.stored += 1;
        }

        result
    }

    /// Batch get events (for network sync)
    pub async fn get_events_batch(&self, event_ids: &[EventId]) -> Vec<Event> {
        event_ids
            .iter()
            .filter_map(|id| self.events.get(id).map(|r| r.value().clone()))
            .collect()
    }
}

impl Clone for EventStore {
    fn clone(&self) -> Self {
        Self {
            events: Arc::clone(&self.events),
            by_creator: Arc::clone(&self.by_creator),
            by_status: Arc::clone(&self.by_status),
            depths: Arc::clone(&self.depths),
        }
    }
}

impl Default for EventStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{EventType, VLCSnapshot, VectorClock};

    fn create_event(creator: &str) -> Event {
        Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 1,
                physical_time: 1000,
            },
            creator.to_string(),
        )
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let store = EventStore::new();
        let event = create_event("node1");
        let event_id = event.id.clone();

        store.store(event).await.unwrap();

        let retrieved = store.get(&event_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, event_id);
    }

    #[tokio::test]
    async fn test_get_by_creator() {
        let store = EventStore::new();

        store.store(create_event("node1")).await.unwrap();
        store.store(create_event("node1")).await.unwrap();
        store.store(create_event("node2")).await.unwrap();

        let node1_events = store.get_by_creator("node1").await;
        assert_eq!(node1_events.len(), 2);
    }

    #[tokio::test]
    async fn test_update_status() {
        let store = EventStore::new();
        let event = create_event("node1");
        let event_id = event.id.clone();

        store.store(event).await.unwrap();
        store.update_status(&event_id, EventStatus::Executed).await;

        let updated = store.get(&event_id).await.unwrap();
        assert_eq!(updated.status, EventStatus::Executed);
    }

    #[tokio::test]
    async fn test_concurrent_stores() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let store = Arc::new(EventStore::new());
        let counter = Arc::new(AtomicU64::new(0));
        let mut tasks = JoinSet::new();

        // Spawn 100 concurrent store operations
        // Use unique logical_time to ensure unique event IDs
        for i in 0..100 {
            let store = Arc::clone(&store);
            let counter = Arc::clone(&counter);
            tasks.spawn(async move {
                let unique_time = counter.fetch_add(1, Ordering::SeqCst);
                let event = Event::new(
                    EventType::Transfer,
                    vec![],
                    VLCSnapshot {
                        vector_clock: VectorClock::new(),
                        logical_time: unique_time,
                        physical_time: unique_time * 1000,
                    },
                    format!("node{}", i % 10),
                );
                store.store(event).await.unwrap();
            });
        }

        // Wait for all tasks
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }

        assert_eq!(store.count().await, 100);
    }
}
