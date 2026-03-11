//! FoldObserver watches for fold commits and triggers economic processing.
//!
//! Sits as a parallel path alongside existing consensus — does not modify
//! anchor building, voting, or state application.

use setu_types::pocw::{EmissionState, PoCWConfig, FoldEconomics};
use setu_types::Event;
use tracing::info;

use crate::dag::Dag;
use super::processor;

/// Observes fold commits and produces economic summaries.
pub struct FoldObserver {
    config: PoCWConfig,
    emission: EmissionState,
    /// History of fold economics for diagnostics
    history: Vec<FoldEconomics>,
}

impl FoldObserver {
    pub fn new(config: PoCWConfig) -> Self {
        let kappa = config.kappa;
        Self {
            config,
            emission: EmissionState::new(kappa),
            history: Vec::new(),
        }
    }

    /// Called after a fold is committed. Processes economics if enabled.
    pub fn on_fold_committed(&mut self, events: &[Event], dag: &Dag) -> Option<FoldEconomics> {
        let result = processor::process_fold(&self.config, events, dag, &mut self.emission)?;

        info!(
            event_count = result.event_count,
            total_flux_burned = result.total_flux_burned,
            total_power_consumed = result.total_power_consumed,
            total_solver_rewards = result.total_solver_rewards,
            solver_count = result.solver_rewards.len(),
            "Fold economics processed"
        );

        self.history.push(result.clone());
        Some(result)
    }

    pub fn history(&self) -> &[FoldEconomics] {
        &self.history
    }

    pub fn config(&self) -> &PoCWConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::{EventType, VLCSnapshot};

    fn make_transfer(solver: &str) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::new(),
            "validator-1".to_string(),
        );
        event.executed_by = Some(solver.to_string());
        event
    }

    #[test]
    fn test_disabled_produces_none() {
        let mut observer = FoldObserver::new(PoCWConfig::default());
        let dag = Dag::new();
        let events = vec![make_transfer("solver-1")];
        assert!(observer.on_fold_committed(&events, &dag).is_none());
        assert!(observer.history().is_empty());
    }

    #[test]
    fn test_enabled_produces_economics() {
        let config = PoCWConfig {
            enabled: true,
            solver_transfer_reward_enabled: true,
            ..Default::default()
        };
        let mut observer = FoldObserver::new(config);
        let dag = Dag::new();
        let events = vec![
            make_transfer("solver-1"),
            make_transfer("solver-2"),
            make_transfer("solver-1"),
        ];

        let result = observer.on_fold_committed(&events, &dag).unwrap();
        assert_eq!(result.event_count, 3);
        assert_eq!(result.total_flux_burned, 21_000 * 3);
        // flux_minted = κ(1.0) × 3 = 3, plus transfer rewards = 3
        assert_eq!(result.flux_minted, 3);
        assert_eq!(result.solver_rewards.len(), 2);
        assert_eq!(observer.history().len(), 1);
    }

    #[test]
    fn test_history_accumulates() {
        let config = PoCWConfig {
            enabled: true,
            ..Default::default()
        };
        let mut observer = FoldObserver::new(config);
        let dag = Dag::new();

        observer.on_fold_committed(&[make_transfer("s1")], &dag);
        observer.on_fold_committed(&[make_transfer("s2"), make_transfer("s2")], &dag);

        assert_eq!(observer.history().len(), 2);
        assert_eq!(observer.history()[0].event_count, 1);
        assert_eq!(observer.history()[1].event_count, 2);
    }
}
