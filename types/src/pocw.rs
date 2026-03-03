//! PoCW (Proof of Causal Work) and Flux economic types.
//!
//! Covers both FluxTransfer (flat fee) and TaskSubmit (formula-based burn)
//! paths through the 5-stage Flux pipeline.

use serde::{Deserialize, Serialize};

/// Economic configuration for the Flux system.
///
/// Controls burn fees, power drain, and solver rewards. Transfer fields are
/// flat values; task fields use the alpha/beta/gamma formula coefficients.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PoCWConfig {
    /// Master toggle for PoCW economics
    pub enabled: bool,

    // -- Transfer (flat) --
    /// Fee per FluxTransfer (burned)
    pub transfer_fee: u64,
    /// Flat power drain per FluxTransfer
    pub transfer_power_drain: u64,
    /// Whether solvers receive a nominal reward for processing FluxTransfer transactions
    pub solver_transfer_reward_enabled: bool,
    /// Nominal Flux reward per FluxTransfer processed by a solver
    pub solver_transfer_reward: u64,

    // -- Task burn (Stage 1): burn = alpha*C + beta*R + gamma*S --
    /// Complexity weight
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    /// Risk weight
    #[serde(default = "default_beta")]
    pub beta: f64,
    /// Structural tension weight
    #[serde(default = "default_gamma")]
    pub gamma: f64,

    // -- Task power (Stage 2) --
    /// Power drain rate for solver events: deltaP = rate * gas_used
    #[serde(default = "default_solver_power_rate")]
    pub solver_power_rate: f64,
    /// Initial power budget per solver
    #[serde(default = "default_initial_power_budget")]
    pub initial_power_budget: u64,

    // -- Minting (Stage 3) --
    /// Minting coefficient: FluxMinted = kappa * deltaPower_total
    #[serde(default = "default_kappa")]
    pub kappa: f64,

    // -- PoCW distribution (Stage 4) --
    /// Weight for distance scoring (proximity to causal chain)
    #[serde(default = "default_pocw_distance_weight")]
    pub pocw_distance_weight: f64,
    /// Weight for necessity scoring (relevant events / total events)
    #[serde(default = "default_pocw_necessity_weight")]
    pub pocw_necessity_weight: f64,
    /// Weight for contribution scoring (agent depth / total depth)
    #[serde(default = "default_pocw_contribution_weight")]
    pub pocw_contribution_weight: f64,

    // -- Emission adjustment (Stage 5) --
    /// Target Flux minted per fold
    #[serde(default = "default_target_velocity")]
    pub target_velocity: f64,
    /// Number of recent folds to observe
    #[serde(default = "default_observation_window")]
    pub observation_window: usize,
    /// Dampening factor for kappa adjustment
    #[serde(default = "default_dampening")]
    pub dampening: f64,
    /// Minimum kappa
    #[serde(default = "default_kappa_min")]
    pub kappa_min: f64,
    /// Maximum kappa
    #[serde(default = "default_kappa_max")]
    pub kappa_max: f64,
}

fn default_alpha() -> f64 { 0.40 }
fn default_beta() -> f64 { 0.35 }
fn default_gamma() -> f64 { 0.25 }
fn default_solver_power_rate() -> f64 { 0.001 }
fn default_initial_power_budget() -> u64 { 21_000_000 }
fn default_kappa() -> f64 { 1.0 }
fn default_pocw_distance_weight() -> f64 { 0.35 }
fn default_pocw_necessity_weight() -> f64 { 0.35 }
fn default_pocw_contribution_weight() -> f64 { 0.30 }
fn default_target_velocity() -> f64 { 1000.0 }
fn default_observation_window() -> usize { 10 }
fn default_dampening() -> f64 { 0.5 }
fn default_kappa_min() -> f64 { 0.1 }
fn default_kappa_max() -> f64 { 10.0 }

impl Default for PoCWConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transfer_fee: 21_000,
            transfer_power_drain: 1,
            solver_transfer_reward_enabled: false,
            solver_transfer_reward: 1,
            alpha: default_alpha(),
            beta: default_beta(),
            gamma: default_gamma(),
            solver_power_rate: default_solver_power_rate(),
            initial_power_budget: default_initial_power_budget(),
            kappa: default_kappa(),
            pocw_distance_weight: default_pocw_distance_weight(),
            pocw_necessity_weight: default_pocw_necessity_weight(),
            pocw_contribution_weight: default_pocw_contribution_weight(),
            target_velocity: default_target_velocity(),
            observation_window: default_observation_window(),
            dampening: default_dampening(),
            kappa_min: default_kappa_min(),
            kappa_max: default_kappa_max(),
        }
    }
}

// ========== Event-level metrics ==========

/// Metrics computed by the validator after TEE execution.
/// All fields derived from TeeExecutionResult + SolverTask input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetrics {
    /// Solver that executed this event
    pub solver_id: String,
    /// Execution time in microseconds
    pub compute_time_us: u64,
    /// Gas consumed
    pub gas_used: u64,
    /// Number of state mutations
    pub write_count: usize,
    /// Number of read_set entries
    pub read_count: usize,
    /// Value transferred (0 for non-transfer events)
    pub value_transferred: u64,
    /// DAG depth of the event at insertion time
    pub dag_depth: u64,
    /// Flux burn computed for this event
    pub flux_burn: u64,
    /// Power consumed by this event
    pub power_delta: u64,
}

// ========== Per-solver power state ==========

/// Tracks a solver's power budget across folds.
/// Stored in ROOT SMT under "power:{solver_id}".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverPowerState {
    pub solver_id: String,
    pub remaining_power: u64,
    pub total_consumed: u64,
    pub event_count: u64,
}

// ========== Emission state ==========

/// Tracks kappa and velocity history for Stage 5 emission adjustment.
/// Stored in ROOT SMT under "emission:" keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmissionState {
    pub kappa: f64,
    pub velocity_history: Vec<u64>,
}

impl EmissionState {
    pub fn new(initial_kappa: f64) -> Self {
        Self {
            kappa: initial_kappa,
            velocity_history: Vec::new(),
        }
    }
}

// ========== Solver reward ==========

/// Solver reward record for a single solver within a fold.
///
/// Scoring signals from the PoCW spec:
/// - Distance: proximity to winning agent's causal chain (1 / (1 + Distance_i))
/// - Necessity: fraction of agent's events leading to accepted output
/// - Contribution: share of causal depth in accepted chain (agent_depth / total_depth)
/// - w_i (weight): α*Distance + β*Necessity + γ*Contribution, normalized
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverReward {
    pub solver_id: String,
    /// Number of FluxTransfer events this solver processed
    #[serde(default)]
    pub transfer_count: u64,
    /// Number of TaskSubmit events this solver processed
    #[serde(default)]
    pub task_count: u64,
    /// Distance score: proximity to causal chain (0.0-1.0)
    #[serde(default)]
    pub distance_score: f64,
    /// Necessity score: relevant_events / total_events (0.0-1.0)
    #[serde(default)]
    pub necessity_score: f64,
    /// Contribution score: agent_depth / total_depth (0.0-1.0)
    #[serde(default)]
    pub contribution_score: f64,
    /// Normalized weight w_i = α*Distance + β*Necessity + γ*Contribution
    #[serde(default)]
    pub weight: f64,
    /// Flux reward for this solver
    pub flux_reward: u64,
}

// ========== Fold economics ==========

/// Fold-level economic summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldEconomics {
    /// Number of events in this fold
    pub event_count: usize,
    /// Total Flux burned across all events in this fold
    pub total_flux_burned: u64,
    /// Total power drained across all events in this fold
    pub total_power_consumed: u64,
    /// Flux minted this fold (kappa * total_power)
    #[serde(default)]
    pub flux_minted: u64,
    /// Total Flux distributed as solver rewards
    pub total_solver_rewards: u64,
    /// Kappa before this fold
    #[serde(default = "default_kappa")]
    pub kappa_before: f64,
    /// Kappa after emission adjustment
    #[serde(default = "default_kappa")]
    pub kappa_after: f64,
    /// Per-solver reward breakdown
    pub solver_rewards: Vec<SolverReward>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pocw_config_defaults() {
        let config = PoCWConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.transfer_fee, 21_000);
        assert_eq!(config.transfer_power_drain, 1);
        assert!(!config.solver_transfer_reward_enabled);
        assert_eq!(config.solver_transfer_reward, 1);
        assert!((config.alpha - 0.40).abs() < f64::EPSILON);
        assert!((config.beta - 0.35).abs() < f64::EPSILON);
        assert!((config.gamma - 0.25).abs() < f64::EPSILON);
        assert!((config.solver_power_rate - 0.001).abs() < f64::EPSILON);
        assert_eq!(config.initial_power_budget, 21_000_000);
        assert!((config.kappa - 1.0).abs() < f64::EPSILON);
        assert!((config.pocw_distance_weight - 0.35).abs() < f64::EPSILON);
        assert!((config.pocw_necessity_weight - 0.35).abs() < f64::EPSILON);
        assert!((config.pocw_contribution_weight - 0.30).abs() < f64::EPSILON);
        assert!((config.target_velocity - 1000.0).abs() < f64::EPSILON);
        assert_eq!(config.observation_window, 10);
        assert!((config.dampening - 0.5).abs() < f64::EPSILON);
        assert!((config.kappa_min - 0.1).abs() < f64::EPSILON);
        assert!((config.kappa_max - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pocw_config_serde_roundtrip() {
        let config = PoCWConfig {
            enabled: true,
            alpha: 0.5,
            beta: 0.3,
            gamma: 0.2,
            kappa: 2.5,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let de: PoCWConfig = serde_json::from_str(&json).unwrap();
        assert!(de.enabled);
        assert!((de.alpha - 0.5).abs() < f64::EPSILON);
        assert!((de.beta - 0.3).abs() < f64::EPSILON);
        assert!((de.gamma - 0.2).abs() < f64::EPSILON);
        assert!((de.kappa - 2.5).abs() < f64::EPSILON);
        assert_eq!(de.transfer_fee, 21_000);
    }

    #[test]
    fn test_pocw_config_backward_compat_deserialize() {
        // JSON with only the original PR #15 fields (no new fields)
        let json = r#"{
            "enabled": true,
            "transfer_fee": 21000,
            "transfer_power_drain": 1,
            "solver_transfer_reward_enabled": false,
            "solver_transfer_reward": 1
        }"#;
        let config: PoCWConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.transfer_fee, 21_000);
        // New fields should get defaults
        assert!((config.alpha - 0.40).abs() < f64::EPSILON);
        assert!((config.kappa - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.observation_window, 10);
    }

    #[test]
    fn test_event_metrics_serde() {
        let metrics = EventMetrics {
            solver_id: "solver-1".to_string(),
            compute_time_us: 500,
            gas_used: 1000,
            write_count: 3,
            read_count: 2,
            value_transferred: 0,
            dag_depth: 5,
            flux_burn: 12_000,
            power_delta: 1,
        };
        let json = serde_json::to_string(&metrics).unwrap();
        let de: EventMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(de.solver_id, "solver-1");
        assert_eq!(de.gas_used, 1000);
        assert_eq!(de.write_count, 3);
        assert_eq!(de.flux_burn, 12_000);
    }

    #[test]
    fn test_solver_power_state_serde() {
        let state = SolverPowerState {
            solver_id: "solver-1".to_string(),
            remaining_power: 20_999_000,
            total_consumed: 1_000,
            event_count: 5,
        };
        let json = serde_json::to_string(&state).unwrap();
        let de: SolverPowerState = serde_json::from_str(&json).unwrap();
        assert_eq!(de.remaining_power, 20_999_000);
        assert_eq!(de.total_consumed, 1_000);
    }

    #[test]
    fn test_emission_state_new() {
        let state = EmissionState::new(1.0);
        assert!((state.kappa - 1.0).abs() < f64::EPSILON);
        assert!(state.velocity_history.is_empty());
    }

    #[test]
    fn test_emission_state_serde() {
        let mut state = EmissionState::new(2.0);
        state.velocity_history.push(100);
        state.velocity_history.push(200);
        let json = serde_json::to_string(&state).unwrap();
        let de: EmissionState = serde_json::from_str(&json).unwrap();
        assert!((de.kappa - 2.0).abs() < f64::EPSILON);
        assert_eq!(de.velocity_history, vec![100, 200]);
    }

    #[test]
    fn test_solver_reward_backward_compat() {
        // JSON with only the original fields (no scores)
        let json = r#"{
            "solver_id": "s1",
            "flux_reward": 100
        }"#;
        let reward: SolverReward = serde_json::from_str(json).unwrap();
        assert_eq!(reward.solver_id, "s1");
        assert_eq!(reward.flux_reward, 100);
        assert_eq!(reward.transfer_count, 0);
        assert_eq!(reward.task_count, 0);
        assert!((reward.weight - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_fold_economics_backward_compat() {
        // JSON without the new kappa/minting fields
        let json = r#"{
            "event_count": 3,
            "total_flux_burned": 63000,
            "total_power_consumed": 3,
            "total_solver_rewards": 3,
            "solver_rewards": []
        }"#;
        let econ: FoldEconomics = serde_json::from_str(json).unwrap();
        assert_eq!(econ.event_count, 3);
        assert_eq!(econ.flux_minted, 0);
        assert!((econ.kappa_before - 1.0).abs() < f64::EPSILON);
        assert!((econ.kappa_after - 1.0).abs() < f64::EPSILON);
    }
}
