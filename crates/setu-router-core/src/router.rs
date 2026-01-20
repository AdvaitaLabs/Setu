//! Main router module - Routes transactions to solvers
//!
//! MVP: Single-level routing to 6 solvers using consistent hash.

use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use setu_types::Transfer;

use crate::error::RouterError;
use crate::types::DEFAULT_SHARD_ID;
use crate::solver::{SolverId, SolverInfo, SolverRegistry};
use crate::strategy::{ConsistentHashStrategy, LoadBalancedStrategy, SolverStrategy};

/// Router configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Number of virtual nodes for consistent hashing
    pub virtual_nodes: u32,
    /// Enable load-aware routing (switch to load balanced when overloaded)
    pub load_aware: bool,
    /// Load threshold to trigger load-balanced routing
    pub load_threshold: f64,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            virtual_nodes: 150,
            load_aware: true,
            load_threshold: 0.8,
        }
    }
}

/// Routing decision containing target solver information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Target solver ID
    pub solver_id: SolverId,
    /// Solver network address
    pub solver_address: String,
    /// Shard ID (always DEFAULT_SHARD_ID in MVP)
    pub shard_id: String,
    /// Routing key used for this decision
    pub routing_key: String,
    /// Strategy used for this decision
    pub strategy_name: String,
}

/// Transaction router
///
/// Routes transactions to appropriate solvers based on routing strategy.
/// MVP: Uses consistent hash for deterministic routing.
pub struct Router {
    config: RouterConfig,
    solvers: Arc<SolverRegistry>,
    consistent_hash: ConsistentHashStrategy,
    load_balanced: LoadBalancedStrategy,
}

impl Router {
    /// Create a new router with default configuration
    pub fn new(solvers: Arc<SolverRegistry>) -> Self {
        Self::with_config(RouterConfig::default(), solvers)
    }

    /// Create a router with custom configuration
    pub fn with_config(config: RouterConfig, solvers: Arc<SolverRegistry>) -> Self {
        let consistent_hash = ConsistentHashStrategy::with_virtual_nodes(config.virtual_nodes);
        let load_balanced = LoadBalancedStrategy::new();

        info!(
            virtual_nodes = config.virtual_nodes,
            load_aware = config.load_aware,
            "Router initialized"
        );

        Self {
            config,
            solvers,
            consistent_hash,
            load_balanced,
        }
    }

    /// Create MVP router with 6 solvers
    pub fn new_mvp() -> Self {
        let solvers = Arc::new(SolverRegistry::new());

        // Register 6 solvers for MVP
        for i in 1..=6 {
            let solver = SolverInfo::new(
                format!("solver-{}", i),
                format!("127.0.0.1:{}", 9000 + i),
            );
            solvers.register(solver);
        }

        info!("MVP router created with 6 solvers");
        Self::new(solvers)
    }

    /// Route a transfer to a solver
    pub fn route(&self, transfer: &Transfer) -> Result<RoutingDecision, RouterError> {
        let routing_key = Self::get_routing_key(transfer);
        self.route_by_key(&routing_key)
    }

    /// Route by a specific key (useful for testing or custom routing)
    pub fn route_by_key(&self, routing_key: &str) -> Result<RoutingDecision, RouterError> {
        let available = self.solvers.get_available();

        if available.is_empty() {
            return Err(RouterError::NoSolverAvailable);
        }

        // Select strategy based on load
        let (solver, strategy_name) = if self.should_use_load_balanced(&available) {
            let s = self.load_balanced.select(&available, routing_key)?;
            (s, "LoadBalanced")
        } else {
            let s = self.consistent_hash.select(&available, routing_key)?;
            (s, "ConsistentHash")
        };

        debug!(
            solver_id = %solver.id,
            routing_key = %routing_key,
            strategy = %strategy_name,
            "Transaction routed"
        );

        Ok(RoutingDecision {
            solver_id: solver.id.clone(),
            solver_address: solver.address.clone(),
            shard_id: DEFAULT_SHARD_ID.to_string(),
            routing_key: routing_key.to_string(),
            strategy_name: strategy_name.to_string(),
        })
    }

    /// Route multiple transfers (batch routing)
    pub fn route_batch(&self, transfers: &[Transfer]) -> Vec<Result<RoutingDecision, RouterError>> {
        transfers.iter().map(|t| self.route(t)).collect()
    }

    /// Get routing key from a transfer
    /// Priority: first write resource > first read resource > transfer id
    fn get_routing_key(transfer: &Transfer) -> String {
        transfer.resources.first()
            .cloned()
            .unwrap_or_else(|| transfer.id.clone())
    }

    /// Check if we should use load-balanced routing
    fn should_use_load_balanced(&self, available: &[SolverInfo]) -> bool {
        if !self.config.load_aware {
            return false;
        }

        if available.is_empty() {
            return false;
        }

        // Calculate average load
        let avg_load: f64 = available.iter()
            .map(|s| s.load_ratio())
            .sum::<f64>() / available.len() as f64;

        avg_load > self.config.load_threshold
    }

    /// Get solver registry reference
    pub fn solvers(&self) -> &SolverRegistry {
        &self.solvers
    }

    /// Get shard ID (always default in MVP)
    pub fn shard_id(&self) -> &str {
        DEFAULT_SHARD_ID
    }

    /// Get router configuration
    pub fn config(&self) -> &RouterConfig {
        &self.config
    }
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("config", &self.config)
            .field("solver_count", &self.solvers.count())
            .field("available_count", &self.solvers.available_count())
            .finish()
    }
}
