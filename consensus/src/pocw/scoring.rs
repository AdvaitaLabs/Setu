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
                let flux_reward = (weight * flux_minted as f64).round() as u64;

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
