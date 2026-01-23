// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! DAG Manager Module
//!
//! This module provides the DagManager which encapsulates the three-layer storage
//! architecture for efficient DAG event management with automatic GC.
//!
//! Three-Layer Architecture:
//! - Layer 1: Active DAG (hot data, pending events)
//! - Layer 2: Recent Cache (warm data, finalized metadata)
//! - Layer 3: Event Store (cold data, persistent full events)

use crate::dag::{Dag, DagError};
use crate::recent_cache::{FinalizedEventMeta, RecentEventCache, CacheStatsSnapshot};
use setu_storage::EventStore;
use setu_types::{Anchor, AnchorId, Event, EventId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

/// Configuration for DagManager
#[derive(Debug, Clone)]
pub struct DagManagerConfig {
    /// Recent Cache capacity (default: 15,000)
    pub recent_cache_capacity: usize,
    
    /// Maximum allowed cross-CF depth difference (default: 200)
    /// Events referencing parents older than this will be rejected
    pub max_cross_cf_depth: u64,
    
    /// Whether to enable disk fallback queries (default: true)
    /// When disabled, cache miss returns NotFound directly
    pub enable_disk_fallback: bool,
    
    /// Number of recent anchors to load during warmup (default: 10)
    pub warmup_anchor_count: usize,
    
    /// Maximum size of pending queue during warmup (default: 10,000)
    pub max_pending_queue_size: usize,
}

impl Default for DagManagerConfig {
    fn default() -> Self {
        Self {
            recent_cache_capacity: 15_000,
            max_cross_cf_depth: 200,
            enable_disk_fallback: true,
            warmup_anchor_count: 10,
            max_pending_queue_size: 10_000,
        }
    }
}

/// Information about where a parent was found
#[derive(Debug, Clone)]
pub enum ParentInfo {
    /// Found in Active DAG
    InDag { depth: u64 },
    
    /// Found in Recent Cache
    InCache { depth: u64, anchor_id: AnchorId },
    
    /// Found in Event Store (fallback query)
    InStore { depth: u64 },
}

/// Result of resolving all parents for an event
#[derive(Debug)]
pub struct ResolvedParents {
    /// Information about each parent (parallel to event.parent_ids)
    pub results: Vec<(EventId, ParentInfo)>,
    
    /// Calculated depth for the new event
    pub new_event_depth: u64,
}

/// Statistics from a GC operation
#[derive(Debug, Default, Clone)]
pub struct GcStats {
    /// Number of events removed from DAG
    pub removed: usize,
    
    /// Number of events retained (have active children)
    pub retained: usize,
}

/// Statistics from cache warmup
#[derive(Debug, Clone)]
pub struct WarmupStats {
    /// Number of events loaded into cache
    pub events_loaded: usize,
    
    /// Duration of warmup in milliseconds
    pub duration_ms: u128,
}

/// Errors that can occur in DagManager operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum DagManagerError {
    #[error("Missing parent event: {0}")]
    MissingParent(EventId),
    
    #[error("Parent event too old: {parent_id}, depth_diff: {depth_diff} > max: {max_allowed}")]
    ParentTooOld {
        parent_id: EventId,
        depth_diff: u64,
        max_allowed: u64,
    },
    
    #[error("Parent was GC'd between validation and insertion: {0}")]
    ParentGCed(EventId),
    
    #[error("Duplicate event: {0}")]
    DuplicateEvent(EventId),
    
    #[error("Warmup pending queue is full (max: {max_size})")]
    WarmupQueueFull { max_size: usize },
    
    #[error("DAG error: {0}")]
    Dag(#[from] DagError),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Maximum retry count for TOCTOU issues
const MAX_RETRY: usize = 3;

/// DagManager: Three-layer storage manager
///
/// Responsibilities:
/// 1. Encapsulate three-layer query logic (DAG → Cache → Store)
/// 2. Manage GC triggering and execution
/// 3. Handle finalization events
///
/// # Thread Safety
/// - Active DAG: Arc<RwLock<Dag>>
/// - Recent Cache: Arc<Mutex<RecentEventCache>>
/// - Event Store: Arc<EventStore> (internally synchronized)
pub struct DagManager {
    /// Active DAG - stores pending events
    dag: Arc<RwLock<Dag>>,
    
    /// Recent Cache - LRU cache for finalized event metadata
    recent_cache: Arc<Mutex<RecentEventCache>>,
    
    /// Event Store - persistent storage (accessed via reference)
    event_store: Arc<EventStore>,
    
    /// Configuration
    config: DagManagerConfig,
    
    /// Whether warmup is in progress
    warming_up: AtomicBool,
    
    /// Events queued during warmup
    pending_queue: Mutex<Vec<Event>>,
}

impl DagManager {
    /// Create a new DagManager
    pub fn new(
        dag: Arc<RwLock<Dag>>,
        event_store: Arc<EventStore>,
        config: DagManagerConfig,
    ) -> Self {
        let recent_cache = Arc::new(Mutex::new(
            RecentEventCache::new(config.recent_cache_capacity)
        ));
        
        Self {
            dag,
            recent_cache,
            event_store,
            config,
            warming_up: AtomicBool::new(false),
            pending_queue: Mutex::new(Vec::new()),
        }
    }
    
    /// Create with default configuration
    pub fn with_defaults(dag: Arc<RwLock<Dag>>, event_store: Arc<EventStore>) -> Self {
        Self::new(dag, event_store, DagManagerConfig::default())
    }
    
    /// Get reference to the active DAG
    pub fn dag(&self) -> &Arc<RwLock<Dag>> {
        &self.dag
    }
    
    /// Get reference to the recent cache
    pub fn recent_cache(&self) -> &Arc<Mutex<RecentEventCache>> {
        &self.recent_cache
    }
    
    /// Get reference to the event store
    pub fn event_store(&self) -> &Arc<EventStore> {
        &self.event_store
    }
    
    /// Get configuration
    pub fn config(&self) -> &DagManagerConfig {
        &self.config
    }
    
    // =========================================================================
    // Three-Layer Query Methods
    // =========================================================================
    
    /// Check if an event exists in any layer (DAG → Cache → Store)
    pub async fn exists(&self, event_id: &EventId) -> bool {
        // Layer 1: Check DAG
        {
            let dag = self.dag.read().await;
            if dag.contains(event_id) {
                return true;
            }
        }
        
        // Layer 2: Check Cache
        {
            let cache = self.recent_cache.lock().await;
            if cache.contains(event_id) {
                return true;
            }
        }
        
        // Layer 3: Check Store (if enabled)
        if self.config.enable_disk_fallback {
            return self.event_store.exists(event_id).await;
        }
        
        false
    }
    
    /// Query single parent info (used in Phase 1 of resolve_parents)
    async fn query_parent(&self, parent_id: &EventId) -> Option<ParentInfo> {
        // Step 1: Check Active DAG
        {
            let dag = self.dag.read().await;
            if let Some(depth) = dag.get_depth(parent_id) {
                return Some(ParentInfo::InDag { depth });
            }
        }
        
        // Step 2: Check Recent Cache
        {
            let mut cache = self.recent_cache.lock().await;
            if let Some(meta) = cache.get(parent_id) {
                return Some(ParentInfo::InCache {
                    depth: meta.depth,
                    anchor_id: meta.anchor_id.clone(),
                });
            }
        }
        
        // Step 3: Fallback to Event Store
        if self.config.enable_disk_fallback {
            if let Some(depth) = self.event_store.get_depth(parent_id).await {
                return Some(ParentInfo::InStore { depth });
            }
        }
        
        None
    }
    
    /// Resolve all parents for an event (Two-pass scan)
    ///
    /// Phase 1: Collect all parent depths
    /// Phase 2: Calculate new_event_depth and check depth_diff
    pub async fn resolve_parents(&self, event: &Event) -> Result<ResolvedParents, DagManagerError> {
        let mut parent_results = Vec::with_capacity(event.parent_ids.len());
        let mut max_parent_depth: u64 = 0;
        
        // Phase 1: Collect all parent info (no depth_diff check yet)
        for parent_id in &event.parent_ids {
            match self.query_parent(parent_id).await {
                Some(info) => {
                    let depth = match &info {
                        ParentInfo::InDag { depth } => *depth,
                        ParentInfo::InCache { depth, .. } => *depth,
                        ParentInfo::InStore { depth } => *depth,
                    };
                    max_parent_depth = max_parent_depth.max(depth);
                    parent_results.push((parent_id.clone(), info));
                }
                None => {
                    return Err(DagManagerError::MissingParent(parent_id.clone()));
                }
            }
        }
        
        // Phase 2: Calculate new_event_depth and check depth_diff
        let new_event_depth = if parent_results.is_empty() {
            0 // Genesis event
        } else {
            max_parent_depth + 1
        };
        
        // Check depth_diff for Cache/Store parents
        for (parent_id, info) in &parent_results {
            match info {
                ParentInfo::InDag { .. } => {
                    // DAG parents don't need depth_diff check
                    continue;
                }
                ParentInfo::InCache { depth, .. } | ParentInfo::InStore { depth } => {
                    let depth_diff = new_event_depth.saturating_sub(*depth);
                    if depth_diff > self.config.max_cross_cf_depth {
                        return Err(DagManagerError::ParentTooOld {
                            parent_id: parent_id.clone(),
                            depth_diff,
                            max_allowed: self.config.max_cross_cf_depth,
                        });
                    }
                }
            }
        }
        
        Ok(ResolvedParents {
            results: parent_results,
            new_event_depth,
        })
    }
    
    // =========================================================================
    // Event Addition (Three-Phase Commit)
    // =========================================================================
    
    /// Add an event to the DAG with automatic parent resolution
    ///
    /// This is the ONLY entry point for adding events to the DAG.
    /// All events must go through this method to ensure depth is correctly calculated.
    pub async fn add_event(&self, event: Event) -> Result<EventId, DagManagerError> {
        let event_id = event.id.clone();
        
        // Handle warmup period
        if self.warming_up.load(Ordering::Acquire) {
            match self.resolve_parents(&event).await {
                Ok(_) => { /* Continue normal flow */ }
                Err(DagManagerError::MissingParent(_)) => {
                    // During warmup, queue the event for later processing
                    let mut queue = self.pending_queue.lock().await;
                    if queue.len() >= self.config.max_pending_queue_size {
                        return Err(DagManagerError::WarmupQueueFull {
                            max_size: self.config.max_pending_queue_size,
                        });
                    }
                    queue.push(event);
                    debug!("Event {} queued during warmup", event_id);
                    return Ok(event_id);
                }
                Err(e) => return Err(e),
            }
        }
        
        // Phase 1: Resolve all parents
        let resolved = self.resolve_parents(&event).await?;
        
        // Phase 2: depth_diff check already done in resolve_parents
        
        // Phase 3: Write to DAG (acquire write lock, use Phase 1 results)
        {
            let mut dag = self.dag.write().await;
            
            // Check InDag parents still exist (TOCTOU protection)
            for (parent_id, info) in &resolved.results {
                if matches!(info, ParentInfo::InDag { .. }) {
                    if !dag.contains(parent_id) {
                        // Parent was GC'd between Phase 1 and Phase 3
                        return Err(DagManagerError::ParentGCed(parent_id.clone()));
                    }
                }
                // InCache/InStore parents don't need re-check:
                // - Cache entries are not deleted during finalize
                // - Store data is persistent
            }
            
            // Use add_event_with_depth to skip parent existence check
            dag.add_event_with_depth(event, resolved.new_event_depth)?;
        }
        
        Ok(event_id)
    }
    
    /// Add event with automatic retry on TOCTOU errors
    pub async fn add_event_with_retry(&self, event: Event) -> Result<EventId, DagManagerError> {
        let event_id = event.id.clone();
        
        for attempt in 0..MAX_RETRY {
            match self.add_event(event.clone()).await {
                Ok(id) => return Ok(id),
                Err(DagManagerError::ParentGCed(parent_id)) if attempt < MAX_RETRY - 1 => {
                    debug!(
                        "Parent {} was GC'd during add_event for {}, retrying (attempt {})",
                        parent_id, event_id, attempt + 1
                    );
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        
        Err(DagManagerError::Internal(format!(
            "Max retry ({}) exceeded for event {}",
            MAX_RETRY, event_id
        )))
    }
    
    // =========================================================================
    // Finalization and GC
    // =========================================================================
    
    /// Handle anchor finalization - triggers GC
    ///
    /// MUST be called AFTER persist_finalized_anchor() completes.
    ///
    /// Execution order (per design doc 4.3):
    /// 1. Update event status to Finalized (critical for GC)
    /// 2. Collect event info from DAG (depth, parent_ids)
    /// 3. Create FinalizedEventMeta
    /// 4. Insert into Recent Cache
    /// 5. Remove from DAG (only events without active children)
    pub async fn on_anchor_finalized(&self, anchor: &Anchor) -> Result<GcStats, DagManagerError> {
        let event_ids: Vec<EventId> = anchor.event_ids.clone();
        
        // Step 0: Update event status to Finalized (GC prerequisite!)
        // has_active_children() depends on status == Finalized
        {
            let mut dag = self.dag.write().await;
            dag.finalize_events(&event_ids);
        }
        
        // Steps 1-3: Collect metadata and insert into Cache
        // Note: Events temporarily exist in both Cache and DAG (acceptable transient state)
        {
            let dag = self.dag.read().await;
            let mut cache = self.recent_cache.lock().await;
            
            for event_id in &event_ids {
                if let Some(event) = dag.get_event(event_id) {
                    // P1 Critical: depth MUST exist and be valid
                    // All events must enter DAG through DagManager.add_event()
                    let depth = dag.get_depth(event_id).expect(
                        "BUG: event in DAG must have depth, \
                         ensure all events enter through DagManager.add_event()"
                    );
                    
                    let meta = FinalizedEventMeta::new(
                        depth,
                        anchor.id.clone(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        event.parent_ids.clone(),
                    );
                    
                    // LRU Cache automatically evicts old entries
                    cache.put(event_id.clone(), meta);
                }
            }
        }
        
        // Step 4: Try to remove from DAG (only events without active children)
        let stats = {
            let mut dag = self.dag.write().await;
            dag.gc_finalized_events(&event_ids)
        };
        
        debug!(
            "GC completed for anchor {}: removed={}, retained={} (have active children)",
            anchor.id, stats.removed, stats.retained
        );
        
        Ok(GcStats {
            removed: stats.removed,
            retained: stats.retained,
        })
    }
    
    // =========================================================================
    // Warmup
    // =========================================================================
    
    /// Start warmup mode
    pub fn start_warmup(&self) {
        self.warming_up.store(true, Ordering::Release);
    }
    
    /// Complete warmup and process queued events
    pub async fn complete_warmup(&self) -> Vec<DagManagerError> {
        self.warming_up.store(false, Ordering::Release);
        
        // Process queued events
        let pending = std::mem::take(&mut *self.pending_queue.lock().await);
        let mut errors = Vec::new();
        
        for event in pending {
            if let Err(e) = self.add_event(event).await {
                warn!("Failed to process queued event: {:?}", e);
                errors.push(e);
            }
        }
        
        errors
    }
    
    /// Check if warmup is in progress
    pub fn is_warming_up(&self) -> bool {
        self.warming_up.load(Ordering::Acquire)
    }
    
    /// Load events from recent anchors into cache (warmup)
    ///
    /// This should be called during node startup to populate the cache
    /// with recently finalized events for efficient cross-CF resolution.
    pub async fn warmup_from_anchors(&self, anchors: &[Anchor]) -> WarmupStats {
        let start = std::time::Instant::now();
        let mut loaded = 0;
        
        let mut cache = self.recent_cache.lock().await;
        
        for anchor in anchors {
            for event_id in &anchor.event_ids {
                // Get depth and parent_ids from store
                let depth = self.event_store.get_depth(event_id).await;
                let parent_ids = self.event_store.get_parent_ids(event_id).await;
                
                if let (Some(depth), Some(parent_ids)) = (depth, parent_ids) {
                    let meta = FinalizedEventMeta::new(
                        depth,
                        anchor.id.clone(),
                        anchor.timestamp,
                        parent_ids,
                    );
                    cache.put(event_id.clone(), meta);
                    loaded += 1;
                }
            }
        }
        
        WarmupStats {
            events_loaded: loaded,
            duration_ms: start.elapsed().as_millis(),
        }
    }
    
    // =========================================================================
    // Statistics
    // =========================================================================
    
    /// Get cache statistics
    pub async fn cache_stats(&self) -> CacheStatsSnapshot {
        self.recent_cache.lock().await.stats_snapshot()
    }
    
    /// Get current cache size
    pub async fn cache_size(&self) -> usize {
        self.recent_cache.lock().await.len()
    }
    
    /// Get DAG statistics
    pub async fn dag_stats(&self) -> DagStatsSnapshot {
        let dag = self.dag.read().await;
        DagStatsSnapshot {
            event_count: dag.node_count(),
            pending_count: dag.get_pending_count(),
            tip_count: dag.get_tips().len(),
            max_depth: dag.max_depth(),
        }
    }
}

/// Snapshot of DAG statistics
#[derive(Debug, Clone)]
pub struct DagStatsSnapshot {
    pub event_count: usize,
    pub pending_count: usize,
    pub tip_count: usize,
    pub max_depth: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{EventType, VectorClock, VLCSnapshot};

    fn create_event(id: &str, parents: Vec<&str>, creator: &str) -> Event {
        let parent_ids: Vec<EventId> = parents.iter().map(|s| s.to_string()).collect();
        let mut event = Event::new(
            EventType::Transfer,
            parent_ids,
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 1,
                physical_time: 1000,
            },
            creator.to_string(),
        );
        event.id = id.to_string();
        event
    }

    async fn create_manager() -> DagManager {
        let dag = Arc::new(RwLock::new(Dag::new()));
        let event_store = Arc::new(EventStore::new());
        DagManager::with_defaults(dag, event_store)
    }

    #[tokio::test]
    async fn test_add_genesis_event() {
        let manager = create_manager().await;
        
        let genesis = create_event("genesis", vec![], "node1");
        let result = manager.add_event(genesis).await;
        
        assert!(result.is_ok());
        
        let dag = manager.dag.read().await;
        assert_eq!(dag.node_count(), 1);
        assert_eq!(dag.get_depth(&"genesis".to_string()), Some(0));
    }

    #[tokio::test]
    async fn test_add_event_with_parent() {
        let manager = create_manager().await;
        
        // Add genesis
        let genesis = create_event("genesis", vec![], "node1");
        manager.add_event(genesis).await.unwrap();
        
        // Add child
        let child = create_event("child", vec!["genesis"], "node1");
        manager.add_event(child).await.unwrap();
        
        let dag = manager.dag.read().await;
        assert_eq!(dag.node_count(), 2);
        assert_eq!(dag.get_depth(&"child".to_string()), Some(1));
    }

    #[tokio::test]
    async fn test_missing_parent_error() {
        let manager = create_manager().await;
        
        let event = create_event("orphan", vec!["nonexistent"], "node1");
        let result = manager.add_event(event).await;
        
        assert!(matches!(result, Err(DagManagerError::MissingParent(_))));
    }

    #[tokio::test]
    async fn test_exists_across_layers() {
        let manager = create_manager().await;
        
        // Add event to DAG
        let genesis = create_event("genesis", vec![], "node1");
        manager.add_event(genesis).await.unwrap();
        
        // Should exist in DAG
        assert!(manager.exists(&"genesis".to_string()).await);
        
        // Should not exist
        assert!(!manager.exists(&"nonexistent".to_string()).await);
    }

    #[tokio::test]
    async fn test_config_defaults() {
        let config = DagManagerConfig::default();
        
        assert_eq!(config.recent_cache_capacity, 15_000);
        assert_eq!(config.max_cross_cf_depth, 200);
        assert!(config.enable_disk_fallback);
        assert_eq!(config.warmup_anchor_count, 10);
        assert_eq!(config.max_pending_queue_size, 10_000);
    }
}
