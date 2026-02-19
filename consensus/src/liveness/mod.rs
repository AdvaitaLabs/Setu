// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Liveness Module
//!
//! This module provides leader election and liveness mechanisms for the Setu consensus.
//! It ensures that the consensus can make progress by rotating leaders and handling
//! validator failures gracefully.
//!
//! ## Components
//!
//! - **ProposerElection**: Core trait defining the leader election interface
//! - **RotatingProposer**: Simple round-robin leader rotation
//! - **LeaderReputation**: Reputation-based leader selection (MVP skeleton)
//!
//! ## Usage
//!
//! For MVP, use the `RotatingProposer` for simple and deterministic leader rotation:
//!
//! ```ignore
//! use consensus::liveness::{RotatingProposer, ProposerElection};
//!
//! let validators = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
//! let election = RotatingProposer::new(validators);
//!
//! // Get the leader for round 0
//! let leader = election.get_valid_proposer(0);
//! assert_eq!(leader, Some("v1".to_string()));
//!
//! // Check if a validator is the leader for a round
//! assert!(election.is_valid_proposer(&"v2".to_string(), 1));
//! ```

mod leader_reputation;
mod proposer_election;
mod rotating_proposer_election;

// Re-export main types
pub use proposer_election::{choose_index, ProposerElection, Round, ValidatorId, VotingPower};

pub use rotating_proposer_election::{choose_leader, RotatingProposer};

pub use leader_reputation::{
    // Implementations
    ConsensusFrameAggregation,
    // Types
    ConsensusFrameMetadata,
    InMemoryMetadataBackend,
    LeaderReputation,
    // Traits and interfaces
    MetadataBackend,
    ProposerAndVoterHeuristic,
    ReputationConfig,
    ReputationHeuristic,

    VotingPowerRatio,
};

/// Create a default proposer election for the given validators.
///
/// This creates a simple rotating proposer election suitable for MVP.
pub fn create_default_election(validators: Vec<ValidatorId>) -> RotatingProposer {
    RotatingProposer::new(validators)
}

/// Create a proposer election with contiguous rounds.
///
/// Each validator will be the leader for `contiguous_rounds` consecutive rounds
/// before rotating to the next validator.
pub fn create_election_with_contiguous_rounds(
    validators: Vec<ValidatorId>,
    contiguous_rounds: u32,
) -> RotatingProposer {
    RotatingProposer::with_contiguous_rounds(validators, contiguous_rounds)
}

/// Create a reputation-based election with in-memory backend.
///
/// This is suitable for testing and development. For production, use a
/// persistent backend implementation.
pub fn create_reputation_election(
    epoch: u64,
    candidates: Vec<ValidatorId>,
    config: ReputationConfig,
) -> LeaderReputation<InMemoryMetadataBackend, ProposerAndVoterHeuristic> {
    let backend = InMemoryMetadataBackend::new(config.proposer_window_size * 2);
    let heuristic =
        ProposerAndVoterHeuristic::new(candidates.first().cloned().unwrap_or_default(), config);
    let voting_powers = std::collections::HashMap::new();

    LeaderReputation::new(epoch, candidates, voting_powers, backend, heuristic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_default_election() {
        let validators = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let election = create_default_election(validators);

        assert_eq!(election.get_valid_proposer(0), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(1), Some("v2".to_string()));
        assert_eq!(election.get_valid_proposer(2), Some("v3".to_string()));
    }

    #[test]
    fn test_create_election_with_contiguous_rounds() {
        let validators = vec!["v1".to_string(), "v2".to_string()];
        let election = create_election_with_contiguous_rounds(validators, 3);

        // v1 for rounds 0-2
        assert_eq!(election.get_valid_proposer(0), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(1), Some("v1".to_string()));
        assert_eq!(election.get_valid_proposer(2), Some("v1".to_string()));

        // v2 for rounds 3-5
        assert_eq!(election.get_valid_proposer(3), Some("v2".to_string()));
        assert_eq!(election.get_valid_proposer(4), Some("v2".to_string()));
    }

    #[test]
    fn test_create_reputation_election() {
        let candidates = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let config = ReputationConfig::default();
        let election = create_reputation_election(1, candidates.clone(), config);

        let proposer = election.get_valid_proposer(0);
        assert!(proposer.is_some());
        assert!(candidates.contains(&proposer.unwrap()));
    }
}
