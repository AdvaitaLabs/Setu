//! Setu Router - Transfer routing and load balancing
//!
//! The router is responsible for:
//! - Receiving Transfer intents from users/clients
//! - Quick validation checks
//! - Routing transfers to appropriate Solvers
//! - Load balancing across Solvers
//! - Managing pending queue

mod quick_check;
mod load_balancer;
mod pending_queue;
mod solver_registry;

pub use quick_check::{QuickChecker, QuickCheckError};
pub use load_balancer::{LoadBalancer, LoadBalancingStrategy};
pub use pending_queue::{PendingQueue, PendingTransfer};
pub use solver_registry::{SolverRegistry, SolverInfo, SolverStatus};

use core_types::Transfer;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};

/// Routing strategy for transfer routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Priority 1: Manual solver selection (preferred_solver)
    /// Priority 2: Shard-based routing (shard_id)
    /// Priority 3: Resource affinity routing (resources)
    /// Priority 4: Load balancing
    ManualFirst,
    
    /// Priority 1: Shard-based routing (shard_id)
    /// Priority 2: Resource affinity routing (resources)
    /// Priority 3: Load balancing
    ShardFirst,
    
    /// Priority 1: Resource affinity routing (resources)
    /// Priority 2: Load balancing
    ResourceAffinityFirst,
    
    /// Only use load balancing (ignore manual/shard/resource)
    LoadBalanceOnly,
}

impl Default for RoutingStrategy {
    fn default() -> Self {
        Self::ManualFirst
    }
}

/// Router configuration
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Router node ID
    pub node_id: String,
    
    /// Maximum pending queue size
    pub max_pending_queue_size: usize,
    
    /// Load balancing strategy
    pub load_balancing_strategy: LoadBalancingStrategy,
    
    /// Quick check timeout (ms)
    pub quick_check_timeout_ms: u64,
    
    /// Enable resource-based routing
    pub enable_resource_routing: bool,
    
    /// Routing strategy (priority order)
    pub routing_strategy: RoutingStrategy,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            max_pending_queue_size: 10000,
            load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
            quick_check_timeout_ms: 100,
            enable_resource_routing: true,
            routing_strategy: RoutingStrategy::ManualFirst,
        }
    }
}

/// Router statistics
#[derive(Debug, Clone, Default)]
pub struct RouterStats {
    /// Total transfers received
    pub total_received: u64,
    
    /// Total transfers routed
    pub total_routed: u64,
    
    /// Total transfers rejected
    pub total_rejected: u64,
    
    /// Current pending queue size
    pub pending_queue_size: usize,
    
    /// Active solvers count
    pub active_solvers: usize,
}

/// Main Router struct
pub struct Router {
    config: RouterConfig,
    
    /// Receives transfers from clients
    transfer_rx: mpsc::UnboundedReceiver<Transfer>,
    
    /// Sends transfers to solvers (multiple channels)
    solver_channels: Arc<parking_lot::RwLock<Vec<mpsc::UnboundedSender<Transfer>>>>,
    
    /// Quick checker for validation
    quick_checker: QuickChecker,
    
    /// Load balancer
    load_balancer: LoadBalancer,
    
    /// Pending queue
    pending_queue: PendingQueue,
    
    /// Solver registry
    solver_registry: Arc<SolverRegistry>,
    
    /// Statistics
    stats: Arc<parking_lot::RwLock<RouterStats>>,
}

impl Router {
    /// Create a new router
    pub fn new(
        config: RouterConfig,
        transfer_rx: mpsc::UnboundedReceiver<Transfer>,
    ) -> Self {
        info!(
            node_id = %config.node_id,
            "Creating router node"
        );
        
        let quick_checker = QuickChecker::new(config.quick_check_timeout_ms);
        let load_balancer = LoadBalancer::new(config.load_balancing_strategy);
        let pending_queue = PendingQueue::new(config.max_pending_queue_size);
        let solver_registry = Arc::new(SolverRegistry::new());
        
        Self {
            config,
            transfer_rx,
            solver_channels: Arc::new(parking_lot::RwLock::new(vec![])),
            quick_checker,
            load_balancer,
            pending_queue,
            solver_registry,
            stats: Arc::new(parking_lot::RwLock::new(RouterStats::default())),
        }
    }
    
    /// Register a solver with the router
    pub fn register_solver(
        &self,
        solver_id: String,
        solver_tx: mpsc::UnboundedSender<Transfer>,
        capacity: u32,
    ) {
        info!(
            solver_id = %solver_id,
            capacity = capacity,
            "Registering solver"
        );
        
        // Add to solver registry
        let solver_info = SolverInfo::new(solver_id.clone(), capacity);
        self.solver_registry.register(solver_info);
        
        // Add channel
        let mut channels = self.solver_channels.write();
        channels.push(solver_tx);
        
        // Update load balancer
        self.load_balancer.add_solver(solver_id);
        
        // Update stats
        self.update_active_solvers_count();
    }
    
    /// Register a solver with shard and resource affinity
    pub fn register_solver_with_affinity(
        &self,
        solver_id: String,
        solver_tx: mpsc::UnboundedSender<Transfer>,
        capacity: u32,
        shard_id: Option<String>,
        resources: Vec<String>,
    ) {
        info!(
            solver_id = %solver_id,
            capacity = capacity,
            shard_id = ?shard_id,
            resources = ?resources,
            "Registering solver with affinity"
        );
        
        // Create solver info with affinity
        let mut solver_info = SolverInfo::new(solver_id.clone(), capacity);
        solver_info.shard_id = shard_id;
        solver_info.resources = resources;
        
        self.solver_registry.register(solver_info);
        
        // Add channel
        let mut channels = self.solver_channels.write();
        channels.push(solver_tx);
        
        // Update load balancer
        self.load_balancer.add_solver(solver_id);
        
        // Update stats
        self.update_active_solvers_count();
    }
    
    /// Run the router
    pub async fn run(mut self) {
        info!(
            node_id = %self.config.node_id,
            "Router started, waiting for transfers..."
        );
        
        // Main loop: receive and route transfers
        while let Some(transfer) = self.transfer_rx.recv().await {
            self.update_stats_received();
            
            info!(
                transfer_id = %transfer.id,
                from = %transfer.from,
                to = %transfer.to,
                amount = %transfer.amount,
                "Received transfer intent"
            );
            
            // Process the transfer
            match self.process_transfer(transfer).await {
                Ok(()) => {
                    self.update_stats_routed();
                    debug!("Transfer routed successfully");
                }
                Err(e) => {
                    self.update_stats_rejected();
                    warn!(
                        error = %e,
                        "Transfer rejected"
                    );
                }
            }
        }
        
        info!("Router stopped");
    }
    
    /// Process a single transfer
    async fn process_transfer(&mut self, transfer: Transfer) -> anyhow::Result<()> {
        // Step 1: Quick check
        debug!(
            transfer_id = %transfer.id,
            "Performing quick check"
        );
        
        self.quick_checker.check(&transfer).await?;
        
        debug!(
            transfer_id = %transfer.id,
            "Quick check passed"
        );
        
        // Step 2: Add to pending queue
        self.pending_queue.enqueue(transfer.clone())?;
        
        debug!(
            transfer_id = %transfer.id,
            queue_size = self.pending_queue.size(),
            "Added to pending queue"
        );
        
        // Step 3: Route to solver using layered strategy
        let solver_id = self.route_transfer(&transfer)?;
        
        debug!(
            transfer_id = %transfer.id,
            solver_id = %solver_id,
            "Selected solver"
        );
        
        // Step 4: Send to solver
        self.send_to_solver(&solver_id, transfer.clone()).await?;
        
        // Step 5: Remove from pending queue
        self.pending_queue.dequeue(&transfer.id)?;
        
        info!(
            transfer_id = %transfer.id,
            solver_id = %solver_id,
            "Transfer routed successfully"
        );
        
        Ok(())
    }
    
    /// Route transfer using layered strategy
    fn route_transfer(&self, transfer: &Transfer) -> anyhow::Result<String> {
        match self.config.routing_strategy {
            RoutingStrategy::ManualFirst => {
                // Priority 1: Manual solver selection
                if let Some(solver_id) = &transfer.preferred_solver {
                    // Check if solver is available
                    if let Some(solver_info) = self.solver_registry.get(solver_id) {
                        if solver_info.is_available() {
                            debug!(
                                transfer_id = %transfer.id,
                                solver_id = %solver_id,
                                "Using manually specified solver"
                            );
                            return Ok(solver_id.clone());
                        }
                    }
                    warn!(
                        transfer_id = %transfer.id,
                        solver_id = %solver_id,
                        "Preferred solver not available, falling back"
                    );
                }
                
                // Priority 2: Shard-based routing
                if let Some(shard_id) = &transfer.shard_id {
                    if let Ok(solver_id) = self.route_by_shard(shard_id) {
                        debug!(
                            transfer_id = %transfer.id,
                            shard_id = %shard_id,
                            solver_id = %solver_id,
                            "Using shard-based routing"
                        );
                        return Ok(solver_id);
                    }
                }
                
                // Priority 3: Resource affinity routing
                if self.config.enable_resource_routing {
                    if let Ok(solver_id) = self.route_by_resource(transfer) {
                        debug!(
                            transfer_id = %transfer.id,
                            solver_id = %solver_id,
                            "Using resource affinity routing"
                        );
                        return Ok(solver_id);
                    }
                }
                
                // Priority 4: Load balancing
                debug!(
                    transfer_id = %transfer.id,
                    "Using load balancing"
                );
                Ok(self.load_balancer.select_solver()?)
            }
            
            RoutingStrategy::ShardFirst => {
                // Priority 1: Shard-based routing
                if let Some(shard_id) = &transfer.shard_id {
                    if let Ok(solver_id) = self.route_by_shard(shard_id) {
                        return Ok(solver_id);
                    }
                }
                
                // Priority 2: Resource affinity routing
                if self.config.enable_resource_routing {
                    if let Ok(solver_id) = self.route_by_resource(transfer) {
                        return Ok(solver_id);
                    }
                }
                
                // Priority 3: Load balancing
                Ok(self.load_balancer.select_solver()?)
            }
            
            RoutingStrategy::ResourceAffinityFirst => {
                // Priority 1: Resource affinity routing
                if self.config.enable_resource_routing {
                    if let Ok(solver_id) = self.route_by_resource(transfer) {
                        return Ok(solver_id);
                    }
                }
                
                // Priority 2: Load balancing
                Ok(self.load_balancer.select_solver()?)
            }
            
            RoutingStrategy::LoadBalanceOnly => {
                // Only use load balancing
                Ok(self.load_balancer.select_solver()?)
            }
        }
    }
    
    /// Route by shard ID
    fn route_by_shard(&self, shard_id: &str) -> anyhow::Result<String> {
        let solvers = self.solver_registry.find_by_shard(shard_id);
        
        if solvers.is_empty() {
            return Err(anyhow::anyhow!("No solvers available for shard: {}", shard_id));
        }
        
        // Select least loaded solver in the shard
        let solver_id = solvers
            .iter()
            .filter_map(|id| self.solver_registry.get(id))
            .filter(|info| info.is_available())
            .min_by_key(|info| info.current_load)
            .map(|info| info.id.clone())
            .ok_or_else(|| anyhow::anyhow!("No available solvers in shard: {}", shard_id))?;
        
        Ok(solver_id)
    }
    
    /// Route transfer based on resource affinity
    fn route_by_resource(&self, transfer: &Transfer) -> anyhow::Result<String> {
        // Use the first resource as routing key
        if let Some(resource) = transfer.resources.first() {
            // Find solver that handles this resource
            if let Some(solver_id) = self.solver_registry.find_by_resource(resource) {
                return Ok(solver_id);
            }
        }
        
        // Fallback to load balancing
        Ok(self.load_balancer.select_solver()?)
    }
    
    /// Send transfer to a specific solver
    async fn send_to_solver(&self, solver_id: &str, transfer: Transfer) -> anyhow::Result<()> {
        // Get solver info
        let solver_info = self.solver_registry.get(solver_id)
            .ok_or_else(|| anyhow::anyhow!("Solver not found: {}", solver_id))?;
        
        // Check if solver is active
        if solver_info.status != SolverStatus::Active {
            return Err(anyhow::anyhow!("Solver is not active: {}", solver_id));
        }
        
        // Get solver channel
        let channels = self.solver_channels.read();
        let solver_index = self.solver_registry.get_index(solver_id)
            .ok_or_else(|| anyhow::anyhow!("Solver index not found"))?;
        
        if solver_index >= channels.len() {
            return Err(anyhow::anyhow!("Invalid solver index"));
        }
        
        let solver_tx = &channels[solver_index];
        
        // Send transfer
        solver_tx.send(transfer)
            .map_err(|e| anyhow::anyhow!("Failed to send to solver: {}", e))?;
        
        // Update solver load
        self.solver_registry.increment_load(solver_id);
        
        Ok(())
    }
    
    /// Update statistics - received
    fn update_stats_received(&self) {
        let mut stats = self.stats.write();
        stats.total_received += 1;
        stats.pending_queue_size = self.pending_queue.size();
        self.update_active_solvers_count();
    }
    
    /// Update active solvers count in stats
    fn update_active_solvers_count(&self) {
        let mut stats = self.stats.write();
        stats.active_solvers = self.solver_registry.active_count();
    }
    
    /// Update statistics - routed
    fn update_stats_routed(&self) {
        let mut stats = self.stats.write();
        stats.total_routed += 1;
    }
    
    /// Update statistics - rejected
    fn update_stats_rejected(&self) {
        let mut stats = self.stats.write();
        stats.total_rejected += 1;
    }
    
    /// Get router statistics
    pub fn stats(&self) -> RouterStats {
        self.stats.read().clone()
    }
    
    /// Get node ID
    pub fn node_id(&self) -> &str {
        &self.config.node_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{TransferType, Vlc};
    
    fn create_test_transfer(id: &str) -> Transfer {
        Transfer {
            id: id.to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100,
            transfer_type: TransferType::FluxTransfer,
            resources: vec!["alice".to_string()],
            vlc: Vlc::new(),
            power: 10,
        }
    }
    
    #[tokio::test]
    async fn test_router_creation() {
        let config = RouterConfig::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let router = Router::new(config, rx);
        
        assert!(!router.node_id().is_empty());
    }
    
    #[tokio::test]
    async fn test_register_solver() {
        let config = RouterConfig::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let router = Router::new(config, rx);
        
        let (solver_tx, _solver_rx) = mpsc::unbounded_channel();
        router.register_solver("solver1".to_string(), solver_tx, 100);
        
        let stats = router.stats();
        assert_eq!(stats.active_solvers, 1);
    }
}

