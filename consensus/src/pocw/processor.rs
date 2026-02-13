//! Fold-level economic processor for FluxTransfer transactions.
//!
//! Takes a set of fold events and a PoCWConfig, produces a FoldEconomics summary
//! with per-solver reward breakdown.

use std::collections::HashMap;
use setu_types::event::EventType;
use setu_types::pocw::{PoCWConfig, FoldEconomics, SolverReward};
use setu_types::Event;

use super::flux_burn::calculate_transfer_burn;
use super::power::calculate_transfer_power_drain;

/// Process a fold's events and produce an economic summary.
///
/// Only FluxTransfer events with `executed_by` set are counted for solver rewards.
/// Returns `None` if PoCW is disabled.
pub fn process_fold(config: &PoCWConfig, events: &[Event]) -> Option<FoldEconomics> {
    if !config.enabled {
        return None;
    }

    let burn_per_transfer = calculate_transfer_burn(config);
    let power_per_transfer = calculate_transfer_power_drain(config);

    let mut transfer_count: usize = 0;
    let mut solver_transfers: HashMap<String, u64> = HashMap::new();

    for event in events {
        if event.event_type != EventType::Transfer {
            continue;
        }
        transfer_count += 1;

        if let Some(solver_id) = &event.executed_by {
            *solver_transfers.entry(solver_id.clone()).or_default() += 1;
        }
    }

    let total_flux_burned = burn_per_transfer * transfer_count as u64;
    let total_power_consumed = power_per_transfer * transfer_count as u64;

    let mut solver_rewards = Vec::new();
    let mut total_solver_rewards: u64 = 0;

    if config.solver_transfer_reward_enabled {
        for (solver_id, count) in &solver_transfers {
            let reward = config.solver_transfer_reward * count;
            total_solver_rewards += reward;
            solver_rewards.push(SolverReward {
                solver_id: solver_id.clone(),
                transfer_count: *count,
                flux_reward: reward,
            });
        }
        solver_rewards.sort_by(|a, b| a.solver_id.cmp(&b.solver_id));
    }

    Some(FoldEconomics {
        event_count: events.len(),
        total_flux_burned,
        total_power_consumed,
        total_solver_rewards,
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

    #[test]
    fn test_disabled_returns_none() {
        let config = PoCWConfig::default(); // enabled: false
        let events = vec![make_transfer_event(Some("solver-1"))];
        assert!(process_fold(&config, &events).is_none());
    }

    #[test]
    fn test_empty_fold() {
        let config = enabled_config();
        let result = process_fold(&config, &[]).unwrap();
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

        let result = process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 3);
        assert_eq!(result.total_flux_burned, 21_000 * 3);
        assert_eq!(result.total_power_consumed, 1 * 3);
        assert_eq!(result.total_solver_rewards, 1 * 3);
        assert_eq!(result.solver_rewards.len(), 1);
        assert_eq!(result.solver_rewards[0].solver_id, "solver-1");
        assert_eq!(result.solver_rewards[0].transfer_count, 3);
        assert_eq!(result.solver_rewards[0].flux_reward, 3);
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

        let result = process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 5);
        assert_eq!(result.total_flux_burned, 21_000 * 5);
        assert_eq!(result.total_solver_rewards, 5); // 2 + 3

        // Sorted by solver_id
        assert_eq!(result.solver_rewards[0].solver_id, "solver-1");
        assert_eq!(result.solver_rewards[0].transfer_count, 2);
        assert_eq!(result.solver_rewards[0].flux_reward, 2);
        assert_eq!(result.solver_rewards[1].solver_id, "solver-2");
        assert_eq!(result.solver_rewards[1].transfer_count, 3);
        assert_eq!(result.solver_rewards[1].flux_reward, 3);
    }

    #[test]
    fn test_reward_disabled() {
        let config = PoCWConfig {
            enabled: true,
            solver_transfer_reward_enabled: false,
            ..Default::default()
        };
        let events = vec![make_transfer_event(Some("solver-1"))];

        let result = process_fold(&config, &events).unwrap();
        assert_eq!(result.total_flux_burned, 21_000);
        assert_eq!(result.total_solver_rewards, 0);
        assert!(result.solver_rewards.is_empty());
    }

    #[test]
    fn test_non_transfer_events_ignored() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_genesis_event(), // not a transfer
        ];

        let result = process_fold(&config, &events).unwrap();
        assert_eq!(result.event_count, 2); // total events in fold
        assert_eq!(result.total_flux_burned, 21_000); // only 1 transfer burned
        assert_eq!(result.total_solver_rewards, 1);
    }

    #[test]
    fn test_transfer_without_executed_by() {
        let config = enabled_config();
        let events = vec![
            make_transfer_event(Some("solver-1")),
            make_transfer_event(None), // no solver recorded
        ];

        let result = process_fold(&config, &events).unwrap();
        // Both transfers still burn and drain
        assert_eq!(result.total_flux_burned, 21_000 * 2);
        assert_eq!(result.total_power_consumed, 2);
        // Only the one with executed_by gets a reward
        assert_eq!(result.total_solver_rewards, 1);
        assert_eq!(result.solver_rewards.len(), 1);
    }
}
