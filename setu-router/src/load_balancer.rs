//! Load balancer module

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;

/// Load balancing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadBalancingStrategy {
    /// Round-robin selection
    RoundRobin,
    
    /// Random selection
    Random,
    
    /// Least loaded solver
    LeastLoaded,
    
    /// Weighted by capacity
    WeightedCapacity,
}

/// Load balancer errors
#[derive(Debug, Error)]
pub enum LoadBalancerError {
    #[error("No solvers available")]
    NoSolversAvailable,
    
    #[error("Solver not found: {0}")]
    SolverNotFound(String),
}

/// Load balancer
pub struct LoadBalancer {
    strategy: LoadBalancingStrategy,
    solvers: parking_lot::RwLock<Vec<String>>,
    round_robin_counter: AtomicUsize,
}

impl LoadBalancer {
    pub fn new(strategy: LoadBalancingStrategy) -> Self {
        Self {
            strategy,
            solvers: parking_lot::RwLock::new(vec![]),
            round_robin_counter: AtomicUsize::new(0),
        }
    }
    
    /// Add a solver to the load balancer
    pub fn add_solver(&self, solver_id: String) {
        let mut solvers = self.solvers.write();
        if !solvers.contains(&solver_id) {
            solvers.push(solver_id);
        }
    }
    
    /// Remove a solver from the load balancer
    pub fn remove_solver(&self, solver_id: &str) {
        let mut solvers = self.solvers.write();
        solvers.retain(|s| s != solver_id);
    }
    
    /// Select a solver based on the strategy
    pub fn select_solver(&self) -> Result<String, LoadBalancerError> {
        let solvers = self.solvers.read();
        
        if solvers.is_empty() {
            return Err(LoadBalancerError::NoSolversAvailable);
        }
        
        match self.strategy {
            LoadBalancingStrategy::RoundRobin => {
                let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % solvers.len();
                Ok(solvers[index].clone())
            }
            LoadBalancingStrategy::Random => {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                let index = rng.gen_range(0..solvers.len());
                Ok(solvers[index].clone())
            }
            LoadBalancingStrategy::LeastLoaded => {
                // For now, fallback to round-robin
                // In production, this would query actual load metrics
                let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % solvers.len();
                Ok(solvers[index].clone())
            }
            LoadBalancingStrategy::WeightedCapacity => {
                // For now, fallback to round-robin
                // In production, this would use weighted selection
                let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % solvers.len();
                Ok(solvers[index].clone())
            }
        }
    }
    
    /// Get number of registered solvers
    pub fn solver_count(&self) -> usize {
        self.solvers.read().len()
    }
    
    /// Get all solver IDs
    pub fn get_solvers(&self) -> Vec<String> {
        self.solvers.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_add_solver() {
        let lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);
        lb.add_solver("solver1".to_string());
        lb.add_solver("solver2".to_string());
        
        assert_eq!(lb.solver_count(), 2);
    }
    
    #[test]
    fn test_round_robin() {
        let lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);
        lb.add_solver("solver1".to_string());
        lb.add_solver("solver2".to_string());
        lb.add_solver("solver3".to_string());
        
        let s1 = lb.select_solver().unwrap();
        let s2 = lb.select_solver().unwrap();
        let s3 = lb.select_solver().unwrap();
        let s4 = lb.select_solver().unwrap();
        
        assert_eq!(s1, "solver1");
        assert_eq!(s2, "solver2");
        assert_eq!(s3, "solver3");
        assert_eq!(s4, "solver1"); // Wraps around
    }
    
    #[test]
    fn test_no_solvers() {
        let lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);
        let result = lb.select_solver();
        
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoadBalancerError::NoSolversAvailable));
    }
}

