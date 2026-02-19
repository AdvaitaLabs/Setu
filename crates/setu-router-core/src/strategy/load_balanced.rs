//! Load Balanced Strategy for Solver Selection
//!
//! Selects the solver with the lowest current load.
//! Falls back to weighted selection if all solvers are above threshold.

use tracing::trace;

use super::SolverStrategy;
use crate::error::RouterError;
use crate::solver::SolverInfo;

/// Load-balanced routing strategy
pub struct LoadBalancedStrategy {
    /// Load threshold for weighted selection
    load_threshold: f64,
}

impl LoadBalancedStrategy {
    /// Create a new load balanced strategy
    pub fn new() -> Self {
        Self {
            load_threshold: 0.9,
        }
    }

    /// Create with custom load threshold
    pub fn with_threshold(threshold: f64) -> Self {
        Self {
            load_threshold: threshold,
        }
    }
}

impl Default for LoadBalancedStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SolverStrategy for LoadBalancedStrategy {
    fn select(
        &self,
        available: &[SolverInfo],
        _routing_key: &str,
    ) -> Result<SolverInfo, RouterError> {
        if available.is_empty() {
            return Err(RouterError::NoSolverAvailable);
        }

        // First, try to find solvers below threshold
        let candidates: Vec<_> = available
            .iter()
            .filter(|s| s.load_ratio() < self.load_threshold)
            .collect();

        if !candidates.is_empty() {
            // Select based on weighted capacity: (1 - load) * weight
            let solver = candidates
                .into_iter()
                .max_by(|a, b| {
                    let score_a = (1.0 - a.load_ratio()) * a.weight as f64;
                    let score_b = (1.0 - b.load_ratio()) * b.weight as f64;
                    score_a
                        .partial_cmp(&score_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();

            trace!(solver_id = %solver.id, load = %solver.load_ratio(), "Selected by weighted capacity");
            return Ok(solver.clone());
        }

        // Fallback: select least loaded
        let solver = available
            .iter()
            .min_by(|a, b| {
                a.load_ratio()
                    .partial_cmp(&b.load_ratio())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        trace!(solver_id = %solver.id, load = %solver.load_ratio(), "Selected least loaded");
        Ok(solver.clone())
    }

    fn name(&self) -> &'static str {
        "LoadBalanced"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_solvers(count: usize) -> Vec<SolverInfo> {
        (1..=count)
            .map(|i| SolverInfo::new(format!("solver-{}", i), format!("127.0.0.1:{}", 9000 + i)))
            .collect()
    }

    #[test]
    fn test_load_balanced_selection() {
        let strategy = LoadBalancedStrategy::default();
        let mut solvers = create_test_solvers(3);

        // Set different loads
        solvers[0].pending_load = 100;
        solvers[1].pending_load = 50; // Lowest load
        solvers[2].pending_load = 200;

        let result = strategy.select(&solvers, "any").unwrap();

        // Should select solver with lowest load
        assert_eq!(result.id, "solver-2");
    }

    #[test]
    fn test_empty_solvers() {
        let strategy = LoadBalancedStrategy::default();
        let result = strategy.select(&[], "key");
        assert!(matches!(result, Err(RouterError::NoSolverAvailable)));
    }
}
