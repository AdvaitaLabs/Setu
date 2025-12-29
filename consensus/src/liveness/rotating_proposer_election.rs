// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Rotating Proposer Election
//!
//! This module implements a simple round-robin leader rotation strategy.
//! Validators take turns being the proposer based on the round number.
//!
//! This is the default election strategy for the MVP phase.

use super::proposer_election::{ProposerElection, Round, ValidatorId, VotingPower};

/// Rotating proposer election using round-robin rotation.
///
/// The rotation algorithm:
/// ```text
/// proposer_index = (round / contiguous_rounds) % num_validators
/// ```
///
/// This ensures:
/// - Deterministic leader selection (all validators agree)
/// - Fair rotation among all validators
/// - Optional contiguous rounds for each proposer
#[derive(Debug, Clone)]
pub struct RotatingProposer {
    /// Ordered list of validators (all honest replicas must agree on this order)
    proposers: Vec<ValidatorId>,
    
    /// Number of contiguous rounds a proposer is active in a row.
    /// Default is 1, meaning the proposer changes every round.
    contiguous_rounds: u32,
    
    /// Optional voting power for each validator (for weighted rotation in future)
    voting_powers: Vec<VotingPower>,
}

impl RotatingProposer {
    /// Create a new rotating proposer election with default settings.
    ///
    /// # Arguments
    /// * `proposers` - Ordered list of validator IDs
    pub fn new(proposers: Vec<ValidatorId>) -> Self {
        let voting_powers = vec![1; proposers.len()];
        Self {
            proposers,
            contiguous_rounds: 1,
            voting_powers,
        }
    }

    /// Create a rotating proposer with a specified number of contiguous rounds.
    ///
    /// # Arguments
    /// * `proposers` - Ordered list of validator IDs
    /// * `contiguous_rounds` - Number of rounds each proposer serves consecutively
    pub fn with_contiguous_rounds(proposers: Vec<ValidatorId>, contiguous_rounds: u32) -> Self {
        let voting_powers = vec![1; proposers.len()];
        Self {
            proposers,
            contiguous_rounds: contiguous_rounds.max(1),
            voting_powers,
        }
    }

    /// Create a rotating proposer with voting power information.
    ///
    /// # Arguments
    /// * `proposers` - List of (ValidatorId, VotingPower) pairs
    /// * `contiguous_rounds` - Number of rounds each proposer serves consecutively
    pub fn with_voting_powers(
        proposers: Vec<(ValidatorId, VotingPower)>,
        contiguous_rounds: u32,
    ) -> Self {
        let (ids, powers): (Vec<_>, Vec<_>) = proposers.into_iter().unzip();
        Self {
            proposers: ids,
            contiguous_rounds: contiguous_rounds.max(1),
            voting_powers: powers,
        }
    }

    /// Get the proposer index for a given round.
    fn get_proposer_index(&self, round: Round) -> usize {
        if self.proposers.is_empty() {
            return 0;
        }
        let effective_round = round / u64::from(self.contiguous_rounds);
        (effective_round % self.proposers.len() as u64) as usize
    }

    /// Add a new proposer to the rotation.
    pub fn add_proposer(&mut self, proposer: ValidatorId, voting_power: VotingPower) {
        if !self.proposers.contains(&proposer) {
            self.proposers.push(proposer);
            self.voting_powers.push(voting_power);
            // Re-sort to maintain deterministic order
            self.sort_proposers();
        }
    }

    /// Remove a proposer from the rotation.
    pub fn remove_proposer(&mut self, proposer: &ValidatorId) -> bool {
        if let Some(idx) = self.proposers.iter().position(|p| p == proposer) {
            self.proposers.remove(idx);
            self.voting_powers.remove(idx);
            true
        } else {
            false
        }
    }

    /// Sort proposers deterministically (by ID).
    fn sort_proposers(&mut self) {
        let mut combined: Vec<_> = self.proposers
            .iter()
            .cloned()
            .zip(self.voting_powers.iter().cloned())
            .collect();
        combined.sort_by(|a, b| a.0.cmp(&b.0));
        
        self.proposers = combined.iter().map(|(id, _)| id.clone()).collect();
        self.voting_powers = combined.iter().map(|(_, power)| *power).collect();
    }

    /// Get the total voting power of all proposers.
    pub fn total_voting_power(&self) -> VotingPower {
        self.voting_powers.iter().sum()
    }

    /// Get the voting power of a specific proposer.
    pub fn get_voting_power(&self, proposer: &ValidatorId) -> Option<VotingPower> {
        self.proposers
            .iter()
            .position(|p| p == proposer)
            .map(|idx| self.voting_powers[idx])
    }

    /// Get the number of proposers.
    pub fn proposer_count(&self) -> usize {
        self.proposers.len()
    }
}

impl ProposerElection for RotatingProposer {
    fn get_valid_proposer(&self, round: Round) -> Option<ValidatorId> {
        if self.proposers.is_empty() {
            return None;
        }
        let idx = self.get_proposer_index(round);
        Some(self.proposers[idx].clone())
    }

    fn is_valid_proposer(&self, validator_id: &ValidatorId, round: Round) -> bool {
        self.get_valid_proposer(round).as_ref() == Some(validator_id)
    }

    fn get_candidates(&self) -> Vec<ValidatorId> {
        self.proposers.clone()
    }

    fn contiguous_rounds(&self) -> u32 {
        self.contiguous_rounds
    }
}

/// Helper function to select a single leader from a list of peers.
///
/// This is useful for bootstrapping or when a fixed leader is needed.
/// Selects the validator with the minimum ID for deterministic results.
pub fn choose_leader(peers: Vec<ValidatorId>) -> Option<ValidatorId> {
    peers.into_iter().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotating_proposer_basic() {
        let proposers = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let election = RotatingProposer::new(proposers);

        assert_eq!(election.get_valid_proposer(0), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(1), Some("v2".to_string()));
        assert_eq!(election.get_valid_proposer(2), Some("v3".to_string()));
        assert_eq!(election.get_valid_proposer(3), Some("v1".to_string())); // Wraps around
    }

    #[test]
    fn test_rotating_proposer_contiguous() {
        let proposers = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let election = RotatingProposer::with_contiguous_rounds(proposers, 2);

        // v1 for rounds 0-1, v2 for rounds 2-3, v3 for rounds 4-5
        assert_eq!(election.get_valid_proposer(0), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(1), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(2), Some("v2".to_string()));
        assert_eq!(election.get_valid_proposer(3), Some("v2".to_string()));
        assert_eq!(election.get_valid_proposer(4), Some("v3".to_string()));
        assert_eq!(election.get_valid_proposer(5), Some("v3".to_string()));
        assert_eq!(election.get_valid_proposer(6), Some("v1".to_string())); // Wraps around
    }

    #[test]
    fn test_is_valid_proposer() {
        let proposers = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let election = RotatingProposer::new(proposers);

        assert!(election.is_valid_proposer(&"v1".to_string(), 0));
        assert!(!election.is_valid_proposer(&"v2".to_string(), 0));
        assert!(election.is_valid_proposer(&"v2".to_string(), 1));
    }

    #[test]
    fn test_empty_proposers() {
        let election = RotatingProposer::new(vec![]);
        assert_eq!(election.get_valid_proposer(0), None);
    }

    #[test]
    fn test_single_proposer() {
        let proposers = vec!["v1".to_string()];
        let election = RotatingProposer::new(proposers);

        // Single proposer should always be selected
        for round in 0..10 {
            assert_eq!(election.get_valid_proposer(round), Some("v1".to_string()));
        }
    }

    #[test]
    fn test_add_remove_proposer() {
        let proposers = vec!["v1".to_string(), "v2".to_string()];
        let mut election = RotatingProposer::new(proposers);

        assert_eq!(election.proposer_count(), 2);

        election.add_proposer("v3".to_string(), 1);
        assert_eq!(election.proposer_count(), 3);

        // Adding duplicate should not increase count
        election.add_proposer("v3".to_string(), 1);
        assert_eq!(election.proposer_count(), 3);

        election.remove_proposer(&"v2".to_string());
        assert_eq!(election.proposer_count(), 2);
        assert!(!election.get_candidates().contains(&"v2".to_string()));
    }

    #[test]
    fn test_choose_leader() {
        let peers = vec!["v3".to_string(), "v1".to_string(), "v2".to_string()];
        assert_eq!(choose_leader(peers), Some("v1".to_string()));

        assert_eq!(choose_leader(vec![]), None);
    }

    #[test]
    fn test_voting_power() {
        let proposers = vec![
            ("v1".to_string(), 100),
            ("v2".to_string(), 200),
            ("v3".to_string(), 150),
        ];
        let election = RotatingProposer::with_voting_powers(proposers, 1);

        assert_eq!(election.total_voting_power(), 450);
        assert_eq!(election.get_voting_power(&"v1".to_string()), Some(100));
        assert_eq!(election.get_voting_power(&"v2".to_string()), Some(200));
        assert_eq!(election.get_voting_power(&"unknown".to_string()), None);
    }
}
