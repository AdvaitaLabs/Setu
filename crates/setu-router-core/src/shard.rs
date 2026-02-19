//! Shard types for future multi-shard support
//!
//! MVP: Single shard with a default shard ID.
//! Future: Multiple shards with resource-based routing.

use serde::{Deserialize, Serialize};

use crate::types::{LegacyShardId, DEFAULT_SHARD_ID};

/// Shard configuration (minimal for MVP)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardConfig {
    /// Shard identifier
    pub id: LegacyShardId,

    /// Human-readable name
    pub name: String,

    /// Maximum number of solvers in this shard
    pub max_solvers: usize,
}

impl ShardConfig {
    /// Create default shard config for MVP
    pub fn default_mvp() -> Self {
        Self {
            id: DEFAULT_SHARD_ID.to_string(),
            name: "Default Shard".to_string(),
            max_solvers: 6,
        }
    }

    /// Create a new shard config
    pub fn new(id: LegacyShardId, name: String) -> Self {
        Self {
            id,
            name,
            max_solvers: 10,
        }
    }
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self::default_mvp()
    }
}

// =============================================================================
// Future multi-shard support (placeholder interfaces)
// =============================================================================

/// Trait for shard routing strategy (future use)
///
/// In MVP, all transactions go to the default shard.
/// In the future, implement this trait to route based on resources.
#[allow(dead_code)]
pub trait ShardRouter: Send + Sync {
    /// Route resources to a shard ID
    fn route(&self, resources: &[String]) -> LegacyShardId;
}

/// Simple single-shard router for MVP
#[derive(Debug, Clone, Default)]
pub struct SingleShardRouter;

impl ShardRouter for SingleShardRouter {
    fn route(&self, _resources: &[String]) -> LegacyShardId {
        DEFAULT_SHARD_ID.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_shard_config() {
        let config = ShardConfig::default_mvp();
        assert_eq!(config.id, DEFAULT_SHARD_ID);
        assert_eq!(config.max_solvers, 6);
    }

    #[test]
    fn test_single_shard_router() {
        let router = SingleShardRouter;

        // All resources route to default shard
        assert_eq!(
            router.route(&["account:alice".to_string()]),
            DEFAULT_SHARD_ID
        );
        assert_eq!(router.route(&["nft:token1".to_string()]), DEFAULT_SHARD_ID);
        assert_eq!(router.route(&[]), DEFAULT_SHARD_ID);
    }
}
