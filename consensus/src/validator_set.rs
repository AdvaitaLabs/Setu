// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Validator Set Management
//!
//! This module manages the set of validators participating in consensus.
//! It integrates with the liveness module for leader election.

use setu_types::ValidatorInfo;
#[cfg(test)]
use setu_types::NodeInfo;
use std::collections::HashMap;

use crate::liveness::{
    choose_leader, create_default_election, ProposerElection, ReputationConfig,
    RotatingProposer, Round, ValidatorId, VotingPower,
};

/// Election strategy configuration
#[derive(Debug, Clone)]
pub enum ElectionStrategy {
    /// Simple round-robin rotation
    Rotating {
        /// Number of contiguous rounds per proposer
        contiguous_rounds: u32,
    },
    /// Reputation-based selection (future implementation)
    Reputation(ReputationConfig),
    /// Fixed leader (for testing)
    Fixed(ValidatorId),
}

impl Default for ElectionStrategy {
    fn default() -> Self {
        ElectionStrategy::Rotating {
            contiguous_rounds: 1,
        }
    }
}

/// Manages the set of validators and leader election.
#[derive(Debug, Clone)]
pub struct ValidatorSet {
    /// All registered validators
    validators: HashMap<ValidatorId, ValidatorInfo>,
    
    /// Current leader ID
    leader_id: Option<ValidatorId>,
    
    /// Current round number
    current_round: Round,
    
    /// Election strategy
    strategy: ElectionStrategy,
    
    /// Cached proposer election instance
    election: Option<RotatingProposer>,
}

impl ValidatorSet {
    /// Create a new empty validator set with default settings.
    pub fn new() -> Self {
        Self {
            validators: HashMap::new(),
            leader_id: None,
            current_round: 0,
            strategy: ElectionStrategy::default(),
            election: None,
        }
    }

    /// Create a validator set with a specific election strategy.
    pub fn with_strategy(strategy: ElectionStrategy) -> Self {
        Self {
            validators: HashMap::new(),
            leader_id: None,
            current_round: 0,
            strategy,
            election: None,
        }
    }

    /// Add a validator to the set.
    pub fn add_validator(&mut self, mut info: ValidatorInfo) {
        let is_first = self.validators.is_empty();
        
        // First validator becomes the initial leader
        if is_first {
            info.is_leader = true;
            self.leader_id = Some(info.node.id.clone());
        }
        
        self.validators.insert(info.node.id.clone(), info);
        
        // Rebuild election when validators change
        self.rebuild_election();
    }

    /// Remove a validator from the set.
    pub fn remove_validator(&mut self, validator_id: &str) -> Option<ValidatorInfo> {
        let removed = self.validators.remove(validator_id);
        
        // Rebuild election when validators change (must be done before electing new leader)
        self.rebuild_election();
        
        // If the removed validator was the leader, elect a new one
        if self.leader_id.as_deref() == Some(validator_id) {
            self.elect_next_leader();
        }
        
        removed
    }

    /// Get a validator by ID.
    pub fn get_validator(&self, validator_id: &str) -> Option<&ValidatorInfo> {
        self.validators.get(validator_id)
    }

    /// Get the current leader.
    pub fn get_leader(&self) -> Option<&ValidatorInfo> {
        self.leader_id
            .as_ref()
            .and_then(|id| self.validators.get(id))
    }

    /// Get the current leader ID.
    pub fn get_leader_id(&self) -> Option<&ValidatorId> {
        self.leader_id.as_ref()
    }

    /// Check if a validator is the current leader.
    pub fn is_leader(&self, validator_id: &str) -> bool {
        self.leader_id.as_deref() == Some(validator_id)
    }

    /// Check if a validator is the valid proposer for a specific round.
    pub fn is_valid_proposer(&self, validator_id: &str, round: Round) -> bool {
        match &self.election {
            Some(election) => election.is_valid_proposer(&validator_id.to_string(), round),
            None => self.is_leader(validator_id),
        }
    }

    /// Get the valid proposer for a specific round.
    pub fn get_valid_proposer(&self, round: Round) -> Option<ValidatorId> {
        match &self.election {
            Some(election) => election.get_valid_proposer(round),
            None => self.leader_id.clone(),
        }
    }

    /// Advance to the next round and update the leader.
    pub fn advance_round(&mut self) -> Round {
        self.current_round += 1;
        self.update_leader_for_round(self.current_round);
        self.current_round
    }

    /// Set the current round explicitly and update the leader.
    pub fn set_round(&mut self, round: Round) {
        self.current_round = round;
        self.update_leader_for_round(round);
    }

    /// Rotate to the next leader (legacy method for compatibility).
    pub fn rotate_leader(&mut self) {
        self.advance_round();
    }

    /// Elect a new leader based on the current strategy.
    fn elect_next_leader(&mut self) {
        match &self.strategy {
            ElectionStrategy::Fixed(id) => {
                if self.validators.contains_key(id) {
                    self.set_leader(id.clone());
                } else {
                    self.elect_fallback_leader();
                }
            }
            ElectionStrategy::Rotating { .. } | ElectionStrategy::Reputation(_) => {
                self.update_leader_for_round(self.current_round);
            }
        }
    }

    /// Update the leader for a specific round.
    fn update_leader_for_round(&mut self, round: Round) {
        let new_leader = match &self.election {
            Some(election) => election.get_valid_proposer(round),
            None => {
                // Fallback to simple rotation if no election configured
                let mut ids: Vec<_> = self.validators.keys().cloned().collect();
                ids.sort();
                if ids.is_empty() {
                    None
                } else {
                    Some(ids[(round as usize) % ids.len()].clone())
                }
            }
        };

        if let Some(leader_id) = new_leader {
            self.set_leader(leader_id);
        }
    }

    /// Set a specific validator as the leader.
    fn set_leader(&mut self, leader_id: ValidatorId) {
        for (id, info) in &mut self.validators {
            let is_new_leader = id == &leader_id;
            info.is_leader = is_new_leader;
            if is_new_leader {
                info.leader_round = self.current_round;
            }
        }
        self.leader_id = Some(leader_id);
    }

    /// Elect a fallback leader.
    fn elect_fallback_leader(&mut self) {
        let leader = choose_leader(self.validators.keys().cloned().collect());
        if let Some(leader_id) = leader {
            self.set_leader(leader_id);
        } else {
            self.leader_id = None;
        }
    }

    /// Rebuild the election instance based on current validators.
    fn rebuild_election(&mut self) {
        let mut ids: Vec<_> = self.validators.keys().cloned().collect();
        ids.sort();

        self.election = match &self.strategy {
            ElectionStrategy::Rotating { contiguous_rounds } => {
                Some(RotatingProposer::with_contiguous_rounds(ids, *contiguous_rounds))
            }
            ElectionStrategy::Fixed(_) => None,
            ElectionStrategy::Reputation(_config) => {
                // TODO: Implement reputation-based election
                Some(create_default_election(ids))
            }
        };
    }

    /// Get the number of validators.
    pub fn count(&self) -> usize {
        self.validators.len()
    }

    /// Calculate the quorum size (2f + 1 for 3f + 1 validators).
    pub fn quorum_size(&self) -> usize {
        (self.count() * 2) / 3 + 1
    }

    /// Check if a vote count meets the quorum.
    pub fn has_quorum(&self, vote_count: usize) -> bool {
        vote_count >= self.quorum_size()
    }

    /// Get all validators.
    pub fn all_validators(&self) -> Vec<&ValidatorInfo> {
        self.validators.values().collect()
    }

    /// Get all validator IDs.
    pub fn all_validator_ids(&self) -> Vec<ValidatorId> {
        self.validators.keys().cloned().collect()
    }

    /// Get only active validators.
    pub fn active_validators(&self) -> Vec<&ValidatorInfo> {
        self.validators
            .values()
            .filter(|v| v.node.is_active())
            .collect()
    }

    /// Get the current round number.
    pub fn current_round(&self) -> Round {
        self.current_round
    }

    /// Get the current election strategy.
    pub fn strategy(&self) -> &ElectionStrategy {
        &self.strategy
    }

    /// Update the election strategy.
    pub fn set_strategy(&mut self, strategy: ElectionStrategy) {
        self.strategy = strategy;
        self.rebuild_election();
    }

    /// Notify the set that a round was completed (for reputation tracking).
    pub fn on_round_completed(&mut self, round: Round, proposer: &ValidatorId, success: bool) {
        // TODO: Update reputation metrics when reputation-based election is implemented
        let _ = (round, proposer, success);
    }

    /// Get the total voting power of all validators.
    pub fn total_voting_power(&self) -> VotingPower {
        self.validators
            .values()
            .map(|v| v.node.stake as VotingPower)
            .sum()
    }

    /// Get the voting power of a specific validator.
    pub fn get_voting_power(&self, validator_id: &str) -> VotingPower {
        self.validators
            .get(validator_id)
            .map(|v| v.node.stake as VotingPower)
            .unwrap_or(0)
    }
}

impl Default for ValidatorSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_validator(id: &str) -> ValidatorInfo {
        let node = NodeInfo::new_validator(id.to_string(), "127.0.0.1".to_string(), 8000);
        ValidatorInfo::new(node, false)
    }

    fn create_validator_with_stake(id: &str, stake: u64) -> ValidatorInfo {
        let mut node = NodeInfo::new_validator(id.to_string(), "127.0.0.1".to_string(), 8000);
        node.stake = stake;
        ValidatorInfo::new(node, false)
    }

    #[test]
    fn test_first_validator_is_leader() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));

        assert!(set.is_leader("v1"));
        assert_eq!(set.get_leader_id(), Some(&"v1".to_string()));
    }

    #[test]
    fn test_quorum_calculation() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));
        set.add_validator(create_validator("v3"));

        assert_eq!(set.quorum_size(), 3);
        assert!(set.has_quorum(3));
        assert!(!set.has_quorum(2));
    }

    #[test]
    fn test_leader_rotation() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));
        set.add_validator(create_validator("v3"));

        let first_leader = set.get_leader_id().cloned();
        set.advance_round();
        let second_leader = set.get_leader_id().cloned();

        assert_ne!(first_leader, second_leader);
    }

    #[test]
    fn test_rotating_proposer_integration() {
        let mut set = ValidatorSet::with_strategy(ElectionStrategy::Rotating {
            contiguous_rounds: 1,
        });
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));
        set.add_validator(create_validator("v3"));

        let proposer_0 = set.get_valid_proposer(0);
        let proposer_1 = set.get_valid_proposer(1);
        let proposer_2 = set.get_valid_proposer(2);
        let proposer_3 = set.get_valid_proposer(3);

        assert!(proposer_0.is_some());
        assert!(proposer_1.is_some());
        assert!(proposer_2.is_some());
        assert_eq!(proposer_0, proposer_3);
    }

    #[test]
    fn test_contiguous_rounds() {
        let mut set = ValidatorSet::with_strategy(ElectionStrategy::Rotating {
            contiguous_rounds: 2,
        });
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));

        assert_eq!(set.get_valid_proposer(0), set.get_valid_proposer(1));
        assert_eq!(set.get_valid_proposer(2), set.get_valid_proposer(3));
        assert_ne!(set.get_valid_proposer(0), set.get_valid_proposer(2));
    }

    #[test]
    fn test_is_valid_proposer() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));
        set.add_validator(create_validator("v3"));

        for round in 0..10 {
            let valid_count = ["v1", "v2", "v3"]
                .iter()
                .filter(|id| set.is_valid_proposer(id, round))
                .count();
            assert_eq!(valid_count, 1);
        }
    }

    #[test]
    fn test_remove_leader() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));

        assert!(set.is_leader("v1"));

        set.remove_validator("v1");

        assert!(set.get_leader_id().is_some());
        assert!(!set.is_leader("v1"));
    }

    #[test]
    fn test_voting_power() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator_with_stake("v1", 100));
        set.add_validator(create_validator_with_stake("v2", 200));
        set.add_validator(create_validator_with_stake("v3", 150));

        assert_eq!(set.total_voting_power(), 450);
        assert_eq!(set.get_voting_power("v1"), 100);
        assert_eq!(set.get_voting_power("v2"), 200);
        assert_eq!(set.get_voting_power("unknown"), 0);
    }

    #[test]
    fn test_advance_round() {
        let mut set = ValidatorSet::new();
        set.add_validator(create_validator("v1"));
        set.add_validator(create_validator("v2"));

        assert_eq!(set.current_round(), 0);
        
        let new_round = set.advance_round();
        assert_eq!(new_round, 1);
        assert_eq!(set.current_round(), 1);
    }
}
