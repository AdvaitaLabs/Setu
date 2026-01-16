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
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

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
    /// Channel for sending consensus messages
    message_tx: mpsc::Sender<ConsensusMessage>,
    /// Channel for receiving consensus messages (reserved for future use)
    #[allow(dead_code)]
    message_rx: Arc<RwLock<mpsc::Receiver<ConsensusMessage>>>,
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
            message_rx: Arc::new(RwLock::new(rx)),
        }
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
        let _ = self
            .message_tx
            .send(ConsensusMessage::NewEvent(event))
            .await;

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
            let _ = self
                .message_tx
                .send(ConsensusMessage::ProposeFrame(frame.clone()))
                .await;
        }

        Ok(cf)
    }

    /// Receive a ConsensusFrame from another validator
    pub async fn receive_cf(&self, cf: ConsensusFrame) -> SetuResult<()> {
        let mut manager = self.consensus_manager.write().await;
        manager.receive_cf(cf.clone());

        // Vote for the CF (in MVP, we always approve valid CFs)
        let vote = manager.vote_for_cf(&cf.id, true);
        if let Some(v) = vote {
            let _ = self.message_tx.send(ConsensusMessage::Vote(v)).await;
        }

        Ok(())
    }

    /// Receive a vote from another validator
    pub async fn receive_vote(&self, vote: Vote) -> SetuResult<bool> {
        let mut manager = self.consensus_manager.write().await;
        let finalized = manager.receive_vote(vote);

        if finalized {
            if let Some(cf) = manager.last_finalized_cf() {
                let _ = self
                    .message_tx
                    .send(ConsensusMessage::FrameFinalized(cf.clone()))
                    .await;

                // Advance to the next round after finalization
                drop(manager);
                self.advance_round().await;
            }
        }

        Ok(finalized)
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
