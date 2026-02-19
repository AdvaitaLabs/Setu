//! Setu Router - Transaction Routing Module
//!
//! Routes transactions to appropriate solvers for execution.
//!
//! # Architecture
//!
//! ```text
//! Transaction
//!     │
//!     ▼
//! ┌─────────────────────────┐
//! │     UnifiedRouter       │  Decides: subnet vs object routing
//! │   (Which shard?)        │
//! └───────────┬─────────────┘
//!             │
//!             ▼
//! ┌─────────────────────────┐
//! │        Router           │  Selects solver within shard
//! │   (Which solver?)       │
//! └─────────────────────────┘
//! ```
//!
//! # Routing Strategies
//!
//! ## Shard Selection (strategy module)
//! - **SubnetShardStrategy**: Routes by subnet ID (same subnet → same shard)
//! - **ObjectShardStrategy**: Routes by object ID (same object → same shard)
//!
//! ## Solver Selection (strategy module)
//! - **ConsistentHashStrategy**: Deterministic routing based on keys
//! - **LoadBalancedStrategy**: Routes to least loaded solver
//!
//! # Example
//!
//! ```rust,ignore
//! use setu_router::{UnifiedRouter, RoutingContext};
//!
//! let router = UnifiedRouter::new();
//!
//! // With subnet - routes by subnet
//! let ctx = RoutingContext::with_subnet(subnet_id, object_id);
//! let result = router.route(&ctx);
//!
//! // Without subnet - routes by object
//! let ctx = RoutingContext::with_object(object_id);
//! let result = router.route(&ctx);
//! ```

// Core modules
mod error;
mod shard;
mod solver;
mod types;

// Strategy module (contains all routing strategies)
mod strategy;

// Routers
mod router;
mod unified_router;

#[cfg(test)]
mod tests;

// Re-exports: Error types
pub use error::RouterError;

// Re-exports: Core types
pub use types::{
    LegacyShardId, ObjectId, RoutingMethod, ShardId, SubnetId, DEFAULT_SHARD_COUNT,
    DEFAULT_SHARD_ID, ROOT_SUBNET,
};

// Re-exports: Shard management
pub use shard::{ShardConfig, ShardRouter, SingleShardRouter};

// Re-exports: Solver management
pub use solver::{SolverId, SolverInfo, SolverRegistry, SolverStatus};

// Re-exports: Strategy traits and implementations
pub use strategy::{
    // Solver selection strategies
    ConsistentHashStrategy,
    CrossSubnetRoutingDecision,
    LoadBalancedStrategy,
    ObjectShardStrategy,
    ShardLoadMetrics,
    ShardStrategy,
    // Traits
    SolverStrategy,
    SubnetShardRouter,
    // Shard selection strategies
    SubnetShardStrategy,
};

// Re-exports: Routers
pub use router::{Router, RouterConfig, RoutingDecision};
pub use unified_router::{
    DetailedRoutingResult, RoutingContext, ShardRoutingResult, UnifiedRouter,
    UnifiedRoutingStrategy,
};
