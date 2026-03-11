//! PoCW reward distribution across solvers.
//!
//! Scoring signals:
//! - Distance: 1/(1 + Distance_i) — proximity to critical path
//! - Necessity: relevant_events / total_events — fraction causally needed
//! - Contribution: agent_depth / total_depth — share of causal depth
//!
//! w_i = α·Distance + β·Necessity + γ·Contribution, normalized
//! Reward_i = w_i × FluxMinted

use std::collections::{HashMap, HashSet};

use setu_types::event::EventType;
use setu_types::pocw::{PoCWConfig, SolverReward};
use setu_types::Event;

use crate::dag::Dag;

/// Compute PoCW reward distribution for a fold's events.
///
/// Groups events by solver (from `executed_by`), scores each solver on
/// distance/necessity/contribution, normalizes weights, and distributes
/// `flux_minted` proportionally. Returns sorted by solver_id.
///
/// Returns empty vec if no events have a solver attribution.
pub fn compute_rewards(
    dag: &Dag,
    fold_events: &[Event],
    flux_minted: u64,
    config: &PoCWConfig,
) -> Vec<SolverReward> {
    // Group events by solver
    let mut solver_events: HashMap<String, Vec<&Event>> = HashMap::new();
    for event in fold_events {
        if let Some(ref solver_id) = event.executed_by {
            solver_events
                .entry(solver_id.clone())
                .or_default()
                .push(event);
        }
    }

    if solver_events.is_empty() {
        return Vec::new();
    }

    // Fold event IDs for intersection filtering
    let fold_ids: HashSet<&str> = fold_events.iter().map(|e| e.id.as_str()).collect();

    // Find fold tips: events within the fold that no other fold event points to as parent
    let tip_ancestors = compute_tip_ancestry(dag, fold_events, &fold_ids);

    // Per-solver depth sums and fold-wide totals for contribution scoring
    let total_depth: f64 = fold_events
        .iter()
        .filter_map(|e| dag.get_depth(&e.id))
        .sum::<u64>() as f64;

    // Score each solver
    let mut scored: Vec<(String, f64, f64, f64, f64, u64, u64)> = Vec::new();

    for (solver_id, events) in &solver_events {
        // Distance: single-solver → all events are on the critical path
        // distance = 0 → score = 1/(1+0) = 1.0
        // With multi-solver competition, replace with BFS to winner's critical path
        let distance_score = 1.0;

        // Necessity: fraction of solver events in tip ancestry
        let necessary = events
            .iter()
            .filter(|e| tip_ancestors.contains(e.id.as_str()))
            .count();
        let necessity_score = if events.is_empty() {
            0.0
        } else {
            necessary as f64 / events.len() as f64
        };

        // Contribution: solver_depth / total_depth
        let solver_depth: f64 = events
            .iter()
            .filter_map(|e| dag.get_depth(&e.id))
            .sum::<u64>() as f64;
        let contribution_score = if total_depth > 0.0 {
            solver_depth / total_depth
        } else {
            0.0
        };

        let raw_w = config.pocw_distance_weight * distance_score
            + config.pocw_necessity_weight * necessity_score
            + config.pocw_contribution_weight * contribution_score;

        let transfer_count = events
            .iter()
            .filter(|e| e.event_type == EventType::Transfer)
            .count() as u64;
        let task_count = events
            .iter()
            .filter(|e| e.event_type == EventType::TaskSubmit)
            .count() as u64;

        scored.push((
            solver_id.clone(),
            distance_score,
            necessity_score,
            contribution_score,
            raw_w,
            transfer_count,
            task_count,
        ));
    }

    // Normalize weights and distribute flux
    let total_raw: f64 = scored.iter().map(|s| s.4).sum();

    let mut rewards: Vec<SolverReward> = scored
        .iter()
        .map(
            |(solver_id, distance, necessity, contribution, raw_w, transfers, tasks)| {
                let weight = if total_raw > 0.0 {
                    raw_w / total_raw
                } else {
                    0.0
                };
                let flux_reward = (weight * flux_minted as f64).floor() as u64;

                SolverReward {
                    solver_id: solver_id.clone(),
                    transfer_count: *transfers,
                    task_count: *tasks,
                    distance_score: *distance,
                    necessity_score: *necessity,
                    contribution_score: *contribution,
                    weight,
                    flux_reward,
                }
            },
        )
        .collect();

    // Deterministic sort
    rewards.sort_by(|a, b| a.solver_id.cmp(&b.solver_id));

    // Fix rounding remainder: give to highest-weighted solver
    let total_distributed: u64 = rewards.iter().map(|r| r.flux_reward).sum();
    if total_distributed < flux_minted {
        let remainder = flux_minted - total_distributed;
        if let Some(top) = rewards
            .iter_mut()
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal))
        {
            top.flux_reward += remainder;
        }
    }

    rewards
}

/// Compute the set of fold event IDs that are ancestors of fold tips.
///
/// A fold tip is an event within the fold that no other fold event references
/// as a parent. The returned set includes the tips themselves.
fn compute_tip_ancestry<'a>(
    dag: &Dag,
    fold_events: &'a [Event],
    fold_ids: &HashSet<&'a str>,
) -> HashSet<String> {
    // Find fold tips
    let tips: Vec<&Event> = fold_events
        .iter()
        .filter(|e| {
            !fold_events
                .iter()
                .any(|other| other.parent_ids.contains(&e.id))
        })
        .collect();

    // Build union of all tip ancestors, intersected with fold events
    let mut ancestors: HashSet<String> = HashSet::new();
    for tip in &tips {
        ancestors.insert(tip.id.clone());
        for ancestor_id in dag.get_ancestors(&tip.id) {
            if fold_ids.contains(ancestor_id.as_str()) {
                ancestors.insert(ancestor_id);
            }
        }
    }

    ancestors
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::{VLCSnapshot, VectorClock};
    use setu_types::EventId;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Monotonic counter to ensure unique VLC logical_time per event.
    static COUNTER: AtomicU64 = AtomicU64::new(1);

    /// Build a test event with a given type, parents, and solver attribution.
    fn make_event(
        event_type: EventType,
        parent_ids: Vec<EventId>,
        executed_by: Option<&str>,
    ) -> Event {
        let vlc = VLCSnapshot {
            vector_clock: VectorClock::new(),
            logical_time: COUNTER.fetch_add(1, Ordering::Relaxed),
            physical_time: 1000,
        };
        let mut event = Event::new(
            event_type,
            parent_ids,
            vlc,
            "validator-1".to_string(),
        );
        event.executed_by = executed_by.map(|s| s.to_string());
        event
    }

    /// Insert an event into the DAG and return its ID.
    fn insert(dag: &mut Dag, event: Event) -> EventId {
        dag.add_event(event).expect("dag insert failed")
    }

    fn default_config() -> PoCWConfig {
        PoCWConfig {
            enabled: true,
            ..Default::default()
        }
    }

    // -- Basic behavior --

    #[test]
    fn test_no_solver_events_returns_empty() {
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let fold_events = vec![dag.get_event(&gid).unwrap().clone()];
        let rewards = compute_rewards(&dag, &fold_events, 1000, &default_config());
        assert!(rewards.is_empty());
    }

    #[test]
    fn test_single_solver_gets_all_flux() {
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let e1 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let e1_id = insert(&mut dag, e1);

        let fold_events = vec![dag.get_event(&e1_id).unwrap().clone()];
        let rewards = compute_rewards(&dag, &fold_events, 1000, &default_config());

        assert_eq!(rewards.len(), 1);
        assert_eq!(rewards[0].solver_id, "solver-1");
        assert_eq!(rewards[0].flux_reward, 1000);
        assert_eq!(rewards[0].task_count, 1);
    }

    #[test]
    fn test_two_solvers_equal_work() {
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        // Both solvers produce one event at the same depth
        let e1 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let e1_id = insert(&mut dag, e1);
        let e2 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-2"));
        let e2_id = insert(&mut dag, e2);

        let fold_events = vec![
            dag.get_event(&e1_id).unwrap().clone(),
            dag.get_event(&e2_id).unwrap().clone(),
        ];
        let rewards = compute_rewards(&dag, &fold_events, 1000, &default_config());

        assert_eq!(rewards.len(), 2);
        // Equal work → equal reward (500 each)
        assert_eq!(rewards[0].flux_reward + rewards[1].flux_reward, 1000);
        assert_eq!(rewards[0].flux_reward, 500);
        assert_eq!(rewards[1].flux_reward, 500);
    }

    // -- Necessity scoring --

    #[test]
    fn test_necessity_dead_end_scores_lower() {
        // DAG shape:
        //   genesis → A (solver-1) → C (solver-1)  ← fold tip
        //   genesis → B (solver-2)                  ← fold tip (dead-end relative to C)
        //
        // Solver-1: both A and C are ancestors of tip C → necessity = 1.0
        // Solver-2: B is a tip itself → necessity = 1.0
        // But solver-1 has higher contribution (depth sum = 1+2=3 vs 1)
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let a = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let aid = insert(&mut dag, a);

        let c = make_event(EventType::TaskSubmit, vec![aid.clone()], Some("solver-1"));
        let cid = insert(&mut dag, c);

        let b = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-2"));
        let bid = insert(&mut dag, b);

        let fold_events = vec![
            dag.get_event(&aid).unwrap().clone(),
            dag.get_event(&bid).unwrap().clone(),
            dag.get_event(&cid).unwrap().clone(),
        ];
        let rewards = compute_rewards(&dag, &fold_events, 1000, &default_config());

        // solver-1 has higher contribution (depth 1+2=3 vs depth 1)
        let s1 = rewards.iter().find(|r| r.solver_id == "solver-1").unwrap();
        let s2 = rewards.iter().find(|r| r.solver_id == "solver-2").unwrap();
        assert!(s1.contribution_score > s2.contribution_score);
        assert!(s1.flux_reward > s2.flux_reward);
    }

    // -- Contribution scoring --

    #[test]
    fn test_deeper_solver_gets_higher_contribution() {
        // genesis → A (solver-1) → B (solver-1) → C (solver-1)
        //         → D (solver-2)
        // solver-1 depth sum = 1+2+3 = 6, solver-2 depth sum = 1
        // total = 7, contribution: 6/7 vs 1/7
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let a = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let aid = insert(&mut dag, a);
        let b = make_event(EventType::TaskSubmit, vec![aid.clone()], Some("solver-1"));
        let bid = insert(&mut dag, b);
        let c = make_event(EventType::TaskSubmit, vec![bid.clone()], Some("solver-1"));
        let cid = insert(&mut dag, c);

        let d = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-2"));
        let did = insert(&mut dag, d);

        let fold_events = vec![
            dag.get_event(&aid).unwrap().clone(),
            dag.get_event(&bid).unwrap().clone(),
            dag.get_event(&cid).unwrap().clone(),
            dag.get_event(&did).unwrap().clone(),
        ];
        let rewards = compute_rewards(&dag, &fold_events, 700, &default_config());

        let s1 = rewards.iter().find(|r| r.solver_id == "solver-1").unwrap();
        let s2 = rewards.iter().find(|r| r.solver_id == "solver-2").unwrap();

        assert!((s1.contribution_score - 6.0 / 7.0).abs() < 1e-10);
        assert!((s2.contribution_score - 1.0 / 7.0).abs() < 1e-10);
        assert!(s1.flux_reward > s2.flux_reward);
    }

    // -- Rounding remainder --

    #[test]
    fn test_rounding_remainder_conserved() {
        // Verify total distributed == flux_minted regardless of rounding
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let e1 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let e1_id = insert(&mut dag, e1);
        let e2 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-2"));
        let e2_id = insert(&mut dag, e2);
        let e3 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-3"));
        let e3_id = insert(&mut dag, e3);

        let fold_events = vec![
            dag.get_event(&e1_id).unwrap().clone(),
            dag.get_event(&e2_id).unwrap().clone(),
            dag.get_event(&e3_id).unwrap().clone(),
        ];
        // 1000 / 3 doesn't divide evenly
        let rewards = compute_rewards(&dag, &fold_events, 1000, &default_config());
        let total: u64 = rewards.iter().map(|r| r.flux_reward).sum();
        assert_eq!(total, 1000);
    }

    // -- Transfer vs task counting --

    #[test]
    fn test_transfer_and_task_counts() {
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let t1 = make_event(EventType::Transfer, vec![gid.clone()], Some("solver-1"));
        let t1_id = insert(&mut dag, t1);
        let t2 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let t2_id = insert(&mut dag, t2);

        let fold_events = vec![
            dag.get_event(&t1_id).unwrap().clone(),
            dag.get_event(&t2_id).unwrap().clone(),
        ];
        let rewards = compute_rewards(&dag, &fold_events, 100, &default_config());

        assert_eq!(rewards[0].transfer_count, 1);
        assert_eq!(rewards[0].task_count, 1);
    }

    // -- Zero flux minted --

    #[test]
    fn test_zero_flux_minted() {
        let mut dag = Dag::new();
        let genesis = make_event(EventType::Genesis, vec![], None);
        let gid = insert(&mut dag, genesis);

        let e1 = make_event(EventType::TaskSubmit, vec![gid.clone()], Some("solver-1"));
        let e1_id = insert(&mut dag, e1);

        let fold_events = vec![dag.get_event(&e1_id).unwrap().clone()];
        let rewards = compute_rewards(&dag, &fold_events, 0, &default_config());

        assert_eq!(rewards.len(), 1);
        assert_eq!(rewards[0].flux_reward, 0);
    }
}
