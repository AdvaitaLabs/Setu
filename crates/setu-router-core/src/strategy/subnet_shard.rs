//! Subnet-based Shard Strategy
//!
//! Routes transactions to shards based on subnet ID.
//! Each subnet's transactions are processed by the same shard for state locality.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::ShardStrategy;
use crate::types::{ShardId, SubnetId, DEFAULT_SHARD_COUNT};

/// Subnet to shard mapping strategy
#[derive(Debug, Clone)]
pub enum SubnetShardStrategy {
    /// Hash-based: SubnetId -> ShardId via consistent hash
    /// Good for even distribution, but hot subnets can overload a shard
    HashBased { shard_count: u16 },

    /// Dedicated: Each subnet gets its own shard (or shares with few others)
    /// Good for isolation, but may have many empty shards
    Dedicated {
        /// Explicit subnet -> shard mapping
        mapping: HashMap<SubnetId, ShardId>,
        /// Default shard for unmapped subnets
        default_shard: ShardId,
    },

    /// Hybrid: Popular subnets get dedicated shards, others share
    Hybrid {
        /// Dedicated shards for popular subnets
        dedicated: HashMap<SubnetId, ShardId>,
        /// Remaining shards for hash-based routing
        shared_shard_start: ShardId,
        shared_shard_count: u16,
    },
}

impl Default for SubnetShardStrategy {
    fn default() -> Self {
        Self::HashBased {
            shard_count: DEFAULT_SHARD_COUNT,
        }
    }
}

impl ShardStrategy for SubnetShardStrategy {
    fn route(&self, key: &[u8; 32]) -> ShardId {
        match self {
            SubnetShardStrategy::HashBased { shard_count } => {
                let hash = u16::from_be_bytes([key[0], key[1]]);
                hash % shard_count
            }
            SubnetShardStrategy::Dedicated {
                mapping,
                default_shard,
            } => mapping.get(key).copied().unwrap_or(*default_shard),
            SubnetShardStrategy::Hybrid {
                dedicated,
                shared_shard_start,
                shared_shard_count,
            } => {
                if let Some(&shard) = dedicated.get(key) {
                    shard
                } else {
                    let hash = u16::from_be_bytes([key[0], key[1]]);
                    shared_shard_start + (hash % shared_shard_count)
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            SubnetShardStrategy::HashBased { .. } => "SubnetHashBased",
            SubnetShardStrategy::Dedicated { .. } => "SubnetDedicated",
            SubnetShardStrategy::Hybrid { .. } => "SubnetHybrid",
        }
    }
}

/// Load metrics for a shard
#[derive(Debug, Clone, Default)]
pub struct ShardLoadMetrics {
    /// Number of transactions processed
    pub tx_count: u64,
    /// Number of subnets assigned
    pub subnet_count: u32,
    /// Current pending transactions
    pub pending_tx: u32,
    /// Average processing time (ms)
    pub avg_latency_ms: f64,
}

/// Subnet-based shard router with load monitoring
pub struct SubnetShardRouter {
    strategy: SubnetShardStrategy,
    /// Load metrics per shard (for monitoring)
    shard_load: Arc<RwLock<HashMap<ShardId, ShardLoadMetrics>>>,
}

impl SubnetShardRouter {
    /// Create a new router with hash-based strategy
    pub fn new(shard_count: u16) -> Self {
        Self {
            strategy: SubnetShardStrategy::HashBased { shard_count },
            shard_load: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with custom strategy
    pub fn with_strategy(strategy: SubnetShardStrategy) -> Self {
        Self {
            strategy,
            shard_load: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Route a subnet to a shard
    pub fn route(&self, subnet_id: &SubnetId) -> ShardId {
        self.strategy.route(subnet_id)
    }

    /// Check if a transaction is cross-subnet
    pub fn is_cross_subnet(&self, source_subnet: &SubnetId, target_subnets: &[SubnetId]) -> bool {
        target_subnets.iter().any(|t| t != source_subnet)
    }

    /// Check if a transaction is cross-shard (more expensive)
    pub fn is_cross_shard(&self, source_subnet: &SubnetId, target_subnets: &[SubnetId]) -> bool {
        let source_shard = self.route(source_subnet);
        target_subnets.iter().any(|t| self.route(t) != source_shard)
    }

    /// Get routing decision for a cross-subnet transaction
    pub fn route_cross_subnet(
        &self,
        source_subnet: &SubnetId,
        target_subnets: &[SubnetId],
    ) -> CrossSubnetRoutingDecision {
        let source_shard = self.route(source_subnet);
        let target_shards: Vec<ShardId> = target_subnets.iter().map(|s| self.route(s)).collect();

        let unique_shards: std::collections::HashSet<_> = std::iter::once(source_shard)
            .chain(target_shards.iter().copied())
            .collect();

        if unique_shards.len() == 1 {
            CrossSubnetRoutingDecision::SingleShard {
                shard: source_shard,
            }
        } else {
            CrossSubnetRoutingDecision::MultiShard {
                coordinator_shard: source_shard,
                participant_shards: target_shards,
                requires_2pc: true,
            }
        }
    }

    /// Update load metrics for a shard
    pub fn record_tx(&self, shard: ShardId, latency_ms: f64) {
        let mut load = self.shard_load.write();
        let metrics = load.entry(shard).or_default();
        metrics.tx_count += 1;
        // Exponential moving average for latency
        metrics.avg_latency_ms = metrics.avg_latency_ms * 0.9 + latency_ms * 0.1;
    }

    /// Get load metrics for all shards
    pub fn get_load_metrics(&self) -> HashMap<ShardId, ShardLoadMetrics> {
        self.shard_load.read().clone()
    }

    /// Detect hot shards (for potential rebalancing)
    pub fn detect_hot_shards(&self, threshold_ratio: f64) -> Vec<ShardId> {
        let load = self.shard_load.read();
        if load.is_empty() {
            return vec![];
        }

        let avg_tx: f64 = load.values().map(|m| m.tx_count as f64).sum::<f64>() / load.len() as f64;
        let threshold = avg_tx * threshold_ratio;

        load.iter()
            .filter(|(_, m)| m.tx_count as f64 > threshold)
            .map(|(&shard, _)| shard)
            .collect()
    }
}

/// Routing decision for cross-subnet transactions
#[derive(Debug, Clone)]
pub enum CrossSubnetRoutingDecision {
    /// All subnets map to the same shard - simple case
    SingleShard { shard: ShardId },

    /// Multiple shards involved - needs coordination
    MultiShard {
        /// Shard that coordinates the transaction
        coordinator_shard: ShardId,
        /// Shards that participate in the transaction
        participant_shards: Vec<ShardId>,
        /// Whether 2-phase commit is required
        requires_2pc: bool,
    },
}

impl CrossSubnetRoutingDecision {
    pub fn is_simple(&self) -> bool {
        matches!(self, Self::SingleShard { .. })
    }

    pub fn all_shards(&self) -> Vec<ShardId> {
        match self {
            Self::SingleShard { shard } => vec![*shard],
            Self::MultiShard {
                coordinator_shard,
                participant_shards,
                ..
            } => {
                let mut shards = vec![*coordinator_shard];
                shards.extend(participant_shards.iter().copied());
                shards.sort();
                shards.dedup();
                shards
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_subnet_id(name: &str) -> SubnetId {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(name.as_bytes());
        let result = hasher.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&result);
        id
    }

    #[test]
    fn test_hash_based_routing() {
        let router = SubnetShardRouter::new(16);

        let defi = make_subnet_id("defi-app");
        let gaming = make_subnet_id("gaming-app");

        let shard1 = router.route(&defi);
        let shard2 = router.route(&gaming);

        // Same subnet always routes to same shard
        assert_eq!(router.route(&defi), shard1);
        assert_eq!(router.route(&gaming), shard2);

        // Shards should be in valid range
        assert!(shard1 < 16);
        assert!(shard2 < 16);
    }

    #[test]
    fn test_dedicated_routing() {
        let defi = make_subnet_id("defi-app");
        let gaming = make_subnet_id("gaming-app");
        let other = make_subnet_id("other-app");

        let mut mapping = HashMap::new();
        mapping.insert(defi, 0);
        mapping.insert(gaming, 1);

        let router = SubnetShardRouter::with_strategy(SubnetShardStrategy::Dedicated {
            mapping,
            default_shard: 15,
        });

        assert_eq!(router.route(&defi), 0);
        assert_eq!(router.route(&gaming), 1);
        assert_eq!(router.route(&other), 15); // Default shard
    }

    #[test]
    fn test_cross_subnet_detection() {
        let router = SubnetShardRouter::new(16);

        let defi = make_subnet_id("defi-app");
        let gaming = make_subnet_id("gaming-app");

        // Same subnet - not cross-subnet
        assert!(!router.is_cross_subnet(&defi, &[defi]));

        // Different subnets - cross-subnet
        assert!(router.is_cross_subnet(&defi, &[gaming]));
    }

    #[test]
    fn test_cross_shard_routing() {
        // Force different shards by using dedicated mapping
        let defi = make_subnet_id("defi-app");
        let gaming = make_subnet_id("gaming-app");

        let mut mapping = HashMap::new();
        mapping.insert(defi, 0);
        mapping.insert(gaming, 1);

        let router = SubnetShardRouter::with_strategy(SubnetShardStrategy::Dedicated {
            mapping,
            default_shard: 15,
        });

        let decision = router.route_cross_subnet(&defi, &[gaming]);

        match decision {
            CrossSubnetRoutingDecision::MultiShard { requires_2pc, .. } => {
                assert!(requires_2pc);
            }
            _ => panic!("Expected MultiShard decision"),
        }
    }
}
