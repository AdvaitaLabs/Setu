//! Object-based Shard Strategy
//!
//! Routes transactions to shards based on object ID.
//! Same object always goes to the same shard for state locality.
//!
//! This is used when:
//! - User hasn't joined any subnet
//! - Transaction doesn't specify a subnet
//! - Fallback routing is needed

use super::ShardStrategy;
use crate::types::{ObjectId, ShardId, DEFAULT_SHARD_COUNT};

/// Object-based shard routing strategy
#[derive(Debug, Clone)]
pub struct ObjectShardStrategy {
    /// Number of shards
    shard_count: u16,
}

impl ObjectShardStrategy {
    /// Create with default shard count
    pub fn new() -> Self {
        Self {
            shard_count: DEFAULT_SHARD_COUNT,
        }
    }

    /// Create with custom shard count
    pub fn with_shard_count(shard_count: u16) -> Self {
        Self { shard_count }
    }

    /// Route an object to a shard
    pub fn route_object(&self, object_id: &ObjectId) -> ShardId {
        // Use first 2 bytes of object ID for shard routing
        let hash = u16::from_be_bytes([object_id[0], object_id[1]]);
        hash % self.shard_count
    }

    /// Check if multiple objects would be in different shards
    pub fn is_cross_shard(&self, objects: &[ObjectId]) -> bool {
        if objects.len() <= 1 {
            return false;
        }

        let first_shard = self.route_object(&objects[0]);
        objects
            .iter()
            .skip(1)
            .any(|obj| self.route_object(obj) != first_shard)
    }

    /// Get all shards involved for a set of objects
    pub fn get_involved_shards(&self, objects: &[ObjectId]) -> Vec<ShardId> {
        let mut shards: Vec<_> = objects.iter().map(|obj| self.route_object(obj)).collect();
        shards.sort();
        shards.dedup();
        shards
    }
}

impl Default for ObjectShardStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardStrategy for ObjectShardStrategy {
    fn route(&self, key: &[u8; 32]) -> ShardId {
        self.route_object(key)
    }

    fn name(&self) -> &'static str {
        "ObjectBased"
    }
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

    #[test]
    fn test_same_object_same_shard() {
        let strategy = ObjectShardStrategy::with_shard_count(16);

        let obj = make_object_id("my-coin");

        // Same object always routes to same shard
        assert_eq!(strategy.route_object(&obj), strategy.route_object(&obj));
    }

    #[test]
    fn test_shard_range() {
        let strategy = ObjectShardStrategy::with_shard_count(16);

        for i in 0..100 {
            let obj = make_object_id(&format!("object-{}", i));
            let shard = strategy.route_object(&obj);
            assert!(shard < 16, "shard {} should be < 16", shard);
        }
    }

    #[test]
    fn test_cross_shard_detection() {
        let strategy = ObjectShardStrategy::with_shard_count(256);

        let obj1 = make_object_id("coin-1");
        let obj2 = make_object_id("coin-2");
        let obj3 = make_object_id("coin-3");

        // Single object is never cross-shard
        assert!(!strategy.is_cross_shard(&[obj1]));
        assert!(!strategy.is_cross_shard(&[]));

        // Multiple objects may be cross-shard
        let objects = vec![obj1, obj2, obj3];
        let shards = strategy.get_involved_shards(&objects);
        println!("Objects route to shards: {:?}", shards);
    }

    #[test]
    fn test_get_involved_shards() {
        let strategy = ObjectShardStrategy::with_shard_count(4);

        // Create objects that definitely map to different shards
        // by controlling first 2 bytes
        let mut obj1 = [0u8; 32];
        let mut obj2 = [0u8; 32];
        let mut obj3 = [0u8; 32];

        obj1[0] = 0;
        obj1[1] = 0; // shard 0
        obj2[0] = 0;
        obj2[1] = 1; // shard 1
        obj3[0] = 0;
        obj3[1] = 2; // shard 2

        let shards = strategy.get_involved_shards(&[obj1, obj2, obj3]);
        assert_eq!(shards.len(), 3);
        assert!(shards.contains(&0));
        assert!(shards.contains(&1));
        assert!(shards.contains(&2));
    }
}
