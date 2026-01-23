// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Recent Event Cache Module
//!
//! This module provides a LRU cache for recently finalized events.
//! It stores lightweight metadata to support cross-CF parent resolution
//! without keeping full events in memory.

use lru::LruCache;
use setu_types::{AnchorId, EventId};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Lightweight metadata for finalized events
///
/// This structure stores only the essential information needed for
/// parent resolution, saving ~70% memory compared to full Event storage.
///
/// Memory estimation (with String-based EventId):
/// - depth: 8 bytes
/// - anchor_id: ~88 bytes (String, 64 char hex)
/// - finalized_at: 8 bytes
/// - parent_ids: 24 bytes (Vec header) + N * 88 bytes
/// Total: ~300 bytes for 2 parents (vs ~500 bytes for full Event)
#[derive(Debug, Clone)]
pub struct FinalizedEventMeta {
    /// Event depth in DAG (distance from genesis)
    pub depth: u64,
    
    /// The anchor that finalized this event
    pub anchor_id: AnchorId,
    
    /// Timestamp when event was finalized
    pub finalized_at: u64,
    
    /// Parent event IDs (needed for causality analysis)
    pub parent_ids: Vec<EventId>,
}

impl FinalizedEventMeta {
    /// Create new finalized event metadata
    pub fn new(
        depth: u64,
        anchor_id: AnchorId,
        finalized_at: u64,
        parent_ids: Vec<EventId>,
    ) -> Self {
        Self {
            depth,
            anchor_id,
            finalized_at,
            parent_ids,
        }
    }
}

/// Statistics for the recent event cache
#[derive(Debug, Default)]
pub struct CacheStats {
    /// Number of cache hits
    pub hits: AtomicU64,
    /// Number of cache misses
    pub misses: AtomicU64,
    /// Number of LRU evictions
    pub evictions: AtomicU64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Calculate hit rate as percentage
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            (hits as f64 / total as f64) * 100.0
        }
    }
    
    /// Get current statistics as a snapshot
    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

/// Immutable snapshot of cache statistics
#[derive(Debug, Clone)]
pub struct CacheStatsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl CacheStatsSnapshot {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64 / total as f64) * 100.0
        }
    }
}

/// LRU Cache for recently finalized events
///
/// This cache stores metadata for recently finalized events to support
/// efficient cross-CF parent resolution without hitting persistent storage.
///
/// Layer 2 in the three-layer storage architecture:
/// - Layer 1: Active DAG (hot data, pending events)
/// - Layer 2: Recent Cache (warm data, finalized metadata) <- this
/// - Layer 3: Event Store (cold data, persistent full events)
pub struct RecentEventCache {
    /// The underlying LRU cache
    cache: LruCache<EventId, FinalizedEventMeta>,
    
    /// Cache capacity
    capacity: usize,
    
    /// Statistics
    stats: CacheStats,
}

impl RecentEventCache {
    /// Create a new cache with the specified capacity
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of entries (recommended: 10,000 - 20,000)
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).expect("capacity must be non-zero");
        Self {
            cache: LruCache::new(cap),
            capacity,
            stats: CacheStats::new(),
        }
    }
    
    /// Insert or update an entry in the cache
    ///
    /// If the cache is at capacity and new key is inserted, the least recently used entry will be evicted.
    /// Returns the evicted entry if any.
    pub fn put(&mut self, event_id: EventId, meta: FinalizedEventMeta) -> Option<(EventId, FinalizedEventMeta)> {
        // If updating existing key, no need to evict
        if self.cache.contains(&event_id) {
            self.cache.put(event_id, meta);
            return None;
        }

        let evicted = if self.cache.len() >= self.capacity {
            // Pop the LRU entry before inserting new one
            self.cache.pop_lru()
        } else {
            None
        };
        
        if evicted.is_some() {
            self.stats.record_eviction();
        }
        
        self.cache.put(event_id, meta);
        evicted
    }
    
    /// Get an entry from the cache, updating its recency
    ///
    /// This moves the entry to the "most recently used" position.
    pub fn get(&mut self, event_id: &EventId) -> Option<&FinalizedEventMeta> {
        let result = self.cache.get(event_id);
        if result.is_some() {
            self.stats.record_hit();
        } else {
            self.stats.record_miss();
        }
        result
    }
    
    /// Check if an entry exists in the cache without updating recency
    pub fn contains(&self, event_id: &EventId) -> bool {
        self.cache.contains(event_id)
    }
    
    /// Peek at an entry without updating its recency
    pub fn peek(&self, event_id: &EventId) -> Option<&FinalizedEventMeta> {
        self.cache.peek(event_id)
    }
    
    /// Get the current number of entries in the cache
    pub fn len(&self) -> usize {
        self.cache.len()
    }
    
    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
    
    /// Get the cache capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }
    
    /// Get a snapshot of cache statistics
    pub fn stats_snapshot(&self) -> CacheStatsSnapshot {
        self.stats.snapshot()
    }
    
    /// Clear all entries from the cache
    pub fn clear(&mut self) {
        self.cache.clear();
    }
    
    /// Iterate over all entries (most recent first)
    pub fn iter(&self) -> impl Iterator<Item = (&EventId, &FinalizedEventMeta)> {
        self.cache.iter()
    }
}

impl std::fmt::Debug for RecentEventCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecentEventCache")
            .field("len", &self.cache.len())
            .field("capacity", &self.capacity)
            .field("stats", &self.stats.snapshot())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_meta(depth: u64) -> FinalizedEventMeta {
        FinalizedEventMeta::new(
            depth,
            format!("anchor_{}", depth),
            1000 + depth,
            vec![format!("parent_{}", depth)],
        )
    }

    #[test]
    fn test_cache_basic_operations() {
        let mut cache = RecentEventCache::new(10);
        
        // Insert
        cache.put("event1".to_string(), create_meta(1));
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&"event1".to_string()));
        
        // Get
        let meta = cache.get(&"event1".to_string());
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().depth, 1);
        
        // Miss
        let missing = cache.get(&"nonexistent".to_string());
        assert!(missing.is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = RecentEventCache::new(3);
        
        // Fill cache
        cache.put("event1".to_string(), create_meta(1));
        cache.put("event2".to_string(), create_meta(2));
        cache.put("event3".to_string(), create_meta(3));
        assert_eq!(cache.len(), 3);
        
        // Access event1 to make it most recently used
        cache.get(&"event1".to_string());
        
        // Insert event4, should evict event2 (LRU)
        let evicted = cache.put("event4".to_string(), create_meta(4));
        assert!(evicted.is_some());
        let (evicted_id, _) = evicted.unwrap();
        assert_eq!(evicted_id, "event2".to_string());
        
        // event2 should be gone
        assert!(!cache.contains(&"event2".to_string()));
        // event1 should still be there
        assert!(cache.contains(&"event1".to_string()));
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = RecentEventCache::new(10);
        
        cache.put("event1".to_string(), create_meta(1));
        
        // Hit
        cache.get(&"event1".to_string());
        // Miss
        cache.get(&"nonexistent".to_string());
        
        let stats = cache.stats_snapshot();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate(), 50.0);
    }

    #[test]
    fn test_cache_peek_does_not_update_lru() {
        let mut cache = RecentEventCache::new(3);
        
        cache.put("event1".to_string(), create_meta(1));
        cache.put("event2".to_string(), create_meta(2));
        cache.put("event3".to_string(), create_meta(3));
        
        // Peek at event1 (should not update LRU order)
        let _ = cache.peek(&"event1".to_string());
        
        // Insert event4, should evict event1 (still LRU because peek doesn't update)
        let evicted = cache.put("event4".to_string(), create_meta(4));
        assert!(evicted.is_some());
        let (evicted_id, _) = evicted.unwrap();
        assert_eq!(evicted_id, "event1".to_string());
    }

    #[test]
    fn test_cache_clear() {
        let mut cache = RecentEventCache::new(10);
        
        cache.put("event1".to_string(), create_meta(1));
        cache.put("event2".to_string(), create_meta(2));
        assert_eq!(cache.len(), 2);
        
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }
}
