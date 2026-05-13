//! Router Manager - Manages routing of transfers to solvers
//!
//! This module integrates setu-router-core into the Validator,
//! providing transfer routing and solver management capabilities.
//!
//! ## Subnet Affinity Routing
//!
//! Solvers can be configured with `permitted_subnets`:
//! - Empty = universal solver, serves any subnet
//! - Non-empty = dedicated solver, only serves specified subnets
//!
//! Routing respects subnet affinity: transactions for a subnet
//! are only routed to solvers that permit that subnet.

use setu_types::{Transfer, SubnetId};
use parking_lot::RwLock;
use setu_router_core::{
    UnifiedRouter,
    SolverInfo, SolverStatus,
    ConsistentHashStrategy,
    SolverStrategy,  // Import the trait
    RoutingContext,  // For unified routing
    ObjectId,        // For RoutingContext
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
    /// Assigned shard ID (numeric) for shard-based routing
    /// When set, this solver handles transactions routed to this shard
    pub assigned_shard: Option<u16>,
    pub resources: Vec<String>,
    /// Subnets this solver is permitted to serve.
    /// Empty = universal solver (can serve any subnet).
    pub permitted_subnets: Vec<SubnetId>,
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
            assigned_shard: None,
            resources: vec![],
            permitted_subnets: vec![],
        }
    }
    
    pub fn with_shard(mut self, shard_id: String) -> Self {
        self.shard_id = Some(shard_id);
        self
    }
    
    pub fn with_assigned_shard(mut self, shard: u16) -> Self {
        self.assigned_shard = Some(shard);
        self
    }
    
    pub fn with_resources(mut self, resources: Vec<String>) -> Self {
        self.resources = resources;
        self
    }
    
    pub fn with_permitted_subnets(mut self, subnets: Vec<SubnetId>) -> Self {
        self.permitted_subnets = subnets;
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
    /// Unified router from setu-router-core (routes subnets to shards)
    router: UnifiedRouter,
    
    /// Solver registry
    solver_registry: Arc<RwLock<HashMap<String, SolverConnection>>>,
    
    /// Solver channels for sending transfers
    solver_channels: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<Transfer>>>>,
    
    /// Consistent hash strategy for solver selection within a shard
    consistent_hash: ConsistentHashStrategy,
    
    /// Shard → Solver mapping index (for shard-based routing)
    /// G10: This index is rebuilt from solver_registry on startup/replay
    shard_solvers: Arc<RwLock<HashMap<u16, Vec<String>>>>,
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
            shard_solvers: Arc::new(RwLock::new(HashMap::new())),
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
            shard_solvers: Arc::new(RwLock::new(HashMap::new())),
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
        assigned_shard: Option<u16>,
        resources: Vec<String>,
        permitted_subnets: Vec<SubnetId>,
    ) {
        info!(
            solver_id = %solver_id,
            address = %address,
            capacity = capacity,
            shard_id = ?shard_id,
            assigned_shard = ?assigned_shard,
            resources = ?resources,
            permitted_subnets_count = permitted_subnets.len(),
            "Registering solver with affinity"
        );
        
        let mut connection = SolverConnection::new(solver_id.clone(), address, capacity);
        if let Some(shard) = shard_id {
            connection = connection.with_shard(shard);
        }
        if let Some(shard) = assigned_shard {
            connection = connection.with_assigned_shard(shard);
            // Update shard_solvers index
            self.shard_solvers.write()
                .entry(shard)
                .or_insert_with(Vec::new)
                .push(solver_id.clone());
        }
        if !resources.is_empty() {
            connection = connection.with_resources(resources);
        }
        if !permitted_subnets.is_empty() {
            connection = connection.with_permitted_subnets(permitted_subnets);
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
        
        // Get assigned_shard before removing from registry
        let assigned_shard = self.solver_registry.read()
            .get(solver_id)
            .and_then(|s| s.assigned_shard);
        
        self.solver_registry.write().remove(solver_id);
        self.solver_channels.write().remove(solver_id);
        
        // Remove from shard_solvers index if applicable
        if let Some(shard) = assigned_shard {
            if let Some(solvers) = self.shard_solvers.write().get_mut(&shard) {
                solvers.retain(|id| id != solver_id);
            }
        }
        
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
    /// 
    /// 方案 B: Two-level subnet affinity routing:
    /// 1. Use UnifiedRouter to map subnet_id → ShardId (same subnet always → same shard)
    /// 2. Select solver from shard_solvers[shard] using consistent hash
    /// 3. Fallback to all permitted solvers if no shard assignment
    /// 
    /// Also respects `permitted_subnets` filtering: solvers with non-empty
    /// permitted_subnets only serve listed subnets.
    pub fn route_transfer(&self, transfer: &Transfer) -> Result<String, RouterError> {
        let subnet_id = transfer.get_subnet_id();
        
        // Priority 1: Manual solver selection (preferred_solver)
        // Check if preferred solver is available AND permits this subnet
        if let Some(preferred) = &transfer.preferred_solver {
            if self.is_solver_available_for_subnet(preferred, &subnet_id) {
                debug!(
                    transfer_id = %transfer.id,
                    solver_id = %preferred,
                    "Using preferred solver (subnet-checked)"
                );
                return Ok(preferred.clone());
            }
            warn!(
                transfer_id = %transfer.id,
                solver_id = %preferred,
                subnet_id = ?subnet_id,
                "Preferred solver not available or not permitted for subnet, falling back"
            );
        }
        
        // Priority 2: Shard-based routing via UnifiedRouter
        // Route subnet_id → ShardId, then select solver from that shard
        let routing_ctx = RoutingContext::with_subnet(
            subnet_id.to_bytes(),
            ObjectId::default(), // Not used for SubnetFirst strategy
        );
        let shard_result = self.router.route(&routing_ctx);
        let target_shard = shard_result.primary_shard;
        
        // Try to find solvers assigned to this shard
        if let Some(shard_solver_ids) = self.shard_solvers.read().get(&target_shard).cloned() {
            let candidates: Vec<SolverConnection> = shard_solver_ids.iter()
                .filter_map(|id| self.solver_registry.read().get(id).cloned())
                .filter(|s| s.is_available())
                .filter(|s| self.solver_permits_subnet_inner(s, &subnet_id))
                .collect();
            
            if !candidates.is_empty() {
                // Use consistent hash among shard solvers with subnet_id as routing key
                let routing_key = hex::encode(subnet_id.as_bytes());
                let solver_infos: Vec<SolverInfo> = candidates.iter()
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
                    subnet_id = ?subnet_id,
                    target_shard = target_shard,
                    solver_id = %selected.id,
                    "Using shard-based routing (subnet affinity)"
                );
                return Ok(selected.id);
            }
        }
        
        // Priority 3: Fallback - consistent hash among all permitted solvers
        // This handles the case where no solvers are assigned to shards yet
        let available_solvers = self.get_solvers_for_subnet(&subnet_id);
        
        if available_solvers.is_empty() {
            warn!(
                transfer_id = %transfer.id,
                subnet_id = ?subnet_id,
                "No solver available for subnet"
            );
            return Err(RouterError::NoSolverAvailable);
        }
        
        // Use subnet_id as routing key for consistent hash (ensures same subnet → same solver)
        let routing_key = hex::encode(subnet_id.as_bytes());
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
            subnet_id = ?subnet_id,
            "Using consistent hash routing (fallback, no shard assignment)"
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
    
    /// Check if solver is available
    #[allow(dead_code)] // Used in tests
    fn is_solver_available(&self, solver_id: &str) -> bool {
        self.solver_registry.read()
            .get(solver_id)
            .map(|s| s.is_available())
            .unwrap_or(false)
    }
    
    /// Check if solver is available AND permits the given subnet
    fn is_solver_available_for_subnet(&self, solver_id: &str, subnet_id: &SubnetId) -> bool {
        self.solver_registry.read()
            .get(solver_id)
            .map(|s| s.is_available() && self.solver_permits_subnet_inner(s, subnet_id))
            .unwrap_or(false)
    }
    
    /// Get available solvers
    #[allow(dead_code)] // Potential future use
    fn get_available_solvers(&self) -> Vec<SolverConnection> {
        self.solver_registry.read()
            .values()
            .filter(|s| s.is_available())
            .cloned()
            .collect()
    }
    
    /// Get available solvers that permit the given subnet
    fn get_solvers_for_subnet(&self, subnet_id: &SubnetId) -> Vec<SolverConnection> {
        self.solver_registry.read()
            .values()
            .filter(|s| s.is_available())
            .filter(|s| self.solver_permits_subnet_inner(s, subnet_id))
            .cloned()
            .collect()
    }
    
    /// Check if a solver permits servicing a specific subnet (internal helper)
    /// 
    /// Rules:
    /// - Empty permitted_subnets = universal solver, permits any subnet
    /// - Non-empty permitted_subnets = only permits listed subnets
    fn solver_permits_subnet_inner(&self, solver: &SolverConnection, subnet_id: &SubnetId) -> bool {
        if solver.permitted_subnets.is_empty() {
            // Universal solver: permits any subnet
            true
        } else {
            // Dedicated solver: check if subnet is in permitted list
            solver.permitted_subnets.contains(subnet_id)
        }
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

    /// Route a generic task (non-transfer) to any available solver
    pub fn route_any(&self) -> Result<String, RouterError> {
        let solvers = self.solver_registry.read();
        solvers
            .values()
            .find(|s| s.is_available())
            .map(|s| s.id.clone())
            .ok_or(RouterError::NoSolverAvailable)
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
            .with_type(TransferType::SetuTransfer)
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
    
    // ============================================
    // Subnet Affinity Routing Tests
    // ============================================
    
    fn create_test_subnet(id: u8) -> SubnetId {
        let mut bytes = [0u8; 32];
        bytes[31] = id;
        SubnetId::new(bytes)
    }
    
    fn create_transfer_for_subnet(id: &str, subnet: &SubnetId) -> Transfer {
        Transfer::new(id, "alice", "bob", 100)
            .with_type(TransferType::SetuTransfer)
            .with_resources(vec!["alice".to_string()])
            .with_power(10)
            .with_subnet(hex::encode(subnet.as_bytes()))
    }
    
    #[test]
    fn test_solver_permits_subnet_universal() {
        // Empty permitted_subnets = universal solver
        let manager = RouterManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        
        // Register universal solver (empty permitted_subnets)
        manager.register_solver_with_affinity(
            "solver-universal".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx,
            None,
            None, // assigned_shard
            vec![],
            vec![],  // empty = universal
        );
        
        let subnet_a = create_test_subnet(1);
        let subnet_b = create_test_subnet(2);
        
        // Universal solver should permit any subnet
        assert!(manager.is_solver_available_for_subnet("solver-universal", &subnet_a));
        assert!(manager.is_solver_available_for_subnet("solver-universal", &subnet_b));
        assert!(manager.is_solver_available_for_subnet("solver-universal", &SubnetId::ROOT));
    }
    
    #[test]
    fn test_solver_permits_subnet_dedicated() {
        // Non-empty permitted_subnets = dedicated solver
        let manager = RouterManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        
        let subnet_a = create_test_subnet(1);
        let subnet_b = create_test_subnet(2);
        
        // Register dedicated solver for subnet_a only
        manager.register_solver_with_affinity(
            "solver-dedicated".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx,
            None,
            None, // assigned_shard
            vec![],
            vec![subnet_a.clone()],  // only subnet_a
        );
        
        // Dedicated solver should only permit subnet_a
        assert!(manager.is_solver_available_for_subnet("solver-dedicated", &subnet_a));
        assert!(!manager.is_solver_available_for_subnet("solver-dedicated", &subnet_b));
        assert!(!manager.is_solver_available_for_subnet("solver-dedicated", &SubnetId::ROOT));
    }
    
    #[test]
    fn test_route_transfer_respects_permitted_subnets() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let (tx3, _rx3) = mpsc::unbounded_channel();
        
        let subnet_a = create_test_subnet(1);
        let subnet_b = create_test_subnet(2);
        
        // solver-1: universal
        manager.register_solver_with_affinity(
            "solver-1".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx1,
            None,
            None, // assigned_shard
            vec![],
            vec![],
        );
        
        // solver-2: dedicated to subnet_a
        manager.register_solver_with_affinity(
            "solver-2".to_string(),
            "127.0.0.1:9002".to_string(),
            100,
            tx2,
            None,
            None, // assigned_shard
            vec![],
            vec![subnet_a.clone()],
        );
        
        // solver-3: dedicated to subnet_b
        manager.register_solver_with_affinity(
            "solver-3".to_string(),
            "127.0.0.1:9003".to_string(),
            100,
            tx3,
            None,
            None, // assigned_shard
            vec![],
            vec![subnet_b.clone()],
        );
        
        // Transfer for subnet_a should route to solver-1 or solver-2 (not solver-3)
        let transfer_a = create_transfer_for_subnet("tx-a", &subnet_a);
        let result_a = manager.route_transfer(&transfer_a).unwrap();
        assert!(result_a == "solver-1" || result_a == "solver-2", 
            "subnet_a transfer should route to solver-1 or solver-2, got {}", result_a);
        
        // Transfer for subnet_b should route to solver-1 or solver-3 (not solver-2)
        let transfer_b = create_transfer_for_subnet("tx-b", &subnet_b);
        let result_b = manager.route_transfer(&transfer_b).unwrap();
        assert!(result_b == "solver-1" || result_b == "solver-3",
            "subnet_b transfer should route to solver-1 or solver-3, got {}", result_b);
    }
    
    #[test]
    fn test_route_transfer_no_solver_for_subnet() {
        let manager = RouterManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        
        let subnet_a = create_test_subnet(1);
        let subnet_b = create_test_subnet(2);
        
        // Only register a solver for subnet_a
        manager.register_solver_with_affinity(
            "solver-a".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx,
            None,
            None, // assigned_shard
            vec![],
            vec![subnet_a.clone()],
        );
        
        // Transfer for subnet_b should fail (no solver permits it)
        let transfer_b = create_transfer_for_subnet("tx-b", &subnet_b);
        let result = manager.route_transfer(&transfer_b);
        assert!(matches!(result, Err(RouterError::NoSolverAvailable)));
    }
    
    #[test]
    fn test_consistent_hash_within_permitted_solvers() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        
        let subnet_a = create_test_subnet(1);
        
        // Register two universal solvers
        manager.register_solver_with_affinity(
            "solver-1".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx1,
            None,
            None, // assigned_shard
            vec![],
            vec![],
        );
        manager.register_solver_with_affinity(
            "solver-2".to_string(),
            "127.0.0.1:9002".to_string(),
            100,
            tx2,
            None,
            None, // assigned_shard
            vec![],
            vec![],
        );
        
        // Same subnet + same sender should deterministically route to same solver
        let transfer1 = create_transfer_for_subnet("tx-1", &subnet_a);
        let transfer2 = create_transfer_for_subnet("tx-2", &subnet_a);  // different id, same sender
        
        let result1 = manager.route_transfer(&transfer1).unwrap();
        let result2 = manager.route_transfer(&transfer2).unwrap();
        
        // Both have same routing key ("alice"), so should go to same solver
        assert_eq!(result1, result2, "Same routing key should route to same solver");
    }
    
    #[test]
    fn test_preferred_solver_checked_for_subnet() {
        let manager = RouterManager::new();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        
        let subnet_a = create_test_subnet(1);
        let subnet_b = create_test_subnet(2);
        
        // solver-1: universal
        manager.register_solver_with_affinity(
            "solver-1".to_string(),
            "127.0.0.1:9001".to_string(),
            100,
            tx1,
            None,
            None, // assigned_shard
            vec![],
            vec![],
        );
        
        // solver-2: dedicated to subnet_b only
        manager.register_solver_with_affinity(
            "solver-2".to_string(),
            "127.0.0.1:9002".to_string(),
            100,
            tx2,
            None,
            None, // assigned_shard
            vec![],
            vec![subnet_b.clone()],
        );
        
        // Try to use solver-2 for subnet_a (should fall back to solver-1)
        let mut transfer = create_transfer_for_subnet("tx-a", &subnet_a);
        transfer.preferred_solver = Some("solver-2".to_string());
        
        let result = manager.route_transfer(&transfer).unwrap();
        assert_eq!(result, "solver-1", "Should fall back to solver-1 when solver-2 doesn't permit subnet_a");
    }
}
