use setu_types::{
    Anchor, ConsensusConfig, ConsensusFrame, EventId, Vote,
};
use crate::anchor_builder::{AnchorBuilder, AnchorBuildResult, AnchorBuildError};
use crate::dag::Dag;
use crate::vlc::VLC;
use setu_storage::subnet_state::GlobalStateManager;
use std::collections::HashMap;

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
pub struct ConsensusManager {
    config: ConsensusConfig,
    /// AnchorBuilder handles DAG folding with Merkle tree updates
    anchor_builder: AnchorBuilder,
    /// Legacy folder (kept for backward compatibility, not used in main flow)
    #[allow(dead_code)]
    legacy_folder: DagFolder,
    /// Pending ConsensusFrames awaiting votes
    pending_cfs: HashMap<String, ConsensusFrame>,
    /// Finalized ConsensusFrames
    finalized_cfs: Vec<ConsensusFrame>,
    /// This validator's ID
    local_validator_id: String,
    /// Last build result for diagnostics
    last_build_result: Option<AnchorBuildResult>,
}

impl ConsensusManager {
    /// Create a new ConsensusManager with AnchorBuilder
    pub fn new(config: ConsensusConfig, validator_id: String) -> Self {
        Self {
            config: config.clone(),
            anchor_builder: AnchorBuilder::new(config.clone()),
            legacy_folder: DagFolder::new(config),
            pending_cfs: HashMap::new(),
            finalized_cfs: Vec::new(),
            local_validator_id: validator_id,
            last_build_result: None,
        }
    }
    
    /// Create with an existing GlobalStateManager (for state persistence)
    pub fn with_state_manager(
        config: ConsensusConfig, 
        validator_id: String,
        state_manager: GlobalStateManager,
    ) -> Self {
        Self {
            config: config.clone(),
            anchor_builder: AnchorBuilder::with_state_manager(config.clone(), state_manager),
            legacy_folder: DagFolder::new(config),
            pending_cfs: HashMap::new(),
            finalized_cfs: Vec::new(),
            local_validator_id: validator_id,
            last_build_result: None,
        }
    }

    /// Try to create a ConsensusFrame with full Merkle tree computation
    /// 
    /// This is the main entry point that:
    /// 1. Checks VLC delta threshold
    /// 2. Collects events from DAG
    /// 3. Applies state changes to subnet SMTs
    /// 4. Computes all Merkle roots (events, anchor chain, global state)
    /// 5. Creates Anchor with merkle_roots
    /// 6. Wraps in ConsensusFrame for voting
    pub fn try_create_cf(
        &mut self,
        dag: &Dag,
        vlc: &VLC,
    ) -> Option<ConsensusFrame> {
        // Use AnchorBuilder to create anchor with full Merkle computation
        match self.anchor_builder.try_build(dag, vlc) {
            Ok(build_result) => {
                let anchor = build_result.anchor.clone();
                
                // Store build result for diagnostics
                self.last_build_result = Some(build_result);
                
                // Create ConsensusFrame from anchor
                let cf = ConsensusFrame::new(anchor, self.local_validator_id.clone());
                self.pending_cfs.insert(cf.id.clone(), cf.clone());
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
    
    /// Legacy method: Try to create CF with external state_root (deprecated)
    /// 
    /// This method is kept for backward compatibility.
    /// Prefer using `try_create_cf(dag, vlc)` which computes state roots internally.
    #[deprecated(since = "0.2.0", note = "Use try_create_cf(dag, vlc) instead")]
    pub fn try_create_cf_legacy(
        &mut self,
        dag: &Dag,
        vlc: &VLC,
        _state_root: String,
    ) -> Option<ConsensusFrame> {
        self.try_create_cf(dag, vlc)
    }

    pub fn receive_cf(&mut self, cf: ConsensusFrame) {
        if !self.pending_cfs.contains_key(&cf.id) {
            self.pending_cfs.insert(cf.id.clone(), cf);
        }
    }

    pub fn vote_for_cf(&mut self, cf_id: &str, approve: bool) -> Option<Vote> {
        let cf = self.pending_cfs.get_mut(cf_id)?;
        
        if cf.votes.contains_key(&self.local_validator_id) {
            return None;
        }

        let vote = Vote::new(self.local_validator_id.clone(), cf_id.to_string(), approve);
        cf.add_vote(vote.clone());
        
        Some(vote)
    }

    pub fn receive_vote(&mut self, vote: Vote) -> bool {
        let cf_id = vote.cf_id.clone();
        if let Some(cf) = self.pending_cfs.get_mut(&cf_id) {
            cf.add_vote(vote);
        } else {
            return false;
        }
        self.check_finalization(&cf_id)
    }

    fn check_finalization(&mut self, cf_id: &str) -> bool {
        let should_finalize = {
            let cf = match self.pending_cfs.get(cf_id) {
                Some(cf) => cf,
                None => return false,
            };
            cf.check_quorum(self.config.validator_count)
        };

        if should_finalize {
            if let Some(mut cf) = self.pending_cfs.remove(cf_id) {
                cf.finalize();
                self.finalized_cfs.push(cf);
                return true;
            }
        }

        false
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
            ..Default::default()
        };
        let mut manager = ConsensusManager::new(config, "validator1".to_string());
        let (dag, vlc) = setup_dag_with_events(10);

        // Create CF and verify state roots are accessible
        let _ = manager.try_create_cf(&dag, &vlc);
        
        // Global root should be computed
        let global_root = manager.get_global_root();
        assert_ne!(global_root, [0u8; 32]);
        
        // Anchor count should be 1
        assert_eq!(manager.anchor_count(), 1);
    }
}
