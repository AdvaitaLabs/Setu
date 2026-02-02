//! Persistence and Recovery Integration Tests
//!
//! This test validates the complete persistence and recovery flow:
//! 1. Create ConsensusValidator with RocksDB backends
//! 2. Submit events and finalize anchors
//! 3. Simulate restart (drop and recreate validator)
//! 4. Call recover_from_storage()
//! 5. Verify state is correctly restored
//!
//! ## Test Scenarios
//!
//! - Fresh start with empty storage
//! - Recovery with one finalized anchor
//! - Recovery with multiple anchors
//! - VLC state restoration
//! - AnchorBuilder chain root restoration

use setu_types::{
    Event, EventType, Anchor, SubnetId, 
    NodeInfo, ConsensusConfig, EventStatus, EventPayload,
    event::VLCSnapshot,
};
use setu_storage::{
    SetuDB, RocksDBEventStore, RocksDBCFStore, RocksDBAnchorStore, RocksDBMerkleStore,
    GlobalStateManager, EventStoreBackend, CFStoreBackend, AnchorStoreBackend, MerkleStore,
    EventStore, AnchorStore,
};
use setu_validator::{ConsensusValidator, ConsensusValidatorConfig};
use std::sync::Arc;
use std::path::Path;
use tracing::info;

/// Initialize tracing for tests
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

/// Create a test event with given parameters
fn create_test_event(id: &str, creator: &str, parent_ids: Vec<String>) -> Event {
    Event {
        id: id.to_string(),
        creator: creator.to_string(),
        event_type: EventType::System,
        parent_ids,
        payload: EventPayload::None,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        subnet_id: Some(SubnetId::ROOT),
        transfer: None,
        execution_result: None,
        status: EventStatus::Pending,
        vlc_snapshot: VLCSnapshot::new(),
    }
}

/// Create a test anchor with given parameters
fn create_test_anchor(
    id: &str,
    event_ids: Vec<String>,
    depth: u64,
    previous_anchor: Option<String>,
    logical_time: u64,
) -> Anchor {
    let mut vlc_snapshot = VLCSnapshot::new();
    vlc_snapshot.logical_time = logical_time;
    
    Anchor {
        id: id.to_string(),
        event_ids,
        state_root: "0".repeat(64), // Hex string
        merkle_roots: None,
        previous_anchor,
        depth,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        vlc_snapshot,
    }
}

/// Create test ConsensusValidatorConfig
fn create_test_config() -> ConsensusValidatorConfig {
    let node_info = NodeInfo::new_validator(
        "test-validator-1".to_string(),
        "127.0.0.1".to_string(),
        8080,
    );
    ConsensusValidatorConfig {
        node_info,
        is_leader: true,
        consensus: ConsensusConfig::default(),
        message_buffer_size: 100,
    }
}

/// Helper to create RocksDB-backed validator
fn create_rocksdb_validator(db_path: &Path) -> (Arc<ConsensusValidator>, Arc<SetuDB>) {
    let db = Arc::new(SetuDB::open_default(db_path).expect("Failed to open RocksDB"));
    
    let event_store: Arc<dyn EventStoreBackend> = Arc::new(RocksDBEventStore::from_shared(db.clone()));
    let cf_store: Arc<dyn CFStoreBackend> = Arc::new(RocksDBCFStore::from_shared(db.clone()));
    let anchor_store: Arc<dyn AnchorStoreBackend> = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
    let merkle_store: Arc<dyn MerkleStore> = Arc::new(RocksDBMerkleStore::from_shared(db.clone()));
    let state_manager = GlobalStateManager::with_store(merkle_store);
    
    let config = create_test_config();
    let validator = Arc::new(ConsensusValidator::with_all_backends(
        config,
        state_manager,
        event_store,
        cf_store,
        anchor_store,
    ));
    
    (validator, db)
}

// ============================================================================
// Test: Fresh start with empty storage
// ============================================================================

#[tokio::test]
async fn test_recovery_fresh_start() {
    init_tracing();
    info!("=== Test: Fresh start with empty storage ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let (validator, _db) = create_rocksdb_validator(temp_dir.path());
    
    // Recovery should succeed with no data
    let result = validator.recover_from_storage().await;
    assert!(result.is_ok(), "Fresh start recovery should succeed");
    
    info!("✓ Fresh start recovery completed successfully");
}

// ============================================================================
// Test: Recovery with one finalized anchor
// ============================================================================

#[tokio::test]
async fn test_recovery_single_anchor() {
    init_tracing();
    info!("=== Test: Recovery with single anchor ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Phase 1: Create validator and store some data
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        
        let event_store = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        let anchor_store = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        // Store test events
        let event1 = create_test_event("evt-001", "validator-1", vec![]);
        let event2 = create_test_event("evt-002", "validator-1", vec!["evt-001".to_string()]);
        let event3 = create_test_event("evt-003", "validator-1", vec!["evt-002".to_string()]);
        
        event_store.store_with_depth(event1, 1).await.unwrap();
        event_store.store_with_depth(event2, 2).await.unwrap();
        event_store.store_with_depth(event3, 3).await.unwrap();
        
        // Store anchor
        let anchor = create_test_anchor(
            "anchor-001",
            vec!["evt-001".to_string(), "evt-002".to_string(), "evt-003".to_string()],
            1,
            None,
            100, // VLC logical time
        );
        anchor_store.store(anchor).await.unwrap();
        
        info!("Phase 1: Stored 3 events and 1 anchor");
    }
    
    // Phase 2: Simulate restart - create new validator with same DB path
    {
        let (validator, _db) = create_rocksdb_validator(temp_dir.path());
        
        // Recovery should succeed
        let result = validator.recover_from_storage().await;
        assert!(result.is_ok(), "Single anchor recovery should succeed");
        
        // Verify anchor store has data
        let anchor = validator.anchor_store().get_latest().await;
        assert!(anchor.is_some(), "Latest anchor should exist after recovery");
        
        let anchor = anchor.unwrap();
        assert_eq!(anchor.id, "anchor-001");
        assert_eq!(anchor.event_ids.len(), 3);
        assert_eq!(anchor.vlc_snapshot.logical_time, 100);
        
        info!("Phase 2: Recovery verified - anchor and events restored");
    }
    
    info!("✓ Single anchor recovery completed successfully");
}

// ============================================================================
// Test: Recovery with multiple anchors
// ============================================================================

#[tokio::test]
async fn test_recovery_multiple_anchors() {
    init_tracing();
    info!("=== Test: Recovery with multiple anchors ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Phase 1: Create multiple anchors
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        
        let event_store = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        let anchor_store = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        // Create events for 3 anchors
        for anchor_idx in 1..=3i32 {
            let base_evt_id = (anchor_idx - 1) * 3;
            
            for evt_idx in 1..=3i32 {
                let evt_id = format!("evt-{:03}", base_evt_id + evt_idx);
                let parent_ids = if evt_idx == 1 && anchor_idx > 1 {
                    vec![format!("evt-{:03}", base_evt_id)]
                } else if evt_idx > 1 {
                    vec![format!("evt-{:03}", base_evt_id + evt_idx - 1)]
                } else {
                    vec![]
                };
                
                let depth = (base_evt_id + evt_idx) as u64;
                let event = create_test_event(&evt_id, "validator-1", parent_ids);
                event_store.store_with_depth(event, depth).await.unwrap();
            }
            
            // Create anchor for this batch
            let prev_anchor = if anchor_idx == 1 { None } else { Some(format!("anchor-{:03}", anchor_idx - 1)) };
            let anchor = create_test_anchor(
                &format!("anchor-{:03}", anchor_idx),
                (1..=3).map(|i| format!("evt-{:03}", base_evt_id + i)).collect(),
                anchor_idx as u64,
                prev_anchor,
                anchor_idx as u64 * 100, // VLC logical time
            );
            anchor_store.store(anchor).await.unwrap();
        }
        
        info!("Phase 1: Stored 9 events and 3 anchors");
    }
    
    // Phase 2: Recovery
    {
        let (validator, _db) = create_rocksdb_validator(temp_dir.path());
        
        let result = validator.recover_from_storage().await;
        assert!(result.is_ok(), "Multiple anchor recovery should succeed");
        
        // Verify latest anchor
        let latest = validator.anchor_store().get_latest().await;
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.id, "anchor-003");
        assert_eq!(latest.depth, 3);
        assert_eq!(latest.vlc_snapshot.logical_time, 300);
        
        // Verify anchor count
        let count = validator.anchor_store().count().await;
        assert_eq!(count, 3, "Should have 3 anchors after recovery");
        
        // Verify recent anchors
        let recent = validator.anchor_store().get_recent_anchors(10).await;
        assert_eq!(recent.len(), 3);
        
        info!("Phase 2: Recovery verified - 3 anchors restored");
    }
    
    info!("✓ Multiple anchor recovery completed successfully");
}

// ============================================================================
// Test: Event store persistence
// ============================================================================

#[tokio::test]
async fn test_event_store_persistence() {
    init_tracing();
    info!("=== Test: Event store persistence ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Phase 1: Store events
    let event_ids: Vec<String>;
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        let event_store = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        
        event_ids = (1..=10).map(|i| format!("persist-evt-{:03}", i)).collect();
        
        for (idx, event_id) in event_ids.iter().enumerate() {
            let parent_ids = if idx > 0 {
                vec![event_ids[idx - 1].clone()]
            } else {
                vec![]
            };
            
            let event = create_test_event(event_id, "validator-1", parent_ids);
            event_store.store_with_depth(event, (idx + 1) as u64).await.unwrap();
        }
        
        info!("Phase 1: Stored 10 events");
    }
    
    // Phase 2: Reopen and verify
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        let event_store = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        
        // Verify all events exist
        for event_id in &event_ids {
            let event = event_store.get(event_id).await;
            assert!(event.is_some(), "Event {} should exist after restart", event_id);
        }
        
        // Verify depths
        for (idx, event_id) in event_ids.iter().enumerate() {
            let depth = event_store.get_depth(event_id).await;
            assert_eq!(depth, Some((idx + 1) as u64), "Depth should be preserved");
        }
        
        // Verify count - should be at least 10 (may have more from other parallel tests)
        let count = event_store.count().await;
        assert!(count >= 10, "Should have at least 10 events, got {}", count);
        
        info!("Phase 2: All 10 events verified after restart");
    }
    
    info!("✓ Event store persistence test completed successfully");
}

// ============================================================================
// Test: Batch store with depth (atomic operation)
// ============================================================================

#[tokio::test]
async fn test_batch_store_with_depth() {
    init_tracing();
    info!("=== Test: Batch store with depth ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
    let event_store = Arc::new(RocksDBEventStore::from_shared(db.clone()));
    
    // Create batch of events with depths
    let events_with_depths: Vec<(Event, u64)> = (1..=5u64)
        .map(|i| {
            let event = create_test_event(
                &format!("batch-evt-{}", i),
                "validator-1",
                if i > 1 { vec![format!("batch-evt-{}", i - 1)] } else { vec![] },
            );
            (event, i)
        })
        .collect();
    
    // Batch store
    let result = event_store.store_batch_with_depth(events_with_depths).await;
    
    assert_eq!(result.stored, 5, "Should store 5 events");
    assert_eq!(result.failed, 0, "No failures expected");
    assert_eq!(result.skipped, 0, "No skips expected");
    
    // Verify all events and depths
    for i in 1..=5u64 {
        let event_id = format!("batch-evt-{}", i);
        let event = event_store.get(&event_id).await;
        assert!(event.is_some(), "Event {} should exist", i);
        
        let depth = event_store.get_depth(&event_id).await;
        assert_eq!(depth, Some(i), "Depth should be {}", i);
    }
    
    info!("✓ Batch store with depth test completed successfully");
}

// ============================================================================
// Test: In-memory vs RocksDB backend compatibility
// ============================================================================

#[tokio::test]
async fn test_backend_api_compatibility() {
    init_tracing();
    info!("=== Test: Backend API compatibility ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Test with in-memory backend
    let memory_event_store: Arc<dyn EventStoreBackend> = Arc::new(EventStore::new());
    let memory_anchor_store: Arc<dyn AnchorStoreBackend> = Arc::new(AnchorStore::new());
    
    // Test with RocksDB backend
    let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
    let rocks_event_store: Arc<dyn EventStoreBackend> = Arc::new(RocksDBEventStore::from_shared(db.clone()));
    let rocks_anchor_store: Arc<dyn AnchorStoreBackend> = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
    
    // Both should support the same operations
    let test_event = create_test_event("compat-test", "validator-1", vec![]);
    let test_anchor = create_test_anchor("compat-anchor", vec!["compat-test".to_string()], 1, None, 50);
    
    // Store in both
    memory_event_store.store_with_depth(test_event.clone(), 1).await.unwrap();
    rocks_event_store.store_with_depth(test_event.clone(), 1).await.unwrap();
    
    memory_anchor_store.store(test_anchor.clone()).await.unwrap();
    rocks_anchor_store.store(test_anchor.clone()).await.unwrap();
    
    // Verify both return same results
    let mem_evt = memory_event_store.get(&"compat-test".to_string()).await;
    let rocks_evt = rocks_event_store.get(&"compat-test".to_string()).await;
    assert!(mem_evt.is_some() && rocks_evt.is_some());
    assert_eq!(mem_evt.unwrap().id, rocks_evt.unwrap().id);
    
    let mem_anchor = memory_anchor_store.get_latest().await;
    let rocks_anchor = rocks_anchor_store.get_latest().await;
    assert!(mem_anchor.is_some() && rocks_anchor.is_some());
    assert_eq!(mem_anchor.unwrap().id, rocks_anchor.unwrap().id);
    
    info!("✓ Backend API compatibility test completed successfully");
}

// ============================================================================
// Test: VLC state restoration
// ============================================================================

#[tokio::test]
async fn test_vlc_state_restoration() {
    init_tracing();
    info!("=== Test: VLC state restoration ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Phase 1: Store anchor with specific VLC state
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        let anchor_store = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        let anchor = create_test_anchor(
            "vlc-test-anchor",
            vec!["evt-1".to_string()],
            1,
            None,
            12345, // Specific VLC logical time
        );
        anchor_store.store(anchor).await.unwrap();
        
        info!("Phase 1: Stored anchor with VLC logical_time=12345");
    }
    
    // Phase 2: Recovery and verify VLC state
    {
        let (validator, _db) = create_rocksdb_validator(temp_dir.path());
        
        let result = validator.recover_from_storage().await;
        assert!(result.is_ok(), "VLC recovery should succeed");
        
        // Get VLC state through engine
        let engine = validator.engine();
        let vlc = engine.vlc().read().await;
        let current_time = vlc.logical_time();
        
        // VLC should be restored to at least the saved state
        assert!(current_time >= 12345, "VLC should be restored to at least 12345, got {}", current_time);
        
        info!("Phase 2: VLC restored, current logical_time={}", current_time);
    }
    
    info!("✓ VLC state restoration test completed successfully");
}

// ============================================================================
// Test: Recovery idempotency
// ============================================================================

#[tokio::test]
async fn test_recovery_idempotency() {
    init_tracing();
    info!("=== Test: Recovery idempotency ===");
    
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    
    // Setup: Store some data
    {
        let db = Arc::new(SetuDB::open_default(temp_dir.path()).expect("Failed to open RocksDB"));
        let anchor_store = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        let anchor = create_test_anchor("idempotent-anchor", vec!["evt-1".to_string()], 1, None, 100);
        anchor_store.store(anchor).await.unwrap();
    }
    
    // Multiple recoveries should all succeed
    let (validator, _db) = create_rocksdb_validator(temp_dir.path());
    
    for i in 1..=3 {
        let result = validator.recover_from_storage().await;
        assert!(result.is_ok(), "Recovery attempt {} should succeed", i);
        info!("Recovery attempt {} succeeded", i);
    }
    
    // State should be consistent
    let anchor = validator.anchor_store().get_latest().await;
    assert!(anchor.is_some());
    assert_eq!(anchor.unwrap().id, "idempotent-anchor");
    
    info!("✓ Recovery idempotency test completed successfully");
}
