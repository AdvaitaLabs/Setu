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
//! - **Leaf Storage (B4)**: Raw leaf data is stored separately for recovery.
//! - **Versioning**: State is versioned by anchor/epoch, allowing historical queries.
//! - **Per-Subnet Isolation**: Each subnet's SMT is stored in its own namespace.
//!
//! # Column Family Layout (for RocksDB)
//!
//! ```text
//! CF: "merkle_nodes"   - Key: (subnet_id, node_hash) -> Value: SMTNode
//! CF: "merkle_roots"   - Key: (subnet_id, anchor_id) -> Value: HashValue (root)
//! CF: "merkle_leaves"  - Key: (subnet_id, object_id) -> Value: leaf_value (B4 scheme)
//! CF: "merkle_meta"    - Key: metadata_key -> Value: metadata (B4 scheme)
//! CF: "aggregation"    - Key: anchor_id -> Value: GlobalStateRoot
//! ```

use std::collections::HashMap;
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
    fn put_node(&self, subnet_id: &SubnetId, hash: &HashValue, node: &SparseMerkleNode) -> MerkleResult<()>;

    /// Retrieve a node by its hash.
    fn get_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<Option<SparseMerkleNode>>;

    /// Check if a node exists.
    fn has_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<bool>;

    /// Delete a node.
    fn delete_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<()>;

    /// Batch put multiple nodes atomically.
    fn batch_put_nodes(&self, subnet_id: &SubnetId, nodes: &[(HashValue, SparseMerkleNode)]) -> MerkleResult<()>;
}

/// A trait for storing subnet state roots with versioning.
pub trait MerkleRootStore: Send + Sync {
    /// Store a subnet's state root for a specific anchor.
    fn put_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()>;

    /// Get a subnet's state root at a specific anchor.
    fn get_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>>;

    /// Get the latest state root for a subnet.
    fn get_latest_subnet_root(&self, subnet_id: &SubnetId) -> MerkleResult<Option<(AnchorId, HashValue)>>;

    /// Store the global state root for an anchor.
    fn put_global_root(&self, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()>;

    /// Get the global state root at a specific anchor.
    fn get_global_root(&self, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>>;

    /// Get the latest global state root.
    fn get_latest_global_root(&self) -> MerkleResult<Option<(AnchorId, HashValue)>>;

    /// List all anchors for a subnet within a range.
    fn list_anchors(&self, subnet_id: &SubnetId, start: AnchorId, end: AnchorId) -> MerkleResult<Vec<AnchorId>>;
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

/// B4 Scheme: A trait for storing and retrieving raw leaf data.
///
/// This trait supports the B4 (Batch delayed persistence) scheme where:
/// - Leaf data is tracked as dirty during transaction execution
/// - All dirty leaves are persisted atomically at Anchor commit
/// - Recovery reconstructs SMT from persisted leaf data
///
/// Key format: (subnet_id, object_id) -> leaf_value
pub trait MerkleLeafStore: Send + Sync {
    /// Batch insert/update multiple leaves atomically.
    /// 
    /// # Arguments
    /// * `subnet_id` - The subnet identifier
    /// * `leaves` - Slice of (object_id, value) pairs to upsert
    fn batch_put_leaves(
        &self, 
        subnet_id: &SubnetId, 
        leaves: &[(&HashValue, &[u8])]
    ) -> MerkleResult<()>;

    /// Batch delete multiple leaves atomically.
    /// 
    /// # Arguments
    /// * `subnet_id` - The subnet identifier
    /// * `object_ids` - Slice of object IDs to delete
    fn batch_delete_leaves(
        &self, 
        subnet_id: &SubnetId, 
        object_ids: &[&HashValue]
    ) -> MerkleResult<()>;

    /// Load all leaves for a subnet (for SMT recovery).
    /// 
    /// This is used during node restart to reconstruct the SMT
    /// from persisted leaf data.
    /// 
    /// # Returns
    /// A HashMap of (object_id -> leaf_value) for the subnet
    fn load_all_leaves(&self, subnet_id: &SubnetId) -> MerkleResult<HashMap<HashValue, Vec<u8>>>;

    /// List all subnet IDs that have persisted leaves.
    /// 
    /// Used during recovery to discover which subnets need to be restored.
    fn list_subnets(&self) -> MerkleResult<Vec<SubnetId>>;

    /// Get a single leaf value.
    fn get_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<Option<Vec<u8>>>;

    /// Check if a leaf exists.
    fn has_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<bool>;

    /// Get the count of leaves for a subnet.
    fn leaf_count(&self, subnet_id: &SubnetId) -> MerkleResult<usize>;
}

/// B4 Scheme: Metadata storage for SMT persistence.
///
/// Stores metadata required for B4 scheme operation:
/// - Subnet registry (which subnets exist)
/// - Last committed anchor per subnet
/// - Recovery checkpoints
pub trait MerkleMetaStore: Send + Sync {
    /// Record a subnet as active (has data).
    fn register_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()>;

    /// Remove a subnet from the registry.
    fn unregister_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()>;

    /// Check if a subnet is registered.
    fn is_subnet_registered(&self, subnet_id: &SubnetId) -> MerkleResult<bool>;

    /// Get all registered subnet IDs.
    fn list_registered_subnets(&self) -> MerkleResult<Vec<SubnetId>>;

    /// Store the last committed anchor for a subnet.
    fn set_last_anchor(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<()>;

    /// Get the last committed anchor for a subnet.
    fn get_last_anchor(&self, subnet_id: &SubnetId) -> MerkleResult<Option<AnchorId>>;

    /// Store global metadata (key-value).
    fn set_meta(&self, key: &str, value: &[u8]) -> MerkleResult<()>;

    /// Get global metadata.
    fn get_meta(&self, key: &str) -> MerkleResult<Option<Vec<u8>>>;
}

/// B4 Scheme: Combined storage trait with atomic WriteBatch support.
///
/// This trait combines all B4 storage operations and provides WriteBatch-based
/// atomic commit capability. All operations within a single commit() call will
/// be written atomically using a single WriteBatch.
///
/// # Atomicity Guarantee
///
/// The B4 scheme ensures that either all changes are persisted or none:
/// - Create a batch via `begin_batch()`
/// - Accumulate operations via `batch_*` methods
/// - Commit atomically via `commit_batch()`
///
/// # Type Erasure
///
/// This trait uses `Box<dyn Any + Send>` for the batch type to allow
/// trait object usage. Implementations should downcast to their concrete
/// batch type internally.
pub trait B4Store: MerkleLeafStore + MerkleMetaStore + MerkleRootStore + Send + Sync {
    /// Create a new WriteBatch for accumulating operations.
    /// Returns an opaque batch handle as `Box<dyn Any + Send>`.
    fn begin_batch(&self) -> MerkleResult<Box<dyn std::any::Any + Send>>;

    /// Commit all operations in the WriteBatch atomically.
    /// After commit, the batch is consumed and cannot be reused.
    fn commit_batch(&self, batch: Box<dyn std::any::Any + Send>) -> MerkleResult<()>;

    /// Batch insert/update leaves into the WriteBatch (not committed yet).
    fn batch_put_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        leaves: &[(&HashValue, &[u8])],
    ) -> MerkleResult<()>;

    /// Batch delete leaves into the WriteBatch (not committed yet).
    fn batch_delete_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        object_ids: &[&HashValue],
    ) -> MerkleResult<()>;

    /// Register a subnet in the WriteBatch (not committed yet).
    fn batch_register_subnet(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
    ) -> MerkleResult<()>;

    /// Set last anchor for a subnet in the WriteBatch (not committed yet).
    fn batch_set_last_anchor(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
    ) -> MerkleResult<()>;

    /// Put subnet root in the WriteBatch (not committed yet).
    fn batch_put_subnet_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()>;

    /// Put global root in the WriteBatch (not committed yet).
    fn batch_put_global_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()>;
}

/// An in-memory implementation of MerkleStore for testing.
#[derive(Clone, Default)]
pub struct InMemoryMerkleStore {
    nodes: Arc<std::sync::RwLock<std::collections::HashMap<(SubnetId, HashValue), SparseMerkleNode>>>,
    subnet_roots: Arc<std::sync::RwLock<std::collections::HashMap<(SubnetId, AnchorId), HashValue>>>,
    global_roots: Arc<std::sync::RwLock<std::collections::HashMap<AnchorId, HashValue>>>,
    // B4 scheme: leaf storage
    leaves: Arc<std::sync::RwLock<std::collections::HashMap<(SubnetId, HashValue), Vec<u8>>>>,
    // B4 scheme: metadata storage
    registered_subnets: Arc<std::sync::RwLock<std::collections::HashSet<SubnetId>>>,
    last_anchors: Arc<std::sync::RwLock<std::collections::HashMap<SubnetId, AnchorId>>>,
    meta: Arc<std::sync::RwLock<std::collections::HashMap<String, Vec<u8>>>>,
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
    fn put_node(&self, subnet_id: &SubnetId, hash: &HashValue, node: &SparseMerkleNode) -> MerkleResult<()> {
        self.nodes.write().unwrap().insert((*subnet_id, *hash), node.clone());
        Ok(())
    }

    fn get_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<Option<SparseMerkleNode>> {
        Ok(self.nodes.read().unwrap().get(&(*subnet_id, *hash)).cloned())
    }

    fn has_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<bool> {
        Ok(self.nodes.read().unwrap().contains_key(&(*subnet_id, *hash)))
    }

    fn delete_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<()> {
        self.nodes.write().unwrap().remove(&(*subnet_id, *hash));
        Ok(())
    }

    fn batch_put_nodes(&self, subnet_id: &SubnetId, nodes: &[(HashValue, SparseMerkleNode)]) -> MerkleResult<()> {
        let mut store = self.nodes.write().unwrap();
        for (hash, node) in nodes {
            store.insert((*subnet_id, *hash), node.clone());
        }
        Ok(())
    }
}

impl MerkleRootStore for InMemoryMerkleStore {
    fn put_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()> {
        self.subnet_roots.write().unwrap().insert((*subnet_id, anchor_id), *root);
        Ok(())
    }

    fn get_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>> {
        Ok(self.subnet_roots.read().unwrap().get(&(*subnet_id, anchor_id)).copied())
    }

    fn get_latest_subnet_root(&self, subnet_id: &SubnetId) -> MerkleResult<Option<(AnchorId, HashValue)>> {
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

    fn list_anchors(&self, subnet_id: &SubnetId, start: AnchorId, end: AnchorId) -> MerkleResult<Vec<AnchorId>> {
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

impl MerkleLeafStore for InMemoryMerkleStore {
    fn batch_put_leaves(
        &self, 
        subnet_id: &SubnetId, 
        leaves: &[(&HashValue, &[u8])]
    ) -> MerkleResult<()> {
        let mut store = self.leaves.write().unwrap();
        for (object_id, value) in leaves {
            store.insert((*subnet_id, **object_id), value.to_vec());
        }
        Ok(())
    }

    fn batch_delete_leaves(
        &self, 
        subnet_id: &SubnetId, 
        object_ids: &[&HashValue]
    ) -> MerkleResult<()> {
        let mut store = self.leaves.write().unwrap();
        for object_id in object_ids {
            store.remove(&(*subnet_id, **object_id));
        }
        Ok(())
    }

    fn load_all_leaves(&self, subnet_id: &SubnetId) -> MerkleResult<HashMap<HashValue, Vec<u8>>> {
        let store = self.leaves.read().unwrap();
        let result = store
            .iter()
            .filter(|((sid, _), _)| sid == subnet_id)
            .map(|((_, oid), val)| (*oid, val.clone()))
            .collect();
        Ok(result)
    }

    fn list_subnets(&self) -> MerkleResult<Vec<SubnetId>> {
        let store = self.leaves.read().unwrap();
        let mut subnets: Vec<SubnetId> = store
            .keys()
            .map(|(sid, _)| *sid)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        subnets.sort();
        Ok(subnets)
    }

    fn get_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<Option<Vec<u8>>> {
        let store = self.leaves.read().unwrap();
        Ok(store.get(&(*subnet_id, *object_id)).cloned())
    }

    fn has_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<bool> {
        let store = self.leaves.read().unwrap();
        Ok(store.contains_key(&(*subnet_id, *object_id)))
    }

    fn leaf_count(&self, subnet_id: &SubnetId) -> MerkleResult<usize> {
        let store = self.leaves.read().unwrap();
        let count = store.keys().filter(|(sid, _)| sid == subnet_id).count();
        Ok(count)
    }
}

impl MerkleMetaStore for InMemoryMerkleStore {
    fn register_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()> {
        self.registered_subnets.write().unwrap().insert(*subnet_id);
        Ok(())
    }

    fn unregister_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()> {
        self.registered_subnets.write().unwrap().remove(subnet_id);
        Ok(())
    }

    fn is_subnet_registered(&self, subnet_id: &SubnetId) -> MerkleResult<bool> {
        Ok(self.registered_subnets.read().unwrap().contains(subnet_id))
    }

    fn list_registered_subnets(&self) -> MerkleResult<Vec<SubnetId>> {
        let mut subnets: Vec<_> = self.registered_subnets.read().unwrap().iter().copied().collect();
        subnets.sort();
        Ok(subnets)
    }

    fn set_last_anchor(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<()> {
        self.last_anchors.write().unwrap().insert(*subnet_id, anchor_id);
        Ok(())
    }

    fn get_last_anchor(&self, subnet_id: &SubnetId) -> MerkleResult<Option<AnchorId>> {
        Ok(self.last_anchors.read().unwrap().get(subnet_id).copied())
    }

    fn set_meta(&self, key: &str, value: &[u8]) -> MerkleResult<()> {
        self.meta.write().unwrap().insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn get_meta(&self, key: &str) -> MerkleResult<Option<Vec<u8>>> {
        Ok(self.meta.read().unwrap().get(key).cloned())
    }
}

/// In-memory batch for B4Store implementation
#[derive(Default)]
pub struct InMemoryBatch {
    leaf_upserts: Vec<(SubnetId, HashValue, Vec<u8>)>,
    leaf_deletes: Vec<(SubnetId, HashValue)>,
    subnet_registrations: Vec<SubnetId>,
    last_anchors: Vec<(SubnetId, AnchorId)>,
    subnet_roots: Vec<(SubnetId, AnchorId, HashValue)>,
    global_roots: Vec<(AnchorId, HashValue)>,
}

impl B4Store for InMemoryMerkleStore {
    fn begin_batch(&self) -> MerkleResult<Box<dyn std::any::Any + Send>> {
        Ok(Box::new(InMemoryBatch::default()))
    }

    fn commit_batch(&self, batch: Box<dyn std::any::Any + Send>) -> MerkleResult<()> {
        // Downcast to concrete type
        let batch = batch.downcast::<InMemoryBatch>()
            .map_err(|_| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
            
        // Apply all operations atomically (for in-memory, just apply in sequence)
        // In production RocksDB, this would use WriteBatch::write()
        
        // Leaf upserts
        {
            let mut store = self.leaves.write().unwrap();
            for (subnet_id, object_id, value) in batch.leaf_upserts {
                store.insert((subnet_id, object_id), value);
            }
        }

        // Leaf deletes
        {
            let mut store = self.leaves.write().unwrap();
            for (subnet_id, object_id) in batch.leaf_deletes {
                store.remove(&(subnet_id, object_id));
            }
        }

        // Subnet registrations
        {
            let mut store = self.registered_subnets.write().unwrap();
            for subnet_id in batch.subnet_registrations {
                store.insert(subnet_id);
            }
        }

        // Last anchors
        {
            let mut store = self.last_anchors.write().unwrap();
            for (subnet_id, anchor_id) in batch.last_anchors {
                store.insert(subnet_id, anchor_id);
            }
        }

        // Subnet roots
        {
            let mut store = self.subnet_roots.write().unwrap();
            for (subnet_id, anchor_id, root) in batch.subnet_roots {
                store.insert((subnet_id, anchor_id), root);
            }
        }

        // Global roots
        {
            let mut store = self.global_roots.write().unwrap();
            for (anchor_id, root) in batch.global_roots {
                store.insert(anchor_id, root);
            }
        }

        Ok(())
    }

    fn batch_put_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        leaves: &[(&HashValue, &[u8])],
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        for (object_id, value) in leaves {
            batch.leaf_upserts.push((*subnet_id, **object_id, value.to_vec()));
        }
        Ok(())
    }

    fn batch_delete_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        object_ids: &[&HashValue],
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        for object_id in object_ids {
            batch.leaf_deletes.push((*subnet_id, **object_id));
        }
        Ok(())
    }

    fn batch_register_subnet(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        batch.subnet_registrations.push(*subnet_id);
        Ok(())
    }

    fn batch_set_last_anchor(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        batch.last_anchors.push((*subnet_id, anchor_id));
        Ok(())
    }

    fn batch_put_subnet_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        batch.subnet_roots.push((*subnet_id, anchor_id, *root));
        Ok(())
    }

    fn batch_put_global_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<InMemoryBatch>()
            .ok_or_else(|| crate::error::MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        batch.global_roots.push((anchor_id, *root));
        Ok(())
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
        assert_eq!(store.get_node(&TEST_SUBNET, &hash).unwrap(), Some(node.clone()));

        // Different subnet shouldn't have it
        assert!(!store.has_node(&APP_SUBNET, &hash).unwrap());

        // Delete node
        store.delete_node(&TEST_SUBNET, &hash).unwrap();
        assert!(!store.has_node(&TEST_SUBNET, &hash).unwrap());
    }

    #[test]
    fn test_in_memory_batch_put() {
        let store = InMemoryMerkleStore::new();

        let nodes: Vec<(HashValue, SparseMerkleNode)> = (1..5)
            .map(|i| (make_hash(i), make_leaf_node(i)))
            .collect();

        store.batch_put_nodes(&TEST_SUBNET, &nodes).unwrap();

        assert_eq!(store.node_count(), 4);
        for (hash, node) in &nodes {
            assert_eq!(store.get_node(&TEST_SUBNET, hash).unwrap(), Some(node.clone()));
        }
    }

    #[test]
    fn test_in_memory_root_store() {
        let store = InMemoryMerkleStore::new();

        // Store subnet roots
        store.put_subnet_root(&TEST_SUBNET, 1, &make_hash(10)).unwrap();
        store.put_subnet_root(&TEST_SUBNET, 2, &make_hash(20)).unwrap();
        store.put_subnet_root(&TEST_SUBNET, 3, &make_hash(30)).unwrap();

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
        store.put_subnet_root(&TEST_SUBNET, 1, &make_hash(1)).unwrap();
        store.put_subnet_root(&TEST_SUBNET, 2, &make_hash(2)).unwrap();
        store.put_subnet_root(&TEST_SUBNET, 5, &make_hash(5)).unwrap();
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
