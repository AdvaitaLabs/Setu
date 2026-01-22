// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Consensus Engine
//!
//! The main consensus engine that orchestrates the DAG-based consensus process.
//! It integrates VLC-based timing, leader election, and ConsensusFrame management.
//!
//! ## Main Flow
//!
//! 1. Events enter the DAG from solvers (with TEE execution proofs)
//! 2. Validators verify execution results
//! 3. Each validator maintains a VLC clock
//! 4. Leader is selected via round-robin rotation
//! 5. When leader's VLC delta reaches threshold, it folds the DAG
//! 6. Other validators vote on the fold validity
//! 7. After quorum votes, the ConsensusFrame is finalized
//! 8. Next round begins with the finalized frame as anchor

use setu_types::{ConsensusConfig, ConsensusFrame, Event, EventId, SetuResult, Vote};
use setu_vlc::VLCSnapshot;
use setu_storage::subnet_state::GlobalStateManager;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Mutex};
use tracing::{debug, info, warn};

use crate::broadcaster::ConsensusBroadcaster;
use crate::dag::Dag;
use crate::folder::ConsensusManager;
use crate::liveness::Round;
use crate::validator_set::ValidatorSet;
use crate::vlc::VLC;

/// Messages exchanged between consensus components
#[derive(Debug, Clone)]
pub enum ConsensusMessage {
    /// New event added to DAG
    NewEvent(Event),
    /// Leader proposes a ConsensusFrame
    ProposeFrame(ConsensusFrame),
    /// Validator votes for a frame
    Vote(Vote),
    /// Frame has been finalized
    FrameFinalized(ConsensusFrame),
    /// Leader rotation occurred
    LeaderChanged { round: Round, new_leader: String },
}

/// The main consensus engine
pub struct ConsensusEngine {
    /// Configuration
    config: ConsensusConfig,
    /// The DAG storing all events
    dag: Arc<RwLock<Dag>>,
    /// Local VLC clock
    vlc: Arc<RwLock<VLC>>,
    /// Set of validators with leader election
    validator_set: Arc<RwLock<ValidatorSet>>,
    /// ConsensusFrame manager (folder)
    consensus_manager: Arc<RwLock<ConsensusManager>>,
    /// This validator's ID
    local_validator_id: String,
    /// Channel for sending consensus messages (legacy, for internal use)
    message_tx: mpsc::Sender<ConsensusMessage>,
    /// Channel for receiving consensus messages (reserved for future use)
    #[allow(dead_code)]
    message_rx: Arc<Mutex<mpsc::Receiver<ConsensusMessage>>>,
    /// Network broadcaster for P2P message delivery (optional)
    broadcaster: Arc<RwLock<Option<Arc<dyn ConsensusBroadcaster>>>>,
}

impl ConsensusEngine {
    /// Create a new consensus engine
    pub fn new(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);

        Self {
            config: config.clone(),
            dag: Arc::new(RwLock::new(Dag::new())),
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::new(
                config,
                validator_id.clone(),
            ))),
            local_validator_id: validator_id,
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create a new consensus engine with state manager for Merkle tree persistence
    /// 
    /// This constructor allows injecting a pre-configured GlobalStateManager
    /// with MerkleStore for persisting state roots.
    pub fn with_state_manager(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
        state_manager: GlobalStateManager,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);

        Self {
            config: config.clone(),
            dag: Arc::new(RwLock::new(Dag::new())),
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::with_state_manager(
                config,
                validator_id.clone(),
                state_manager,
            ))),
            local_validator_id: validator_id,
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the network broadcaster for P2P message delivery
    /// 
    /// This should be called after the network layer is initialized.
    /// Without a broadcaster, consensus messages are only sent to internal channels.
    pub async fn set_broadcaster(&self, broadcaster: Arc<dyn ConsensusBroadcaster>) {
        let mut b = self.broadcaster.write().await;
        *b = Some(broadcaster);
        info!("Consensus broadcaster configured");
    }
    
    /// Check if a broadcaster is configured
    pub async fn has_broadcaster(&self) -> bool {
        self.broadcaster.read().await.is_some()
    }

    /// Add an event to the DAG and try to create a CF if conditions are met
    pub async fn add_event(&self, event: Event) -> SetuResult<EventId> {
        // Update local VLC by merging with the event's VLC
        {
            let mut vlc = self.vlc.write().await;
            vlc.merge(&event.vlc_snapshot);
            vlc.tick();
        }

        // Add event to DAG
        let event_id = {
            let mut dag = self.dag.write().await;
            dag.add_event(event.clone())
                .map_err(|e| setu_types::SetuError::InvalidData(e.to_string()))?
        };

        // Broadcast the new event
        // We broadcast regardless of whether we are the leader, as all validators
        // need the event for their DAGs.
        {
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                // Background this to avoid blocking?
                // For now, we await it but log errors instead of failing.
                // Event propagation should be best-effort; state sync fixes gaps.
                if let Err(e) = b.broadcast_event(&event).await {
                    warn!(event_id = %event.id, error = %e, "Failed to broadcast event");
                } else {
                    debug!(event_id = %event.id, "Event broadcasted");
                }
            }
            
            // Still send to internal channel for backward compatibility or local monitoring
            let _ = self
                .message_tx
                .send(ConsensusMessage::NewEvent(event))
                .await;
        }

        // Try to create a ConsensusFrame if we're the leader
        self.try_create_cf().await?;

        Ok(event_id)
    }

    /// Receive an event from the network (does not broadcast again)
    ///
    /// This is used when receiving events from other validators.
    /// Unlike `add_event`, this does not broadcast the event again.
    pub async fn receive_event_from_network(&self, event: Event) -> SetuResult<EventId> {
        // Update local VLC by merging with the event's VLC
        {
            let mut vlc = self.vlc.write().await;
            vlc.merge(&event.vlc_snapshot);
            vlc.tick();
        }

        // Add event to DAG
        let event_id = {
            let mut dag = self.dag.write().await;
            dag.add_event(event.clone())
                .map_err(|e| setu_types::SetuError::InvalidData(e.to_string()))?
        };

        // Note: We do NOT broadcast the event here since it came from the network
        
        // Try to create a ConsensusFrame if we're the leader
        self.try_create_cf().await?;

        Ok(event_id)
    }

    /// Create a new event with the given parent IDs
    pub async fn create_event(&self, parent_ids: Vec<EventId>) -> SetuResult<Event> {
        let vlc_snapshot = {
            let mut vlc = self.vlc.write().await;
            vlc.tick();
            vlc.snapshot()
        };

        let event = Event::new(
            setu_types::EventType::Transfer,
            parent_ids,
            vlc_snapshot,
            self.local_validator_id.clone(),
        );

        Ok(event)
    }

    /// Check if this validator is the current leader
    pub async fn is_current_leader(&self) -> bool {
        let validator_set = self.validator_set.read().await;
        validator_set.is_leader(&self.local_validator_id)
    }

    /// Check if this validator is the valid proposer for a specific round
    pub async fn is_valid_proposer_for_round(&self, round: Round) -> bool {
        let validator_set = self.validator_set.read().await;
        validator_set.is_valid_proposer(&self.local_validator_id, round)
    }

    /// Get the current round
    pub async fn current_round(&self) -> Round {
        let validator_set = self.validator_set.read().await;
        validator_set.current_round()
    }

    /// Get the valid proposer for a specific round
    pub async fn get_valid_proposer(&self, round: Round) -> Option<String> {
        let validator_set = self.validator_set.read().await;
        validator_set.get_valid_proposer(round)
    }

    /// Advance to the next round
    pub async fn advance_round(&self) -> Round {
        let mut validator_set = self.validator_set.write().await;
        let new_round = validator_set.advance_round();

        // Notify about leader change
        if let Some(new_leader) = validator_set.get_leader_id() {
            let _ = self
                .message_tx
                .send(ConsensusMessage::LeaderChanged {
                    round: new_round,
                    new_leader: new_leader.clone(),
                })
                .await;
        }

        new_round
    }

    /// Try to create a ConsensusFrame if conditions are met
    async fn try_create_cf(&self) -> SetuResult<Option<ConsensusFrame>> {
        let _current_round = {
            let validator_set = self.validator_set.read().await;
            let round = validator_set.current_round();

            // Check if we are the valid proposer for the current round
            if !validator_set.is_valid_proposer(&self.local_validator_id, round) {
                return Ok(None);
            }
            round
        };

        let vlc = self.vlc.read().await;
        let mut manager = self.consensus_manager.write().await;

        if !manager.should_fold(&vlc) {
            return Ok(None);
        }

        let dag = self.dag.read().await;
        // AnchorBuilder now handles all Merkle tree computation internally
        let cf = manager.try_create_cf(&dag, &vlc);

        if let Some(ref frame) = cf {
            // Send to internal channel (legacy)
            let _ = self
                .message_tx
                .send(ConsensusMessage::ProposeFrame(frame.clone()))
                .await;
            
            // Broadcast to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_cf(frame).await {
                    Ok(result) => {
                        info!(
                            cf_id = %frame.id,
                            success = result.success_count,
                            total = result.total_peers,
                            "CF broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %frame.id, error = %e, "Failed to broadcast CF");
                    }
                }
            } else {
                debug!(cf_id = %frame.id, "No broadcaster configured, CF not sent to network");
            }
        }

        Ok(cf)
    }

    /// Receive a ConsensusFrame from another validator (Follower path)
    /// 
    /// When a follower receives a CF from the leader:
    /// 1. Verify proposer is valid for current round
    /// 2. Check if we've already processed this CF (idempotency)
    /// 3. **Ensure all referenced events are in local DAG (fetch if missing)**
    /// 4. Verify the CF's merkle roots are valid
    /// 5. Apply the state changes from the anchor's events to local SMT
    /// 6. Verify resulting state matches the anchor's state root
    /// 7. Vote for the CF
    /// 8. Check if our vote causes finalization
    /// 
    /// Returns (finalized, Option<Anchor>) so the caller can persist when finalized.
    /// This ensures followers have consistent state with the leader.
    pub async fn receive_cf(&self, cf: ConsensusFrame) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        // Step 1: Verify proposer is valid for current round
        // This prevents malicious nodes from creating fake CFs
        {
            let validator_set = self.validator_set.read().await;
            let current_round = validator_set.current_round();
            if !validator_set.is_valid_proposer(&cf.proposer, current_round) {
                return Err(setu_types::SetuError::InvalidData(
                    format!("CF proposer {} is not valid for round {}", cf.proposer, current_round)
                ));
            }
        }
        
        // Step 2: Idempotency check (before acquiring write lock)
        {
            let manager = self.consensus_manager.read().await;
            if manager.has_cf(&cf.id) {
                return Ok((false, None));
            }
        }
        
        // Step 3: Ensure all referenced events are available in local DAG
        // This is critical for state consistency - we cannot verify or apply
        // state changes without having all the events.
        {
            let dag = self.dag.read().await;
            let missing_event_ids: Vec<EventId> = cf.anchor.event_ids
                .iter()
                .filter(|id| !dag.contains(id))
                .cloned()
                .collect();
            
            if !missing_event_ids.is_empty() {
                // Release dag lock before network call
                drop(dag);
                
                // Try to fetch missing events from peers
                let broadcaster = self.broadcaster.read().await;
                if let Some(ref b) = *broadcaster {
                    info!(
                        cf_id = %cf.id,
                        missing_count = missing_event_ids.len(),
                        "Fetching missing events for CF"
                    );
                    
                    match b.request_events(&missing_event_ids).await {
                        Ok(fetched_events) => {
                            // Add fetched events to DAG
                            let mut dag = self.dag.write().await;
                            for event in fetched_events {
                                // Update VLC when adding fetched events
                                {
                                    let mut vlc = self.vlc.write().await;
                                    vlc.merge(&event.vlc_snapshot);
                                }
                                
                                if let Err(e) = dag.add_event(event.clone()) {
                                    // DuplicateEvent is OK (race condition), other errors are problematic
                                    if !matches!(e, crate::dag::DagError::DuplicateEvent(_)) {
                                        warn!(event_id = %event.id, error = %e, "Failed to add fetched event");
                                    }
                                }
                            }
                            drop(dag);
                            
                            // Re-check if we still have missing events
                            let dag = self.dag.read().await;
                            let still_missing: Vec<_> = cf.anchor.event_ids
                                .iter()
                                .filter(|id| !dag.contains(id))
                                .collect();
                            
                            if !still_missing.is_empty() {
                                return Err(setu_types::SetuError::InvalidData(
                                    format!(
                                        "CF {} references {} events not available locally after fetch attempt",
                                        cf.id,
                                        still_missing.len()
                                    )
                                ));
                            }
                        }
                        Err(e) => {
                            return Err(setu_types::SetuError::InvalidData(
                                format!(
                                    "CF {} references {} missing events and fetch failed: {}",
                                    cf.id,
                                    missing_event_ids.len(),
                                    e
                                )
                            ));
                        }
                    }
                } else {
                    // No broadcaster configured - cannot fetch missing events
                    return Err(setu_types::SetuError::InvalidData(
                        format!(
                            "CF {} references {} events not in local DAG (no broadcaster to fetch)",
                            cf.id,
                            missing_event_ids.len()
                        )
                    ));
                }
            }
        }
        
        // Now we have all events, proceed with verification
        let dag = self.dag.read().await;
        let mut manager = self.consensus_manager.write().await;
        
        // Double-check idempotency (another thread may have processed while we fetched)
        if manager.has_cf(&cf.id) {
            return Ok((false, None));
        }
        
        // Step 4: Verify the CF's merkle roots are internally consistent
        if !manager.verify_cf_merkle_roots(&cf) {
            return Err(setu_types::SetuError::InvalidData(
                "CF merkle roots verification failed".to_string()
            ));
        }
        
        // Step 5-6: Apply state changes from the CF's events to maintain consistent state
        // This also verifies the resulting state matches the anchor's state root
        let state_verified = manager.apply_cf_state_changes(&dag, &cf);
        if !state_verified {
            return Err(setu_types::SetuError::InvalidData(
                "State root mismatch after applying CF state changes".to_string()
            ));
        }
        
        let cf_id = cf.id.clone();
        
        // Receive the CF
        manager.receive_cf(cf.clone());

        // Vote for the CF (in MVP, we always approve valid CFs)
        let vote = manager.vote_for_cf(&cf_id, true);
        if let Some(ref v) = vote {
            // Broadcast vote to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_vote(v).await {
                    Ok(result) => {
                        debug!(
                            cf_id = %cf_id,
                            success = result.success_count,
                            "Vote broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %cf_id, error = %e, "Failed to broadcast vote");
                    }
                }
            }
            
            // Check if our vote caused finalization
            // (vote_for_cf adds vote but doesn't check finalization, so we check here)
            let finalized = manager.check_finalization(&cf_id);
            if finalized {
                return self.handle_finalization(&mut manager).await;
            }
        }

        Ok((false, None))
    }
    
    /// Handle CF finalization: broadcast notification and return anchor for persistence
    /// 
    /// Note: This method extracts data from manager before acquiring other locks
    /// to avoid potential deadlock from holding multiple write locks.
    async fn handle_finalization(&self, manager: &mut tokio::sync::RwLockWriteGuard<'_, ConsensusManager>) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        // Extract data from manager first, before acquiring other locks
        let cf_data = manager.last_finalized_cf().map(|cf| (cf.id.clone(), cf.anchor.clone(), cf.clone()));
        
        let finalized_anchor = if let Some((cf_id, anchor, cf)) = cf_data {
            // Send finalization to internal channel (for local listeners)
            let _ = self
                .message_tx
                .send(ConsensusMessage::FrameFinalized(cf))
                .await;

            // Broadcast finalization to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_finalized(&cf_id).await {
                    Ok(result) => {
                        info!(
                            cf_id = %cf_id,
                            success = result.success_count,
                            "CF finalization broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %cf_id, error = %e, "Failed to broadcast finalization");
                    }
                }
            }
            // Release broadcaster lock before acquiring validator_set lock
            drop(broadcaster);

            // Advance to next round (ValidatorSet manages rounds)
            let mut validator_set = self.validator_set.write().await;
            validator_set.advance_round();

            Some(anchor)
        } else {
            None
        };

        Ok((true, finalized_anchor))
    }

    /// Receive a vote from another validator
    /// 
    /// Returns (finalized, Option<Anchor>) - the anchor is returned when finalized
    /// so the caller can persist it to storage.
    pub async fn receive_vote(&self, vote: Vote) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        let mut manager = self.consensus_manager.write().await;
        let finalized = manager.receive_vote(vote);

        if finalized {
            self.handle_finalization(&mut manager).await
        } else {
            Ok((false, None))
        }
    }

    /// Compute the state root from the DAG (legacy method)
    /// 
    /// This is a simple hash-based computation for backward compatibility.
    /// The real state root is now computed by AnchorBuilder using SMTs.
    #[deprecated(since = "0.2.0", note = "State root is now computed internally by ConsensusManager/AnchorBuilder")]
    fn compute_state_root_internal(&self, dag: &Dag) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dag.node_count().to_le_bytes());
        hasher.update(dag.max_depth().to_le_bytes());
        hex::encode(hasher.finalize())
    }

    /// Compute the state root (async version, legacy)
    #[deprecated(since = "0.2.0", note = "Use get_global_state_root() instead")]
    pub async fn compute_state_root(&self) -> String {
        let dag = self.dag.read().await;
        #[allow(deprecated)]
        self.compute_state_root_internal(&dag)
    }
    
    /// Get the current global state root from AnchorBuilder
    pub async fn get_global_state_root(&self) -> [u8; 32] {
        let manager = self.consensus_manager.read().await;
        manager.get_global_root()
    }
    
    /// Get a subnet's current state root
    pub async fn get_subnet_state_root(&self, subnet_id: &setu_types::SubnetId) -> Option<[u8; 32]> {
        let manager = self.consensus_manager.read().await;
        manager.get_subnet_root(subnet_id)
    }
    
    /// Get the number of anchors created
    pub async fn get_anchor_count(&self) -> usize {
        let manager = self.consensus_manager.read().await;
        manager.anchor_count()
    }
    
    /// Mark an anchor as successfully persisted to storage
    /// 
    /// Call this after successfully storing the anchor to AnchorStore.
    /// This enables safe garbage collection of finalized CFs from memory.
    pub async fn mark_anchor_persisted(&self, anchor_id: &str) {
        let mut manager = self.consensus_manager.write().await;
        manager.mark_anchor_persisted(anchor_id);
    }

    /// Get the message sender for external communication
    pub fn message_sender(&self) -> mpsc::Sender<ConsensusMessage> {
        self.message_tx.clone()
    }

    /// Get DAG statistics
    pub async fn get_dag_stats(&self) -> DagStats {
        let dag = self.dag.read().await;
        DagStats {
            node_count: dag.node_count(),
            max_depth: dag.max_depth(),
            tip_count: dag.get_tips().len(),
            pending_count: dag.get_pending_count(),
        }
    }

    /// Get the current VLC snapshot
    pub async fn get_vlc_snapshot(&self) -> VLCSnapshot {
        self.vlc.read().await.snapshot()
    }

    /// Get the current tips of the DAG
    pub async fn get_tips(&self) -> Vec<EventId> {
        self.dag.read().await.get_tips()
    }

    /// Get events by their IDs from the DAG
    /// 
    /// This is used to retrieve events for persistence when a CF is finalized.
    /// Returns events that exist in the DAG.
    pub async fn get_events_by_ids(&self, event_ids: &[EventId]) -> Vec<Event> {
        let dag = self.dag.read().await;
        event_ids
            .iter()
            .filter_map(|id| dag.get_event(id).cloned())
            .collect()
    }

    /// Get the local validator ID
    pub fn local_validator_id(&self) -> &str {
        &self.local_validator_id
    }

    /// Get the configuration
    pub fn config(&self) -> &ConsensusConfig {
        &self.config
    }
}

/// DAG statistics
#[derive(Debug, Clone)]
pub struct DagStats {
    pub node_count: usize,
    pub max_depth: u64,
    pub tip_count: usize,
    pub pending_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{NodeInfo, ValidatorInfo};
    use setu_vlc::VectorClock;

    fn create_validator_set() -> ValidatorSet {
        let mut set = ValidatorSet::new();
        for i in 1..=3 {
            let node = NodeInfo::new_validator(
                format!("v{}", i),
                "127.0.0.1".to_string(),
                8000 + i as u16,
            );
            set.add_validator(ValidatorInfo::new(node, false));
        }
        set
    }

    #[tokio::test]
    async fn test_engine_create_event() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let event = engine.create_event(vec![]).await.unwrap();
        assert_eq!(event.creator, "v1");
    }

    #[tokio::test]
    async fn test_engine_add_event() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let genesis = Event::genesis(
            "v1".to_string(),
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
        );

        let _event_id = engine.add_event(genesis).await.unwrap();

        let stats = engine.get_dag_stats().await;
        assert_eq!(stats.node_count, 1);
    }

    #[tokio::test]
    async fn test_engine_leader_check() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        // First validator should be the leader
        assert!(engine.is_current_leader().await);
    }

    #[tokio::test]
    async fn test_engine_advance_round() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let round0 = engine.current_round().await;
        assert_eq!(round0, 0);

        let round1 = engine.advance_round().await;
        assert_eq!(round1, 1);
    }

    #[tokio::test]
    async fn test_engine_valid_proposer() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        // Check proposer for different rounds
        let proposer_0 = engine.get_valid_proposer(0).await;
        let proposer_1 = engine.get_valid_proposer(1).await;
        let proposer_2 = engine.get_valid_proposer(2).await;

        assert!(proposer_0.is_some());
        assert!(proposer_1.is_some());
        assert!(proposer_2.is_some());

        // Proposers should rotate
        assert_ne!(proposer_0, proposer_1);
    }
}
