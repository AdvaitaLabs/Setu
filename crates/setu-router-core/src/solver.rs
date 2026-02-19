//! Solver registry - tracks available solvers and their status

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Unique identifier for a solver
pub type SolverId = String;

/// Solver health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverStatus {
    /// Solver is healthy and accepting transactions
    Online,
    /// Solver is temporarily unavailable (e.g., high load)
    Busy,
    /// Solver is offline
    Offline,
    /// Solver status is unknown (no recent heartbeat)
    Unknown,
}

impl Default for SolverStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Information about a solver
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverInfo {
    /// Unique solver identifier
    pub id: SolverId,

    /// Network address (e.g., "127.0.0.1:9001")
    pub address: String,

    /// Current status
    pub status: SolverStatus,

    /// Resource domains this solver handles
    /// Empty means solver can handle any resource
    pub resource_domains: Vec<String>,

    /// Current load (number of pending transactions)
    pub pending_load: u64,

    /// Maximum capacity
    pub max_capacity: u64,

    /// Weight for load balancing (higher = more traffic)
    pub weight: u32,

    /// Last heartbeat timestamp (milliseconds since epoch)
    #[serde(skip)]
    pub last_heartbeat: Option<Instant>,
}

impl SolverInfo {
    /// Create a new solver info
    pub fn new(id: SolverId, address: String) -> Self {
        Self {
            id,
            address,
            status: SolverStatus::Online,
            resource_domains: vec![],
            pending_load: 0,
            max_capacity: 10000,
            weight: 100,
            last_heartbeat: Some(Instant::now()),
        }
    }

    /// Create solver with specific resource domains
    pub fn with_domains(mut self, domains: Vec<String>) -> Self {
        self.resource_domains = domains;
        self
    }

    /// Set solver weight
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Set max capacity
    pub fn with_capacity(mut self, capacity: u64) -> Self {
        self.max_capacity = capacity;
        self
    }

    /// Check if solver is available for routing
    pub fn is_available(&self) -> bool {
        matches!(self.status, SolverStatus::Online) && self.pending_load < self.max_capacity
    }

    /// Get load ratio (0.0 - 1.0)
    pub fn load_ratio(&self) -> f64 {
        if self.max_capacity == 0 {
            1.0
        } else {
            self.pending_load as f64 / self.max_capacity as f64
        }
    }

    /// Check if solver can handle the given resource
    pub fn can_handle_resource(&self, resource: &str) -> bool {
        // Empty domains means can handle any resource
        if self.resource_domains.is_empty() {
            return true;
        }

        // Check if resource matches any domain
        self.resource_domains
            .iter()
            .any(|domain| resource.starts_with(domain) || domain == "*")
    }
}

/// Registry for tracking available solvers
#[derive(Debug)]
pub struct SolverRegistry {
    /// Map of solver ID to solver info
    solvers: Arc<RwLock<HashMap<SolverId, SolverInfo>>>,

    /// Heartbeat timeout duration
    heartbeat_timeout: Duration,
}

impl SolverRegistry {
    /// Create a new solver registry
    pub fn new() -> Self {
        Self {
            solvers: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_timeout: Duration::from_secs(30),
        }
    }

    /// Create registry with custom heartbeat timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            solvers: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_timeout: timeout,
        }
    }

    /// Register a new solver
    pub fn register(&self, mut solver: SolverInfo) {
        solver.last_heartbeat = Some(Instant::now());
        info!(
            solver_id = %solver.id,
            address = %solver.address,
            "Registering solver"
        );

        let mut solvers = self.solvers.write();
        solvers.insert(solver.id.clone(), solver);
    }

    /// Unregister a solver
    pub fn unregister(&self, solver_id: &SolverId) {
        info!(solver_id = %solver_id, "Unregistering solver");
        let mut solvers = self.solvers.write();
        solvers.remove(solver_id);
    }

    /// Update solver heartbeat
    pub fn heartbeat(&self, solver_id: &SolverId) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.last_heartbeat = Some(Instant::now());
            if solver.status == SolverStatus::Unknown {
                solver.status = SolverStatus::Online;
            }
            debug!(solver_id = %solver_id, "Heartbeat received");
        }
    }

    /// Update solver status
    pub fn update_status(&self, solver_id: &SolverId, status: SolverStatus) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.status = status;
            debug!(
                solver_id = %solver_id,
                status = ?status,
                "Solver status updated"
            );
        }
    }

    /// Update solver load
    pub fn update_load(&self, solver_id: &SolverId, pending_load: u64) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.pending_load = pending_load;
            debug!(
                solver_id = %solver_id,
                pending_load = pending_load,
                "Solver load updated"
            );
        }
    }

    /// Get solver info by ID
    pub fn get(&self, solver_id: &SolverId) -> Option<SolverInfo> {
        let solvers = self.solvers.read();
        solvers.get(solver_id).cloned()
    }

    /// Get all registered solvers
    pub fn get_all(&self) -> Vec<SolverInfo> {
        let solvers = self.solvers.read();
        solvers.values().cloned().collect()
    }

    /// Get all available solvers (online and not at capacity)
    pub fn get_available(&self) -> Vec<SolverInfo> {
        self.check_timeouts();

        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.is_available())
            .cloned()
            .collect()
    }

    /// Get available solvers that can handle a specific resource
    pub fn get_available_for_resource(&self, resource: &str) -> Vec<SolverInfo> {
        self.check_timeouts();

        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.is_available() && s.can_handle_resource(resource))
            .cloned()
            .collect()
    }

    /// Check for timed out solvers and update their status
    fn check_timeouts(&self) {
        let mut solvers = self.solvers.write();
        let now = Instant::now();

        for solver in solvers.values_mut() {
            if let Some(last_hb) = solver.last_heartbeat {
                if now.duration_since(last_hb) > self.heartbeat_timeout {
                    if solver.status != SolverStatus::Unknown {
                        warn!(
                            solver_id = %solver.id,
                            "Solver heartbeat timeout, marking as unknown"
                        );
                        solver.status = SolverStatus::Unknown;
                    }
                }
            }
        }
    }

    /// Get the count of registered solvers
    pub fn count(&self) -> usize {
        let solvers = self.solvers.read();
        solvers.len()
    }

    /// Get the count of available solvers
    pub fn available_count(&self) -> usize {
        self.get_available().len()
    }
}

impl Default for SolverRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solver_info_creation() {
        let solver = SolverInfo::new("solver-1".to_string(), "127.0.0.1:9001".to_string());

        assert_eq!(solver.id, "solver-1");
        assert_eq!(solver.status, SolverStatus::Online);
        assert!(solver.is_available());
    }

    #[test]
    fn test_solver_resource_handling() {
        let solver = SolverInfo::new("solver-1".to_string(), "127.0.0.1:9001".to_string())
            .with_domains(vec!["account:".to_string(), "coin:".to_string()]);

        assert!(solver.can_handle_resource("account:alice"));
        assert!(solver.can_handle_resource("coin:btc"));
        assert!(!solver.can_handle_resource("nft:token1"));
    }

    #[test]
    fn test_registry_operations() {
        let registry = SolverRegistry::new();

        let solver1 = SolverInfo::new("solver-1".to_string(), "127.0.0.1:9001".to_string());
        let solver2 = SolverInfo::new("solver-2".to_string(), "127.0.0.1:9002".to_string());

        registry.register(solver1);
        registry.register(solver2);

        assert_eq!(registry.count(), 2);
        assert_eq!(registry.available_count(), 2);

        registry.unregister(&"solver-1".to_string());
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_load_ratio() {
        let mut solver = SolverInfo::new("solver-1".to_string(), "127.0.0.1:9001".to_string())
            .with_capacity(1000);

        solver.pending_load = 500;
        assert!((solver.load_ratio() - 0.5).abs() < 0.001);

        solver.pending_load = 1000;
        assert!(!solver.is_available());
    }
}
