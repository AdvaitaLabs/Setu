//! Solver registry for tracking available solvers

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use tracing::{debug, info, warn};

/// Solver status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverStatus {
    /// Solver is active and accepting transfers
    Active,
    
    /// Solver is temporarily unavailable
    Inactive,
    
    /// Solver is overloaded
    Overloaded,
    
    /// Solver has failed
    Failed,
}

/// Solver information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverInfo {
    /// Unique solver ID
    pub id: String,
    
    /// Current status
    pub status: SolverStatus,
    
    /// Maximum capacity (transfers per second)
    pub capacity: u32,
    
    /// Current load (active transfers)
    pub current_load: u32,
    
    /// Total transfers processed
    pub total_processed: u64,
    
    /// Shard ID (optional)
    pub shard_id: Option<String>,
    
    /// Resources this solver handles (for affinity routing)
    pub resources: Vec<String>,
    
    /// Last heartbeat timestamp
    pub last_heartbeat: u64,
}

impl SolverInfo {
    /// Create a new solver info
    pub fn new(id: String, capacity: u32) -> Self {
        Self {
            id,
            status: SolverStatus::Active,
            capacity,
            current_load: 0,
            total_processed: 0,
            shard_id: None,
            resources: vec![],
            last_heartbeat: Self::current_time(),
        }
    }
    
    /// Check if solver is available
    pub fn is_available(&self) -> bool {
        self.status == SolverStatus::Active && self.current_load < self.capacity
    }
    
    /// Get load percentage
    pub fn load_percentage(&self) -> f32 {
        if self.capacity == 0 {
            return 100.0;
        }
        (self.current_load as f32 / self.capacity as f32) * 100.0
    }
    
    /// Update heartbeat
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Self::current_time();
    }
    
    /// Check if solver is stale (no heartbeat for 30s)
    pub fn is_stale(&self) -> bool {
        let now = Self::current_time();
        now.saturating_sub(self.last_heartbeat) > 30_000
    }
    
    fn current_time() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}

/// Solver registry
pub struct SolverRegistry {
    /// Map of solver ID to solver info
    solvers: Arc<RwLock<HashMap<String, SolverInfo>>>,
    
    /// Resource to solver mapping (for affinity routing)
    resource_map: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl SolverRegistry {
    /// Create a new solver registry
    pub fn new() -> Self {
        Self {
            solvers: Arc::new(RwLock::new(HashMap::new())),
            resource_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Register a solver
    pub fn register(&self, solver_info: SolverInfo) {
        let solver_id = solver_info.id.clone();
        
        info!(
            solver_id = %solver_id,
            capacity = solver_info.capacity,
            shard_id = ?solver_info.shard_id,
            "Registering solver"
        );
        
        // Update resource mapping
        for resource in &solver_info.resources {
            let mut resource_map = self.resource_map.write();
            resource_map
                .entry(resource.clone())
                .or_insert_with(Vec::new)
                .push(solver_id.clone());
        }
        
        // Add to registry
        let mut solvers = self.solvers.write();
        solvers.insert(solver_id, solver_info);
    }
    
    /// Unregister a solver
    pub fn unregister(&self, solver_id: &str) {
        info!(
            solver_id = %solver_id,
            "Unregistering solver"
        );
        
        // Remove from registry
        let mut solvers = self.solvers.write();
        if let Some(solver_info) = solvers.remove(solver_id) {
            // Remove from resource mapping
            let mut resource_map = self.resource_map.write();
            for resource in &solver_info.resources {
                if let Some(solver_ids) = resource_map.get_mut(resource) {
                    solver_ids.retain(|id| id != solver_id);
                }
            }
        }
    }
    
    /// Get solver info
    pub fn get(&self, solver_id: &str) -> Option<SolverInfo> {
        let solvers = self.solvers.read();
        solvers.get(solver_id).cloned()
    }
    
    /// Get all solvers
    pub fn get_all(&self) -> Vec<SolverInfo> {
        let solvers = self.solvers.read();
        solvers.values().cloned().collect()
    }
    
    /// Get active solvers
    pub fn get_active(&self) -> Vec<SolverInfo> {
        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.status == SolverStatus::Active)
            .cloned()
            .collect()
    }
    
    /// Get available solvers (active and not overloaded)
    pub fn get_available(&self) -> Vec<SolverInfo> {
        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.is_available())
            .cloned()
            .collect()
    }
    
    /// Find solver by resource
    pub fn find_by_resource(&self, resource: &str) -> Option<String> {
        let resource_map = self.resource_map.read();
        let solver_ids = resource_map.get(resource)?;
        
        // Find the least loaded solver for this resource
        let solvers = self.solvers.read();
        solver_ids
            .iter()
            .filter_map(|id| solvers.get(id))
            .filter(|s| s.is_available())
            .min_by_key(|s| s.current_load)
            .map(|s| s.id.clone())
    }
    
    /// Find solver by shard
    pub fn find_by_shard(&self, shard_id: &str) -> Vec<String> {
        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.shard_id.as_deref() == Some(shard_id) && s.is_available())
            .map(|s| s.id.clone())
            .collect()
    }
    
    /// Update solver status
    pub fn update_status(&self, solver_id: &str, status: SolverStatus) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            debug!(
                solver_id = %solver_id,
                old_status = ?solver.status,
                new_status = ?status,
                "Updating solver status"
            );
            solver.status = status;
        }
    }
    
    /// Increment solver load
    pub fn increment_load(&self, solver_id: &str) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.current_load += 1;
            solver.total_processed += 1;
            
            // Check if overloaded
            if solver.current_load >= solver.capacity {
                warn!(
                    solver_id = %solver_id,
                    load = solver.current_load,
                    capacity = solver.capacity,
                    "Solver is overloaded"
                );
                solver.status = SolverStatus::Overloaded;
            }
        }
    }
    
    /// Decrement solver load
    pub fn decrement_load(&self, solver_id: &str) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.current_load = solver.current_load.saturating_sub(1);
            
            // If was overloaded, mark as active again
            if solver.status == SolverStatus::Overloaded && solver.current_load < solver.capacity {
                debug!(
                    solver_id = %solver_id,
                    "Solver recovered from overload"
                );
                solver.status = SolverStatus::Active;
            }
        }
    }
    
    /// Update solver heartbeat
    pub fn heartbeat(&self, solver_id: &str) {
        let mut solvers = self.solvers.write();
        if let Some(solver) = solvers.get_mut(solver_id) {
            solver.heartbeat();
        }
    }
    
    /// Get solver index (for channel lookup)
    pub fn get_index(&self, solver_id: &str) -> Option<usize> {
        let solvers = self.solvers.read();
        solvers
            .keys()
            .position(|id| id == solver_id)
    }
    
    /// Get total solver count
    pub fn count(&self) -> usize {
        let solvers = self.solvers.read();
        solvers.len()
    }
    
    /// Get active solver count
    pub fn active_count(&self) -> usize {
        let solvers = self.solvers.read();
        solvers
            .values()
            .filter(|s| s.status == SolverStatus::Active)
            .count()
    }
    
    /// Remove stale solvers
    pub fn remove_stale(&self) -> Vec<String> {
        let mut solvers = self.solvers.write();
        let stale_ids: Vec<String> = solvers
            .iter()
            .filter(|(_, s)| s.is_stale())
            .map(|(id, _)| id.clone())
            .collect();
        
        for id in &stale_ids {
            warn!(
                solver_id = %id,
                "Removing stale solver"
            );
            solvers.remove(id);
        }
        
        stale_ids
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
    fn test_register_solver() {
        let registry = SolverRegistry::new();
        let solver = SolverInfo::new("solver1".to_string(), 100);
        
        registry.register(solver);
        
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.active_count(), 1);
    }
    
    #[test]
    fn test_get_solver() {
        let registry = SolverRegistry::new();
        let solver = SolverInfo::new("solver1".to_string(), 100);
        
        registry.register(solver);
        
        let retrieved = registry.get("solver1").unwrap();
        assert_eq!(retrieved.id, "solver1");
        assert_eq!(retrieved.capacity, 100);
    }
    
    #[test]
    fn test_increment_load() {
        let registry = SolverRegistry::new();
        let solver = SolverInfo::new("solver1".to_string(), 2);
        
        registry.register(solver);
        
        registry.increment_load("solver1");
        let info = registry.get("solver1").unwrap();
        assert_eq!(info.current_load, 1);
        
        registry.increment_load("solver1");
        let info = registry.get("solver1").unwrap();
        assert_eq!(info.current_load, 2);
        assert_eq!(info.status, SolverStatus::Overloaded);
    }
    
    #[test]
    fn test_find_by_resource() {
        let registry = SolverRegistry::new();
        
        let mut solver1 = SolverInfo::new("solver1".to_string(), 100);
        solver1.resources = vec!["alice".to_string()];
        registry.register(solver1);
        
        let mut solver2 = SolverInfo::new("solver2".to_string(), 100);
        solver2.resources = vec!["bob".to_string()];
        registry.register(solver2);
        
        let found = registry.find_by_resource("alice");
        assert_eq!(found, Some("solver1".to_string()));
    }
}

