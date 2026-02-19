//! Anchor Builder - Integrates DAG folding with Merkle tree state management.
//!
//! This module provides the complete flow for creating Anchors:
//! 1. Collect events from DAG
//! 2. Route events by subnet
//! 3. Apply state changes from execution results
//! 4. Compute all Merkle roots
//! 5. Create the Anchor with merkle_roots
//!
//! ## Deferred Commit Mode
//!
//! This module implements a deferred commit pattern for safe state management:
//! - `prepare_build()`: Computes all data without modifying state (returns `PendingAnchorBuild`)
//! - `commit_build()`: Applies state changes after CF is finalized
//!
//! This ensures that rejected/timeout CFs don't corrupt state.

use crate::dag::Dag;
use crate::merkle_integration::compute_events_root;
use crate::router::{EventRouter, RoutedEvents};
use crate::vlc::VLC;
use setu_storage::{GlobalStateManager, StateApplyError, StateApplySummary};
use setu_types::{
    event::StateChange, Anchor, AnchorMerkleRoots, ConsensusConfig, ConsensusFrame, Event, EventId,
    SubnetId,
};
use std::collections::HashMap;

// ============================================================================
// Types for Deferred Commit Mode
// ============================================================================

/// 单个状态变更条目
#[derive(Debug, Clone)]
pub struct StateChangeEntry {
    pub event_id: String,
    pub subnet_id: SubnetId,
    pub changes: Vec<StateChange>,
}

/// AnchorBuilder 状态快照（轻量级，不包含 SMT）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderStateSnapshot {
    pub last_anchor_id: Option<String>,
    pub anchor_depth: u64,
    pub last_fold_vlc: u64,
    pub last_anchor_chain_root: [u8; 32],
    pub total_anchor_count: u64,
}

/// 待提交的 Anchor 构建结果
///
/// 在 CF finalized 之前，所有状态变更都保存在这里，
/// 不会实际修改 AnchorBuilder 或 GlobalStateManager。
#[derive(Debug, Clone)]
pub struct PendingAnchorBuild {
    /// 构建的 Anchor
    pub anchor: Anchor,

    /// 路由后的事件
    pub routed_events: RoutedEvents,

    /// 待应用的状态变更（按 subnet 分组）
    pub pending_state_changes: HashMap<SubnetId, Vec<StateChangeEntry>>,

    /// 构建前的快照（用于验证）
    pub pre_build_snapshot: BuilderStateSnapshot,

    /// 计算得到的 events_root
    pub events_root: [u8; 32],

    /// 新的 anchor_chain_root (构建后)
    pub new_anchor_chain_root: [u8; 32],

    /// 新的 anchor_depth (构建后)
    pub new_anchor_depth: u64,

    /// 新的 last_fold_vlc (构建后)
    pub new_last_fold_vlc: u64,
}

impl PendingAnchorBuild {
    /// Get all events from routed_events
    pub fn all_events(&self) -> Vec<Event> {
        let mut events = self.routed_events.root_events.clone();
        for app_events in self.routed_events.app_events.values() {
            events.extend(app_events.iter().cloned());
        }
        events.extend(self.routed_events.unrouted_events.iter().cloned());
        events
    }
}

// ============================================================================
// AnchorBuildResult and AnchorBuildError
// ============================================================================

/// Complete Anchor creation result
#[derive(Debug)]
pub struct AnchorBuildResult {
    /// The created anchor
    pub anchor: Anchor,
    /// Summary of state changes applied
    pub state_summary: StateApplySummary,
    /// Events routed by subnet
    pub routed_events: RoutedEvents,
}

/// Error types for anchor building
#[derive(Debug)]
pub enum AnchorBuildError {
    /// Not enough events to fold
    InsufficientEvents { required: usize, found: usize },
    /// VLC delta not reached
    DeltaNotReached { required: u64, current: u64 },
    /// State apply failed
    StateApplyError(StateApplyError),
    /// Merkle operation failed
    MerkleError(String),
    /// No events to process
    NoEvents,
    /// Snapshot mismatch during commit (并发冲突)
    ///
    /// 当 commit_build 时发现当前状态与 prepare_build 时的快照不一致。
    /// 这通常意味着另一个 CF 已经被 commit，当前 pending build 已失效。
    SnapshotMismatch {
        expected_depth: u64,
        actual_depth: u64,
    },
    /// Missing events during Follower sync
    MissingEvents { expected: usize, found: usize },
    /// State root mismatch during Follower verification
    RootMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
}

impl std::fmt::Display for AnchorBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorBuildError::InsufficientEvents { required, found } => {
                write!(
                    f,
                    "Insufficient events: required {}, found {}",
                    required, found
                )
            }
            AnchorBuildError::DeltaNotReached { required, current } => {
                write!(
                    f,
                    "VLC delta not reached: required {}, current {}",
                    required, current
                )
            }
            AnchorBuildError::StateApplyError(e) => write!(f, "State apply error: {}", e),
            AnchorBuildError::MerkleError(e) => write!(f, "Merkle error: {}", e),
            AnchorBuildError::NoEvents => write!(f, "No events to process"),
            AnchorBuildError::SnapshotMismatch {
                expected_depth,
                actual_depth,
            } => {
                write!(
                    f,
                    "Snapshot mismatch: expected depth {}, actual {}",
                    expected_depth, actual_depth
                )
            }
            AnchorBuildError::MissingEvents { expected, found } => {
                write!(f, "Missing events: expected {}, found {}", expected, found)
            }
            AnchorBuildError::RootMismatch { expected, actual } => {
                write!(
                    f,
                    "Root mismatch: expected {:?}, actual {:?}",
                    hex::encode(expected),
                    hex::encode(actual)
                )
            }
        }
    }
}

impl std::error::Error for AnchorBuildError {}

impl From<StateApplyError> for AnchorBuildError {
    fn from(e: StateApplyError) -> Self {
        AnchorBuildError::StateApplyError(e)
    }
}

impl From<setu_merkle::MerkleError> for AnchorBuildError {
    fn from(e: setu_merkle::MerkleError) -> Self {
        AnchorBuildError::MerkleError(e.to_string())
    }
}

/// Anchor Builder with integrated Merkle tree management
pub struct AnchorBuilder {
    config: ConsensusConfig,
    /// Global state manager (owns all subnet SMTs)
    state_manager: GlobalStateManager,
    /// Last created anchor
    last_anchor: Option<Anchor>,
    /// Current anchor depth
    anchor_depth: u64,
    /// Last fold VLC timestamp
    last_fold_vlc: u64,
    /// Cumulative anchor chain root (chain hash of all previous anchors)
    ///
    /// Uses chain hashing: new_root = hash(prev_root || anchor_hash)
    /// This provides O(1) memory and O(1) computation while maintaining
    /// cryptographic commitment to the entire anchor history.
    last_anchor_chain_root: [u8; 32],
    /// Total number of anchors created (for statistics)
    total_anchor_count: u64,
}

impl AnchorBuilder {
    /// Create a new AnchorBuilder
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            config,
            state_manager: GlobalStateManager::new(),
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            last_anchor_chain_root: [0u8; 32], // Genesis: all zeros
            total_anchor_count: 0,
        }
    }

    /// Create with an existing GlobalStateManager
    pub fn with_state_manager(config: ConsensusConfig, state_manager: GlobalStateManager) -> Self {
        Self {
            config,
            state_manager,
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            last_anchor_chain_root: [0u8; 32], // Genesis: all zeros
            total_anchor_count: 0,
        }
    }

    /// Check if we should attempt to fold
    pub fn should_fold(&self, current_vlc: &VLC) -> bool {
        let delta = current_vlc
            .logical_time()
            .saturating_sub(self.last_fold_vlc);
        delta >= self.config.vlc_delta_threshold
    }

    /// Get the global state manager (read-only)
    pub fn state_manager(&self) -> &GlobalStateManager {
        &self.state_manager
    }

    /// Restore AnchorBuilder state from storage after restart
    ///
    /// This should be called during node initialization to recover:
    /// - last_anchor_chain_root: for continuing the chain hash
    /// - anchor_depth: for creating next anchor
    /// - total_anchor_count: for statistics
    pub fn restore_state(
        &mut self,
        last_anchor_chain_root: [u8; 32],
        anchor_depth: u64,
        total_anchor_count: u64,
        last_fold_vlc: u64,
    ) {
        self.last_anchor_chain_root = last_anchor_chain_root;
        self.anchor_depth = anchor_depth;
        self.total_anchor_count = total_anchor_count;
        self.last_fold_vlc = last_fold_vlc;
    }

    /// Get the global state manager (mutable)
    pub fn state_manager_mut(&mut self) -> &mut GlobalStateManager {
        &mut self.state_manager
    }

    // ========================================================================
    // Deferred Commit Mode API
    // ========================================================================

    /// Take a snapshot of current builder state (lightweight, no SMT)
    pub fn take_snapshot(&self) -> BuilderStateSnapshot {
        BuilderStateSnapshot {
            last_anchor_id: self.last_anchor.as_ref().map(|a| a.id.clone()),
            anchor_depth: self.anchor_depth,
            last_fold_vlc: self.last_fold_vlc,
            last_anchor_chain_root: self.last_anchor_chain_root,
            total_anchor_count: self.total_anchor_count,
        }
    }

    /// Verify that current state matches a snapshot
    fn verify_snapshot(&self, snapshot: &BuilderStateSnapshot) -> bool {
        self.anchor_depth == snapshot.anchor_depth
            && self.last_fold_vlc == snapshot.last_fold_vlc
            && self.last_anchor_chain_root == snapshot.last_anchor_chain_root
            && self.last_anchor.as_ref().map(|a| &a.id) == snapshot.last_anchor_id.as_ref()
    }

    /// Prepare to build an anchor (read-only, does not modify state)
    ///
    /// This is the new main entry point for deferred commit mode. It:
    /// 1. Checks if folding conditions are met
    /// 2. Collects events from DAG
    /// 3. Routes events by subnet
    /// 4. Collects pending state changes (without applying)
    /// 5. Computes Merkle roots using a temporary SMT clone
    /// 6. Returns PendingAnchorBuild for later commit
    ///
    /// The actual state modifications happen in `commit_build()` after CF is finalized.
    pub fn prepare_build(
        &self,
        dag: &Dag,
        vlc: &VLC,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        // Check VLC delta threshold
        let delta = vlc.logical_time().saturating_sub(self.last_fold_vlc);
        if delta < self.config.vlc_delta_threshold {
            return Err(AnchorBuildError::DeltaNotReached {
                required: self.config.vlc_delta_threshold,
                current: delta,
            });
        }

        // Collect events from DAG
        let from_depth = self.anchor_depth;
        let to_depth = dag.max_depth();
        let events: Vec<Event> = dag
            .get_events_in_range(from_depth, to_depth)
            .into_iter()
            .cloned()
            .collect();

        // Check minimum events
        if events.len() < self.config.min_events_per_cf {
            return Err(AnchorBuildError::InsufficientEvents {
                required: self.config.min_events_per_cf,
                found: events.len(),
            });
        }

        if events.is_empty() {
            return Err(AnchorBuildError::NoEvents);
        }

        self.prepare_build_internal(events, vlc, to_depth)
    }

    /// Force prepare build from specific events (bypasses checks)
    pub fn force_prepare_build(
        &self,
        events: Vec<Event>,
        vlc: &VLC,
        depth: u64,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        if events.is_empty() {
            return Err(AnchorBuildError::NoEvents);
        }
        self.prepare_build_internal(events, vlc, depth)
    }

    /// Internal prepare build implementation
    fn prepare_build_internal(
        &self,
        events: Vec<Event>,
        vlc: &VLC,
        to_depth: u64,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        // Take snapshot before any computation
        let pre_build_snapshot = self.take_snapshot();

        // Route events by subnet
        let routed = EventRouter::route_events(&events);

        // Collect state changes without applying
        let pending_state_changes = self.collect_state_changes(&events);

        // Compute events root (Binary Merkle Tree)
        let events_root_hash = compute_events_root(&events);
        let events_root = *events_root_hash.as_bytes();

        // Compute pending state root using temporary clone
        let (global_state_root, subnet_roots) =
            self.compute_pending_state_root(&pending_state_changes);

        // Compute new anchor chain root (what it will be after commit)
        let anchor_chain_root = self.last_anchor_chain_root;

        // Build merkle roots
        let merkle_roots = AnchorMerkleRoots {
            events_root,
            global_state_root,
            anchor_chain_root, // Store "before" value
            subnet_roots,
        };

        // Collect event IDs (with limit)
        let event_ids: Vec<EventId> = events
            .iter()
            .take(self.config.max_events_per_cf)
            .map(|e| e.id.clone())
            .collect();

        // Create anchor (but don't store it yet)
        let anchor = Anchor::with_merkle_roots(
            event_ids,
            vlc.snapshot(),
            merkle_roots.clone(),
            self.last_anchor.as_ref().map(|a| a.id.clone()),
            to_depth,
        );

        // Compute what the new chain root will be
        let anchor_hash = anchor.compute_hash();
        let new_anchor_chain_root = Self::chain_hash(&self.last_anchor_chain_root, &anchor_hash);

        Ok(PendingAnchorBuild {
            anchor,
            routed_events: routed,
            pending_state_changes,
            pre_build_snapshot,
            events_root,
            new_anchor_chain_root,
            new_anchor_depth: to_depth + 1,
            new_last_fold_vlc: vlc.logical_time(),
        })
    }

    /// Commit a prepared build after CF is finalized
    ///
    /// This applies all the state changes that were prepared in `prepare_build()`.
    /// Should only be called when the CF has been successfully finalized.
    pub fn commit_build(
        &mut self,
        pending: PendingAnchorBuild,
    ) -> Result<StateApplySummary, AnchorBuildError> {
        // Verify snapshot consistency (detect concurrent modifications)
        if !self.verify_snapshot(&pending.pre_build_snapshot) {
            return Err(AnchorBuildError::SnapshotMismatch {
                expected_depth: pending.pre_build_snapshot.anchor_depth,
                actual_depth: self.anchor_depth,
            });
        }

        // Apply state changes to SMT
        let events = pending.all_events();
        let state_summary = self.state_manager.apply_committed_events(&events);

        // Commit state
        let anchor_id = self.anchor_depth + 1;
        self.state_manager.commit(anchor_id)?;

        // Update AnchorBuilder state
        self.last_anchor = Some(pending.anchor);
        self.anchor_depth = pending.new_anchor_depth;
        self.last_fold_vlc = pending.new_last_fold_vlc;
        self.last_anchor_chain_root = pending.new_anchor_chain_root;
        self.total_anchor_count += 1;

        Ok(state_summary)
    }

    /// Apply a finalized CF as a Follower (verify then apply)
    ///
    /// This is used by Follower nodes to apply state from a CF that was
    /// finalized by the network. It:
    /// 1. Verifies that we have all required events
    /// 2. Computes expected state root and verifies against CF
    /// 3. Applies state changes
    /// 4. Updates metadata
    pub fn apply_follower_finalized_cf(
        &mut self,
        events: &[Event],
        cf: &ConsensusFrame,
    ) -> Result<StateApplySummary, AnchorBuildError> {
        // 1. Completeness check
        if events.len() != cf.anchor.event_ids.len() {
            return Err(AnchorBuildError::MissingEvents {
                expected: cf.anchor.event_ids.len(),
                found: events.len(),
            });
        }

        // 2. Verify state root (compute expected vs actual)
        if let Some(ref merkle_roots) = cf.anchor.merkle_roots {
            let pending_changes = self.collect_state_changes(events);
            let (expected_root, _) = self.compute_pending_state_root(&pending_changes);

            if expected_root != merkle_roots.global_state_root {
                return Err(AnchorBuildError::RootMismatch {
                    expected: expected_root,
                    actual: merkle_roots.global_state_root,
                });
            }
        }

        // 3. Apply state changes
        let state_summary = self.state_manager.apply_committed_events(events);

        // 4. Commit and update metadata
        let anchor_id = self.anchor_depth + 1;
        self.state_manager.commit(anchor_id)?;
        self.synchronize_finalized_anchor(&cf.anchor);

        Ok(state_summary)
    }

    /// Collect state changes from events without applying them
    fn collect_state_changes(&self, events: &[Event]) -> HashMap<SubnetId, Vec<StateChangeEntry>> {
        let mut changes: HashMap<SubnetId, Vec<StateChangeEntry>> = HashMap::new();

        for event in events {
            let subnet_id = event.get_subnet_id();
            if let Some(result) = &event.execution_result {
                if result.success && !result.state_changes.is_empty() {
                    let entry = StateChangeEntry {
                        event_id: event.id.clone(),
                        subnet_id,
                        changes: result.state_changes.clone(),
                    };
                    changes.entry(subnet_id).or_default().push(entry);
                }
            }
        }

        changes
    }

    /// Compute state root after applying pending changes (using temporary clone)
    fn compute_pending_state_root(
        &self,
        pending_changes: &HashMap<SubnetId, Vec<StateChangeEntry>>,
    ) -> ([u8; 32], HashMap<SubnetId, [u8; 32]>) {
        // Clone the state manager for temporary computation
        let mut temp_manager = self.state_manager.clone();

        // Apply pending changes to the clone
        for (subnet_id, entries) in pending_changes {
            for entry in entries {
                for change in &entry.changes {
                    temp_manager.apply_state_change(*subnet_id, change);
                }
            }
        }

        // Compute and return the root
        temp_manager.compute_global_root_bytes()
    }

    // ========================================================================
    // Getters
    // ========================================================================

    /// Get the last created anchor
    pub fn last_anchor(&self) -> Option<&Anchor> {
        self.last_anchor.as_ref()
    }

    /// Get the current anchor depth
    pub fn anchor_depth(&self) -> u64 {
        self.anchor_depth
    }

    /// Get the last fold VLC timestamp
    pub fn last_fold_vlc(&self) -> u64 {
        self.last_fold_vlc
    }

    /// Get total anchor count
    pub fn anchor_count(&self) -> usize {
        self.total_anchor_count as usize
    }

    /// Get the current anchor chain root
    pub fn anchor_chain_root(&self) -> [u8; 32] {
        self.last_anchor_chain_root
    }

    /// Synchronize state after a CF is finalized (Follower path, metadata only)
    ///
    /// This is called by follower nodes when a CF is finalized to synchronize their
    /// anchor chain state with the leader. It updates:
    /// - last_anchor_chain_root: Computes the new chain root by hashing
    /// - last_anchor: Stores the finalized anchor
    /// - anchor_depth: Updates to the next depth
    /// - last_fold_vlc: Updates VLC timestamp
    /// - total_anchor_count: Increments counter
    ///
    /// Note: This only updates metadata. For Followers, use `apply_follower_finalized_cf`
    /// which also applies state changes with verification.
    pub fn synchronize_finalized_anchor(&mut self, anchor: &Anchor) {
        // Update anchor chain root by hashing the stored root with this anchor's hash
        // The anchor stores the "before" root, we compute the "after" root
        if let Some(ref merkle_roots) = anchor.merkle_roots {
            let anchor_hash = anchor.compute_hash();
            self.last_anchor_chain_root =
                Self::chain_hash(&merkle_roots.anchor_chain_root, &anchor_hash);
        }

        // Update other state to match the finalized anchor
        self.last_anchor = Some(anchor.clone());
        self.anchor_depth = anchor.depth + 1;
        self.last_fold_vlc = anchor.vlc_snapshot.logical_time;
        self.total_anchor_count += 1;
    }

    /// Get a subnet's current state root
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<[u8; 32]> {
        self.state_manager.get_subnet_root_bytes(subnet_id)
    }

    /// Get the current global state root
    pub fn get_global_root(&self) -> [u8; 32] {
        let (root, _) = self.state_manager.compute_global_root_bytes();
        root
    }

    /// Chain hash: combines previous chain root with new anchor hash
    ///
    /// new_root = SHA256(prev_root || anchor_hash)
    ///
    /// This creates an append-only commitment where:
    /// - R0 = [0; 32] (genesis)
    /// - R1 = hash(R0 || H1)
    /// - R2 = hash(R1 || H2)
    /// - Rn = hash(R_{n-1} || Hn)
    ///
    /// Each Rn commits to the entire history [H1, H2, ..., Hn]
    fn chain_hash(prev_root: &[u8; 32], anchor_hash: &[u8; 32]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(prev_root);
        hasher.update(anchor_hash);
        let result = hasher.finalize();
        let mut output = [0u8; 32];
        output.copy_from_slice(&result);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::VLCSnapshot;
    use setu_types::{EventType, ExecutionResult, StateChange};

    fn create_vlc(node_id: &str, time: u64) -> VLC {
        let mut vlc = VLC::new(node_id.to_string());
        for _ in 0..time {
            vlc.tick();
        }
        vlc
    }

    fn create_event_with_result(subnet_id: SubnetId, changes: Vec<StateChange>) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        event = event.with_subnet(subnet_id);
        event.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes: changes,
        });
        event
    }

    #[test]
    fn test_prepare_build_does_not_modify_state() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);

        // Create event with state changes
        let event = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: Some(vec![0; 8]),
                new_value: Some(vec![100; 8]),
            }],
        );

        // Take snapshot before prepare
        let snapshot_before = builder.take_snapshot();
        let root_before = builder.get_global_root();

        // Prepare build (should not modify state)
        let pending = builder.force_prepare_build(vec![event], &vlc, 1).unwrap();

        // Verify state unchanged
        assert_eq!(builder.take_snapshot(), snapshot_before);
        assert_eq!(builder.get_global_root(), root_before);

        // But pending should have computed values
        assert!(pending.anchor.merkle_roots.is_some());
        assert_eq!(pending.new_anchor_depth, 2);
    }

    #[test]
    fn test_commit_build_updates_state() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let mut builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);

        // Create event
        let event = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: None,
                new_value: Some(vec![100; 8]),
            }],
        );

        // Prepare and commit
        let pending = builder.force_prepare_build(vec![event], &vlc, 1).unwrap();
        let expected_depth = pending.new_anchor_depth;
        let expected_vlc = pending.new_last_fold_vlc;

        let result = builder.commit_build(pending);
        assert!(result.is_ok());

        // Verify state updated
        assert_eq!(builder.anchor_depth(), expected_depth);
        assert_eq!(builder.last_fold_vlc(), expected_vlc);
        assert_eq!(builder.anchor_count(), 1);
    }

    #[test]
    fn test_discard_build_no_state_change() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);

        // Take initial snapshot
        let initial_snapshot = builder.take_snapshot();

        // Create event
        let event = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: None,
                new_value: Some(vec![100; 8]),
            }],
        );

        // Prepare but don't commit (simulate rejection)
        let pending = builder.force_prepare_build(vec![event], &vlc, 1).unwrap();
        drop(pending); // Discard the pending build

        // Verify state unchanged (no rollback needed!)
        assert_eq!(builder.take_snapshot(), initial_snapshot);
    }

    #[test]
    fn test_snapshot_mismatch_on_concurrent_commit() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let mut builder = AnchorBuilder::new(config);
        let vlc1 = create_vlc("node1", 10);
        let vlc2 = create_vlc("node1", 20);

        // Create two events
        let event1 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: None,
                new_value: Some(vec![100; 8]),
            }],
        );
        let event2 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:bob".to_string(),
                old_value: None,
                new_value: Some(vec![200; 8]),
            }],
        );

        // Prepare two builds concurrently
        let pending1 = builder.force_prepare_build(vec![event1], &vlc1, 1).unwrap();
        let pending2 = builder.force_prepare_build(vec![event2], &vlc2, 2).unwrap();

        // Commit first one
        builder.commit_build(pending1).unwrap();

        // Try to commit second one - should fail with SnapshotMismatch
        let result = builder.commit_build(pending2);
        assert!(matches!(
            result,
            Err(AnchorBuildError::SnapshotMismatch { .. })
        ));
    }

    #[test]
    fn test_multi_subnet_prepare_and_commit() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let mut builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);

        // Create events for different subnets
        let app_subnet = SubnetId::from_str_id("my-app");

        let events = vec![
            create_event_with_result(
                SubnetId::ROOT,
                vec![StateChange {
                    key: "balance:alice".to_string(),
                    old_value: None,
                    new_value: Some(vec![100; 8]),
                }],
            ),
            create_event_with_result(
                app_subnet,
                vec![StateChange {
                    key: "nft:token1".to_string(),
                    old_value: None,
                    new_value: Some(vec![1; 32]),
                }],
            ),
        ];

        // Prepare and commit
        let pending = builder.force_prepare_build(events, &vlc, 1).unwrap();

        // Verify pending has computed roots for both subnets
        let merkle_roots = pending.anchor.merkle_roots.as_ref().unwrap();
        assert_eq!(merkle_roots.subnet_roots.len(), 2);

        let result = builder.commit_build(pending).unwrap();

        // Should have processed events from both subnets
        assert_eq!(result.subnets_updated(), 2);
    }

    #[test]
    fn test_anchor_chain_continuity() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };

        let mut builder = AnchorBuilder::new(config);

        // Build first anchor
        let vlc1 = create_vlc("node1", 10);
        let event1 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: None,
                new_value: Some(vec![100; 8]),
            }],
        );
        let pending1 = builder.force_prepare_build(vec![event1], &vlc1, 1).unwrap();
        builder.commit_build(pending1).unwrap();

        let chain_root1 = builder.anchor_chain_root();

        // Build second anchor
        let vlc2 = create_vlc("node1", 20);
        let event2 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:bob".to_string(),
                old_value: None,
                new_value: Some(vec![200; 8]),
            }],
        );
        let pending2 = builder.force_prepare_build(vec![event2], &vlc2, 2).unwrap();
        builder.commit_build(pending2).unwrap();

        let chain_root2 = builder.anchor_chain_root();

        // Chain roots should be different (linked)
        assert_ne!(chain_root1, chain_root2);

        // Anchor count should be 2
        assert_eq!(builder.anchor_count(), 2);

        // Last anchor should link to first
        let last_anchor = builder.last_anchor().unwrap();
        assert!(last_anchor.previous_anchor.is_some());
    }
}
