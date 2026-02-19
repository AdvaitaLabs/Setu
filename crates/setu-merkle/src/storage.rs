//! Storage traits and interfaces for Merkle tree persistence.
//!
//! This module defines the storage abstraction for persisting Merkle tree nodes
//! and state. Implementations can use various backends like RocksDB, in-memory
//! storage, or remote storage systems.
//!
//! # Design
//!
//! The storage layer is designed around these key concepts:
//!
//! - **Node Storage**: Individual tree nodes (leaves and internal nodes) are stored
//!   by their hash.
//! - **Versioning**: State is versioned by anchor/epoch, allowing historical queries.
//! - **Per-Subnet Isolation**: Each subnet's SMT is stored in its own namespace.
//!
//! # Column Family Layout (for RocksDB)
//!
//! ```text
//! CF: "smt_nodes"      - Key: (subnet_id, node_hash) -> Value: SMTNode
//! CF: "smt_roots"      - Key: (subnet_id, anchor_id) -> Value: HashValue (root)
//! CF: "aggregation"    - Key: anchor_id -> Value: GlobalStateRoot
//! ```

use std::sync::Arc;

use crate::error::MerkleResult;
use crate::hash::HashValue;
use crate::sparse::SparseMerkleNode;

/// SubnetId for storage operations.
pub type SubnetId = [u8; 32];

/// Anchor/Version identifier.
pub type AnchorId = u64;

/// A trait for storing and retrieving Merkle tree nodes.
///
/// This trait abstracts over the storage backend, allowing different
/// implementations for testing, production, and various database systems.
pub trait MerkleNodeStore: Send + Sync {
    /// Store a node by its hash.
    fn put_node(
        &self,
        subnet_id: &SubnetId,
        hash: &HashValue,
        node: &SparseMerkleNode,
    ) -> MerkleResult<()>;

    /// Retrieve a node by its hash.
    fn get_node(
        &self,
        subnet_id: &SubnetId,
        hash: &HashValue,
    ) -> MerkleResult<Option<SparseMerkleNode>>;

    /// Check if a node exists.
    fn has_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<bool>;

    /// Delete a node.
    fn delete_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<()>;

    /// Batch put multiple nodes atomically.
    fn batch_put_nodes(
        &self,
        subnet_id: &SubnetId,
        nodes: &[(HashValue, SparseMerkleNode)],
    ) -> MerkleResult<()>;
}

/// A trait for storing subnet state roots with versioning.
pub trait MerkleRootStore: Send + Sync {
    /// Store a subnet's state root for a specific anchor.
    fn put_subnet_root(
        &self,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()>;

    /// Get a subnet's state root at a specific anchor.
    fn get_subnet_root(
        &self,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
    ) -> MerkleResult<Option<HashValue>>;

    /// Get the latest state root for a subnet.
    fn get_latest_subnet_root(
        &self,
        subnet_id: &SubnetId,
    ) -> MerkleResult<Option<(AnchorId, HashValue)>>;

    /// Store the global state root for an anchor.
    fn put_global_root(&self, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()>;

    /// Get the global state root at a specific anchor.
    fn get_global_root(&self, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>>;

    /// Get the latest global state root.
    fn get_latest_global_root(&self) -> MerkleResult<Option<(AnchorId, HashValue)>>;

    /// List all anchors for a subnet within a range.
    fn list_anchors(
        &self,
        subnet_id: &SubnetId,
        start: AnchorId,
        end: AnchorId,
    ) -> MerkleResult<Vec<AnchorId>>;
}

/// Combined storage interface for Merkle trees.
pub trait MerkleStore: MerkleNodeStore + MerkleRootStore {
    /// Flush pending writes to persistent storage.
    fn flush(&self) -> MerkleResult<()>;

    /// Create a checkpoint/snapshot at the current state.
    fn checkpoint(&self, anchor_id: AnchorId) -> MerkleResult<()>;

    /// Prune old data before a given anchor.
    fn prune_before(&self, anchor_id: AnchorId) -> MerkleResult<u64>;
}

/// An in-memory implementation of MerkleStore for testing.
#[derive(Clone, Default)]
pub struct InMemoryMerkleStore {
    nodes:
        Arc<std::sync::RwLock<std::collections::HashMap<(SubnetId, HashValue), SparseMerkleNode>>>,
    subnet_roots:
        Arc<std::sync::RwLock<std::collections::HashMap<(SubnetId, AnchorId), HashValue>>>,
    global_roots: Arc<std::sync::RwLock<std::collections::HashMap<AnchorId, HashValue>>>,
}

impl InMemoryMerkleStore {
    /// Create a new in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of stored nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.read().unwrap().len()
    }
}

impl MerkleNodeStore for InMemoryMerkleStore {
    fn put_node(
        &self,
        subnet_id: &SubnetId,
        hash: &HashValue,
        node: &SparseMerkleNode,
    ) -> MerkleResult<()> {
        self.nodes
            .write()
            .unwrap()
            .insert((*subnet_id, *hash), node.clone());
        Ok(())
    }

    fn get_node(
        &self,
        subnet_id: &SubnetId,
        hash: &HashValue,
    ) -> MerkleResult<Option<SparseMerkleNode>> {
        Ok(self
            .nodes
            .read()
            .unwrap()
            .get(&(*subnet_id, *hash))
            .cloned())
    }

    fn has_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<bool> {
        Ok(self
            .nodes
            .read()
            .unwrap()
            .contains_key(&(*subnet_id, *hash)))
    }

    fn delete_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<()> {
        self.nodes.write().unwrap().remove(&(*subnet_id, *hash));
        Ok(())
    }

    fn batch_put_nodes(
        &self,
        subnet_id: &SubnetId,
        nodes: &[(HashValue, SparseMerkleNode)],
    ) -> MerkleResult<()> {
        let mut store = self.nodes.write().unwrap();
        for (hash, node) in nodes {
            store.insert((*subnet_id, *hash), node.clone());
        }
        Ok(())
    }
}

impl MerkleRootStore for InMemoryMerkleStore {
    fn put_subnet_root(
        &self,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()> {
        self.subnet_roots
            .write()
            .unwrap()
            .insert((*subnet_id, anchor_id), *root);
        Ok(())
    }

    fn get_subnet_root(
        &self,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
    ) -> MerkleResult<Option<HashValue>> {
        Ok(self
            .subnet_roots
            .read()
            .unwrap()
            .get(&(*subnet_id, anchor_id))
            .copied())
    }

    fn get_latest_subnet_root(
        &self,
        subnet_id: &SubnetId,
    ) -> MerkleResult<Option<(AnchorId, HashValue)>> {
        let store = self.subnet_roots.read().unwrap();
        let result = store
            .iter()
            .filter(|((sid, _), _)| sid == subnet_id)
            .max_by_key(|((_, aid), _)| *aid)
            .map(|((_, aid), root)| (*aid, *root));
        Ok(result)
    }

    fn put_global_root(&self, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()> {
        self.global_roots.write().unwrap().insert(anchor_id, *root);
        Ok(())
    }

    fn get_global_root(&self, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>> {
        Ok(self.global_roots.read().unwrap().get(&anchor_id).copied())
    }

    fn get_latest_global_root(&self) -> MerkleResult<Option<(AnchorId, HashValue)>> {
        let store = self.global_roots.read().unwrap();
        let result = store
            .iter()
            .max_by_key(|(aid, _)| *aid)
            .map(|(aid, root)| (*aid, *root));
        Ok(result)
    }

    fn list_anchors(
        &self,
        subnet_id: &SubnetId,
        start: AnchorId,
        end: AnchorId,
    ) -> MerkleResult<Vec<AnchorId>> {
        let store = self.subnet_roots.read().unwrap();
        let mut anchors: Vec<AnchorId> = store
            .keys()
            .filter(|(sid, aid)| sid == subnet_id && *aid >= start && *aid <= end)
            .map(|(_, aid)| *aid)
            .collect();
        anchors.sort();
        Ok(anchors)
    }
}

impl MerkleStore for InMemoryMerkleStore {
    fn flush(&self) -> MerkleResult<()> {
        // No-op for in-memory store
        Ok(())
    }

    fn checkpoint(&self, _anchor_id: AnchorId) -> MerkleResult<()> {
        // No-op for in-memory store
        Ok(())
    }

    fn prune_before(&self, anchor_id: AnchorId) -> MerkleResult<u64> {
        let mut count = 0u64;

        // Prune subnet roots
        {
            let mut store = self.subnet_roots.write().unwrap();
            let keys_to_remove: Vec<_> = store
                .keys()
                .filter(|(_, aid)| *aid < anchor_id)
                .cloned()
                .collect();
            for key in keys_to_remove {
                store.remove(&key);
                count += 1;
            }
        }

        // Prune global roots
        {
            let mut store = self.global_roots.write().unwrap();
            let keys_to_remove: Vec<_> = store
                .keys()
                .filter(|aid| **aid < anchor_id)
                .cloned()
                .collect();
            for key in keys_to_remove {
                store.remove(&key);
                count += 1;
            }
        }

        Ok(count)
    }
}

/// Builder for creating a persistent SMT with storage backend.
pub struct PersistentSmtBuilder<S: MerkleStore> {
    store: Arc<S>,
    subnet_id: SubnetId,
}

impl<S: MerkleStore> PersistentSmtBuilder<S> {
    /// Create a new builder with the given store and subnet.
    pub fn new(store: Arc<S>, subnet_id: SubnetId) -> Self {
        Self { store, subnet_id }
    }

    /// Get the store reference.
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Get the subnet ID.
    pub fn subnet_id(&self) -> &SubnetId {
        &self.subnet_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SUBNET: SubnetId = [0u8; 32];
    const APP_SUBNET: SubnetId = [1u8; 32];

    fn make_hash(byte: u8) -> HashValue {
        HashValue::new([byte; 32])
    }

    fn make_leaf_node(byte: u8) -> SparseMerkleNode {
        SparseMerkleNode::Leaf {
            key: make_hash(byte),
            value_hash: make_hash(byte + 100),
        }
    }

    #[test]
    fn test_in_memory_node_store() {
        let store = InMemoryMerkleStore::new();
        let hash = make_hash(1);
        let node = make_leaf_node(1);

        // Initially empty
        assert!(!store.has_node(&TEST_SUBNET, &hash).unwrap());
        assert!(store.get_node(&TEST_SUBNET, &hash).unwrap().is_none());

        // Store node
        store.put_node(&TEST_SUBNET, &hash, &node).unwrap();
        assert!(store.has_node(&TEST_SUBNET, &hash).unwrap());
        assert_eq!(
            store.get_node(&TEST_SUBNET, &hash).unwrap(),
            Some(node.clone())
        );

        // Different subnet shouldn't have it
        assert!(!store.has_node(&APP_SUBNET, &hash).unwrap());

        // Delete node
        store.delete_node(&TEST_SUBNET, &hash).unwrap();
        assert!(!store.has_node(&TEST_SUBNET, &hash).unwrap());
    }

    #[test]
    fn test_in_memory_batch_put() {
        let store = InMemoryMerkleStore::new();

        let nodes: Vec<(HashValue, SparseMerkleNode)> =
            (1..5).map(|i| (make_hash(i), make_leaf_node(i))).collect();

        store.batch_put_nodes(&TEST_SUBNET, &nodes).unwrap();

        assert_eq!(store.node_count(), 4);
        for (hash, node) in &nodes {
            assert_eq!(
                store.get_node(&TEST_SUBNET, hash).unwrap(),
                Some(node.clone())
            );
        }
    }

    #[test]
    fn test_in_memory_root_store() {
        let store = InMemoryMerkleStore::new();

        // Store subnet roots
        store
            .put_subnet_root(&TEST_SUBNET, 1, &make_hash(10))
            .unwrap();
        store
            .put_subnet_root(&TEST_SUBNET, 2, &make_hash(20))
            .unwrap();
        store
            .put_subnet_root(&TEST_SUBNET, 3, &make_hash(30))
            .unwrap();

        // Get specific version
        assert_eq!(
            store.get_subnet_root(&TEST_SUBNET, 2).unwrap(),
            Some(make_hash(20))
        );

        // Get latest
        let (anchor, root) = store.get_latest_subnet_root(&TEST_SUBNET).unwrap().unwrap();
        assert_eq!(anchor, 3);
        assert_eq!(root, make_hash(30));

        // List anchors
        let anchors = store.list_anchors(&TEST_SUBNET, 1, 10).unwrap();
        assert_eq!(anchors, vec![1, 2, 3]);
    }

    #[test]
    fn test_in_memory_global_root_store() {
        let store = InMemoryMerkleStore::new();

        store.put_global_root(100, &make_hash(1)).unwrap();
        store.put_global_root(101, &make_hash(2)).unwrap();

        assert_eq!(store.get_global_root(100).unwrap(), Some(make_hash(1)));

        let (anchor, root) = store.get_latest_global_root().unwrap().unwrap();
        assert_eq!(anchor, 101);
        assert_eq!(root, make_hash(2));
    }

    #[test]
    fn test_in_memory_prune() {
        let store = InMemoryMerkleStore::new();

        // Add data at various anchors
        store
            .put_subnet_root(&TEST_SUBNET, 1, &make_hash(1))
            .unwrap();
        store
            .put_subnet_root(&TEST_SUBNET, 2, &make_hash(2))
            .unwrap();
        store
            .put_subnet_root(&TEST_SUBNET, 5, &make_hash(5))
            .unwrap();
        store.put_global_root(1, &make_hash(10)).unwrap();
        store.put_global_root(3, &make_hash(30)).unwrap();

        // Prune before anchor 3
        let pruned = store.prune_before(3).unwrap();
        assert_eq!(pruned, 3); // 2 subnet roots + 1 global root

        // Check what remains
        assert!(store.get_subnet_root(&TEST_SUBNET, 1).unwrap().is_none());
        assert!(store.get_subnet_root(&TEST_SUBNET, 2).unwrap().is_none());
        assert!(store.get_subnet_root(&TEST_SUBNET, 5).unwrap().is_some());
        assert!(store.get_global_root(1).unwrap().is_none());
        assert!(store.get_global_root(3).unwrap().is_some());
    }

    #[test]
    fn test_flush_and_checkpoint() {
        let store = InMemoryMerkleStore::new();

        // These are no-ops for in-memory store but should succeed
        assert!(store.flush().is_ok());
        assert!(store.checkpoint(100).is_ok());
    }
}
