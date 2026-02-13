use setu_types::{
    Anchor, ConsensusConfig, ConsensusFrame, EventId, Vote,
};
use crate::anchor_builder::{AnchorBuilder, AnchorBuildResult, AnchorBuildError, PendingAnchorBuild};
use crate::dag::Dag;
use crate::pocw::fold_observer::FoldObserver;
use crate::vlc::VLC;
use setu_storage::subnet_state::GlobalStateManager;
use std::collections::HashMap;

/// Decision outcome for a ConsensusFrame
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CFDecision {
    Finalize,  // 2/3+1 approve votes
    Reject,    // 1/3+1 reject votes
    Timeout,   // Exceeded timeout threshold
}

/// Legacy DagFolder - kept for backward compatibility
/// For new code, use AnchorBuilder directly or through ConsensusManager
#[derive(Debug)]
pub struct DagFolder {
    config: ConsensusConfig,
    last_anchor: Option<Anchor>,
    anchor_depth: u64,
    last_fold_vlc: u64,
}

impl DagFolder {
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            config,
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
        }
    }

    pub fn should_fold(&self, current_vlc: &VLC) -> bool {
        let delta = current_vlc.logical_time().saturating_sub(self.last_fold_vlc);
        delta >= self.config.vlc_delta_threshold
    }

    pub fn fold(&mut self, dag: &Dag, vlc: &VLC, state_root: String) -> Option<Anchor> {
        if !self.should_fold(vlc) {
            return None;
        }

        let from_depth = self.anchor_depth;
        let to_depth = dag.max_depth();

        let events = dag.get_events_in_range(from_depth, to_depth);
        
        if events.len() < self.config.min_events_per_cf {
            return None;
        }

        let event_ids: Vec<EventId> = events
            .iter()
            .take(self.config.max_events_per_cf)
            .map(|e| e.id.clone())
            .collect();

        let anchor = Anchor::new(
            event_ids,
            vlc.snapshot(),
            state_root,
            self.last_anchor.as_ref().map(|a| a.id.clone()),
            to_depth,
        );

        self.last_anchor = Some(anchor.clone());
        self.anchor_depth = to_depth + 1;
        self.last_fold_vlc = vlc.logical_time();

        Some(anchor)
    }

    pub fn last_anchor(&self) -> Option<&Anchor> {
        self.last_anchor.as_ref()
    }

    pub fn anchor_depth(&self) -> u64 {
        self.anchor_depth
    }
}

/// ConsensusManager with integrated AnchorBuilder for Merkle tree management
/// 
/// This manager handles:
/// - Anchor creation with full Merkle tree computation (via AnchorBuilder)
/// - ConsensusFrame creation, voting, and finalization
/// - State management across all subnets
///
/// ## Deferred Commit Mode
/// 
/// Uses a deferred commit pattern for safe state management:
/// - `try_create_cf()` calls `prepare_build()` which computes but doesn't modify state
/// - On finalization, `commit_build()` applies the pending state changes
/// - On rejection/timeout, pending_builds are simply discarded (no rollback needed)
pub struct ConsensusManager {
    config: ConsensusConfig,
    /// AnchorBuilder handles DAG folding with Merkle tree updates
    anchor_builder: AnchorBuilder,
    /// Legacy folder (kept for backward compatibility, not used in main flow)
    #[allow(dead_code)]
    legacy_folder: DagFolder,
    /// Pending ConsensusFrames awaiting votes
    pending_cfs: HashMap<String, ConsensusFrame>,
    /// Pending anchor builds awaiting finalization (cf_id -> PendingAnchorBuild)
    pending_builds: HashMap<String, PendingAnchorBuild>,
    /// Finalized ConsensusFrames
    finalized_cfs: Vec<ConsensusFrame>,
    /// Set of anchor IDs that have been persisted to storage
    /// Used to safely garbage collect finalized_cfs
    persisted_anchor_ids: std::collections::HashSet<String>,
    /// This validator's ID
    local_validator_id: String,
    /// Last build result for diagnostics
    last_build_result: Option<AnchorBuildResult>,
    /// PoCW fold observer (active when PoCWConfig::enabled)
    fold_observer: Option<FoldObserver>,
}

impl ConsensusManager {
    /// Create a new ConsensusManager with AnchorBuilder
    pub fn new(config: ConsensusConfig, validator_id: String) -> Self {
        let fold_observer = config.pocw
            .filter(|p| p.enabled)
            .map(FoldObserver::new);
        Self {
            config: config.clone(),
            anchor_builder: AnchorBuilder::new(config.clone()),
            legacy_folder: DagFolder::new(config),
            pending_cfs: HashMap::new(),
            pending_builds: HashMap::new(),
            finalized_cfs: Vec::new(),
            persisted_anchor_ids: std::collections::HashSet::new(),
            local_validator_id: validator_id,
            last_build_result: None,
            fold_observer,
        }
    }
    
    /// Create with an existing GlobalStateManager (for state persistence)
    pub fn with_state_manager(
        config: ConsensusConfig,
        validator_id: String,
        state_manager: GlobalStateManager,
    ) -> Self {
        let fold_observer = config.pocw
            .filter(|p| p.enabled)
            .map(FoldObserver::new);
        Self {
            config: config.clone(),
            anchor_builder: AnchorBuilder::with_state_manager(config.clone(), state_manager),
            legacy_folder: DagFolder::new(config),
            pending_cfs: HashMap::new(),
            pending_builds: HashMap::new(),
            finalized_cfs: Vec::new(),
            persisted_anchor_ids: std::collections::HashSet::new(),
            local_validator_id: validator_id,
            last_build_result: None,
            fold_observer,
        }
    }

    /// Try to create a ConsensusFrame with full Merkle tree computation
    /// 
    /// Uses deferred commit mode:
    /// 1. Calls prepare_build() which computes but doesn't modify state
    /// 2. Stores PendingAnchorBuild for later commit on finalization
    /// 3. Creates ConsensusFrame for voting
    /// 
    /// Note: The CF is NOT added to pending_cfs here. The caller should
    /// call receive_cf() to add it and vote for it.
    pub fn try_create_cf(
        &mut self,
        dag: &Dag,
        vlc: &VLC,
    ) -> Option<ConsensusFrame> {
        // Use AnchorBuilder.prepare_build (deferred commit mode)
        match self.anchor_builder.prepare_build(dag, vlc) {
            Ok(pending_build) => {
                let anchor = pending_build.anchor.clone();
                
                // Create ConsensusFrame from anchor
                let cf = ConsensusFrame::new(anchor, self.local_validator_id.clone());
                
                // Store pending_build for later commit (keyed by cf_id)
                self.pending_builds.insert(cf.id.clone(), pending_build);
                // NOTE: Do NOT add to pending_cfs here - let receive_cf() handle it
                
                Some(cf)
            }
            Err(AnchorBuildError::DeltaNotReached { .. }) => None,
            Err(AnchorBuildError::InsufficientEvents { .. }) => None,
            Err(AnchorBuildError::NoEvents) => None,
            Err(e) => {
                // Log error but don't crash
                eprintln!("AnchorBuilder error: {}", e);
                None
            }
        }
    }

    /// Check if a CF (pending or finalized) already exists
    pub fn has_cf(&self, cf_id: &str) -> bool {
        self.pending_cfs.contains_key(cf_id) ||
            self.finalized_cfs.iter().any(|cf| cf.id == cf_id)
    }

    pub fn receive_cf(&mut self, cf: ConsensusFrame) {
        if !self.pending_cfs.contains_key(&cf.id) {
            self.pending_cfs.insert(cf.id.clone(), cf);
        }
    }

    /// Vote for a ConsensusFrame
    /// 
    /// Args:
    /// - cf_id: The CF ID to vote for
    /// - approve: Whether to approve (true) or reject (false)
    /// - private_key: Optional private key for signing the vote (32 bytes for ed25519)
    /// 
    /// Returns the vote if successful, None if:
    /// - CF not found
    /// - Already voted for this CF
    pub fn vote_for_cf(
        &mut self, 
        cf_id: &str, 
        approve: bool,
        private_key: Option<&[u8]>
    ) -> Option<Vote> {
        let cf = self.pending_cfs.get_mut(cf_id)?;
        
        if cf.votes.contains_key(&self.local_validator_id) {
            return None;
        }

        let mut vote = Vote::new(self.local_validator_id.clone(), cf_id.to_string(), approve);
        
        // Sign the vote if private key is provided
        if let Some(key) = private_key {
            if let Err(e) = vote.sign(key) {
                tracing::warn!(
                    cf_id = %cf_id,
                    error = %e,
                    "Failed to sign vote - continuing without signature for backward compatibility"
                );
            }
        }
        
        cf.add_vote(vote.clone());
        
        Some(vote)
    }

    /// Receive a vote from another validator
    /// 
    /// Returns true if the CF is finalized after this vote.
    /// Duplicate votes from the same validator are ignored (idempotent).
    pub fn receive_vote(&mut self, vote: Vote) -> bool {
        let cf_id = vote.cf_id.clone();
        let voter_id = vote.validator_id.clone();
        
        if let Some(cf) = self.pending_cfs.get_mut(&cf_id) {
            // Skip if this validator already voted (idempotency)
            if cf.votes.contains_key(&voter_id) {
                return false;
            }
            cf.add_vote(vote);
        } else {
            return false;
        }
        self.check_finalization(&cf_id)
    }

    /// Check if a CF has reached quorum (finalize), rejection threshold (reject), or timeout
    /// 
    /// This is called after adding a vote to check if finalization/rejection should occur.
    /// Public because engine.receive_cf() needs to check after vote_for_cf().
    /// 
    /// Returns true if CF was finalized or rejected (removed from pending).
    pub fn check_finalization(&mut self, cf_id: &str) -> bool {
        let decision = {
            let cf = match self.pending_cfs.get(cf_id) {
                Some(cf) => cf,
                None => return false,
            };
            
            // Check if CF should be finalized (2/3+1 approve)
            if cf.check_quorum(self.config.validator_count) {
                Some(CFDecision::Finalize)
            }
            // Check if CF should be rejected (1/3+1 reject)
            else if cf.check_rejection(self.config.validator_count) {
                Some(CFDecision::Reject)
            }
            // Check if CF has timed out
            else if cf.is_timeout(self.config.cf_timeout_ms) {
                Some(CFDecision::Timeout)
            } else {
                None  // still pending
            }
        };

        match decision {
            Some(CFDecision::Finalize) => {
                if let Some(mut cf) = self.pending_cfs.remove(cf_id) {
                    cf.finalize();
                    
                    // Check if this is our CF (we have a pending_build for it)
                    if let Some(pending_build) = self.pending_builds.remove(cf_id) {
                        // Leader path: commit the pending build
                        match self.anchor_builder.commit_build(pending_build.clone()) {
                            Ok(state_summary) => {
                                // PoCW: observe the committed fold
                                if let Some(ref mut observer) = self.fold_observer {
                                    let fold_events = pending_build.all_events();
                                    observer.on_fold_committed(&fold_events);
                                }

                                // Store result for diagnostics
                                self.last_build_result = Some(AnchorBuildResult {
                                    anchor: cf.anchor.clone(),
                                    state_summary,
                                    routed_events: pending_build.routed_events,
                                });
                            }
                            Err(AnchorBuildError::SnapshotMismatch { .. }) => {
                                // Another CF was committed first - use Follower path
                                eprintln!("Snapshot mismatch during commit, falling back to follower path");
                                let events = pending_build.all_events();
                                if let Err(e) = self.anchor_builder.apply_follower_finalized_cf(&events, &cf) {
                                    eprintln!("Follower fallback failed: {}, syncing metadata only", e);
                                    self.anchor_builder.synchronize_finalized_anchor(&cf.anchor);
                                }
                            }
                            Err(e) => {
                                // Other error - sync metadata at minimum
                                eprintln!("Commit failed: {}, syncing metadata only", e);
                                self.anchor_builder.synchronize_finalized_anchor(&cf.anchor);
                            }
                        }
                    } else {
                        // Follower path: we didn't create this CF
                        // For now, just sync metadata. Full state sync requires events from DAG
                        self.anchor_builder.synchronize_finalized_anchor(&cf.anchor);
                    }
                    
                    self.finalized_cfs.push(cf);
                    
                    // Trigger safe garbage collection
                    self.gc_finalized_cfs();
                    
                    return true;
                }
            }
            Some(CFDecision::Reject) | Some(CFDecision::Timeout) => {
                // Remove rejected/timeout CF from pending
                if let Some(mut cf) = self.pending_cfs.remove(cf_id) {
                    // Simply discard the pending_build if it exists (no rollback needed!)
                    self.pending_builds.remove(cf_id);
                    cf.reject();
                    return true;
                }
            }
            None => {}
        }
        false
    }
    
    /// Mark an anchor as persisted to storage
    /// 
    /// Call this after successfully storing the anchor to AnchorStore.
    /// This enables safe garbage collection of finalized CFs.
    pub fn mark_anchor_persisted(&mut self, anchor_id: &str) {
        self.persisted_anchor_ids.insert(anchor_id.to_string());
        // Note: persisted_anchor_ids is cleaned up when corresponding CFs are GC'd
        // in gc_finalized_cfs(), so it won't grow unbounded
    }
    
    /// Garbage collect finalized CFs, only removing those that have been persisted
    fn gc_finalized_cfs(&mut self) {
        const MAX_FINALIZED_CFS: usize = 1000;
        
        if self.finalized_cfs.len() <= MAX_FINALIZED_CFS {
            return;
        }
        
        let excess = self.finalized_cfs.len() - MAX_FINALIZED_CFS;
        
        // Collect anchor IDs that will be removed (for cleaning persisted_anchor_ids)
        let mut removed_anchor_ids = Vec::new();
        let mut removed_count = 0;
        
        // Only remove CFs that have been persisted
        self.finalized_cfs.retain(|cf| {
            if removed_count >= excess {
                return true;
            }
            if self.persisted_anchor_ids.contains(&cf.anchor.id) {
                removed_anchor_ids.push(cf.anchor.id.clone());
                removed_count += 1;
                false // remove this CF
            } else {
                true // keep unpersisted CF
            }
        });
        
        // Clean up persisted_anchor_ids for removed CFs
        for id in removed_anchor_ids {
            self.persisted_anchor_ids.remove(&id);
        }
        
        // Safety valve: if too many unpersisted CFs, log warning but don't drop
    }
    
    /// Clean up pending CFs that have timed out
    /// 
    /// This prevents memory leaks from CFs that never reach quorum due to:
    /// - Network partitions
    /// - Node failures
    /// - Insufficient votes
    /// 
    /// Should be called periodically (e.g., every few seconds) by the consensus engine.
    /// Returns the number of CFs that were removed.
    pub fn cleanup_timeout_cfs(&mut self) -> usize {
        let timeout_ms = self.config.cf_timeout_ms;
        let timeout_ids: Vec<String> = self.pending_cfs
            .iter()
            .filter(|(_, cf)| cf.is_timeout(timeout_ms))
            .map(|(id, _)| id.clone())
            .collect();
        
        let count = timeout_ids.len();
        for id in timeout_ids {
            if let Some(mut cf) = self.pending_cfs.remove(&id) {
                // Simply discard the pending_build (no rollback needed in deferred commit mode!)
                self.pending_builds.remove(&id);
                cf.reject();
            }
        }
        count
    }
    
    /// Get the last finalized anchor (for storage)
    pub fn get_last_finalized_anchor(&self) -> Option<setu_types::Anchor> {
        self.finalized_cfs.last().map(|cf| cf.anchor.clone())
    }

    pub fn get_pending_cf(&self, cf_id: &str) -> Option<&ConsensusFrame> {
        self.pending_cfs.get(cf_id)
    }

    pub fn finalized_count(&self) -> usize {
        self.finalized_cfs.len()
    }

    pub fn last_finalized_cf(&self) -> Option<&ConsensusFrame> {
        self.finalized_cfs.last()
    }

    pub fn should_fold(&self, vlc: &VLC) -> bool {
        self.anchor_builder.should_fold(vlc)
    }
    
    // =========================================================================
    // New methods for Merkle tree access
    // =========================================================================
    
    /// Get the AnchorBuilder (read-only)
    pub fn anchor_builder(&self) -> &AnchorBuilder {
        &self.anchor_builder
    }
    
    /// Get the AnchorBuilder (mutable)
    pub fn anchor_builder_mut(&mut self) -> &mut AnchorBuilder {
        &mut self.anchor_builder
    }
    
    /// Get the GlobalStateManager (read-only)
    pub fn state_manager(&self) -> &GlobalStateManager {
        self.anchor_builder.state_manager()
    }
    
    /// Get the GlobalStateManager (mutable)
    pub fn state_manager_mut(&mut self) -> &mut GlobalStateManager {
        self.anchor_builder.state_manager_mut()
    }
    
    /// Get the last build result (for diagnostics)
    pub fn last_build_result(&self) -> Option<&AnchorBuildResult> {
        self.last_build_result.as_ref()
    }
    
    /// Get a subnet's current state root
    pub fn get_subnet_root(&self, subnet_id: &setu_types::SubnetId) -> Option<[u8; 32]> {
        self.anchor_builder.get_subnet_root(subnet_id)
    }
    
    /// Get the current global state root
    pub fn get_global_root(&self) -> [u8; 32] {
        self.anchor_builder.get_global_root()
    }
    
    /// Get anchor count
    pub fn anchor_count(&self) -> usize {
        self.anchor_builder.anchor_count()
    }
    
    // =========================================================================
    // Follower State Synchronization
    // =========================================================================
    
    /// Apply state changes from a received ConsensusFrame (follower path)
    /// 
    /// When a follower receives a CF from the leader, it needs to apply
    /// the same state changes to maintain consistency. This method:
    /// 1. Gets the events referenced in the CF's anchor
    /// 2. Applies their state changes to the local SMT
    /// 3. Verifies the resulting state root matches the anchor's merkle_roots
    /// 
    /// Returns true if state was applied and verified successfully.
    pub fn apply_cf_state_changes(&mut self, dag: &Dag, cf: &setu_types::ConsensusFrame) -> bool {
        // Get events from the anchor's event_ids
        let events: Vec<setu_types::Event> = cf.anchor.event_ids
            .iter()
            .filter_map(|id| dag.get_event(id).cloned())
            .collect();
        
        if events.is_empty() {
            // No events to apply, but anchor might be empty - check merkle roots
            return cf.anchor.merkle_roots.is_none() || 
                   cf.anchor.merkle_roots.as_ref()
                       .map(|r| r.global_state_root == self.get_global_root())
                       .unwrap_or(true);
        }
        
        // Apply state changes from these events
        let _ = self.anchor_builder.state_manager_mut().apply_committed_events(&events);
        
        // Verify the resulting global state root matches the anchor's
        if let Some(ref merkle_roots) = cf.anchor.merkle_roots {
            let local_root = self.get_global_root();
            if local_root != merkle_roots.global_state_root {
                eprintln!(
                    "State root mismatch! Local: {:?}, Anchor: {:?}",
                    hex::encode(local_root),
                    hex::encode(merkle_roots.global_state_root)
                );
                return false;
            }
        }
        
        true
    }
    
    /// Verify a ConsensusFrame's merkle roots without applying state
    /// 
    /// This is a lighter verification that just checks the anchor's
    /// merkle roots are internally consistent.
    pub fn verify_cf_merkle_roots(&self, cf: &setu_types::ConsensusFrame) -> bool {
        let Some(ref merkle_roots) = cf.anchor.merkle_roots else {
            // No merkle roots to verify (legacy anchor)
            return true;
        };
        
        // Verify events_root is not all zeros (unless no events)
        if cf.anchor.event_ids.is_empty() && merkle_roots.events_root != [0u8; 32] {
            return false;
        }
        
        // Verify global_state_root is not all zeros (should have at least ROOT subnet)
        if merkle_roots.global_state_root == [0u8; 32] && !merkle_roots.subnet_roots.is_empty() {
            return false;
        }
        
        // Verify subnet_roots contains at least ROOT subnet
        if !merkle_roots.subnet_roots.is_empty() {
            if !merkle_roots.subnet_roots.contains_key(&setu_types::SubnetId::ROOT) {
                return false;
            }
        }
        
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Event, EventType};

    fn create_vlc(node_id: &str, time: u64) -> VLC {
        let mut vlc = VLC::new(node_id.to_string());
        for _ in 0..time {
            vlc.tick();
        }
        vlc
    }

    fn setup_dag_with_events(count: usize) -> (Dag, VLC) {
        let mut dag = Dag::new();
        let mut vlc = VLC::new("node1".to_string());

        let genesis = Event::genesis("node1".to_string(), vlc.snapshot());
        let mut last_id = dag.add_event(genesis).unwrap();

        for _ in 1..count {
            vlc.tick();
            let event = Event::new(
                EventType::Transfer,
                vec![last_id.clone()],
                vlc.snapshot(),
                "node1".to_string(),
            );
            last_id = dag.add_event(event).unwrap();
        }

        (dag, vlc)
    }

    #[test]
    fn test_folder_should_fold() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            ..Default::default()
        };
        let folder = DagFolder::new(config);
        
        let vlc = create_vlc("node1", 5);
        assert!(!folder.should_fold(&vlc));

        let vlc = create_vlc("node1", 10);
        assert!(folder.should_fold(&vlc));
    }

    #[test]
    fn test_consensus_manager_create_cf() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            ..Default::default()
        };
        let mut manager = ConsensusManager::new(config, "validator1".to_string());
        let (dag, vlc) = setup_dag_with_events(10);

        // New API: try_create_cf without external state_root
        let cf = manager.try_create_cf(&dag, &vlc);
        assert!(cf.is_some());
        
        // Verify anchor has merkle_roots
        let cf = cf.unwrap();
        assert!(cf.anchor.merkle_roots.is_some());
    }
    
    #[test]
    fn test_consensus_manager_state_access() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            validator_count: 1,  // Single validator for immediate finalization
            ..Default::default()
        };
        let mut manager = ConsensusManager::new(config, "validator1".to_string());
        let (dag, vlc) = setup_dag_with_events(10);

        // Create CF (deferred commit mode - state not modified yet)
        let cf = manager.try_create_cf(&dag, &vlc);
        assert!(cf.is_some());
        let cf_id = cf.unwrap().id.clone();
        
        // State not committed yet (prepare_build only)
        assert_eq!(manager.anchor_count(), 0);
        
        // Vote to finalize (single validator, so immediate finalization)
        manager.vote_for_cf(&cf_id, true, None);
        let finalized = manager.check_finalization(&cf_id);
        assert!(finalized, "CF should be finalized with single validator");
        
        // Now state should be committed
        assert_eq!(manager.anchor_count(), 1);
        
        // Global root should be computed
        let global_root = manager.get_global_root();
        assert_ne!(global_root, [0u8; 32]);
    }
}
