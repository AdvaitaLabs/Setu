//! PoCW (Proof of Causal Work) and Flux economic types for FluxTransfer
//!
//! Defines configuration and economic structures for the FluxTransfer path:
//! fixed burn fee, flat power drain, and optional nominal solver reward.

use serde::{Deserialize, Serialize};

/// Economic configuration for the Flux system.
///
/// Controls burn fees, power drain, and solver rewards for FluxTransfer transactions.
/// Use `enabled` to toggle the entire economic subsystem at runtime.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PoCWConfig {
    /// Master toggle for PoCW economics
    pub enabled: bool,
    /// Fixed gas fee for FluxTransfer transactions
    pub transfer_fixed_fee: u64,
    /// Flat power drain per FluxTransfer
    pub transfer_power_drain: u64,
    /// Whether solvers receive a nominal reward for processing FluxTransfer transactions
    pub solver_transfer_reward_enabled: bool,
    /// Nominal Flux reward per FluxTransfer processed by a solver
    pub solver_transfer_reward: u64,
}

impl Default for PoCWConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transfer_fixed_fee: 21_000,
            transfer_power_drain: 1,
            solver_transfer_reward_enabled: false,
            solver_transfer_reward: 1,
        }
    }
}

/// Solver reward record for a single solver within a fold
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverReward {
    pub solver_id: String,
    /// Number of FluxTransfer events this solver processed in the fold
    pub transfer_count: u64,
    /// Flux reward for this solver
    pub flux_reward: u64,
}

/// Fold-level economic summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldEconomics {
    /// Number of events in this fold
    pub event_count: usize,
    /// Total Flux burned across all events in this fold
    pub total_flux_burned: u64,
    /// Total power drained across all events in this fold
    pub total_power_consumed: u64,
    /// Total Flux minted as solver rewards in this fold
    pub total_solver_rewards: u64,
    /// Per-solver reward breakdown
    pub solver_rewards: Vec<SolverReward>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = PoCWConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.transfer_fixed_fee, 21_000);
        assert_eq!(config.transfer_power_drain, 1);
        assert!(!config.solver_transfer_reward_enabled);
        assert_eq!(config.solver_transfer_reward, 1);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = PoCWConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: PoCWConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.transfer_fixed_fee, deserialized.transfer_fixed_fee);
        assert_eq!(config.transfer_power_drain, deserialized.transfer_power_drain);
        assert_eq!(config.solver_transfer_reward_enabled, deserialized.solver_transfer_reward_enabled);
        assert_eq!(config.solver_transfer_reward, deserialized.solver_transfer_reward);
    }

    #[test]
    fn test_custom_config() {
        let config = PoCWConfig {
            enabled: true,
            solver_transfer_reward_enabled: true,
            solver_transfer_reward: 5,
            transfer_fixed_fee: 10_000,
            ..Default::default()
        };

        assert!(config.enabled);
        assert!(config.solver_transfer_reward_enabled);
        assert_eq!(config.solver_transfer_reward, 5);
        assert_eq!(config.transfer_fixed_fee, 10_000);
        assert_eq!(config.transfer_power_drain, 1);
    }
}
