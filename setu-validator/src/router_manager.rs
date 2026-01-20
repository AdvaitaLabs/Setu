//! Router Manager - Manages routing of transfers to solvers
//!
//! This module integrates setu-router-core into the Validator,
//! providing transfer routing and solver management capabilities.

use setu_types::Transfer;
use parking_lot::RwLock;
use setu_router_core::{
    UnifiedRouter,
    SolverInfo, SolverStatus,
    ConsistentHashStrategy,
    SolverStrategy,  // Import the trait
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};

/// Router manager error
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("No solver available")]
    NoSolverAvailable,
    
    #[error("Solver not found: {0}")]
    SolverNotFound(String),
    
    #[error("Failed to send transfer: {0}")]
    SendFailed(String),
    
    #[error("Routing failed: {0}")]
    RoutingFailed(String),
}

/// Solver connection info
#[derive(Debug, Clone)]
pub struct SolverConnection {
    pub id: String,
    pub address: String,
    pub capacity: u32,
    pub current_load: u32,
    pub status: SolverStatus,
    pub shard_id: Option<String>,
    pub resources: Vec<String>,
}

impl SolverConnection {
    pub fn new(id: String, address: String, capacity: u32) -> Self {
        Self {
            id,
            address,
            capacity,
            current_load: 0,
            status: SolverStatus::Online,
            shard_id: None,
            resources: vec![],
        }
    }
    
    pub fn with_shard(mut self, shard_id: String) -> Self {
        self.shard_id = Some(shard_id);
        self
    }
    
    pub fn with_resources(mut self, resources: Vec<String>) -> Self {
        self.resources = resources;
        self
    }
    
    pub fn is_available(&self) -> bool {
        self.status == SolverStatus::Online && self.current_load < self.capacity
    }
    
    pub fn load_ratio(&self) -> f64 {
        if self.capacity == 0 {
            1.0
        } else {
            self.current_load as f64 / self.capacity as f64
        }
    }
}

/// Router manager for the Validator
pub struct RouterManager {
    /// Unified router from setu-router-core
    #[allow(dead_code)] // Reserved for future unified router integration
    router: UnifiedRouter,
    
    /// Solver registry
    solver_registry: Arc<RwLock<HashMap<String, SolverConnection>>>,
    
    /// Solver channels for sending transfers
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<Transfer>>>>,
    
    /// Consistent hash strategy for deterministic routing
    consistent_hash: ConsistentHashStrategy,
}

impl RouterManager {
    /// Create a new router manager
    pub fn new() -> Self {
        info!("Creating RouterManager with UnifiedRouter");
        
        Self {
            router: UnifiedRouter::new(),
            solver_registry: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            consistent_hash: ConsistentHashStrategy::new(),
        }
    }
    
    /// Create with custom shard count
    pub fn with_shard_count(shard_count: u16) -> Self {
        info!(shard_count = shard_count, "Creating RouterManager with custom shard count");
        
        Self {
            router: UnifiedRouter::with_shard_count(shard_count),
            solver_registry: Arc::new(RwLock::new(HashMap::new())),
            solver_channels: Arc::new(RwLock::new(HashMap::new())),
            consistent_hash: ConsistentHashStrategy::new(),
        }
    }
    
    /// Register a solver
    pub fn register_solver(
        &self,
        solver_id: String,
        address: String,
        capacity: u32,
        channel: mpsc::UnboundedSender<Transfer>,
    ) {
        info!(
            solver_id = %solver_id,
            address = %address,
            capacity = capacity,
            "Registering solver"
        );
        
        let connection = SolverConnection::new(solver_id.clone(), address, capacity);
        
        self.solver_registry.write().insert(solver_id.clone(), connection);
        self.solver_channels.write().insert(solver_id.clone(), channel);
        
        info!(
            solver_id = %solver_id,
            total_solvers = self.solver_count(),
            "Solver registered successfully"
        );
    }
    
    /// Register a solver with shard and resource affinity
    pub fn register_solver_with_affinity(
        &self,
        solver_id: String,
        address: String,
        capacity: u32,
        channel: mpsc::UnboundedSender<Transfer>,
        shard_id: Option<String>,
        resources: Vec<String>,
    ) {
        info!(
            solver_id = %solver_id,
            address = %address,
            capacity = capacity,
            shard_id = ?shard_id,
            resources = ?resources,
            "Registering solver with affinity"
        );
        
        let mut connection = SolverConnection::new(solver_id.clone(), address, capacity);
        if let Some(shard) = shard_id {
            connection = connection.with_shard(shard);
        }
        if !resources.is_empty() {
            connection = connection.with_resources(resources);
        }
        
        self.solver_registry.write().insert(solver_id.clone(), connection);
        self.solver_channels.write().insert(solver_id.clone(), channel);
        
        info!(
            solver_id = %solver_id,
            total_solvers = self.solver_count(),
            "Solver registered with affinity"
        );
    }
    
    /// Unregister a solver
    pub fn unregister_solver(&self, solver_id: &str) {
        info!(solver_id = %solver_id, "Unregistering solver");
        
        self.solver_registry.write().remove(solver_id);
        self.solver_channels.write().remove(solver_id);
        
        info!(
            solver_id = %solver_id,
            total_solvers = self.solver_count(),
            "Solver unregistered"
        );
    }
    
    /// Update solver load
    pub fn update_solver_load(&self, solver_id: &str, load: u32) {
        if let Some(solver) = self.solver_registry.write().get_mut(solver_id) {
            solver.current_load = load;
            debug!(
                solver_id = %solver_id,
                load = load,
                "Updated solver load"
            );
        }
    }
    
    /// Update solver status
    pub fn update_solver_status(&self, solver_id: &str, status: SolverStatus) {
        if let Some(solver) = self.solver_registry.write().get_mut(solver_id) {
            solver.status = status;
            info!(
                solver_id = %solver_id,
                status = ?status,
                "Updated solver status"
            );
        }
    }
    
    /// Route a transfer to a solver
    pub fn route_transfer(&self, transfer: &Transfer) -> Result<String, RouterError> {
        // Priority 1: Manual solver selection (preferred_solver)
        if let Some(preferred) = &transfer.preferred_solver {
            if self.is_solver_available(preferred) {
                debug!(
                    transfer_id = %transfer.id,
                    solver_id = %preferred,
                    "Using preferred solver"
                );
                return Ok(preferred.clone());
            }
            warn!(
                transfer_id = %transfer.id,
                solver_id = %preferred,
                "Preferred solver not available, falling back"
            );
        }
        
        // Priority 2: Shard-based routing
        if let Some(shard_id) = &transfer.shard_id {
            if let Some(solver_id) = self.find_solver_by_shard(shard_id) {
                debug!(
                    transfer_id = %transfer.id,
                    shard_id = %shard_id,
                    solver_id = %solver_id,
                    "Using shard-based routing"
                );
                return Ok(solver_id);
            }
        }
        
        // Priority 3: Consistent hash based on resources
        let routing_key = self.get_routing_key(transfer);
        let available_solvers = self.get_available_solvers();
        
        if available_solvers.is_empty() {
            return Err(RouterError::NoSolverAvailable);
        }
        
        // Convert to SolverInfo for consistent hash
        let solver_infos: Vec<SolverInfo> = available_solvers
            .iter()
            .map(|s| {
                let mut info = SolverInfo::new(s.id.clone(), s.address.clone());
                info.pending_load = s.current_load as u64;
                info.max_capacity = s.capacity as u64;
                info
            })
            .collect();
        
        let selected = self.consistent_hash
            .select(&solver_infos, &routing_key)
            .map_err(|e| RouterError::RoutingFailed(e.to_string()))?;
        
        debug!(
            transfer_id = %transfer.id,
            routing_key = %routing_key,
            solver_id = %selected.id,
            "Using consistent hash routing"
        );
        
        Ok(selected.id)
    }
    
    /// Send transfer to a solver
    pub async fn send_to_solver(
        &self,
        solver_id: &str,
        transfer: Transfer,
    ) -> Result<(), RouterError> {
        // Get channel
        let channels = self.solver_channels.read();
        let channel = channels.get(solver_id)
            .ok_or_else(|| RouterError::SolverNotFound(solver_id.to_string()))?;
        
        // Send transfer
        channel.send(transfer)
            .map_err(|e| RouterError::SendFailed(e.to_string()))?;
        
        // Increment load
        drop(channels);
        self.increment_solver_load(solver_id);
        
        debug!(solver_id = %solver_id, "Transfer sent to solver");
        
        Ok(())
    }
    
    /// Route and send transfer in one operation
    pub async fn route_and_send(&self, transfer: Transfer) -> Result<String, RouterError> {
        let solver_id = self.route_transfer(&transfer)?;
        self.send_to_solver(&solver_id, transfer).await?;
        Ok(solver_id)
    }
    
    /// Get routing key from transfer
    fn get_routing_key(&self, transfer: &Transfer) -> String {
        // Priority: first resource > from address > transfer id
        transfer.resources.first()
            .cloned()
            .unwrap_or_else(|| transfer.from.clone())
    }
    
    /// Find solver by shard
    fn find_solver_by_shard(&self, shard_id: &str) -> Option<String> {
        let registry = self.solver_registry.read();
        
        // Find all solvers in this shard
        let shard_solvers: Vec<_> = registry
            .values()
            .filter(|s| s.shard_id.as_deref() == Some(shard_id) && s.is_available())
            .collect();
        
        if shard_solvers.is_empty() {
            return None;
        }
        
        // Select least loaded solver in the shard
        shard_solvers
            .iter()
            .min_by_key(|s| s.current_load)
            .map(|s| s.id.clone())
    }
    
    /// Check if solver is available
    fn is_solver_available(&self, solver_id: &str) -> bool {
        self.solver_registry.read()
            .get(solver_id)
            .map(|s| s.is_available())
            .unwrap_or(false)
    }
    
    /// Get available solvers
    fn get_available_solvers(&self) -> Vec<SolverConnection> {
        self.solver_registry.read()
            .values()
            .filter(|s| s.is_available())
            .cloned()
            .collect()
    }
    
    /// Increment solver load
    fn increment_solver_load(&self, solver_id: &str) {
        if let Some(solver) = self.solver_registry.write().get_mut(solver_id) {
            solver.current_load += 1;
        }
    }
    
    /// Decrement solver load (called when transfer completes)
    pub fn decrement_solver_load(&self, solver_id: &str) {
        if let Some(solver) = self.solver_registry.write().get_mut(solver_id) {
            solver.current_load = solver.current_load.saturating_sub(1);
        }
    }
    
    /// Get solver count
    pub fn solver_count(&self) -> usize {
        self.solver_registry.read().len()
    }
    
    /// Get available solver count
    pub fn available_solver_count(&self) -> usize {
        self.solver_registry.read()
            .values()
            .filter(|s| s.is_available())
            .count()
    }
    
    /// Get solver info
    pub fn get_solver(&self, solver_id: &str) -> Option<SolverConnection> {
        self.solver_registry.read().get(solver_id).cloned()
    }
    
    /// Get all solvers
    pub fn get_all_solvers(&self) -> Vec<SolverConnection> {
        self.solver_registry.read().values().cloned().collect()
    }
}

impl Default for RouterManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::TransferType;
    
    fn create_test_transfer(id: &str) -> Transfer {
        Transfer::new(id, "alice", "bob", 100)
            .with_type(TransferType::FluxTransfer)
            .with_resources(vec!["alice".to_string()])
            .with_power(10)
    }
    
    #[test]
    fn test_router_manager_creation() {
        let manager = RouterManager::new();
        assert_eq!(manager.solver_count(), 0);
    }
    
    #[test]
    fn test_register_solver() {
        let manager = RouterManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        
        manager.register_solver(
            "solver-1".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx,
        );
        
        assert_eq!(manager.solver_count(), 1);
        assert!(manager.is_solver_available("solver-1"));
    }
    
    #[test]
    fn test_route_transfer() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        
        manager.register_solver("solver-1".to_string(), "127.0.0.1:9001".to_string(), 100, tx1);
        manager.register_solver("solver-2".to_string(), "127.0.0.1:9002".to_string(), 100, tx2);
        
        let transfer = create_test_transfer("tx-1");
        let result = manager.route_transfer(&transfer);
        
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_preferred_solver() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        
        manager.register_solver("solver-1".to_string(), "127.0.0.1:9001".to_string(), 100, tx1);
        manager.register_solver("solver-2".to_string(), "127.0.0.1:9002".to_string(), 100, tx2);
        
        let mut transfer = create_test_transfer("tx-1");
        transfer.preferred_solver = Some("solver-2".to_string());
        
        let result = manager.route_transfer(&transfer).unwrap();
        assert_eq!(result, "solver-2");
    }
    
    #[test]
    fn test_consistent_hash_deterministic() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let (tx3, _rx3) = mpsc::unbounded_channel();
        
        manager.register_solver("solver-1".to_string(), "127.0.0.1:9001".to_string(), 100, tx1);
        manager.register_solver("solver-2".to_string(), "127.0.0.1:9002".to_string(), 100, tx2);
        manager.register_solver("solver-3".to_string(), "127.0.0.1:9003".to_string(), 100, tx3);
        
        let transfer = create_test_transfer("tx-1");
        
        let result1 = manager.route_transfer(&transfer).unwrap();
        let result2 = manager.route_transfer(&transfer).unwrap();
        
        assert_eq!(result1, result2, "Same transfer should route to same solver");
    }
    
    #[test]
    fn test_no_solver_available() {
        let manager = RouterManager::new();
        let transfer = create_test_transfer("tx-1");
        
        let result = manager.route_transfer(&transfer);
        assert!(matches!(result, Err(RouterError::NoSolverAvailable)));
    }
}

