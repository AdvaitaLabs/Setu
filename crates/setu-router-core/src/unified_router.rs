//! Unified Router
//!
//! Provides a unified routing approach that handles both:
//! 1. Subnet-based routing: transactions within a subnet go to the same shard
//! 2. Object-based routing: transactions without subnet go by primary object
//!
//! # Routing Decision Tree
//!
//! ```text
//! Transaction arrives
//!        │
//!        ▼
//! Has SubnetId? ──Yes──► Route by Subnet
//!        │                    │
//!        No                   ▼
//!        │              SubnetId → ShardId
//!        ▼                    │
//! Route by Object             │
//! (same object → same shard)  │
//!        │                    │
//!        ▼                    ▼
//!    ObjectId → ShardId ──────┴──► ShardId → Solver
//! ```
//!
//! # Design Rationale
//!
//! - **With Subnet**: All transactions in a subnet share state, route together
//! - **Without Subnet**: Object is the conflict domain, same object = same shard
//! - This ensures state locality and minimizes cross-shard coordination

use std::collections::HashMap;

use crate::strategy::{ObjectShardStrategy, SubnetShardRouter, SubnetShardStrategy};
use crate::types::{ObjectId, RoutingMethod, ShardId, SubnetId, DEFAULT_SHARD_COUNT, ROOT_SUBNET};

/// Routing context for a transaction
#[derive(Debug, Clone)]
pub struct RoutingContext {
    /// Subnet ID (None = no subnet, use object-based routing)
    pub subnet_id: Option<SubnetId>,

    /// Primary object being accessed (for object-based routing)
    /// Usually the sender's account or the main object being modified
    pub primary_object: ObjectId,

    /// All objects involved in the transaction
    pub touched_objects: Vec<ObjectId>,

    /// Sender address (for fallback routing)
    pub sender: Option<ObjectId>,
}

impl RoutingContext {
    /// Create context for a subnet transaction
    pub fn with_subnet(subnet_id: SubnetId, primary_object: ObjectId) -> Self {
        Self {
            subnet_id: Some(subnet_id),
            primary_object,
            touched_objects: vec![primary_object],
            sender: None,
        }
    }

    /// Create context for an object-only transaction (no subnet)
    pub fn with_object(primary_object: ObjectId) -> Self {
        Self {
            subnet_id: None,
            primary_object,
            touched_objects: vec![primary_object],
            sender: None,
        }
    }

    /// Add touched objects
    pub fn with_touched_objects(mut self, objects: Vec<ObjectId>) -> Self {
        self.touched_objects = objects;
        self
    }

    /// Add sender
    pub fn with_sender(mut self, sender: ObjectId) -> Self {
        self.sender = Some(sender);
        self
    }
}

/// Unified routing strategy configuration
#[derive(Debug, Clone)]
pub enum UnifiedRoutingStrategy {
    /// Route by subnet when available, fallback to object
    SubnetFirst {
        subnet_strategy: SubnetShardStrategy,
        object_shard_count: u16,
    },

    /// Always route by object (ignore subnet)
    ObjectOnly { shard_count: u16 },

    /// Always route by subnet (treat no-subnet as ROOT)
    SubnetOnly {
        subnet_strategy: SubnetShardStrategy,
    },
}

impl Default for UnifiedRoutingStrategy {
    fn default() -> Self {
        Self::SubnetFirst {
            subnet_strategy: SubnetShardStrategy::default(),
            object_shard_count: DEFAULT_SHARD_COUNT,
        }
    }
}

/// Unified router that handles both subnet and object routing
pub struct UnifiedRouter {
    strategy: UnifiedRoutingStrategy,
    subnet_router: SubnetShardRouter,
    object_strategy: ObjectShardStrategy,
}

impl UnifiedRouter {
    /// Create with default strategy (subnet-first)
    pub fn new() -> Self {
        Self {
            strategy: UnifiedRoutingStrategy::default(),
            subnet_router: SubnetShardRouter::new(DEFAULT_SHARD_COUNT),
            object_strategy: ObjectShardStrategy::new(),
        }
    }

    /// Create with custom shard count
    pub fn with_shard_count(shard_count: u16) -> Self {
        Self {
            strategy: UnifiedRoutingStrategy::SubnetFirst {
                subnet_strategy: SubnetShardStrategy::HashBased { shard_count },
                object_shard_count: shard_count,
            },
            subnet_router: SubnetShardRouter::new(shard_count),
            object_strategy: ObjectShardStrategy::with_shard_count(shard_count),
        }
    }

    /// Create with custom strategy
    pub fn with_strategy(strategy: UnifiedRoutingStrategy) -> Self {
        let (subnet_router, object_strategy) = match &strategy {
            UnifiedRoutingStrategy::SubnetFirst {
                subnet_strategy,
                object_shard_count,
            } => (
                SubnetShardRouter::with_strategy(subnet_strategy.clone()),
                ObjectShardStrategy::with_shard_count(*object_shard_count),
            ),
            UnifiedRoutingStrategy::SubnetOnly { subnet_strategy } => (
                SubnetShardRouter::with_strategy(subnet_strategy.clone()),
                ObjectShardStrategy::new(),
            ),
            UnifiedRoutingStrategy::ObjectOnly { shard_count } => (
                SubnetShardRouter::new(*shard_count),
                ObjectShardStrategy::with_shard_count(*shard_count),
            ),
        };

        Self {
            strategy,
            subnet_router,
            object_strategy,
        }
    }

    /// Route a transaction to a shard
    pub fn route(&self, ctx: &RoutingContext) -> ShardRoutingResult {
        match &self.strategy {
            UnifiedRoutingStrategy::SubnetFirst { .. } => {
                if let Some(subnet_id) = &ctx.subnet_id {
                    // Has subnet - route by subnet
                    let shard = self.subnet_router.route(subnet_id);
                    ShardRoutingResult {
                        primary_shard: shard,
                        routing_method: RoutingMethod::BySubnet,
                        is_cross_shard: self.check_cross_shard_objects(ctx, shard),
                    }
                } else {
                    // No subnet - route by primary object
                    let shard = self.object_strategy.route_object(&ctx.primary_object);
                    ShardRoutingResult {
                        primary_shard: shard,
                        routing_method: RoutingMethod::ByObject,
                        is_cross_shard: self.check_cross_shard_objects(ctx, shard),
                    }
                }
            }
            UnifiedRoutingStrategy::ObjectOnly { .. } => {
                let shard = self.object_strategy.route_object(&ctx.primary_object);
                ShardRoutingResult {
                    primary_shard: shard,
                    routing_method: RoutingMethod::ByObject,
                    is_cross_shard: self.check_cross_shard_objects(ctx, shard),
                }
            }
            UnifiedRoutingStrategy::SubnetOnly { .. } => {
                let subnet_id = ctx.subnet_id.as_ref().unwrap_or(&ROOT_SUBNET);
                let shard = self.subnet_router.route(subnet_id);
                ShardRoutingResult {
                    primary_shard: shard,
                    routing_method: RoutingMethod::BySubnet,
                    is_cross_shard: false, // Subnet-only doesn't check object shards
                }
            }
        }
    }

    /// Check if any touched objects would go to different shards
    fn check_cross_shard_objects(&self, ctx: &RoutingContext, primary_shard: ShardId) -> bool {
        ctx.touched_objects
            .iter()
            .any(|obj| self.object_strategy.route_object(obj) != primary_shard)
    }

    /// Get detailed routing for all objects in a transaction
    pub fn route_detailed(&self, ctx: &RoutingContext) -> DetailedRoutingResult {
        let primary_result = self.route(ctx);

        // Map each object to its shard
        let object_shards: HashMap<ObjectId, ShardId> = ctx
            .touched_objects
            .iter()
            .map(|obj| (*obj, self.object_strategy.route_object(obj)))
            .collect();

        // Find all unique shards
        let mut all_shards: Vec<ShardId> = object_shards.values().copied().collect();
        all_shards.push(primary_result.primary_shard);
        all_shards.sort();
        all_shards.dedup();

        let requires_coordination = all_shards.len() > 1;

        DetailedRoutingResult {
            primary_shard: primary_result.primary_shard,
            routing_method: primary_result.routing_method,
            object_shards,
            all_shards,
            requires_coordination,
        }
    }
}

impl Default for UnifiedRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of shard routing
#[derive(Debug, Clone)]
pub struct ShardRoutingResult {
    /// Primary shard for this transaction
    pub primary_shard: ShardId,

    /// Method used for routing
    pub routing_method: RoutingMethod,

    /// Whether this transaction touches multiple shards
    pub is_cross_shard: bool,
}

/// Detailed routing result with per-object shard mapping
#[derive(Debug, Clone)]
pub struct DetailedRoutingResult {
    /// Primary shard for coordination
    pub primary_shard: ShardId,

    /// Method used for routing
    pub routing_method: RoutingMethod,

    /// Shard assignment for each object
    pub object_shards: HashMap<ObjectId, ShardId>,

    /// All shards involved
    pub all_shards: Vec<ShardId>,

    /// Whether multi-shard coordination is needed
    pub requires_coordination: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn make_object_id(name: &str) -> ObjectId {
        let mut hasher = Sha256::new();
        hasher.update(b"OBJECT:");
        hasher.update(name.as_bytes());
        let result = hasher.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&result);
        id
    }

    fn make_subnet_id(name: &str) -> SubnetId {
        let mut hasher = Sha256::new();
        hasher.update(b"SUBNET:");
        hasher.update(name.as_bytes());
        let result = hasher.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&result);
        id
    }

    #[test]
    fn test_subnet_first_with_subnet() {
        let router = UnifiedRouter::new();

        let subnet = make_subnet_id("defi-app");
        let obj = make_object_id("coin-1");

        let ctx = RoutingContext::with_subnet(subnet, obj);
        let result = router.route(&ctx);

        assert_eq!(result.routing_method, RoutingMethod::BySubnet);
    }

    #[test]
    fn test_subnet_first_without_subnet() {
        let router = UnifiedRouter::new();

        let obj = make_object_id("coin-1");
        let ctx = RoutingContext::with_object(obj);
        let result = router.route(&ctx);

        assert_eq!(result.routing_method, RoutingMethod::ByObject);
    }

    #[test]
    fn test_same_object_same_shard() {
        let router = UnifiedRouter::with_shard_count(16);

        let obj = make_object_id("my-coin");

        // Multiple transactions on same object should go to same shard
        let ctx1 = RoutingContext::with_object(obj);
        let ctx2 = RoutingContext::with_object(obj);

        let result1 = router.route(&ctx1);
        let result2 = router.route(&ctx2);

        assert_eq!(result1.primary_shard, result2.primary_shard);
    }

    #[test]
    fn test_cross_shard_detection() {
        let router = UnifiedRouter::with_shard_count(256);

        let obj1 = make_object_id("coin-1");
        let obj2 = make_object_id("coin-2");
        let obj3 = make_object_id("coin-3");

        let ctx = RoutingContext::with_object(obj1).with_touched_objects(vec![obj1, obj2, obj3]);

        let result = router.route_detailed(&ctx);

        println!("All shards: {:?}", result.all_shards);
        println!("Object shards: {:?}", result.object_shards);
    }

    #[test]
    fn test_object_only_strategy() {
        let router =
            UnifiedRouter::with_strategy(UnifiedRoutingStrategy::ObjectOnly { shard_count: 16 });

        let subnet = make_subnet_id("defi-app");
        let obj = make_object_id("coin-1");

        // Even with subnet, should route by object
        let ctx = RoutingContext::with_subnet(subnet, obj);
        let result = router.route(&ctx);

        assert_eq!(result.routing_method, RoutingMethod::ByObject);
    }
}
