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
use crate::outcome_sink::OutcomeSink;
use crate::router::{EventRouter, RoutedEvents};
use crate::vlc::VLC;
use setu_merkle::HashValue;
use setu_storage::{GlobalStateManager, SharedStateManager, StateApplyError, StateApplySummary};
use setu_types::{
    event::StateChange, Anchor, AnchorMerkleRoots, ConsensusConfig, ConsensusFrame, Event, EventId,
    ExecutionOutcome, SubnetId,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// DIAG (docs/feat/consensus-root-self-consistency/design.md §3.2):
// AnchorId is needed by the feature-gated `prepare_base_roots` sidecar map.
#[cfg(feature = "diag-root-drift")]
use setu_types::AnchorId;

// ============================================================================
// γ — Strict Same-Key CF Fold Policy
// (docs/feat/strict-same-key-cf-fold/)
// ============================================================================

/// γ fold claimed-write-key: `(target_subnet, ObjectId)`.
///
/// **Invariant Inv-γ-Key-Format**: must match the exact key format used by
/// `apply_committed_events` in `pending_writes` projection
/// ([storage/src/state/manager.rs:1046]), i.e.
/// `target = change.target_subnet.unwrap_or(event.get_subnet_id())` +
/// `object_id = GlobalStateManager::parse_state_change_key(&change.key)`.
/// If you change one side, change the other.
type ClaimedWriteKey = (SubnetId, HashValue);

/// Extract the set of SMT write-keys produced by an event.
///
/// Returns an empty set for:
/// - events with `execution_result = None` (control events, pre-exec events)
/// - events whose `execution_result.success == false` (apply_committed_events
///   skips them, so γ must not claim their keys)
///
/// Within a single event, duplicate keys are deduplicated via HashSet — this
/// allows MergeThenTransfer etc. to write the same key twice inside ONE event
/// while γ still counts it as one slot.
///
/// See `docs/feat/strict-same-key-cf-fold/design.md` §3.3.
fn collect_event_write_keys(event: &Event) -> HashSet<ClaimedWriteKey> {
    let mut keys = HashSet::new();
    let Some(result) = event.execution_result.as_ref() else {
        return keys;
    };
    if !result.success {
        return keys;
    }
    let subnet_id = event.get_subnet_id();
    for change in &result.state_changes {
        let target = change.target_subnet.unwrap_or(subnet_id);
        let object_id = GlobalStateManager::parse_state_change_key(&change.key);
        keys.insert((target, object_id));
    }
    keys
}

/// γ strict same-key CF fold policy.
///
/// Greedy scan of VLC-sorted events; an event is **kept** only if its write-key
/// set is disjoint from the union of already-kept events' write-keys. Otherwise
/// the event is **deferred** — it stays in the DAG pending set and will be
/// reconsidered on the next `prepare_build` round (once previously-kept events
/// move to `in_flight_event_ids`).
///
/// **Inv-γ-Sort**: events are sorted by `(vlc_snapshot.logical_time asc, id asc)`
/// BEFORE scanning. This MUST stay identical to the sort inside
/// `apply_committed_events` in storage/src/state/manager.rs (leader-follower
/// consensus depends on the two layers agreeing on order).
///
/// **tie-break**: event_id equality would require a BLAKE3 collision; treated
/// as impossible. See design §3.2.
///
/// See `docs/feat/strict-same-key-cf-fold/design.md` §3.2.
fn apply_strict_same_key_fold_policy(events: Vec<Event>) -> (Vec<Event>, Vec<Event>) {
    let mut sorted = events;
    sorted.sort_by(|a, b| {
        match a
            .vlc_snapshot
            .logical_time
            .cmp(&b.vlc_snapshot.logical_time)
        {
            Ordering::Equal => a.id.cmp(&b.id),
            other => other,
        }
    });

    let mut claimed: HashSet<ClaimedWriteKey> = HashSet::new();
    let mut kept: Vec<Event> = Vec::with_capacity(sorted.len());
    let mut deferred: Vec<Event> = Vec::new();

    for event in sorted {
        let write_keys = collect_event_write_keys(&event);
        let conflict = write_keys.iter().any(|k| claimed.contains(k));
        if conflict {
            deferred.push(event);
        } else {
            for k in write_keys {
                claimed.insert(k);
            }
            kept.push(event);
        }
    }

    (kept, deferred)
}

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
        for system_events in self.routed_events.system_events.values() {
            events.extend(system_events.iter().cloned());
        }
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
    /// Shared state manager (read-write separated)
    pub(crate) shared: Arc<SharedStateManager>,
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
    /// Wall-clock time of the last successful commit_build() / synchronize_finalized_anchor().
    /// Used by heartbeat to detect "long time no CF" condition.
    last_fold_instant: Option<std::time::Instant>,
    /// R5 · Optional outcome sink for apply-phase observability.
    /// Default None; `set_outcomes_sink` wires production sinks (e.g. DashMapOutcomeSink).
    outcomes_sink: Option<Arc<dyn OutcomeSink>>,

    /// DIAG-only sidecar: captures the write-GSM `global_state_root` observed
    /// at `prepare_build_internal` time, keyed by the anchor id that is
    /// about to be shipped. `commit_build` looks it up and compares to the
    /// write-GSM root observed after re-acquiring the lock; divergence signals
    /// H2 (base-state drift between prepare and commit).
    /// See docs/feat/consensus-root-self-consistency/design.md §3.2.
    #[cfg(feature = "diag-root-drift")]
    prepare_base_roots: parking_lot::Mutex<HashMap<AnchorId, [u8; 32]>>,
}

impl AnchorBuilder {
    /// Create a new AnchorBuilder with its own GlobalStateManager
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            config,
            shared: Arc::new(SharedStateManager::new(GlobalStateManager::new())),
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            last_anchor_chain_root: [0u8; 32], // Genesis: all zeros
            total_anchor_count: 0,
            last_fold_instant: None,
            outcomes_sink: None,
            #[cfg(feature = "diag-root-drift")]
            prepare_base_roots: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Create with a shared GlobalStateManager
    ///
    /// This allows sharing state between components (e.g., TaskPreparer and ConsensusValidator)
    pub fn with_shared_state_manager(
        config: ConsensusConfig,
        state_manager: Arc<SharedStateManager>,
    ) -> Self {
        Self {
            config,
            shared: state_manager,
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            last_anchor_chain_root: [0u8; 32], // Genesis: all zeros
            total_anchor_count: 0,
            last_fold_instant: None,
            outcomes_sink: None,
            #[cfg(feature = "diag-root-drift")]
            prepare_base_roots: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Check if we should attempt to fold
    pub fn should_fold(&self, current_vlc: &VLC) -> bool {
        let delta = current_vlc
            .logical_time()
            .saturating_sub(self.last_fold_vlc);
        delta >= self.config.vlc_delta_threshold
    }

    /// R5 · Inject the outcome sink (optional; default = no sink).
    ///
    /// Called once by `ConsensusValidator::new` via the three-layer passthrough:
    /// `ConsensusEngine::set_outcomes_sink` →
    /// `ConsensusManager::set_outcomes_sink` → here.
    pub fn set_outcomes_sink(&mut self, sink: Arc<dyn OutcomeSink>) {
        self.outcomes_sink = Some(sink);
    }

    /// R5 · Write per-event apply outcomes to the sink.
    ///
    /// Called from both `commit_build` (Leader) and `apply_follower_finalized_cf`
    /// (Follower), so Leader and Follower paths produce identical outcomes
    /// (closes §1.2 gap-3).
    ///
    /// Genesis events short-circuit to `Applied` regardless of conflict set —
    /// Genesis is pre-applied at startup, so any apply-phase "conflict" here
    /// is an expected re-apply, not a real stale read (R1-ISSUE-1).
    fn ingest_outcomes(&self, cf_id: &str, applied_events: &[Event], summary: &StateApplySummary) {
        let Some(sink) = self.outcomes_sink.as_ref() else {
            return;
        };

        let failed: HashSet<&str> = summary.failed_events.iter().map(String::as_str).collect();
        let conflicted: HashMap<&str, &str> = summary
            .conflicted_events
            .iter()
            .map(|r| (r.event_id.as_str(), r.conflicting_object.as_str()))
            .collect();

        for ev in applied_events {
            if ev.is_genesis() {
                sink.record(
                    ev.id.clone(),
                    ExecutionOutcome::Applied {
                        cf_id: cf_id.to_string(),
                    },
                );
                continue;
            }
            let outcome = if let Some(obj) = conflicted.get(ev.id.as_str()) {
                ExecutionOutcome::StaleRead {
                    cf_id: cf_id.to_string(),
                    conflicting_object: (*obj).to_string(),
                    retry_hint: format!(
                        "object {} was concurrently modified; re-read and retry",
                        obj
                    ),
                }
            } else if failed.contains(ev.id.as_str()) {
                let reason = ev.execution_result.as_ref().and_then(|r| r.message.clone());
                ExecutionOutcome::ExecutionFailed {
                    cf_id: cf_id.to_string(),
                    reason,
                }
            } else {
                ExecutionOutcome::Applied {
                    cf_id: cf_id.to_string(),
                }
            };
            sink.record(ev.id.clone(), outcome);
        }
    }

    /// Get the shared state manager
    pub fn shared_state_manager(&self) -> Arc<SharedStateManager> {
        Arc::clone(&self.shared)
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

    /// Get write access to the global state manager
    ///
    /// Note: This acquires the write lock. Caller must ensure not to hold it across await points.
    pub fn with_state_manager_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut GlobalStateManager) -> R,
    {
        let mut guard = self.shared.lock_write();
        f(&mut guard)
    }

    /// Get read access to the global state manager
    pub fn with_state_manager<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&GlobalStateManager) -> R,
    {
        let snapshot = self.shared.load_snapshot();
        f(&snapshot)
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
    ///
    /// D1 (docs/feat/anchor-builder-fold-policy/design.md): event selection is
    /// driven by the DAG's `pending` status-set, not by a depth range. Callers
    /// pass `in_flight_event_ids` for the set of events already referenced by
    /// in-flight CFs (their own `pending_builds` + `pending_cf_events`); those
    /// are filtered out to avoid double-folding.
    pub fn prepare_build(
        &self,
        dag: &Dag,
        vlc: &VLC,
        in_flight_event_ids: &HashSet<EventId>,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        // Check VLC delta threshold
        let delta = vlc.logical_time().saturating_sub(self.last_fold_vlc);
        if delta < self.config.vlc_delta_threshold {
            return Err(AnchorBuildError::DeltaNotReached {
                required: self.config.vlc_delta_threshold,
                current: delta,
            });
        }

        // D1: depth-independent event selection via DAG pending-set.
        // `to_depth = dag.max_depth()` is still needed for the new-anchor-depth
        // arithmetic (to_depth + 1) and for the anchor payload.
        let to_depth = dag.max_depth();

        // Debug: log selection inputs
        tracing::debug!(
            from_depth = self.anchor_depth,
            to_depth = to_depth,
            dag_node_count = dag.node_count(),
            dag_pending_count = dag.get_pending_count(),
            in_flight_count = in_flight_event_ids.len(),
            "prepare_build: selecting pending events"
        );

        let events: Vec<Event> = dag
            .get_pending_events()
            .into_iter()
            .filter(|e| !in_flight_event_ids.contains(&e.id))
            .cloned()
            .collect();

        // γ strict same-key CF fold: eliminate cross-event same-key writes in
        // one CF (docs/feat/strict-same-key-cf-fold/ §3.4).
        let (mut events, deferred_same_key) = apply_strict_same_key_fold_policy(events);
        let deferred_capacity = if events.len() > self.config.max_events_per_cf {
            events.split_off(self.config.max_events_per_cf)
        } else {
            Vec::new()
        };
        if !deferred_same_key.is_empty() || !deferred_capacity.is_empty() {
            tracing::info!(
                kept = events.len(),
                deferred_same_key = deferred_same_key.len(),
                deferred_capacity = deferred_capacity.len(),
                "γ fold: deferred events to next CF"
            );
        }

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
    ///
    /// WARNING: bypasses γ strict-same-key fold policy (docs/feat/strict-same-key-cf-fold/).
    /// This is safe for leader/follower symmetry (both run identical
    /// `apply_committed_events` on the input event set) but opts out of γ's
    /// defense-in-depth benefit. Only acceptable for unit tests where the
    /// caller constructs a small, controlled event set. `max_events_per_cf`
    /// trim is still applied to keep `event_ids` / `events_root` / `state_root`
    /// in lockstep (§2.0).
    pub fn force_prepare_build(
        &self,
        events: Vec<Event>,
        vlc: &VLC,
        depth: u64,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        if events.is_empty() {
            return Err(AnchorBuildError::NoEvents);
        }
        let mut events = events;
        if events.len() > self.config.max_events_per_cf {
            events.truncate(self.config.max_events_per_cf);
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

        // Collect state changes (for metadata in PendingAnchorBuild)
        let pending_state_changes = self.collect_state_changes(&events);

        // Compute events root (Binary Merkle Tree)
        let events_root_hash = compute_events_root(&events);
        let events_root = *events_root_hash.as_bytes();

        // Compute state root using same deterministic logic as Follower:
        // VLC-sorted, conflict-detected, cloned from write GSM
        #[cfg(feature = "diag-root-drift")]
        let mut prepare_base: [u8; 32] = [0u8; 32];
        let (global_state_root, subnet_roots) = self.compute_state_root_from_events(
            &events,
            #[cfg(feature = "diag-root-drift")]
            &mut prepare_base,
        );

        // Compute new anchor chain root (what it will be after commit)
        let anchor_chain_root = self.last_anchor_chain_root;

        // Build merkle roots
        let merkle_roots = AnchorMerkleRoots {
            events_root,
            global_state_root,
            anchor_chain_root, // Store "before" value
            subnet_roots,
        };

        // Collect event IDs. γ + trim has already capped `events.len()` to
        // `max_events_per_cf` at the `prepare_build` layer (design §2.0), so
        // do NOT re-trim here — `events_root` and `state_root` above were
        // computed on this exact `events` slice and must match the event_ids.
        let event_ids: Vec<EventId> = events.iter().map(|e| e.id.clone()).collect();

        // Create anchor (but don't store it yet)
        let anchor = Anchor::with_merkle_roots(
            event_ids,
            vlc.snapshot(),
            merkle_roots.clone(),
            self.last_anchor.as_ref().map(|a| a.id.clone()),
            to_depth,
        );

        // DIAG (H2): record the prepare-time base root for later commit-time
        // comparison. See §3.2 of the FDP design.
        #[cfg(feature = "diag-root-drift")]
        self.prepare_base_roots
            .lock()
            .insert(anchor.id.clone(), prepare_base);

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

    /// Clear speculative-overlay entries owned by the events of a finalized CF.
    ///
    /// Invariant F-A (docs/feat/overlay-clear-on-error-path/design.md):
    /// once a CF reaches a terminal post-consensus state (success OR any error),
    /// every overlay entry keyed by one of its event_ids MUST be removed before
    /// the function returns. This prevents stale speculative values from poisoning
    /// reads after the authoritative SMT path has diverged or aborted.
    fn clear_overlay_for_finalized(&self, events: &[Event]) {
        let ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        let _stats = self.shared.clear_overlay_events(&ids);
    }

    /// Same as `clear_overlay_for_finalized` but keyed directly by event-ids.
    /// Used on the follower MissingEvents path where the received `events` slice
    /// is incomplete — we use `cf.anchor.event_ids` (the authoritative list) instead.
    fn clear_overlay_for_finalized_ids(&self, ids: &[String]) {
        let _stats = self.shared.clear_overlay_events(ids);
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
            // F-A: clear overlay on SnapshotMismatch error path.
            let events = pending.all_events();
            self.clear_overlay_for_finalized(&events);
            return Err(AnchorBuildError::SnapshotMismatch {
                expected_depth: pending.pre_build_snapshot.anchor_depth,
                actual_depth: self.anchor_depth,
            });
        }

        // Apply state changes to SMT and commit in a single write lock
        let events = pending.all_events();
        let anchor_id = self.anchor_depth + 1;
        let cf_id = pending.anchor.id.clone();

        // DIAG (H2): look up the prepare-time base root recorded by
        // `prepare_build_internal`. We remove it unconditionally so the
        // sidecar does not leak entries across failed commits.
        #[cfg(feature = "diag-root-drift")]
        let prepare_base = self.prepare_base_roots.lock().remove(&cf_id);

        // Inner result lets us drop the write guard before running any overlay-clear
        // side effect on the error path (avoids holding two locks at once).
        let inner: Result<StateApplySummary, AnchorBuildError> = {
            let mut guard = self.shared.lock_write();

            // DIAG H2: compare prepare-time base vs commit-time base (under the
            // same lock that apply will run under). Divergence signals that a
            // non-CF writer mutated the write GSM between prepare and commit.
            #[cfg(feature = "diag-root-drift")]
            if let Some(prep) = prepare_base {
                let (commit_base, _) = (*guard).compute_global_root_bytes();
                if commit_base != prep {
                    tracing::error!(
                        target: "consensus::diag::leader_base_drift",
                        cf_id = %cf_id,
                        prepare_base = %hex::encode(prep),
                        commit_base  = %hex::encode(commit_base),
                        n_events = events.len(),
                        "DIAG H2: leader base state changed between prepare-clone and commit-lock"
                    );
                }
            }

            // Phase 3 H4 probes (design.md §5): record pre-apply base root,
            // CF event-list fingerprint, and per-event post-apply deltas so we
            // can pair leader vs follower evidence by cf_id.
            #[cfg(feature = "diag-root-drift")]
            Self::diag_h4_probes(&cf_id, "leader", &guard, &events);

            let summary = guard.apply_committed_events(&events);
            match guard.commit(anchor_id) {
                Ok(()) => {
                    // DIAG H1: after the real apply+commit, the write GSM's
                    // actual root must match what was declared in the anchor
                    // shipped to followers. Any divergence here is a direct
                    // root cause of follower RootMismatch.
                    #[cfg(feature = "diag-root-drift")]
                    match pending.anchor.merkle_roots.as_ref() {
                        Some(roots) => {
                            let (actual_root, _) = (*guard).compute_global_root_bytes();
                            if actual_root != roots.global_state_root {
                                tracing::error!(
                                    target: "consensus::diag::leader_root_self_mismatch",
                                    cf_id = %cf_id,
                                    declared_root = %hex::encode(roots.global_state_root),
                                    actual_root   = %hex::encode(actual_root),
                                    n_events = events.len(),
                                    legacy_anchor = false,
                                    "DIAG H1: leader declared state_root != actual state_root after real apply"
                                );
                            }
                        }
                        None => {
                            tracing::debug!(
                                target: "consensus::diag::leader_root_self_mismatch",
                                cf_id = %cf_id,
                                legacy_anchor = true,
                                "DIAG H1: skipped (legacy anchor has no merkle_roots)"
                            );
                        }
                    }

                    // DIAG P5 (cf_apply_progress): one structured INFO line per
                    // successful CF apply, keyed by (cf_id, role), so future
                    // finalize-progress triage can pair leader/follower roots
                    // by cf_id and detect lag (max anchor_id per node) from
                    // logs alone. See docs/feat/add-cf-apply-progress-diag/.
                    #[cfg(feature = "diag-root-drift")]
                    {
                        let (post_apply_root, _) = (*guard).compute_global_root_bytes();
                        tracing::info!(
                            target: "consensus::diag::cf_apply_progress",
                            cf_id = %cf_id,
                            role = "leader",
                            anchor_id = anchor_id,
                            n_events = events.len(),
                            post_apply_root = %hex::encode(post_apply_root),
                            "DIAG P5: CF applied"
                        );
                    }

                    // Publish snapshot while still holding Mutex (atomic consistency)
                    self.shared.publish_snapshot(&guard);
                    Ok(summary)
                }
                Err(e) => Err(e.into()),
            }
        };
        let state_summary = match inner {
            Ok(s) => s,
            Err(e) => {
                // F-A: clear overlay on commit-propagation error path.
                self.clear_overlay_for_finalized(&events);
                return Err(e);
            }
        };

        // R5: record per-event outcomes after apply (Leader path).
        self.ingest_outcomes(&cf_id, &events, &state_summary);

        // M4: CF finalized — clear speculative overlay entries owned by these events.
        // Other validators may not have staged anything (no-op for them), but the
        // validator that executed the MoveCall needs the entries removed so SMT
        // becomes the sole source of truth.
        let finalized_event_ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        let _cleared = self.shared.clear_overlay_events(&finalized_event_ids);

        // Update AnchorBuilder state
        self.last_anchor = Some(pending.anchor);
        self.anchor_depth = pending.new_anchor_depth;
        self.last_fold_vlc = pending.new_last_fold_vlc;
        self.last_anchor_chain_root = pending.new_anchor_chain_root;
        self.total_anchor_count += 1;
        self.last_fold_instant = Some(std::time::Instant::now());

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
            // F-A: clear overlay on MissingEvents error path.
            // Use cf.anchor.event_ids as the authoritative list since the received
            // `events` slice is incomplete by definition on this branch.
            self.clear_overlay_for_finalized_ids(&cf.anchor.event_ids);
            return Err(AnchorBuildError::MissingEvents {
                expected: cf.anchor.event_ids.len(),
                found: events.len(),
            });
        }

        // Hold write lock for the ENTIRE verify+commit to prevent race conditions.
        // Without this, concurrent CF proposals can clone the same base state,
        // and the second one's verification becomes stale after the first commits.
        let anchor_id = self.anchor_depth + 1;
        // Inner result lets us drop the write guard before any overlay-clear side
        // effect runs on the error path.
        let inner: Result<StateApplySummary, AnchorBuildError> = {
            let mut guard = self.shared.lock_write();

            // Phase 3 H4 probes (design.md §5): mirror leader-side probes on
            // the follower so `same_key_divergence.sh` can pair pre_apply_root,
            // events_fp, and event_apply_delta by cf_id across roles.
            #[cfg(feature = "diag-root-drift")]
            Self::diag_h4_probes(&cf.anchor.id, "follower", &guard, events);

            // 2. Verify state root (compute expected vs actual)
            if let Some(ref merkle_roots) = cf.anchor.merkle_roots {
                // Clone from write GSM under the lock
                let mut temp_manager = (*guard).clone();
                let verify_summary = temp_manager.apply_committed_events(events);
                let (expected_root, _) = temp_manager.compute_global_root_bytes();

                if expected_root != merkle_roots.global_state_root {
                    // DIAG (docs/bugs/20260422-stress-same-key-divergence.md):
                    // Dump per-event + per-conflict detail BEFORE returning so
                    // the first-cause investigation can compare three nodes'
                    // views of the same CF. overlay_stats() is captured here
                    // (still populated) — the F-A clear runs AFTER the guard
                    // drops below.
                    Self::log_follower_root_mismatch_diag(
                        &cf.anchor.id,
                        events,
                        &verify_summary,
                        &guard,
                        &expected_root,
                        &merkle_roots.global_state_root,
                        self.shared.overlay_stats(),
                    );
                    // Write GSM NOT mutated — F1 safety preserved
                    Err(AnchorBuildError::RootMismatch {
                        expected: expected_root,
                        actual: merkle_roots.global_state_root,
                    })
                } else {
                    // 3. Apply state changes and commit (same lock scope)
                    let summary = guard.apply_committed_events(events);

                    // DIAG H5 (R2-ISSUE-8): the verify-clone root matched the
                    // declared root, but the second apply runs on the real
                    // guard which may have been mutated by a non-CF writer
                    // between steps 1 and 3. Recompute the real root and
                    // alarm if it drifted.
                    #[cfg(feature = "diag-root-drift")]
                    {
                        let (post_apply_root, _) = (*guard).compute_global_root_bytes();
                        if post_apply_root != merkle_roots.global_state_root {
                            tracing::error!(
                                target: "consensus::diag::follower_post_apply_root_drift",
                                cf_id = %cf.anchor.id,
                                verify_root = %hex::encode(expected_root),
                                commit_root = %hex::encode(post_apply_root),
                                declared    = %hex::encode(merkle_roots.global_state_root),
                                n_events = events.len(),
                                "DIAG H5: follower verify_root != commit_root (post-real-apply drift from verify clone)"
                            );
                        }
                    }

                    match guard.commit(anchor_id) {
                        Ok(()) => {
                            // DIAG P5 (cf_apply_progress): see leader-side note.
                            #[cfg(feature = "diag-root-drift")]
                            {
                                let (post_apply_root, _) = (*guard).compute_global_root_bytes();
                                tracing::info!(
                                    target: "consensus::diag::cf_apply_progress",
                                    cf_id = %cf.anchor.id,
                                    role = "follower",
                                    anchor_id = anchor_id,
                                    n_events = events.len(),
                                    post_apply_root = %hex::encode(post_apply_root),
                                    "DIAG P5: CF applied"
                                );
                            }
                            self.shared.publish_snapshot(&guard);
                            Ok(summary)
                        }
                        Err(e) => Err(e.into()),
                    }
                }
            } else {
                // No merkle_roots to verify — apply directly
                let summary = guard.apply_committed_events(events);
                match guard.commit(anchor_id) {
                    Ok(()) => {
                        // DIAG P5 (cf_apply_progress): see leader-side note.
                        #[cfg(feature = "diag-root-drift")]
                        {
                            let (post_apply_root, _) = (*guard).compute_global_root_bytes();
                            tracing::info!(
                                target: "consensus::diag::cf_apply_progress",
                                cf_id = %cf.anchor.id,
                                role = "follower",
                                anchor_id = anchor_id,
                                n_events = events.len(),
                                post_apply_root = %hex::encode(post_apply_root),
                                "DIAG P5: CF applied (legacy anchor, no merkle_roots)"
                            );
                        }
                        self.shared.publish_snapshot(&guard);
                        Ok(summary)
                    }
                    Err(e) => Err(e.into()),
                }
            }
        };
        let state_summary = match inner {
            Ok(s) => s,
            Err(e) => {
                // F-A: clear overlay on RootMismatch / commit-propagation paths.
                // DIAG above has already captured pre-clear overlay_stats().
                self.clear_overlay_for_finalized(events);
                return Err(e);
            }
        };

        // R5: record per-event outcomes after apply (Follower path).
        self.ingest_outcomes(&cf.anchor.id, events, &state_summary);

        // M4: CF finalized — clear speculative overlay entries owned by these events.
        let finalized_event_ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        let _cleared = self.shared.clear_overlay_events(&finalized_event_ids);

        self.synchronize_finalized_anchor(&cf.anchor);

        Ok(state_summary)
    }

    /// DIAG only: Phase 3 H4 instrumentation. Emits three probes per CF on
    /// both leader and follower so that `tests/stress/same_key_divergence.sh`
    /// can pair evidence across roles by `cf_id`:
    ///
    /// - `consensus::diag::pre_apply_base_root` — GSM root under the real
    ///   write-lock, immediately before apply. If leader and follower disagree
    ///   on this value for the same cf_id, divergence predates this CF.
    /// - `consensus::diag::cf_event_fingerprint` — blake3 of the concatenated
    ///   event-ids in input order. If this differs across roles, consensus
    ///   delivery is non-deterministic (G1 violation).
    /// - `consensus::diag::event_apply_delta` — post-apply root on a sandbox
    ///   clone after every single event (in the same VLC-sorted order that
    ///   `apply_committed_events` will use). Identifies the first event at
    ///   which the two roles' roots diverge.
    ///
    /// Feature-gated on `diag-root-drift`; zero cost in production builds.
    /// See docs/feat/consensus-root-self-consistency/design.md \u00a75.
    #[cfg(feature = "diag-root-drift")]
    fn diag_h4_probes(
        cf_id: &str,
        role: &'static str,
        guard: &GlobalStateManager,
        events: &[Event],
    ) {
        // P1 — pre-apply base root
        let (pre_root, _) = guard.compute_global_root_bytes();
        tracing::info!(
            target: "consensus::diag::pre_apply_base_root",
            cf_id = %cf_id,
            role = %role,
            pre_apply_root = %hex::encode(pre_root),
            n_events = events.len(),
            "DIAG P1: pre-apply base root"
        );

        // P2 — CF event-list fingerprint (blake3 over concatenated event-ids)
        let mut hasher = blake3::Hasher::new();
        for ev in events {
            hasher.update(ev.id.as_bytes());
            hasher.update(b"|");
        }
        let events_fp = hasher.finalize();
        tracing::info!(
            target: "consensus::diag::cf_event_fingerprint",
            cf_id = %cf_id,
            role = %role,
            events_fp = %hex::encode(events_fp.as_bytes()),
            n_events = events.len(),
            "DIAG P2: CF event-list fingerprint"
        );

        // P3 — per-event post-apply deltas on a sandbox clone. Replicates the
        // VLC-then-id sort done by `apply_committed_events` so the logged
        // sequence matches production order exactly.
        let mut sorted: Vec<&Event> = events.iter().collect();
        sorted.sort_by(|a, b| {
            match a
                .vlc_snapshot
                .logical_time
                .cmp(&b.vlc_snapshot.logical_time)
            {
                std::cmp::Ordering::Equal => a.id.cmp(&b.id),
                other => other,
            }
        });
        let mut sandbox = guard.clone();
        for (idx, ev) in sorted.iter().enumerate() {
            let _ = sandbox.apply_committed_events(std::slice::from_ref(*ev));
            let (r, _) = sandbox.compute_global_root_bytes();
            tracing::debug!(
                target: "consensus::diag::event_apply_delta",
                cf_id = %cf_id,
                role = %role,
                event_idx = idx,
                event_id = %ev.id,
                event_type = ?ev.event_type,
                post_event_root = %hex::encode(r),
                "DIAG P3: per-event apply delta"
            );
        }
    }

    /// DIAG only: structured dump of a follower's RootMismatch.
    ///
    /// Used by `apply_follower_finalized_cf` to capture enough per-CF state
    /// for cross-node triage of
    /// `docs/bugs/20260422-stress-same-key-divergence.md`. Not called on the
    /// happy path.
    fn log_follower_root_mismatch_diag(
        cf_id: &str,
        events: &[Event],
        summary: &StateApplySummary,
        write_gsm: &GlobalStateManager,
        expected_root: &[u8; 32],
        actual_root: &[u8; 32],
        overlay_stats: setu_storage::state::OverlayStats,
    ) {
        use std::fmt::Write as _;

        let mut event_dump = String::new();
        for (idx, ev) in events.iter().enumerate() {
            let n_changes = ev
                .execution_result
                .as_ref()
                .map(|r| r.state_changes.len())
                .unwrap_or(0);
            let exec_ok = ev
                .execution_result
                .as_ref()
                .map(|r| r.success)
                .unwrap_or(false);
            let _ = write!(
                event_dump,
                "\n  [{idx}] id={id} type={ty:?} vlc_logical={vlc} exec_ok={exec_ok} n_changes={nc}",
                idx = idx,
                id = ev.id,
                ty = ev.event_type,
                vlc = ev.vlc_snapshot.logical_time,
                exec_ok = exec_ok,
                nc = n_changes,
            );
        }

        // For each conflicted event, dump expected_old vs current_SMT bytes
        // for the reported conflicting object.
        let mut conflict_dump = String::new();
        for rec in &summary.conflicted_events {
            let Some(ev) = events.iter().find(|e| e.id == rec.event_id) else {
                let _ = write!(
                    conflict_dump,
                    "\n  event_id={} object={} (event not in CF — should be impossible)",
                    rec.event_id, rec.conflicting_object
                );
                continue;
            };
            let sc = ev.execution_result.as_ref().and_then(|r| {
                r.state_changes
                    .iter()
                    .find(|sc| sc.key == rec.conflicting_object)
            });
            let (expected_old_hex, target_subnet, new_hex) = match sc {
                Some(sc) => {
                    let exp = sc
                        .old_value
                        .as_ref()
                        .map(|b| format!("Some[{} bytes]={}", b.len(), hex::encode(b)))
                        .unwrap_or_else(|| "None".to_string());
                    let newv = sc
                        .new_value
                        .as_ref()
                        .map(|b| format!("Some[{} bytes]={}", b.len(), hex::encode(b)))
                        .unwrap_or_else(|| "None".to_string());
                    let target = sc.target_subnet.unwrap_or(ev.get_subnet_id());
                    (exp, target, newv)
                }
                None => (
                    "<state_change not found>".to_string(),
                    ev.get_subnet_id(),
                    "<unknown>".to_string(),
                ),
            };
            // Look up current SMT value for the conflicting object
            let current_hex = rec
                .conflicting_object
                .strip_prefix("oid:")
                .and_then(|hex_str| hex::decode(hex_str).ok())
                .and_then(|bytes| {
                    if bytes.len() == 32 {
                        setu_merkle::HashValue::from_slice(&bytes).ok()
                    } else {
                        None
                    }
                })
                .map(|hv| {
                    write_gsm
                        .get_subnet(&target_subnet)
                        .and_then(|smt| smt.get(&hv))
                        .map(|v| format!("Some[{} bytes]={}", v.len(), hex::encode(v)))
                        .unwrap_or_else(|| "None".to_string())
                })
                .unwrap_or_else(|| "<malformed key>".to_string());
            let _ = write!(
                conflict_dump,
                "\n  event_id={} subnet={:?} object={}\n    expected_old={}\n    current_smt={}\n    proposed_new={}",
                rec.event_id,
                target_subnet,
                rec.conflicting_object,
                expected_old_hex,
                current_hex,
                new_hex,
            );
        }

        tracing::error!(
            target: "consensus::diag::follower_root_mismatch",
            cf_id = %cf_id,
            expected_root = %hex::encode(expected_root),
            actual_root   = %hex::encode(actual_root),
            n_events = events.len(),
            n_conflicted = summary.conflicted_events.len(),
            n_failed = summary.failed_events.len(),
            overlay_entries = overlay_stats.entry_count,
            overlay_unique_events = overlay_stats.unique_events,
            overlay_oldest_age_ms = overlay_stats
                .oldest_age
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            events = %event_dump,
            conflicts = %conflict_dump,
            "DIAG follower RootMismatch — see docs/bugs/20260422-stress-same-key-divergence.md"
        );
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

    /// Compute state root by applying events using the same deterministic logic
    /// as Follower verification (`apply_committed_events`).
    ///
    /// This ensures Leader and Follower always compute identical state roots:
    /// - Same base state source (write GSM)
    /// - Same event ordering (VLC sort inside `apply_committed_events`)
    /// - Same conflict detection (old_value mismatch → skip event)
    /// - Same genesis duplicate handling
    ///
    /// The write lock is held only for the duration of the clone, not during
    /// the actual computation.
    ///
    /// DIAG (`diag-root-drift`): the optional `out_prepare_base` out-parameter
    /// captures the write-GSM `global_state_root` *inside* the same lock that
    /// clones the manager, so `commit_build` can compare this pre-apply base
    /// against the post-lock base and detect H2 drift.
    fn compute_state_root_from_events(
        &self,
        events: &[Event],
        #[cfg(feature = "diag-root-drift")] out_prepare_base: &mut [u8; 32],
    ) -> ([u8; 32], HashMap<SubnetId, [u8; 32]>) {
        // Clone from write GSM (same base state as Follower verification)
        let mut temp_manager = {
            let guard = self.shared.lock_write();
            #[cfg(feature = "diag-root-drift")]
            {
                let (base, _) = (*guard).compute_global_root_bytes();
                *out_prepare_base = base;
            }
            (*guard).clone()
        };
        // Mutex released — computation is on a detached clone

        // Apply using identical logic to Follower:
        // VLC-sorted, conflict-detected, genesis-aware
        temp_manager.apply_committed_events(events);

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

    /// Time elapsed since the last CF was committed.
    /// Returns Duration::MAX if no CF has been committed yet (fresh start → heartbeat fires).
    pub fn elapsed_since_last_fold(&self) -> std::time::Duration {
        self.last_fold_instant
            .map(|t| t.elapsed())
            .unwrap_or(std::time::Duration::from_secs(u64::MAX))
    }

    /// Heartbeat variant of prepare_build: time-guarded pending-event flush.
    ///
    /// Conditions (ALL must be true):
    /// - elapsed_since_last_fold() > heartbeat_interval
    /// - after filtering in-flight CFs, at least one pending event remains
    /// - events.len() >= min_events_per_cf
    ///
    /// Unlike the regular fast path, heartbeat deliberately does not require
    /// a new VLC tick. Gamma-deferred same-key events can remain pending after
    /// a heartbeat CF commits, but they are old events; requiring `delta >= 1`
    /// would stall them until an unrelated new event arrives.
    ///
    /// Does NOT bypass min_events_per_cf — if DAG has 0 pending events, returns NoEvents.
    ///
    /// D1: uses the same pending-status selection as `prepare_build`.
    pub fn prepare_build_heartbeat(
        &self,
        dag: &Dag,
        vlc: &VLC,
        heartbeat_interval: std::time::Duration,
        in_flight_event_ids: &HashSet<EventId>,
    ) -> Result<PendingAnchorBuild, AnchorBuildError> {
        if self.elapsed_since_last_fold() < heartbeat_interval {
            let delta = vlc.logical_time().saturating_sub(self.last_fold_vlc);
            return Err(AnchorBuildError::DeltaNotReached {
                required: self.config.vlc_delta_threshold,
                current: delta,
            });
        }

        let to_depth = dag.max_depth();
        let events: Vec<Event> = dag
            .get_pending_events()
            .into_iter()
            .filter(|e| !in_flight_event_ids.contains(&e.id))
            .cloned()
            .collect();

        // γ + trim (mirror of `prepare_build`; design §3.4c)
        let (mut events, deferred_same_key) = apply_strict_same_key_fold_policy(events);
        let deferred_capacity = if events.len() > self.config.max_events_per_cf {
            events.split_off(self.config.max_events_per_cf)
        } else {
            Vec::new()
        };
        if !deferred_same_key.is_empty() || !deferred_capacity.is_empty() {
            tracing::info!(
                kept = events.len(),
                deferred_same_key = deferred_same_key.len(),
                deferred_capacity = deferred_capacity.len(),
                path = "heartbeat",
                "γ fold: deferred events"
            );
        }

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
        self.last_fold_instant = Some(std::time::Instant::now());
    }

    /// Get a subnet's current state root
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<[u8; 32]> {
        let snapshot = self.shared.load_snapshot();
        snapshot.get_subnet_root_bytes(subnet_id)
    }

    /// Get the current global state root
    pub fn get_global_root(&self) -> [u8; 32] {
        let snapshot = self.shared.load_snapshot();
        let (root, _) = snapshot.compute_global_root_bytes();
        root
    }

    /// Chain hash: combines previous chain root with new anchor hash
    ///
    /// Anchor chain append hash.
    ///
    /// new_root = BLAKE3("SETU_ANCHOR_CHAIN:" || prev_root || anchor_hash)
    ///
    /// Delegates to the canonical implementation in `hash_utils::chain_hash`.
    fn chain_hash(prev_root: &[u8; 32], anchor_hash: &[u8; 32]) -> [u8; 32] {
        setu_types::hash_utils::chain_hash(prev_root, anchor_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_storage::ConflictRecord;
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

    /// Generate a canonical "oid:{hex}" key for tests.
    /// Hashes the seed with BLAKE3 to produce a deterministic 32-byte ObjectId.
    fn test_oid_key(seed: &str) -> String {
        let hash = setu_types::hash_utils::setu_hash(seed.as_bytes());
        format!("oid:{}", hex::encode(hash))
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
                key: test_oid_key("balance:alice"),
                old_value: Some(vec![0; 8]),
                new_value: Some(vec![100; 8]),
                target_subnet: None,
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
                key: test_oid_key("balance:alice"),
                old_value: None,
                new_value: Some(vec![100; 8]),
                target_subnet: None,
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
                key: test_oid_key("balance:alice"),
                old_value: None,
                new_value: Some(vec![100; 8]),
                target_subnet: None,
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
                key: test_oid_key("balance:alice"),
                old_value: None,
                new_value: Some(vec![100; 8]),
                target_subnet: None,
            }],
        );
        let event2 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: test_oid_key("balance:bob"),
                old_value: None,
                new_value: Some(vec![200; 8]),
                target_subnet: None,
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
                    key: test_oid_key("balance:alice"),
                    old_value: None,
                    new_value: Some(vec![100; 8]),
                    target_subnet: None,
                }],
            ),
            create_event_with_result(
                app_subnet,
                vec![StateChange {
                    key: test_oid_key("nft:token1"),
                    old_value: None,
                    new_value: Some(vec![1; 32]),
                    target_subnet: None,
                }],
            ),
        ];

        // Prepare and commit
        let pending = builder.force_prepare_build(events, &vlc, 1).unwrap();

        // Verify pending has computed roots for both subnets
        let merkle_roots = pending.anchor.merkle_roots.as_ref().unwrap();
        assert_eq!(merkle_roots.subnet_roots.len(), 3); // ROOT + GOVERNANCE + app_subnet

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
                key: test_oid_key("balance:alice"),
                old_value: None,
                new_value: Some(vec![100; 8]),
                target_subnet: None,
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
                key: test_oid_key("balance:bob"),
                old_value: None,
                new_value: Some(vec![200; 8]),
                target_subnet: None,
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

    // ============================================
    // R5 · ingest_outcomes tests (U3–U8)
    // ============================================

    /// In-memory OutcomeSink that captures every `record()` call for
    /// assertions. Only used in tests.
    #[derive(Default)]
    struct CapturingSink {
        records: std::sync::Mutex<Vec<(String, ExecutionOutcome)>>,
    }

    impl CapturingSink {
        fn recorded(&self) -> Vec<(String, ExecutionOutcome)> {
            self.records.lock().unwrap().clone()
        }
    }

    impl OutcomeSink for CapturingSink {
        fn record(&self, event_id: String, outcome: ExecutionOutcome) {
            self.records.lock().unwrap().push((event_id, outcome));
        }
    }

    fn make_event_transfer(id_suffix: &str) -> Event {
        let mut ev = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            format!("creator-{id_suffix}"),
        );
        ev.id = format!("ev-{id_suffix}");
        ev.execution_result = Some(ExecutionResult::success());
        ev
    }

    /// U3: No sink wired → `ingest_outcomes` is a no-op (no panic, no record).
    #[test]
    fn test_ingest_outcomes_none_sink_noop() {
        let builder = AnchorBuilder::new(ConsensusConfig::default());
        let events = vec![make_event_transfer("a")];
        let summary = StateApplySummary::default();
        // Should not panic; nothing to observe.
        builder.ingest_outcomes("cf-x", &events, &summary);
    }

    /// U4: Applied path — no failure/conflict records → every event recorded
    /// as `Applied { cf_id }`.
    #[test]
    fn test_ingest_outcomes_applied() {
        let mut builder = AnchorBuilder::new(ConsensusConfig::default());
        let sink = Arc::new(CapturingSink::default());
        builder.set_outcomes_sink(sink.clone());

        let events = vec![make_event_transfer("a"), make_event_transfer("b")];
        let summary = StateApplySummary::default();
        builder.ingest_outcomes("cf-1", &events, &summary);

        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 2);
        for (_, outcome) in &recorded {
            assert_eq!(outcome.kind(), "applied");
            assert_eq!(outcome.cf_id(), "cf-1");
        }
    }

    /// U5: Conflicted event → `StaleRead` with populated `conflicting_object`
    /// and deterministic `retry_hint`.
    #[test]
    fn test_ingest_outcomes_stale_read() {
        let mut builder = AnchorBuilder::new(ConsensusConfig::default());
        let sink = Arc::new(CapturingSink::default());
        builder.set_outcomes_sink(sink.clone());

        let ev = make_event_transfer("a");
        let oid = test_oid_key("coin-1");
        let summary = StateApplySummary {
            conflicted_events: vec![ConflictRecord {
                event_id: ev.id.clone(),
                conflicting_object: oid.clone(),
            }],
            ..Default::default()
        };

        builder.ingest_outcomes("cf-2", &[ev.clone()], &summary);
        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 1);
        match &recorded[0].1 {
            ExecutionOutcome::StaleRead {
                cf_id,
                conflicting_object,
                retry_hint,
            } => {
                assert_eq!(cf_id, "cf-2");
                assert_eq!(conflicting_object, &oid);
                assert!(retry_hint.contains(&oid));
                assert!(retry_hint.contains("re-read and retry"));
            }
            other => panic!("expected StaleRead, got {:?}", other),
        }
    }

    /// U6: Failed event (execution_result.success=false) → `ExecutionFailed`
    /// with `reason` pulled from `event.execution_result.message`.
    #[test]
    fn test_ingest_outcomes_execution_failed() {
        let mut builder = AnchorBuilder::new(ConsensusConfig::default());
        let sink = Arc::new(CapturingSink::default());
        builder.set_outcomes_sink(sink.clone());

        let mut ev = make_event_transfer("a");
        ev.execution_result = Some(ExecutionResult::failure("insufficient funds"));
        let summary = StateApplySummary {
            failed_events: vec![ev.id.clone()],
            ..Default::default()
        };

        builder.ingest_outcomes("cf-3", &[ev.clone()], &summary);
        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 1);
        match &recorded[0].1 {
            ExecutionOutcome::ExecutionFailed { cf_id, reason } => {
                assert_eq!(cf_id, "cf-3");
                assert_eq!(reason.as_deref(), Some("insufficient funds"));
            }
            other => panic!("expected ExecutionFailed, got {:?}", other),
        }
    }

    /// U7: Genesis event — even if `conflicted_events` lists it (re-apply at
    /// startup), it must be recorded as `Applied` (R1-ISSUE-1 regression).
    #[test]
    fn test_ingest_outcomes_genesis_short_circuit() {
        let mut builder = AnchorBuilder::new(ConsensusConfig::default());
        let sink = Arc::new(CapturingSink::default());
        builder.set_outcomes_sink(sink.clone());

        let mut genesis = Event::new(
            EventType::Genesis,
            vec![],
            VLCSnapshot::default(),
            "bootstrap".to_string(),
        );
        genesis.id = "ev-genesis".to_string();

        // Adversarial: conflict record tries to mark Genesis as stale.
        let summary = StateApplySummary {
            conflicted_events: vec![ConflictRecord {
                event_id: genesis.id.clone(),
                conflicting_object: test_oid_key("system"),
            }],
            ..Default::default()
        };

        builder.ingest_outcomes("cf-genesis", &[genesis.clone()], &summary);
        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1.kind(), "applied");
    }

    /// U8: Mixed batch — Applied + StaleRead + ExecutionFailed in one
    /// invocation; each event gets its correct verdict.
    #[test]
    fn test_ingest_outcomes_mixed_batch() {
        let mut builder = AnchorBuilder::new(ConsensusConfig::default());
        let sink = Arc::new(CapturingSink::default());
        builder.set_outcomes_sink(sink.clone());

        let ev_ok = make_event_transfer("ok");
        let ev_conflict = make_event_transfer("conflict");
        let mut ev_failed = make_event_transfer("failed");
        ev_failed.execution_result = Some(ExecutionResult::failure("bad sig"));
        let oid = test_oid_key("coin-x");

        let summary = StateApplySummary {
            failed_events: vec![ev_failed.id.clone()],
            conflicted_events: vec![ConflictRecord {
                event_id: ev_conflict.id.clone(),
                conflicting_object: oid.clone(),
            }],
            ..Default::default()
        };

        builder.ingest_outcomes(
            "cf-mix",
            &[ev_ok.clone(), ev_conflict.clone(), ev_failed.clone()],
            &summary,
        );

        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 3);
        // Order preserved.
        assert_eq!(recorded[0].0, ev_ok.id);
        assert_eq!(recorded[0].1.kind(), "applied");
        assert_eq!(recorded[1].0, ev_conflict.id);
        assert_eq!(recorded[1].1.kind(), "stale_read");
        assert_eq!(recorded[2].0, ev_failed.id);
        assert_eq!(recorded[2].1.kind(), "execution_failed");
    }

    // ========================================================================
    // F-A (docs/feat/overlay-clear-on-error-path): on every terminal path of
    // commit_build / apply_follower_finalized_cf — success OR error — any
    // overlay entries owned by the CF's event-ids MUST be cleared before the
    // function returns.
    // ========================================================================

    use setu_types::{merkle::AnchorMerkleRoots, Anchor, ConsensusFrame};

    fn fa_make_event(id_suffix: &str, changes: Vec<StateChange>) -> Event {
        let mut ev = create_event_with_result(SubnetId::ROOT, changes);
        ev.id = format!("ev-fa-{id_suffix}");
        ev
    }

    fn fa_stage_overlay(
        shared: &std::sync::Arc<SharedStateManager>,
        event_id: &str,
        oid_seed: &str,
    ) {
        let key = test_oid_key(oid_seed);
        let change = StateChange::insert(key, vec![1, 2, 3]);
        shared
            .stage_overlay(event_id, SubnetId::ROOT, &[change])
            .expect("stage should succeed for canonical oid key");
    }

    fn fa_builder_with_overlay(n_entries: usize) -> (AnchorBuilder, Vec<Event>) {
        let builder = AnchorBuilder::new(ConsensusConfig::default());
        let shared = builder.shared_state_manager();
        let mut events = Vec::with_capacity(n_entries);
        for i in 0..n_entries {
            let key = test_oid_key(&format!("fa-coin-{i}"));
            let change = StateChange::insert(key, vec![i as u8; 4]);
            let ev = fa_make_event(&format!("{i}"), vec![change.clone()]);
            fa_stage_overlay(&shared, &ev.id, &format!("fa-coin-{i}"));
            events.push(ev);
        }
        assert_eq!(
            builder.shared_state_manager().overlay_stats().entry_count,
            n_entries
        );
        (builder, events)
    }

    fn fa_make_cf(events: &[Event], global_state_root: [u8; 32], depth: u64) -> ConsensusFrame {
        let event_ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        let anchor = Anchor::with_merkle_roots(
            event_ids,
            VLCSnapshot::default(),
            AnchorMerkleRoots::with_roots([0u8; 32], global_state_root, [0u8; 32]),
            None,
            depth,
        );
        ConsensusFrame::new(anchor, "v1".to_string())
    }

    /// T1: RootMismatch on follower path clears overlay.
    #[test]
    fn fa_apply_follower_root_mismatch_clears_overlay() {
        let (mut builder, events) = fa_builder_with_overlay(2);
        // Deliberately-wrong global_state_root — any non-empty SMT diff produces
        // a different root than our all-0xFF value.
        let cf = fa_make_cf(&events, [0xFFu8; 32], 1);

        let result = builder.apply_follower_finalized_cf(&events, &cf);
        assert!(
            matches!(result, Err(AnchorBuildError::RootMismatch { .. })),
            "expected RootMismatch, got {result:?}"
        );
        assert_eq!(
            builder.shared_state_manager().overlay_stats().entry_count,
            0,
            "overlay entries must be cleared after RootMismatch"
        );
    }

    /// T2: MissingEvents on follower path clears overlay for ALL cf.anchor.event_ids
    /// (not just the ones we happen to have received).
    #[test]
    fn fa_apply_follower_missing_events_clears_overlay() {
        let (mut builder, events) = fa_builder_with_overlay(2);
        // CF claims 2 events but we only forward 1 → MissingEvents.
        // Build CF with a non-mismatching (zero) state root to isolate the
        // MissingEvents branch from RootMismatch. merkle_roots presence doesn't
        // matter because the length check runs first.
        let cf = fa_make_cf(&events, [0u8; 32], 1);
        let partial: Vec<Event> = events[..1].to_vec();

        let result = builder.apply_follower_finalized_cf(&partial, &cf);
        assert!(
            matches!(result, Err(AnchorBuildError::MissingEvents { .. })),
            "expected MissingEvents, got {result:?}"
        );
        assert_eq!(
            builder.shared_state_manager().overlay_stats().entry_count,
            0,
            "overlay entries for BOTH event-ids must be cleared via cf.anchor.event_ids"
        );
    }

    /// T3: SnapshotMismatch on leader path clears overlay.
    #[test]
    fn fa_commit_build_snapshot_mismatch_clears_overlay() {
        let (mut builder, events) = fa_builder_with_overlay(2);
        let vlc = create_vlc("n1", 10);
        let mut pending = builder
            .force_prepare_build(events.clone(), &vlc, 0)
            .expect("prepare should succeed");
        // Corrupt the pre-build snapshot so verify_snapshot fails deterministically.
        pending.pre_build_snapshot.anchor_depth = 99;

        let result = builder.commit_build(pending);
        assert!(
            matches!(result, Err(AnchorBuildError::SnapshotMismatch { .. })),
            "expected SnapshotMismatch, got {result:?}"
        );
        assert_eq!(
            builder.shared_state_manager().overlay_stats().entry_count,
            0,
            "overlay entries must be cleared after SnapshotMismatch"
        );
    }

    /// T4 (regression): leader happy-path commit still clears overlay.
    #[test]
    fn fa_commit_build_success_still_clears_overlay() {
        let (mut builder, events) = fa_builder_with_overlay(2);
        let vlc = create_vlc("n1", 10);
        let pending = builder
            .force_prepare_build(events.clone(), &vlc, 0)
            .expect("prepare should succeed");

        let result = builder.commit_build(pending);
        assert!(
            result.is_ok(),
            "leader commit should succeed, got {result:?}"
        );
        assert_eq!(
            builder.shared_state_manager().overlay_stats().entry_count,
            0,
            "success path must still clear overlay (M4 invariant)"
        );
    }

    /// T5 (regression): follower happy-path apply still clears overlay.
    #[test]
    fn fa_apply_follower_success_still_clears_overlay() {
        // Build an expected root by running force_prepare_build on a throwaway
        // builder that shares no state with the one under test.
        let scratch = AnchorBuilder::new(ConsensusConfig::default());
        let events_template: Vec<Event> = (0..2)
            .map(|i| {
                let key = test_oid_key(&format!("fa-coin-{i}"));
                fa_make_event(
                    &format!("{i}"),
                    vec![StateChange::insert(key, vec![i as u8; 4])],
                )
            })
            .collect();
        let vlc = create_vlc("n1", 10);
        let pending = scratch
            .force_prepare_build(events_template.clone(), &vlc, 0)
            .expect("scratch prepare");
        let expected_root = pending
            .anchor
            .merkle_roots
            .as_ref()
            .expect("merkle_roots present")
            .global_state_root;

        // Now set up the follower builder with overlay entries keyed by the SAME event-ids.
        let follower = AnchorBuilder::new(ConsensusConfig::default());
        let shared = follower.shared_state_manager();
        for (i, ev) in events_template.iter().enumerate() {
            fa_stage_overlay(&shared, &ev.id, &format!("fa-coin-{i}"));
        }
        assert_eq!(
            follower.shared_state_manager().overlay_stats().entry_count,
            2
        );

        let cf = fa_make_cf(&events_template, expected_root, 1);
        let mut follower = follower;
        let result = follower.apply_follower_finalized_cf(&events_template, &cf);
        assert!(
            result.is_ok(),
            "follower apply should succeed, got {result:?}"
        );
        assert_eq!(
            follower.shared_state_manager().overlay_stats().entry_count,
            0,
            "success path must still clear overlay (M4 invariant)"
        );
    }

    /// T6: Mixed sequence — success then error both leave overlay empty.
    /// Guards against regressions where one helper forgets to clear.
    #[test]
    fn fa_clear_is_idempotent_across_success_and_error() {
        let builder = AnchorBuilder::new(ConsensusConfig::default());
        let shared = builder.shared_state_manager();

        // Round 1: stage + SnapshotMismatch error
        let ev_a = fa_make_event(
            "round1-a",
            vec![StateChange::insert(test_oid_key("r1a"), vec![1])],
        );
        fa_stage_overlay(&shared, &ev_a.id, "r1a");
        assert_eq!(shared.overlay_stats().entry_count, 1);
        let mut builder = builder;
        let vlc = create_vlc("n1", 10);
        let mut pending = builder
            .force_prepare_build(vec![ev_a.clone()], &vlc, 0)
            .expect("prepare");
        pending.pre_build_snapshot.anchor_depth = 42;
        assert!(builder.commit_build(pending).is_err());
        assert_eq!(shared.overlay_stats().entry_count, 0, "after error round");

        // Round 2: stage + success
        let ev_b = fa_make_event(
            "round2-b",
            vec![StateChange::insert(test_oid_key("r2b"), vec![2])],
        );
        fa_stage_overlay(&shared, &ev_b.id, "r2b");
        assert_eq!(shared.overlay_stats().entry_count, 1);
        let pending_ok = builder
            .force_prepare_build(vec![ev_b], &vlc, builder.anchor_depth)
            .expect("prepare 2");
        assert!(builder.commit_build(pending_ok).is_ok());
        assert_eq!(shared.overlay_stats().entry_count, 0, "after success round");
    }

    // ========================================================================
    // D1 (docs/feat/anchor-builder-fold-policy): pending-status selection
    // ========================================================================
    //
    // These tests exercise the contract that `prepare_build` selects events
    // from `Dag::get_pending_events()` (not a depth range), filtered by
    // `in_flight_event_ids`. Each test uses a low VLC-delta threshold so that
    // the delta gate does not interfere with the selection-path under test.

    use crate::dag::Dag as ConsensusDag;

    fn d1_config() -> ConsensusConfig {
        ConsensusConfig {
            vlc_delta_threshold: 1,
            min_events_per_cf: 1,
            ..Default::default()
        }
    }

    /// Create a minimal transfer event with a fixed id. No parents → genesis-like.
    fn d1_make_event(id: &str) -> Event {
        let mut ev = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "creator".to_string(),
        );
        ev.id = id.to_string();
        ev.execution_result = Some(ExecutionResult::success());
        ev
    }

    /// Insert an event directly into the DAG via the (`pub(crate)`)
    /// `add_event_with_depth`, so we can force any depth independent of
    /// parent arithmetic. This is the test-only escape hatch used to
    /// simulate the TOCTOU race described in design.md §2.1.
    fn d1_insert_at_depth(dag: &mut ConsensusDag, id: &str, depth: u64) {
        // Clear parents so add_event_with_depth does not try to index children.
        let mut ev = d1_make_event(id);
        ev.parent_ids.clear();
        dag.add_event_with_depth(ev, depth)
            .expect("add_event_with_depth");
    }

    /// T4: CORE artefact scenario. Builder's `anchor_depth = 11` while
    /// events sit at depth 3–5 (below `anchor_depth`, exactly what the
    /// `from_depth=11 to_depth=4` log showed). Pre-D1 code returned
    /// `InsufficientEvents`; post-D1 must fold them.
    #[test]
    fn d1_prepare_build_includes_stranded_events() {
        let mut builder = AnchorBuilder::new(d1_config());
        builder.anchor_depth = 11;

        let mut dag = ConsensusDag::new();
        d1_insert_at_depth(&mut dag, "stranded-a", 3);
        d1_insert_at_depth(&mut dag, "stranded-b", 4);
        d1_insert_at_depth(&mut dag, "stranded-c", 5);

        let vlc = create_vlc("n1", 20);
        let in_flight: HashSet<EventId> = HashSet::new();

        let pending = builder
            .prepare_build(&dag, &vlc, &in_flight)
            .expect("should fold stranded events");
        let folded_ids: HashSet<EventId> = pending.anchor.event_ids.iter().cloned().collect();
        assert_eq!(folded_ids.len(), 3, "all 3 stranded events must be folded");
        assert!(folded_ids.contains("stranded-a"));
        assert!(folded_ids.contains("stranded-b"));
        assert!(folded_ids.contains("stranded-c"));
    }

    /// T5: `in_flight_event_ids` filter excludes events already referenced
    /// by in-flight CFs.
    #[test]
    fn d1_prepare_build_excludes_in_flight_cf_events() {
        let builder = AnchorBuilder::new(d1_config());

        let mut dag = ConsensusDag::new();
        d1_insert_at_depth(&mut dag, "p1", 1);
        d1_insert_at_depth(&mut dag, "p2", 1);
        d1_insert_at_depth(&mut dag, "p3", 1);

        let vlc = create_vlc("n1", 10);
        let mut in_flight: HashSet<EventId> = HashSet::new();
        in_flight.insert("p2".to_string());

        let pending = builder
            .prepare_build(&dag, &vlc, &in_flight)
            .expect("should fold non-in-flight events");
        let folded_ids: HashSet<EventId> = pending.anchor.event_ids.iter().cloned().collect();
        assert_eq!(folded_ids.len(), 2);
        assert!(folded_ids.contains("p1"));
        assert!(folded_ids.contains("p3"));
        assert!(
            !folded_ids.contains("p2"),
            "in-flight event must be excluded"
        );
    }

    /// T6: Events already finalized in the DAG are skipped (they are no
    /// longer in `dag.pending`).
    #[test]
    fn d1_prepare_build_excludes_finalized_events() {
        let builder = AnchorBuilder::new(d1_config());

        let mut dag = ConsensusDag::new();
        d1_insert_at_depth(&mut dag, "f1", 1);
        d1_insert_at_depth(&mut dag, "p1", 1);
        dag.finalize_events(&["f1".to_string()]);

        let vlc = create_vlc("n1", 10);
        let in_flight: HashSet<EventId> = HashSet::new();

        let pending = builder
            .prepare_build(&dag, &vlc, &in_flight)
            .expect("should fold the one remaining pending event");
        let folded_ids: HashSet<EventId> = pending.anchor.event_ids.iter().cloned().collect();
        assert_eq!(folded_ids.len(), 1);
        assert!(folded_ids.contains("p1"));
        assert!(
            !folded_ids.contains("f1"),
            "finalized event must be excluded"
        );
    }

    /// T7: All pending events are in-flight → `InsufficientEvents`.
    #[test]
    fn d1_prepare_build_insufficient_when_all_in_flight() {
        let builder = AnchorBuilder::new(d1_config());

        let mut dag = ConsensusDag::new();
        d1_insert_at_depth(&mut dag, "a", 1);
        d1_insert_at_depth(&mut dag, "b", 1);

        let vlc = create_vlc("n1", 10);
        let mut in_flight: HashSet<EventId> = HashSet::new();
        in_flight.insert("a".to_string());
        in_flight.insert("b".to_string());

        let err = builder
            .prepare_build(&dag, &vlc, &in_flight)
            .expect_err("should error when no selectable events remain");
        assert!(
            matches!(
                err,
                AnchorBuildError::InsufficientEvents {
                    required: 1,
                    found: 0
                }
            ),
            "expected InsufficientEvents, got {err:?}"
        );
    }

    /// T8: Empty DAG pending set → `InsufficientEvents` (min_events_per_cf=1).
    #[test]
    fn d1_prepare_build_insufficient_on_empty_pending() {
        let builder = AnchorBuilder::new(d1_config());
        let dag = ConsensusDag::new();
        let vlc = create_vlc("n1", 10);
        let in_flight: HashSet<EventId> = HashSet::new();

        let err = builder
            .prepare_build(&dag, &vlc, &in_flight)
            .expect_err("empty pending set → error");
        assert!(
            matches!(
                err,
                AnchorBuildError::InsufficientEvents {
                    required: 1,
                    found: 0
                }
            ),
            "expected InsufficientEvents, got {err:?}"
        );
    }

    /// T9: Pending-set iteration is non-deterministic, but the resulting
    /// `events_root` and `global_state_root` must be deterministic across
    /// runs — the VLC-sort inside prepare_build_internal normalises the
    /// order. Insertion sequence A then B must give the same roots as
    /// B then A.
    #[test]
    fn d1_prepare_build_determinism_via_vlc_sort() {
        let vlc = create_vlc("n1", 10);
        let in_flight: HashSet<EventId> = HashSet::new();

        let build_with_order = |order: [&str; 3]| -> ([u8; 32], [u8; 32]) {
            let builder = AnchorBuilder::new(d1_config());
            let mut dag = ConsensusDag::new();
            for (i, id) in order.iter().enumerate() {
                d1_insert_at_depth(&mut dag, id, (i + 1) as u64);
            }
            let pending = builder
                .prepare_build(&dag, &vlc, &in_flight)
                .expect("prepare_build");
            let events_root = pending.events_root;
            let merkle = pending.anchor.merkle_roots.expect("merkle_roots");
            (events_root, merkle.global_state_root)
        };

        let (er1, gr1) = build_with_order(["a", "b", "c"]);
        let (er2, gr2) = build_with_order(["c", "a", "b"]);
        let (er3, gr3) = build_with_order(["b", "c", "a"]);

        // events_root: computed from events in input order. The selection
        // path returns them in HashSet order, which is arbitrary. If this
        // assertion fails, prepare_build_internal needs to sort events
        // before computing events_root (determinism gap). We leave this
        // assertion strict so the gap becomes visible if it exists.
        assert_eq!(
            er1, er2,
            "events_root must be stable across insertion orders"
        );
        assert_eq!(
            er2, er3,
            "events_root must be stable across insertion orders"
        );

        // global_state_root goes through compute_state_root_from_events
        // which already VLC-sorts internally; it must be stable.
        assert_eq!(
            gr1, gr2,
            "global_state_root must be stable across insertion orders"
        );
        assert_eq!(
            gr2, gr3,
            "global_state_root must be stable across insertion orders"
        );
    }

    // ------------------------------------------------------------------
    // γ — Strict Same-Key CF Fold Policy
    // docs/feat/strict-same-key-cf-fold/
    // Tests #1–#16 from design.md §5 / test matrix.
    // ------------------------------------------------------------------

    fn gamma_config(max_events: usize) -> ConsensusConfig {
        ConsensusConfig {
            vlc_delta_threshold: 1,
            min_events_per_cf: 1,
            max_events_per_cf: max_events,
            ..Default::default()
        }
    }

    /// Build a success-result event with a fixed id, a specific logical_time,
    /// and a set of state-change keys targeting `subnet`.
    fn gamma_make_event(id: &str, subnet: SubnetId, logical_time: u64, keys: &[&str]) -> Event {
        let mut ev = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "creator".to_string(),
        );
        ev.id = id.to_string();
        ev.vlc_snapshot.logical_time = logical_time;
        ev = ev.with_subnet(subnet);
        let state_changes: Vec<StateChange> = keys
            .iter()
            .map(|k| StateChange {
                key: test_oid_key(k),
                old_value: None,
                new_value: Some(vec![1u8; 8]),
                target_subnet: None,
            })
            .collect();
        ev.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes,
        });
        ev
    }

    // --- #1 ---
    #[test]
    fn gamma_empty_events_returns_empty() {
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![]);
        assert!(kept.is_empty());
        assert!(deferred.is_empty());
    }

    // --- #2 ---
    #[test]
    fn gamma_disjoint_keys_all_kept() {
        let events = vec![
            gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]),
            gamma_make_event("e2", SubnetId::ROOT, 2, &["B"]),
            gamma_make_event("e3", SubnetId::ROOT, 3, &["C"]),
        ];
        let (kept, deferred) = apply_strict_same_key_fold_policy(events);
        assert_eq!(kept.len(), 3);
        assert!(deferred.is_empty());
    }

    // --- #3 ---
    #[test]
    fn gamma_same_key_two_events_lower_vlc_wins() {
        let e1 = gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]);
        let e2 = gamma_make_event("e2", SubnetId::ROOT, 2, &["A"]);
        // Feed out of order to force γ's internal sort to matter.
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e2.clone(), e1.clone()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "e1");
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].id, "e2");
    }

    // --- #4 ---
    #[test]
    fn gamma_same_key_equal_vlc_lower_event_id_wins() {
        let e_big = gamma_make_event("zzz", SubnetId::ROOT, 5, &["A"]);
        let e_small = gamma_make_event("aaa", SubnetId::ROOT, 5, &["A"]);
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e_big, e_small]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "aaa");
        assert_eq!(deferred[0].id, "zzz");
    }

    // --- #5 MergeThenTransfer: single event, same key twice ---
    #[test]
    fn gamma_merge_then_transfer_single_event_kept() {
        let mut ev = gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]);
        // add a SECOND state_change writing the same key (different new_value).
        let dup = StateChange {
            key: test_oid_key("A"),
            old_value: Some(vec![1u8; 8]),
            new_value: Some(vec![2u8; 8]),
            target_subnet: None,
        };
        ev.execution_result
            .as_mut()
            .unwrap()
            .state_changes
            .push(dup);

        let keys = collect_event_write_keys(&ev);
        assert_eq!(
            keys.len(),
            1,
            "duplicate same-key state_changes must dedupe"
        );

        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![ev]);
        assert_eq!(kept.len(), 1);
        assert!(deferred.is_empty());
    }

    // --- #6 failed execution_result does not claim keys ---
    #[test]
    fn gamma_failed_execution_result_not_blocking() {
        let mut e1 = gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]);
        e1.execution_result.as_mut().unwrap().success = false;
        let e2 = gamma_make_event("e2", SubnetId::ROOT, 2, &["A"]);
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e1, e2]);
        assert_eq!(kept.len(), 2);
        assert!(deferred.is_empty());
    }

    // --- #7 no execution_result ---
    #[test]
    fn gamma_no_execution_result_not_blocking() {
        let mut e1 = gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]);
        e1.execution_result = None;
        let e2 = gamma_make_event("e2", SubnetId::ROOT, 2, &["A"]);
        let (kept, _deferred) = apply_strict_same_key_fold_policy(vec![e1, e2]);
        assert_eq!(
            kept.len(),
            2,
            "events with no execution_result claim no keys"
        );
    }

    // --- #8 cross-subnet target: same ObjectId in different subnets ---
    #[test]
    fn gamma_cross_subnet_target_subnet_respected() {
        let subnet_a = SubnetId::ROOT;
        let subnet_b = SubnetId::new_app_simple(7);
        // Both write key "A", but into different subnets → disjoint ClaimedWriteKey.
        let e1 = gamma_make_event("e1", subnet_a, 1, &["A"]);
        let e2 = gamma_make_event("e2", subnet_b, 2, &["A"]);
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e1, e2]);
        assert_eq!(kept.len(), 2);
        assert!(deferred.is_empty());
    }

    // --- #9 chain deferral of three same-key events ---
    #[test]
    fn gamma_chain_deferral_three_same_key_events() {
        let events = vec![
            gamma_make_event("e1", SubnetId::ROOT, 1, &["A"]),
            gamma_make_event("e2", SubnetId::ROOT, 2, &["A"]),
            gamma_make_event("e3", SubnetId::ROOT, 3, &["A"]),
        ];
        let (kept, deferred) = apply_strict_same_key_fold_policy(events);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "e1");
        let deferred_ids: HashSet<_> = deferred.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            deferred_ids,
            ["e2".to_string(), "e3".to_string()].into_iter().collect()
        );
    }

    // --- #10 multi-key write blocks on any intersect ---
    #[test]
    fn gamma_multi_key_write_blocks_any_intersect() {
        let e1 = gamma_make_event("e1", SubnetId::ROOT, 1, &["A", "B"]);
        let e2 = gamma_make_event("e2", SubnetId::ROOT, 2, &["B", "C"]);
        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e1, e2]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "e1");
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].id, "e2");
    }

    // --- #11 governance target_subnet unification ---
    // Both events target GOVERNANCE writing the same key → second deferred.
    #[test]
    fn gamma_governance_target_subnet_unification() {
        // E1: subnet=ROOT, explicit target_subnet=GOVERNANCE for its state_change
        let mut e1 = Event::new(
            EventType::Governance,
            vec![],
            VLCSnapshot::default(),
            "creator".to_string(),
        );
        e1.id = "e1".to_string();
        e1.vlc_snapshot.logical_time = 1;
        e1 = e1.with_subnet(SubnetId::ROOT);
        e1.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![StateChange {
                key: test_oid_key("G"),
                old_value: None,
                new_value: Some(vec![1u8; 8]),
                target_subnet: Some(SubnetId::GOVERNANCE),
            }],
        });

        // E2: subnet=GOVERNANCE, no explicit target → defaults to event subnet
        let e2 = gamma_make_event("e2", SubnetId::GOVERNANCE, 2, &["G"]);

        let (kept, deferred) = apply_strict_same_key_fold_policy(vec![e1, e2]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "e1");
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].id, "e2");
    }

    // --- #12 Inv-γ-Sort: permutation invariant ---
    #[test]
    fn gamma_sort_stability_permutation_invariant() {
        let make_input = || {
            vec![
                gamma_make_event("e3", SubnetId::ROOT, 3, &["A"]),
                gamma_make_event("e1", SubnetId::ROOT, 1, &["B"]),
                gamma_make_event("e2", SubnetId::ROOT, 2, &["A"]),
                gamma_make_event("e4", SubnetId::ROOT, 4, &["C"]),
            ]
        };
        let (kept_a, def_a) = apply_strict_same_key_fold_policy(make_input());
        let mut perm = make_input();
        perm.reverse();
        let (kept_b, def_b) = apply_strict_same_key_fold_policy(perm);
        let ids_a: Vec<_> = kept_a.iter().map(|e| e.id.clone()).collect();
        let ids_b: Vec<_> = kept_b.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids_a, ids_b, "kept order must be permutation-invariant");
        let def_ids_a: Vec<_> = def_a.iter().map(|e| e.id.clone()).collect();
        let def_ids_b: Vec<_> = def_b.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            def_ids_a, def_ids_b,
            "deferred order must be permutation-invariant"
        );
    }

    // --- #13 prepare_build defers same-key events across rounds ---
    #[test]
    fn gamma_prepare_build_defers_same_key() {
        use crate::dag::Dag as ConsensusDag;
        let builder = AnchorBuilder::new(gamma_config(100));
        let mut dag = ConsensusDag::new();
        for i in 0..3 {
            let mut ev = gamma_make_event(
                &format!("e{}", i + 1),
                SubnetId::ROOT,
                (i + 1) as u64,
                &["HOT"],
            );
            ev.parent_ids.clear();
            dag.add_event_with_depth(ev, (i + 1) as u64)
                .expect("add_event_with_depth");
        }
        let initial_pending = dag.get_pending_count();
        assert_eq!(initial_pending, 3);

        let mut vlc = VLC::new("node1".to_string());
        for _ in 0..10 {
            vlc.tick();
        }

        // Round 1: γ keeps exactly 1 event
        let empty_in_flight: HashSet<EventId> = HashSet::new();
        let pending1 = builder
            .prepare_build(&dag, &vlc, &empty_in_flight)
            .expect("round 1 prepare_build");
        assert_eq!(
            pending1.anchor.event_ids.len(),
            1,
            "γ must keep exactly 1 same-key event"
        );
        // DAG pending is NOT mutated by γ — still 3
        assert_eq!(
            dag.get_pending_count(),
            initial_pending,
            "γ must not mutate DAG pending"
        );

        // Round 2: same-DAG call without in_flight mask → same event kept again (deterministic)
        let pending2 = builder
            .prepare_build(&dag, &vlc, &empty_in_flight)
            .expect("round 2 prepare_build");
        assert_eq!(pending2.anchor.event_ids, pending1.anchor.event_ids);

        // Round 3: mask out the Round-1 event → γ must now pick a different one
        let mut mask: HashSet<EventId> = HashSet::new();
        for id in &pending1.anchor.event_ids {
            mask.insert(id.clone());
        }
        let pending3 = builder
            .prepare_build(&dag, &vlc, &mask)
            .expect("round 3 prepare_build");
        assert_eq!(pending3.anchor.event_ids.len(), 1);
        assert_ne!(pending3.anchor.event_ids[0], pending1.anchor.event_ids[0]);
    }

    // --- #14 max_events_per_cf respected by prepare_build (R4-1 bug fix) ---
    // Core assertion: when candidate set > max_events_per_cf, anchor.event_ids
    // length must equal max_events_per_cf AND events_root/state_root must be
    // consistent with that same trimmed slice (no more pre-existing trim bug).
    #[test]
    fn gamma_prepare_build_respects_max_events_per_cf() {
        use crate::dag::Dag as ConsensusDag;
        let builder = AnchorBuilder::new(gamma_config(3));
        let mut dag = ConsensusDag::new();
        // 5 disjoint-key events — none will be γ-deferred, only capacity-trimmed.
        for i in 0..5 {
            let mut ev = gamma_make_event(
                &format!("e{}", i + 1),
                SubnetId::ROOT,
                (i + 1) as u64,
                &[&format!("K{}", i)],
            );
            ev.parent_ids.clear();
            dag.add_event_with_depth(ev, (i + 1) as u64)
                .expect("add_event_with_depth");
        }

        let mut vlc = VLC::new("node1".to_string());
        for _ in 0..10 {
            vlc.tick();
        }

        let empty_in_flight: HashSet<EventId> = HashSet::new();
        let pending = builder
            .prepare_build(&dag, &vlc, &empty_in_flight)
            .expect("prepare_build");

        assert_eq!(
            pending.anchor.event_ids.len(),
            3,
            "anchor.event_ids must be trimmed to max_events_per_cf",
        );

        // Recompute events_root from the event_ids in anchor; must match.
        // This is the assertion that fails under the pre-existing trim bug:
        // old code computed events_root on 5 events but only listed 3 event_ids.
        let kept_events: Vec<Event> = {
            let mut out = Vec::new();
            for id in &pending.anchor.event_ids {
                let ev = dag.get_event(id).expect("event in dag").clone();
                out.push(ev);
            }
            out
        };
        let recomputed = compute_events_root(&kept_events);
        assert_eq!(
            *recomputed.as_bytes(),
            pending.events_root,
            "events_root must be computed on exactly the anchor.event_ids slice (pre-existing trim bug regression guard)",
        );
    }

    // --- #15 heartbeat path applies γ + trim ---
    #[test]
    fn gamma_heartbeat_path_applies_filter() {
        use crate::dag::Dag as ConsensusDag;
        let builder = AnchorBuilder::new(gamma_config(100));
        let mut dag = ConsensusDag::new();
        for i in 0..2 {
            let mut ev = gamma_make_event(
                &format!("hb{}", i + 1),
                SubnetId::ROOT,
                (i + 1) as u64,
                &["HK"],
            );
            ev.parent_ids.clear();
            dag.add_event_with_depth(ev, (i + 1) as u64)
                .expect("add_event_with_depth");
        }

        let mut vlc = VLC::new("node1".to_string());
        for _ in 0..5 {
            vlc.tick();
        }

        let empty_in_flight: HashSet<EventId> = HashSet::new();
        // Fresh builder → elapsed_since_last_fold == Duration::MAX; any heartbeat_interval satisfied.
        let pending = builder
            .prepare_build_heartbeat(
                &dag,
                &vlc,
                std::time::Duration::from_millis(1),
                &empty_in_flight,
            )
            .expect("heartbeat prepare");
        assert_eq!(
            pending.anchor.event_ids.len(),
            1,
            "heartbeat γ keeps 1 of 2 same-key events"
        );
    }

    // --- #16 heartbeat refolds gamma-deferred same-key events without a new VLC tick ---
    #[test]
    fn gamma_heartbeat_refolds_deferred_same_key_without_new_vlc_tick() {
        use crate::dag::Dag as ConsensusDag;

        let mut builder = AnchorBuilder::new(gamma_config(100));
        let mut dag = ConsensusDag::new();
        for i in 0..2 {
            let mut ev = gamma_make_event(
                &format!("hb-stall-{}", i + 1),
                SubnetId::ROOT,
                (i + 1) as u64,
                &["HOT"],
            );
            ev.parent_ids.clear();
            dag.add_event_with_depth(ev, (i + 1) as u64)
                .expect("add_event_with_depth");
        }

        let mut vlc = VLC::new("node1".to_string());
        for _ in 0..5 {
            vlc.tick();
        }

        let empty_in_flight: HashSet<EventId> = HashSet::new();
        let first = builder
            .prepare_build_heartbeat(
                &dag,
                &vlc,
                std::time::Duration::from_millis(0),
                &empty_in_flight,
            )
            .expect("first heartbeat prepare");
        assert_eq!(first.anchor.event_ids.len(), 1);

        let first_ids = first.anchor.event_ids.clone();
        builder.commit_build(first).expect("commit first heartbeat");
        dag.finalize_events(&first_ids);

        let second = builder
            .prepare_build_heartbeat(
                &dag,
                &vlc,
                std::time::Duration::from_millis(0),
                &empty_in_flight,
            )
            .expect("gamma-deferred event should refold without new VLC tick");

        assert_eq!(second.anchor.event_ids.len(), 1);
        assert_ne!(
            second.anchor.event_ids[0], first_ids[0],
            "second heartbeat must select the deferred same-key event",
        );
    }

    // --- #17 force_prepare_build bypasses γ (still trims) ---
    #[test]
    fn gamma_force_prepare_build_bypasses_policy() {
        let builder = AnchorBuilder::new(gamma_config(10));
        let e1 = gamma_make_event("f1", SubnetId::ROOT, 1, &["SAME"]);
        let e2 = gamma_make_event("f2", SubnetId::ROOT, 2, &["SAME"]);
        let mut vlc = VLC::new("node1".to_string());
        for _ in 0..3 {
            vlc.tick();
        }
        let pending = builder
            .force_prepare_build(vec![e1, e2], &vlc, 1)
            .expect("force_prepare_build");
        assert_eq!(
            pending.anchor.event_ids.len(),
            2,
            "force_prepare_build must bypass γ and keep both same-key events",
        );
    }
}
