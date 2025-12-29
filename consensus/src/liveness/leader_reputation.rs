// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Leader Reputation System
//!
//! This module implements a reputation-based leader election system.
//! The reputation is calculated based on historical performance metrics:
//! - Successful block proposals
//! - Failed proposals
//! - Voting participation
//!
//! For MVP phase, this module provides the interface and skeleton implementation.
//! Full implementation will be added in future versions.

use std::collections::HashMap;

use super::proposer_election::{choose_index, ProposerElection, Round, ValidatorId, VotingPower};

/// Voting power ratio (0.0 to 1.0)
pub type VotingPowerRatio = f64;

/// Interface to query historical consensus metadata.
///
/// This trait abstracts the storage backend for consensus history,
/// allowing different implementations (in-memory, database, etc.)
pub trait MetadataBackend: Send + Sync {
    /// Get the block metadata for a target round.
    ///
    /// Returns a vector of recent consensus frames up to the target round,
    /// and a state root hash for verification.
    fn get_block_metadata(
        &self,
        target_epoch: u64,
        target_round: Round,
    ) -> (Vec<ConsensusFrameMetadata>, [u8; 32]);
}

/// Metadata for a finalized consensus frame.
#[derive(Debug, Clone)]
pub struct ConsensusFrameMetadata {
    /// The epoch this frame belongs to
    pub epoch: u64,
    
    /// The round number
    pub round: Round,
    
    /// The proposer who created this frame
    pub proposer: ValidatorId,
    
    /// Validators who voted for this frame
    pub voters: Vec<ValidatorId>,
    
    /// Whether the proposal was successful
    pub success: bool,
    
    /// Validators who failed to vote (for participation tracking)
    pub failed_voters: Vec<ValidatorId>,
    
    /// Timestamp of the frame
    pub timestamp: u64,
}

/// Interface to calculate weights for proposers based on history.
///
/// Different heuristics can be implemented to weight validators
/// based on their historical performance.
pub trait ReputationHeuristic: Send + Sync {
    /// Calculate weights for all candidates based on historical data.
    ///
    /// # Arguments
    /// * `epoch` - Current epoch
    /// * `epoch_to_candidates` - Mapping of epochs to their candidate lists
    /// * `history` - Historical consensus frame metadata
    ///
    /// # Returns
    /// A vector of weights corresponding to candidates in the current epoch
    fn get_weights(
        &self,
        epoch: u64,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> Vec<u64>;
}

/// Configuration for the reputation-based leader election.
#[derive(Debug, Clone)]
pub struct ReputationConfig {
    /// Size of the voting window for reputation calculation
    pub voter_window_size: usize,
    
    /// Size of the proposer window for reputation calculation
    pub proposer_window_size: usize,
    
    /// Weight for active validators (participated recently)
    pub active_weight: u64,
    
    /// Weight for inactive validators (no recent participation)
    pub inactive_weight: u64,
    
    /// Weight for validators with high failure rate
    pub failed_weight: u64,
    
    /// Failure threshold percentage (0-100)
    /// Above this threshold, validator is considered failing
    pub failure_threshold_percent: u32,
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            voter_window_size: 10,
            proposer_window_size: 10,
            active_weight: 100,
            inactive_weight: 10,
            failed_weight: 1,
            failure_threshold_percent: 20,
        }
    }
}

/// Aggregation of historical consensus frame data for reputation calculation.
#[derive(Debug)]
pub struct ConsensusFrameAggregation {
    voter_window_size: usize,
    proposer_window_size: usize,
}

impl ConsensusFrameAggregation {
    pub fn new(voter_window_size: usize, proposer_window_size: usize) -> Self {
        Self {
            voter_window_size,
            proposer_window_size,
        }
    }

    /// Count votes for each validator in the window.
    pub fn count_votes(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> HashMap<ValidatorId, u32> {
        let window = history.iter().take(self.voter_window_size);
        let mut votes: HashMap<ValidatorId, u32> = HashMap::new();

        for frame in window {
            if !epoch_to_candidates.contains_key(&frame.epoch) {
                continue;
            }
            for voter in &frame.voters {
                *votes.entry(voter.clone()).or_insert(0) += 1;
            }
        }

        votes
    }

    /// Count successful proposals for each validator in the window.
    pub fn count_proposals(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> HashMap<ValidatorId, u32> {
        let window = history.iter().take(self.proposer_window_size);
        let mut proposals: HashMap<ValidatorId, u32> = HashMap::new();

        for frame in window {
            if !epoch_to_candidates.contains_key(&frame.epoch) || !frame.success {
                continue;
            }
            *proposals.entry(frame.proposer.clone()).or_insert(0) += 1;
        }

        proposals
    }

    /// Count failed proposals for each validator in the window.
    pub fn count_failed_proposals(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> HashMap<ValidatorId, u32> {
        let window = history.iter().take(self.proposer_window_size);
        let mut failed: HashMap<ValidatorId, u32> = HashMap::new();

        for frame in window {
            if !epoch_to_candidates.contains_key(&frame.epoch) || frame.success {
                continue;
            }
            *failed.entry(frame.proposer.clone()).or_insert(0) += 1;
        }

        failed
    }

    /// Get aggregated metrics for all validators.
    pub fn get_aggregated_metrics(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> (
        HashMap<ValidatorId, u32>,  // votes
        HashMap<ValidatorId, u32>,  // proposals
        HashMap<ValidatorId, u32>,  // failed_proposals
    ) {
        (
            self.count_votes(epoch_to_candidates, history),
            self.count_proposals(epoch_to_candidates, history),
            self.count_failed_proposals(epoch_to_candidates, history),
        )
    }
}

/// Reputation heuristic based on proposer success rate and voting participation.
///
/// Weight calculation logic:
/// 1. If failure rate > threshold: use failed_weight (lowest priority)
/// 2. If no proposals and no votes: use inactive_weight (low priority)
/// 3. Otherwise: use active_weight (normal priority)
#[derive(Debug)]
pub struct ProposerAndVoterHeuristic {
    #[allow(dead_code)]
    author: ValidatorId,
    config: ReputationConfig,
    aggregation: ConsensusFrameAggregation,
}

impl ProposerAndVoterHeuristic {
    pub fn new(author: ValidatorId, config: ReputationConfig) -> Self {
        Self {
            author,
            aggregation: ConsensusFrameAggregation::new(
                config.voter_window_size,
                config.proposer_window_size,
            ),
            config,
        }
    }
}

impl ReputationHeuristic for ProposerAndVoterHeuristic {
    fn get_weights(
        &self,
        epoch: u64,
        epoch_to_candidates: &HashMap<u64, Vec<ValidatorId>>,
        history: &[ConsensusFrameMetadata],
    ) -> Vec<u64> {
        if !epoch_to_candidates.contains_key(&epoch) {
            return vec![];
        }

        let (votes, proposals, failed_proposals) = self
            .aggregation
            .get_aggregated_metrics(epoch_to_candidates, history);

        epoch_to_candidates[&epoch]
            .iter()
            .map(|author| {
                let cur_votes = *votes.get(author).unwrap_or(&0);
                let cur_proposals = *proposals.get(author).unwrap_or(&0);
                let cur_failed = *failed_proposals.get(author).unwrap_or(&0);

                // Check if failure rate exceeds threshold
                let total_proposals = cur_proposals + cur_failed;
                if total_proposals > 0 {
                    let failure_rate = (cur_failed * 100) / total_proposals;
                    if failure_rate > self.config.failure_threshold_percent {
                        return self.config.failed_weight;
                    }
                }

                // Check if active (has proposals or votes)
                if cur_proposals > 0 || cur_votes > 0 {
                    self.config.active_weight
                } else {
                    self.config.inactive_weight
                }
            })
            .collect()
    }
}

/// In-memory implementation of MetadataBackend for testing.
#[derive(Debug, Default)]
pub struct InMemoryMetadataBackend {
    history: Vec<ConsensusFrameMetadata>,
    max_history_size: usize,
}

impl InMemoryMetadataBackend {
    pub fn new(max_history_size: usize) -> Self {
        Self {
            history: Vec::new(),
            max_history_size,
        }
    }

    pub fn add_frame(&mut self, frame: ConsensusFrameMetadata) {
        self.history.insert(0, frame);
        if self.history.len() > self.max_history_size {
            self.history.pop();
        }
    }

    pub fn history(&self) -> &[ConsensusFrameMetadata] {
        &self.history
    }
}

impl MetadataBackend for InMemoryMetadataBackend {
    fn get_block_metadata(
        &self,
        _target_epoch: u64,
        _target_round: Round,
    ) -> (Vec<ConsensusFrameMetadata>, [u8; 32]) {
        // Return all history up to the target round
        // TODO: Filter by epoch and round
        (self.history.clone(), [0u8; 32])
    }
}

/// Leader election based on reputation.
///
/// This election strategy uses historical performance data to weight
/// validators when selecting the next leader. Validators with better
/// track records (more successful proposals, higher voting participation)
/// have higher chances of being selected.
#[derive(Debug)]
pub struct LeaderReputation<B: MetadataBackend, H: ReputationHeuristic> {
    /// Current epoch
    epoch: u64,
    
    /// Mapping of epoch to validator candidates
    epoch_to_candidates: HashMap<u64, Vec<ValidatorId>>,
    
    /// Voting power for each validator
    voting_powers: HashMap<ValidatorId, VotingPower>,
    
    /// Backend for fetching historical metadata
    backend: B,
    
    /// Heuristic for calculating weights
    heuristic: H,
    
    /// Whether to exclude inactive validators
    exclude_inactive: bool,
}

impl<B: MetadataBackend, H: ReputationHeuristic> LeaderReputation<B, H> {
    pub fn new(
        epoch: u64,
        candidates: Vec<ValidatorId>,
        voting_powers: HashMap<ValidatorId, VotingPower>,
        backend: B,
        heuristic: H,
    ) -> Self {
        let mut epoch_to_candidates = HashMap::new();
        epoch_to_candidates.insert(epoch, candidates);
        
        Self {
            epoch,
            epoch_to_candidates,
            voting_powers,
            backend,
            heuristic,
            exclude_inactive: false,
        }
    }

    /// Set whether to exclude completely inactive validators.
    pub fn set_exclude_inactive(&mut self, exclude: bool) {
        self.exclude_inactive = exclude;
    }

    /// Update the epoch and candidates.
    pub fn update_epoch(&mut self, epoch: u64, candidates: Vec<ValidatorId>) {
        self.epoch = epoch;
        self.epoch_to_candidates.insert(epoch, candidates);
    }

    /// Get the reputation weights for all candidates.
    pub fn get_reputation_weights(&self, round: Round) -> Vec<u64> {
        let (history, _root) = self.backend.get_block_metadata(self.epoch, round);
        self.heuristic.get_weights(self.epoch, &self.epoch_to_candidates, &history)
    }
}

impl<B: MetadataBackend, H: ReputationHeuristic> ProposerElection for LeaderReputation<B, H> {
    fn get_valid_proposer(&self, round: Round) -> Option<ValidatorId> {
        let candidates = self.epoch_to_candidates.get(&self.epoch)?;
        if candidates.is_empty() {
            return None;
        }

        let weights = self.get_reputation_weights(round);
        
        // Convert to VotingPower
        let voting_weights: Vec<VotingPower> = weights
            .into_iter()
            .enumerate()
            .map(|(idx, weight)| {
                let validator = &candidates[idx];
                let stake = self.voting_powers.get(validator).unwrap_or(&1);
                weight as VotingPower * stake
            })
            .collect();

        // Use round as seed for deterministic selection
        let seed = round.to_le_bytes().to_vec();
        let selected_idx = choose_index(voting_weights, seed);
        
        candidates.get(selected_idx).cloned()
    }

    fn get_candidates(&self) -> Vec<ValidatorId> {
        self.epoch_to_candidates
            .get(&self.epoch)
            .cloned()
            .unwrap_or_default()
    }

    fn get_voting_power_participation_ratio(&self, round: Round) -> f64 {
        // TODO: Implement based on historical voting data
        let _ = round;
        1.0
    }
}

// ============================================================================
// TODO: Full implementation for non-MVP version
// ============================================================================

/// TODO: Implement persistent storage backend
/// This should store consensus frame metadata in a database for durability.
pub struct PersistentMetadataBackend {
    // TODO: Add database connection
    // db: Arc<dyn DbReader>,
    // window_size: usize,
}

impl PersistentMetadataBackend {
    #[allow(dead_code)]
    pub fn new(_window_size: usize) -> Self {
        todo!("Implement persistent metadata backend")
    }
}

/// TODO: Implement stake-weighted reputation
/// This heuristic should consider stake amounts when calculating weights.
pub struct StakeWeightedHeuristic {
    // TODO: Add stake information
    // stakes: HashMap<ValidatorId, u64>,
}

impl StakeWeightedHeuristic {
    #[allow(dead_code)]
    pub fn new() -> Self {
        todo!("Implement stake-weighted heuristic")
    }
}

/// TODO: Implement time-decay reputation
/// Recent actions should have more weight than older ones.
pub struct TimeDecayHeuristic {
    // TODO: Add decay configuration
    // half_life_rounds: u64,
}

impl TimeDecayHeuristic {
    #[allow(dead_code)]
    pub fn new() -> Self {
        todo!("Implement time-decay heuristic")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_frame(
        epoch: u64,
        round: Round,
        proposer: &str,
        voters: Vec<&str>,
        success: bool,
    ) -> ConsensusFrameMetadata {
        ConsensusFrameMetadata {
            epoch,
            round,
            proposer: proposer.to_string(),
            voters: voters.into_iter().map(|s| s.to_string()).collect(),
            success,
            failed_voters: vec![],
            timestamp: round * 1000,
        }
    }

    #[test]
    fn test_consensus_frame_aggregation_count_votes() {
        let aggregation = ConsensusFrameAggregation::new(10, 10);
        
        let mut epoch_to_candidates = HashMap::new();
        epoch_to_candidates.insert(1, vec!["v1".to_string(), "v2".to_string(), "v3".to_string()]);

        let history = vec![
            create_test_frame(1, 3, "v1", vec!["v1", "v2", "v3"], true),
            create_test_frame(1, 2, "v2", vec!["v1", "v2"], true),
            create_test_frame(1, 1, "v3", vec!["v1", "v3"], true),
        ];

        let votes = aggregation.count_votes(&epoch_to_candidates, &history);
        
        assert_eq!(votes.get(&"v1".to_string()), Some(&3));
        assert_eq!(votes.get(&"v2".to_string()), Some(&2));
        assert_eq!(votes.get(&"v3".to_string()), Some(&2));
    }

    #[test]
    fn test_consensus_frame_aggregation_count_proposals() {
        let aggregation = ConsensusFrameAggregation::new(10, 10);
        
        let mut epoch_to_candidates = HashMap::new();
        epoch_to_candidates.insert(1, vec!["v1".to_string(), "v2".to_string(), "v3".to_string()]);

        let history = vec![
            create_test_frame(1, 3, "v1", vec!["v1", "v2", "v3"], true),
            create_test_frame(1, 2, "v1", vec!["v1", "v2"], true),
            create_test_frame(1, 1, "v2", vec!["v1", "v3"], true),
        ];

        let proposals = aggregation.count_proposals(&epoch_to_candidates, &history);
        
        assert_eq!(proposals.get(&"v1".to_string()), Some(&2));
        assert_eq!(proposals.get(&"v2".to_string()), Some(&1));
        assert_eq!(proposals.get(&"v3".to_string()), None);
    }

    #[test]
    fn test_proposer_and_voter_heuristic() {
        let config = ReputationConfig::default();
        let heuristic = ProposerAndVoterHeuristic::new("v1".to_string(), config.clone());

        let mut epoch_to_candidates = HashMap::new();
        epoch_to_candidates.insert(1, vec!["v1".to_string(), "v2".to_string(), "v3".to_string()]);

        let history = vec![
            create_test_frame(1, 3, "v1", vec!["v1", "v2"], true),
            create_test_frame(1, 2, "v1", vec!["v1", "v2"], true),
            // v3 has never voted or proposed
        ];

        let weights = heuristic.get_weights(1, &epoch_to_candidates, &history);
        
        // v1 and v2 should have active weight, v3 should have inactive weight
        assert_eq!(weights[0], config.active_weight); // v1
        assert_eq!(weights[1], config.active_weight); // v2
        assert_eq!(weights[2], config.inactive_weight); // v3
    }

    #[test]
    fn test_in_memory_backend() {
        let mut backend = InMemoryMetadataBackend::new(100);
        
        backend.add_frame(create_test_frame(1, 1, "v1", vec!["v1", "v2"], true));
        backend.add_frame(create_test_frame(1, 2, "v2", vec!["v1", "v2", "v3"], true));
        
        assert_eq!(backend.history().len(), 2);
        
        // Most recent should be first
        assert_eq!(backend.history()[0].round, 2);
        assert_eq!(backend.history()[1].round, 1);
    }

    #[test]
    fn test_leader_reputation_election() {
        let backend = InMemoryMetadataBackend::new(100);
        let config = ReputationConfig::default();
        let heuristic = ProposerAndVoterHeuristic::new("v1".to_string(), config);
        
        let candidates = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let voting_powers = HashMap::new();
        
        let election = LeaderReputation::new(
            1,
            candidates.clone(),
            voting_powers,
            backend,
            heuristic,
        );

        // Should return a valid proposer
        let proposer = election.get_valid_proposer(0);
        assert!(proposer.is_some());
        assert!(candidates.contains(&proposer.unwrap()));
    }
}
