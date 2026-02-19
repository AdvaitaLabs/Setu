// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Proposer Election Trait
//!
//! This module defines the core trait for leader/proposer election in the Setu consensus.
//! Different election strategies can be implemented by implementing the `ProposerElection` trait.

use std::cmp::Ordering;

/// Validator identifier type
pub type ValidatorId = String;

/// Round number type (logical time or epoch-based round)
pub type Round = u64;

/// Voting power type
pub type VotingPower = u128;

/// ProposerElection incorporates the logic of choosing a leader among multiple candidates.
///
/// In Setu's DAG-based consensus, the leader is responsible for:
/// 1. Monitoring VLC increments to determine when to fold the DAG
/// 2. Creating ConsensusFrames (CF) when the VLC delta reaches the threshold
/// 3. Proposing the CF to other validators for voting
///
/// This trait is designed to be extensible, allowing for different election strategies:
/// - Round-robin rotation (simple, deterministic)
/// - Reputation-based selection (considers historical performance)
/// - Weighted random selection (considers stake/voting power)
pub trait ProposerElection: Send + Sync {
    /// Check if a given validator is a valid proposer for a given round.
    ///
    /// By default, this checks if the validator matches the result of `get_valid_proposer`.
    /// Implementations can override this for more complex validation logic.
    fn is_valid_proposer(&self, validator_id: &ValidatorId, round: Round) -> bool {
        self.get_valid_proposer(round).as_ref() == Some(validator_id)
    }

    /// Return the valid proposer for a given round.
    ///
    /// This is the core method that determines which validator should be the leader
    /// for the specified round. Returns None if no valid proposer can be determined.
    fn get_valid_proposer(&self, round: Round) -> Option<ValidatorId>;

    /// Return the chain health: a ratio of voting power participating in the consensus.
    ///
    /// This can be used to assess network health and adjust consensus parameters.
    /// Returns a value between 0.0 and 1.0.
    fn get_voting_power_participation_ratio(&self, _round: Round) -> f64 {
        1.0
    }

    /// Return both the proposer and the voting power participation ratio for a round.
    fn get_valid_proposer_and_participation_ratio(
        &self,
        round: Round,
    ) -> (Option<ValidatorId>, f64) {
        (
            self.get_valid_proposer(round),
            self.get_voting_power_participation_ratio(round),
        )
    }

    /// Called when a round is completed (CF finalized) to update internal state.
    ///
    /// This is useful for reputation-based election to track successful/failed rounds.
    fn on_round_completed(&mut self, _round: Round, _proposer: &ValidatorId, _success: bool) {
        // Default implementation does nothing
    }

    /// Get the list of all candidate validators.
    fn get_candidates(&self) -> Vec<ValidatorId>;

    /// Get the number of contiguous rounds a proposer should serve.
    fn contiguous_rounds(&self) -> u32 {
        1
    }
}

/// Helper function to deterministically select an index from weighted candidates.
///
/// Uses a hash-based random selection where the probability of selecting each candidate
/// is proportional to their weight.
///
/// # Arguments
/// * `weights` - A vector of weights for each candidate
/// * `seed` - A seed for deterministic random selection (e.g., round number hash)
///
/// # Returns
/// The index of the selected candidate
pub fn choose_index(weights: Vec<VotingPower>, seed: Vec<u8>) -> usize {
    let mut cumulative_weights = weights;
    let mut total_weight: VotingPower = 0;

    // Convert to cumulative weights
    for w in &mut cumulative_weights {
        total_weight = total_weight.checked_add(*w).expect("Total weight overflow");
        *w = total_weight;
    }

    // Generate a pseudo-random value from seed
    let chosen_weight = next_in_range(seed, total_weight);

    // Binary search to find the selected index
    cumulative_weights
        .binary_search_by(|w| {
            if *w <= chosen_weight {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        })
        .expect_err("Binary search should return Err with the insertion point")
}

/// Generate a deterministic pseudo-random value in [0, max) range from a seed.
fn next_in_range(seed: Vec<u8>, max: VotingPower) -> VotingPower {
    use sha2::{Digest, Sha256};

    let hash = Sha256::digest(&seed);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    let value = u128::from_le_bytes(bytes);

    value % max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_index_uniform() {
        let weights = vec![100, 100, 100, 100];
        let mut counts = [0u32; 4];

        // Test with different seeds
        for i in 0u64..1000 {
            let seed = i.to_le_bytes().to_vec();
            let idx = choose_index(weights.clone(), seed);
            counts[idx] += 1;
        }

        // Each should be selected roughly 250 times (allow 20% variance)
        for count in counts {
            assert!(
                count > 150 && count < 350,
                "Unexpected distribution: {:?}",
                counts
            );
        }
    }

    #[test]
    fn test_choose_index_weighted() {
        // First candidate has 3x the weight
        let weights = vec![300, 100, 100, 100];
        let mut counts = [0u32; 4];

        for i in 0u64..1000 {
            let seed = i.to_le_bytes().to_vec();
            let idx = choose_index(weights.clone(), seed);
            counts[idx] += 1;
        }

        // First should be selected more often
        assert!(
            counts[0] > counts[1],
            "Weighted selection failed: {:?}",
            counts
        );
        assert!(
            counts[0] > counts[2],
            "Weighted selection failed: {:?}",
            counts
        );
        assert!(
            counts[0] > counts[3],
            "Weighted selection failed: {:?}",
            counts
        );
    }

    #[test]
    fn test_next_in_range_bounds() {
        for i in 0u64..100 {
            let seed = i.to_le_bytes().to_vec();
            let value = next_in_range(seed, 1000);
            assert!(value < 1000, "Value out of bounds: {}", value);
        }
    }
}
