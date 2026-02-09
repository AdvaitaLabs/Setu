//! EventStore backend trait for abstracting storage implementations
//!
//! This trait allows switching between in-memory (EventStore) and
//! persistent (RocksDBEventStore) implementations at runtime.

use crate::types::BatchStoreResult;
use async_trait::async_trait;
use setu_types::{Event, EventId, EventStatus, SetuResult};
use std::collections::HashMap;
use std::fmt::Debug;

/// Backend trait for EventStore implementations
///
/// This trait defines the common interface that both in-memory and RocksDB
/// implementations must satisfy. It enables:
/// 1. Runtime switching between storage backends
/// 2. Testing with in-memory storage
/// 3. Production deployment with RocksDB persistence
#[async_trait]
pub trait EventStoreBackend: Send + Sync + Debug {
    // =========================================================================
    // Core CRUD operations
    // =========================================================================

    /// Store an event (without depth - for backwards compatibility)
    async fn store(&self, event: Event) -> SetuResult<()>;

    /// Get an event by ID
    async fn get(&self, event_id: &EventId) -> Option<Event>;

    /// Get multiple events by IDs
    async fn get_many(&self, event_ids: &[EventId]) -> Vec<Event>;

    /// Check if an event exists
    async fn exists(&self, event_id: &EventId) -> bool;

    /// Check if multiple events exist
    async fn exists_many(&self, event_ids: &[EventId]) -> Vec<bool>;

    /// Get total event count
    async fn count(&self) -> usize;

    // =========================================================================
    // Depth-related operations (for DAG integration)
    // =========================================================================

    /// Store an event with its depth (atomic operation)
    async fn store_with_depth(&self, event: Event, depth: u64) -> SetuResult<()>;

    /// Batch store events with depths (for finalization)
    async fn store_batch_with_depth(
        &self,
        events_with_depths: Vec<(Event, u64)>,
    ) -> BatchStoreResult;

    /// Get event depth
    async fn get_depth(&self, event_id: &EventId) -> Option<u64>;

    /// Batch get event depths
    async fn get_depths_batch(&self, event_ids: &[EventId]) -> HashMap<EventId, u64>;

    /// Get event's parent IDs (for cache warmup)
    async fn get_parent_ids(&self, event_id: &EventId) -> Option<Vec<EventId>>;

    // =========================================================================
    // Index queries
    // =========================================================================

    /// Get events by creator
    async fn get_by_creator(&self, creator: &str) -> Vec<Event>;

    /// Get events by status
    async fn get_by_status(&self, status: EventStatus) -> Vec<Event>;

    /// Count events by status
    async fn count_by_status(&self, status: EventStatus) -> usize;

    /// Update event status
    async fn update_status(&self, event_id: &EventId, new_status: EventStatus);

    // =========================================================================
    // Batch operations
    // =========================================================================

    /// Batch get events (for network sync)
    async fn get_events_batch(&self, event_ids: &[EventId]) -> Vec<Event>;

    // =========================================================================
    // Recovery operations (optional - default no-op for in-memory)
    // =========================================================================

    /// Get events by depth range (for recovery/warmup)
    /// Returns events with depth >= min_depth and depth <= max_depth
    async fn get_events_by_depth_range(
        &self,
        _min_depth: u64,
        _max_depth: u64,
    ) -> SetuResult<Vec<(Event, u64)>> {
        // Default: not supported (in-memory doesn't persist across restarts)
        Ok(vec![])
    }

    /// Get the maximum depth stored
    async fn get_max_depth(&self) -> Option<u64> {
        // Default: not supported
        None
    }
}

// ============================================================================
// Implement trait for in-memory EventStore
// ============================================================================

use crate::memory::EventStore;

#[async_trait]
impl EventStoreBackend for EventStore {
    async fn store(&self, event: Event) -> SetuResult<()> {
        EventStore::store(self, event).await
    }

    async fn get(&self, event_id: &EventId) -> Option<Event> {
        EventStore::get(self, event_id).await
    }

    async fn get_many(&self, event_ids: &[EventId]) -> Vec<Event> {
        EventStore::get_many(self, event_ids).await
    }

    async fn exists(&self, event_id: &EventId) -> bool {
        EventStore::exists(self, event_id).await
    }

    async fn exists_many(&self, event_ids: &[EventId]) -> Vec<bool> {
        EventStore::exists_many(self, event_ids).await
    }

    async fn count(&self) -> usize {
        EventStore::count(self).await
    }

    async fn store_with_depth(&self, event: Event, depth: u64) -> SetuResult<()> {
        EventStore::store_with_depth(self, event, depth).await
    }

    async fn store_batch_with_depth(
        &self,
        events_with_depths: Vec<(Event, u64)>,
    ) -> BatchStoreResult {
        EventStore::store_batch_with_depth(self, events_with_depths).await
    }

    async fn get_depth(&self, event_id: &EventId) -> Option<u64> {
        EventStore::get_depth(self, event_id).await
    }

    async fn get_depths_batch(&self, event_ids: &[EventId]) -> HashMap<EventId, u64> {
        EventStore::get_depths_batch(self, event_ids).await
    }

    async fn get_parent_ids(&self, event_id: &EventId) -> Option<Vec<EventId>> {
        EventStore::get_parent_ids(self, event_id).await
    }

    async fn get_by_creator(&self, creator: &str) -> Vec<Event> {
        EventStore::get_by_creator(self, creator).await
    }

    async fn get_by_status(&self, status: EventStatus) -> Vec<Event> {
        EventStore::get_by_status(self, status).await
    }

    async fn count_by_status(&self, status: EventStatus) -> usize {
        EventStore::count_by_status(self, status).await
    }

    async fn update_status(&self, event_id: &EventId, new_status: EventStatus) {
        EventStore::update_status(self, event_id, new_status).await
    }

    async fn get_events_batch(&self, event_ids: &[EventId]) -> Vec<Event> {
        EventStore::get_events_batch(self, event_ids).await
    }
}
