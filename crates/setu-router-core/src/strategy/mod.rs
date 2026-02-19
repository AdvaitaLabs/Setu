//! Routing Strategies
//!
//! This module contains various routing strategies:
//!
//! - `ConsistentHashStrategy`: Deterministic routing based on resource keys
//! - `LoadBalancedStrategy`: Routes to least loaded solver
//! - `SubnetShardStrategy`: Routes subnets to shards
//! - `ObjectShardStrategy`: Routes objects to shards
//!
//! # Strategy Hierarchy
//!
//! ```text
//! Transaction
//!     │
//!     ▼
//! ┌─────────────────────────┐
//! │   Shard Selection       │  (SubnetShardStrategy / ObjectShardStrategy)
//! │   (Which shard?)        │
//! └───────────┬─────────────┘
//!             │
//!             ▼
//! ┌─────────────────────────┐
//! │   Solver Selection      │  (ConsistentHashStrategy / LoadBalancedStrategy)
//! │   (Which solver?)       │
//! └─────────────────────────┘
//! ```

mod consistent_hash;
mod load_balanced;
mod object_shard;
mod subnet_shard;

pub use consistent_hash::ConsistentHashStrategy;
pub use load_balanced::LoadBalancedStrategy;
pub use object_shard::ObjectShardStrategy;
pub use subnet_shard::{
    CrossSubnetRoutingDecision, ShardLoadMetrics, SubnetShardRouter, SubnetShardStrategy,
};

use crate::error::RouterError;
use crate::solver::SolverInfo;

/// Trait for solver selection strategies
pub trait SolverStrategy: Send + Sync {
    /// Select a solver from available solvers based on routing key
    fn select(
        &self,
        available: &[SolverInfo],
        routing_key: &str,
    ) -> Result<SolverInfo, RouterError>;

    /// Strategy name for logging
    fn name(&self) -> &'static str;
}

/// Trait for shard selection strategies
pub trait ShardStrategy: Send + Sync {
    /// Route a key to a shard
    fn route(&self, key: &[u8; 32]) -> crate::types::ShardId;

    /// Strategy name for logging
    fn name(&self) -> &'static str;
}
