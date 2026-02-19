//! Shard management module

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub type ShardId = String;
pub type ResourceKey = String;

/// Shard information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    /// Shard ID
    pub id: ShardId,

    /// Resource domain (which resources this shard handles)
    pub resource_domain: HashSet<ResourceKey>,

    /// Node IDs in this shard
    pub node_ids: Vec<String>,
}

impl Shard {
    pub fn new(id: ShardId) -> Self {
        Self {
            id,
            resource_domain: HashSet::new(),
            node_ids: vec![],
        }
    }

    pub fn add_node(&mut self, node_id: String) {
        if !self.node_ids.contains(&node_id) {
            self.node_ids.push(node_id);
        }
    }

    pub fn add_resource(&mut self, resource_key: ResourceKey) {
        self.resource_domain.insert(resource_key);
    }

    pub fn contains_resource(&self, resource_key: &ResourceKey) -> bool {
        self.resource_domain.contains(resource_key)
    }
}

/// Shard manager
pub struct ShardManager {
    shards: Arc<RwLock<Vec<Shard>>>,
}

impl ShardManager {
    pub fn new() -> Self {
        Self {
            shards: Arc::new(RwLock::new(vec![])),
        }
    }

    /// Register a new shard
    pub fn register_shard(&self, shard: Shard) {
        let mut shards = self.shards.write();

        // Check if shard already exists
        if let Some(existing) = shards.iter_mut().find(|s| s.id == shard.id) {
            *existing = shard;
        } else {
            shards.push(shard);
        }
    }

    /// Get shard by ID
    pub fn get_shard(&self, shard_id: &ShardId) -> Option<Shard> {
        let shards = self.shards.read();
        shards.iter().find(|s| s.id == *shard_id).cloned()
    }

    /// Route resource to shard
    pub fn route_to_shard(&self, resource_key: &ResourceKey) -> Option<ShardId> {
        let shards = self.shards.read();

        // Find shard that contains this resource
        for shard in shards.iter() {
            if shard.contains_resource(resource_key) {
                return Some(shard.id.clone());
            }
        }

        // If no explicit mapping, use hash-based routing
        if !shards.is_empty() {
            let hash = self.hash_resource(resource_key);
            let index = hash % shards.len();
            return Some(shards[index].id.clone());
        }

        None
    }

    /// Hash resource key for routing
    fn hash_resource(&self, resource_key: &ResourceKey) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        resource_key.hash(&mut hasher);
        hasher.finish() as usize
    }
}

impl Default for ShardManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_creation() {
        let mut shard = Shard::new("shard1".to_string());
        assert_eq!(shard.id, "shard1");

        shard.add_node("node1".to_string());
        assert_eq!(shard.node_ids.len(), 1);
    }

    #[test]
    fn test_shard_manager() {
        let manager = ShardManager::new();

        let mut shard1 = Shard::new("shard1".to_string());
        shard1.add_resource("resource1".to_string());

        manager.register_shard(shard1);

        let routed = manager.route_to_shard(&"resource1".to_string());
        assert_eq!(routed, Some("shard1".to_string()));
    }
}
