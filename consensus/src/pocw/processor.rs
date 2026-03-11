//! Fold-level economic processor.
//!
//! Handles both FluxTransfer (flat fee) and TaskSubmit (formula-based) events.
//! Orchestrates burn aggregation, power aggregation, flux minting, PoCW reward
//! distribution, and kappa emission adjustment.

use std::collections::HashMap;
use setu_types::event::EventType;
use setu_types::pocw::{EmissionState, PoCWConfig, FoldEconomics, SolverReward};
use setu_types::Event;

use crate::dag::Dag;
use super::emission::{compute_flux_minted, adjust_kappa};
use super::flux_burn::calculate_transfer_burn;
use super::power::calculate_transfer_power_drain;
use super::scoring;

/// Process a fold's events and produce an economic summary.
///
/// Aggregates burn and power from both Transfer (flat) and TaskSubmit
/// (per-event metrics) events. Mints flux, distributes via PoCW scoring,
/// and adjusts kappa.
///
/// Returns `None` if PoCW is disabled.
pub fn process_fold(
    config: &PoCWConfig,
    events: &[Event],
    dag: &Dag,
    emission: &mut EmissionState,
) -> Option<FoldEconomics> {
    if !config.enabled {
        return None;
    }

    let burn_per_transfer = calculate_transfer_burn(config);
    let power_per_transfer = calculate_transfer_power_drain(config);

    let mut transfer_count: usize = 0;
    let mut task_burn: u64 = 0;
    let mut task_power: u64 = 0;
    let mut solver_transfers: HashMap<String, u64> = HashMap::new();

    for event in events {
        match event.event_type {
            EventType::Transfer => {
                transfer_count += 1;
                if let Some(solver_id) = &event.executed_by {
                    *solver_transfers.entry(solver_id.clone()).or_default() += 1;
                }
            }
            EventType::TaskSubmit => {
                if let Some(ref metrics) = event.event_metrics {
                    task_burn += metrics.flux_burn;
                    task_power += metrics.power_delta;
                }
            }
            _ => {}
        }
    }

    let total_flux_burned = burn_per_transfer * transfer_count as u64 + task_burn;
    let total_power_consumed = power_per_transfer * transfer_count as u64 + task_power;

    // Minting: FluxMinted = κ × ΔPower_total
    let kappa_before = emission.kappa;
    let flux_minted = compute_flux_minted(total_power_consumed, kappa_before);

    // PoCW reward distribution
    let mut solver_rewards = scoring::compute_rewards(dag, events, flux_minted, config);

    // Merge transfer rewards into solver records
    if config.solver_transfer_reward_enabled {
        for (solver_id, count) in &solver_transfers {
            let transfer_reward = config.solver_transfer_reward * count;
            if let Some(existing) = solver_rewards.iter_mut().find(|r| r.solver_id == *solver_id) {
                existing.transfer_count = *count;
                existing.flux_reward += transfer_reward;
            } else {
                solver_rewards.push(SolverReward {
                    solver_id: solver_id.clone(),
                    transfer_count: *count,
                    task_count: 0,
                    distance_score: 0.0,
                    necessity_score: 0.0,
                    contribution_score: 0.0,
                    weight: 0.0,
                    flux_reward: transfer_reward,
                });
            }
        }
        solver_rewards.sort_by(|a, b| a.solver_id.cmp(&b.solver_id));
    }

    let total_solver_rewards = solver_rewards.iter().map(|r| r.flux_reward).sum();

    // Emission adjustment: κ(k+1) = f(κ(k), flux_minted)
    let kappa_after = adjust_kappa(emission, flux_minted, config);

    Some(FoldEconomics {
        event_count: events.len(),
        total_flux_burned,
        total_power_consumed,
        flux_minted,
        total_solver_rewards,
        kappa_before,
        kappa_after,
        solver_rewards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::VLCSnapshot;

    fn make_transfer_event(executed_by: Option<&str>) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::new(),
            "validator-1".to_string(),
        );
        event.executed_by = executed_by.map(|s| s.to_string());
        event
    }

    fn make_genesis_event() -> Event {
        Event::new(
            EventType::Genesis,
            vec![],
            VLCSnapshot::new(),
            "validator-1".to_string(),
        )
    }

    fn enabled_config() -> PoCWConfig {
        PoCWConfig {
            enabled: true,
            solver_transfer_reward_enabled: true,
            ..Default::default()
        }
    }

    fn call_process_fold(config: &PoCWConfig, events: &[Event]) -> Option<FoldEconomics> {
        let dag = Dag::new();
        let mut emission = EmissionState::new(config.kappa);
        process_fold(config, events, &dag, &mut emission)
    }

    #[test]
    fn test_disabled_returns_none() {
        let config = PoCWConfig::default(); // enabled: false
        let events = vec![make_transfer_event(Some("solver-1"))];
        assert!(call_process_fold(&config, &events).is_none());
    }

    #[test]
    fn test_empty_fold() {
        let config = enabled_config();
        let result = call_process_fold(&config, &[]).unwrap();
        assert_eq!(result.event_count, 0);
        assert_eq!(result.total_flux_burned, 0);
        assert_eq!(result.total_power_consumed, 0);
        assert_eq!(result.total_solver_rewards, 0);
        assert!(result.solver_rewards.is_empty());
    }

    #[test]
    fn test_single_solver() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_transfer_event(Some("solver-1")),
            make_transfer_event(Some("solver-1")),
        ];

        let result = call_process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 3);
        assert_eq!(result.total_flux_burned, 21_000 * 3);
        assert_eq!(result.total_power_consumed, 1 * 3);
        // flux_minted = κ(1.0) × 3 = 3
        assert_eq!(result.flux_minted, 3);
        assert_eq!(result.solver_rewards.len(), 1);
        assert_eq!(result.solver_rewards[0].solver_id, "solver-1");
        assert_eq!(result.solver_rewards[0].transfer_count, 3);
        // PoCW reward (3) + transfer reward (3*1) = 6
        assert_eq!(result.solver_rewards[0].flux_reward, 3 + 3);
    }

    #[test]
    fn test_multi_solver() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_transfer_event(Some("solver-2")),
            make_transfer_event(Some("solver-1")),
            make_transfer_event(Some("solver-2")),
            make_transfer_event(Some("solver-2")),
        ];

        let result = call_process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 5);
        assert_eq!(result.total_flux_burned, 21_000 * 5);
        // flux_minted = κ(1.0) × 5 = 5
        assert_eq!(result.flux_minted, 5);

        // PoCW rewards distributed by scoring (equal weight since
        // events not in DAG → equal distance/necessity, zero contribution)
        // Transfer rewards added on top: solver-1 gets 2, solver-2 gets 3
        // Sorted by solver_id
        assert_eq!(result.solver_rewards[0].solver_id, "solver-1");
        assert_eq!(result.solver_rewards[0].transfer_count, 2);
        assert_eq!(result.solver_rewards[1].solver_id, "solver-2");
        assert_eq!(result.solver_rewards[1].transfer_count, 3);
        // Total = PoCW distributed (5) + transfer rewards (5) = 10
        assert_eq!(result.total_solver_rewards, 10);
    }

    #[test]
    fn test_reward_disabled() {
        let config = PoCWConfig {
            enabled: true,
            solver_transfer_reward_enabled: false,
            ..Default::default()
        };
        let events = vec![make_transfer_event(Some("solver-1"))];

        let result = call_process_fold(&config, &events).unwrap();
        assert_eq!(result.total_flux_burned, 21_000);
        // flux_minted = κ(1.0) × 1 = 1, distributed via PoCW scoring
        assert_eq!(result.flux_minted, 1);
        // PoCW reward only (no transfer reward since disabled)
        assert_eq!(result.solver_rewards.len(), 1);
        assert_eq!(result.solver_rewards[0].flux_reward, 1);
    }

    #[test]
    fn test_non_transfer_events_ignored_for_burn() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_genesis_event(), // not a transfer — no burn or power
        ];

        let result = call_process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 2);
        assert_eq!(result.total_flux_burned, 21_000); // only 1 transfer burned
        assert_eq!(result.total_power_consumed, 1);
        assert_eq!(result.flux_minted, 1);
    }

    #[test]
    fn test_transfer_without_executed_by() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_transfer_event(None), // no solver recorded
        ];

        let result = call_process_fold(&config, &events).unwrap();
        // Both transfers still burn and drain
        assert_eq!(result.total_flux_burned, 21_000 * 2);
        assert_eq!(result.total_power_consumed, 2);
        assert_eq!(result.flux_minted, 2);
        // PoCW reward for solver-1 (only executed_by events get scoring)
        // Transfer reward for solver-1: 1*1 = 1
        // solver-1 gets PoCW(2) + transfer(1) = 3
        assert_eq!(result.solver_rewards.len(), 1);
        assert_eq!(result.solver_rewards[0].flux_reward, 2 + 1);
    }
}
