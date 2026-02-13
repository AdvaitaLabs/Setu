//! FoldObserver watches for fold commits and triggers economic processing.
//!
//! Sits as a parallel path alongside existing consensus — does not modify
//! anchor building, voting, or state application.

use setu_types::pocw::{PoCWConfig, FoldEconomics};
use setu_types::Event;
use tracing::info;

use super::processor;

/// Observes fold commits and produces economic summaries.
pub struct FoldObserver {
    config: PoCWConfig,
    /// History of fold economics for diagnostics
    history: Vec<FoldEconomics>,
}

impl FoldObserver {
    pub fn new(config: PoCWConfig) -> Self {
        Self {
            config,
            history: Vec::new(),
        }
    }

    /// Called after a fold is committed. Processes economics if enabled.
    pub fn on_fold_committed(&mut self, events: &[Event]) -> Option<FoldEconomics> {
        let result = processor::process_fold(&self.config, events)?;

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
        let events = vec![make_transfer("solver-1")];
        assert!(observer.on_fold_committed(&events).is_none());
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
        let events = vec![
            make_transfer("solver-1"),
            make_transfer("solver-2"),
            make_transfer("solver-1"),
        ];

        let result = observer.on_fold_committed(&events).unwrap();
        assert_eq!(result.event_count, 3);
        assert_eq!(result.total_flux_burned, 21_000 * 3);
        assert_eq!(result.total_solver_rewards, 3);
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

        observer.on_fold_committed(&[make_transfer("s1")]);
        observer.on_fold_committed(&[make_transfer("s2"), make_transfer("s2")]);

        assert_eq!(observer.history().len(), 2);
        assert_eq!(observer.history()[0].event_count, 1);
        assert_eq!(observer.history()[1].event_count, 2);
    }
}
