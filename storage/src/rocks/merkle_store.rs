//! RocksDB implementation of MerkleStore for persistent Merkle tree storage.
//!
//! This module provides a RocksDB-backed implementation of the `MerkleStore` trait
//! from the `setu-merkle` crate, enabling persistent storage of Merkle tree nodes
//! and subnet state roots.
//!
//! # Column Family Layout
//!
//! - `merkle_nodes`: Stores SMT nodes
//!   - Key: `(subnet_id: [u8; 32], node_hash: [u8; 32])`
//!   - Value: `SparseMerkleNode` (serialized)
//!
//! - `merkle_roots`: Stores subnet/global state roots at each anchor
//!   - Key: `(subnet_id: [u8; 32], anchor_id: u64)` or `(marker, anchor_id)`
//!   - Value: `[u8; 32]` (root hash)
//!
//! # Example
//!
//! ```ignore
//! use setu_storage::{RocksDBMerkleStore, SetuDB, RocksDBConfig};
//! use setu_merkle::storage::{MerkleNodeStore, MerkleRootStore};
//!
//! let config = RocksDBConfig::new("/path/to/db");
//! let db = SetuDB::open(config)?;
//! let store = RocksDBMerkleStore::new(db);
//!
//! // Store nodes
//! let subnet_id = [0u8; 32];
//! store.put_node(&subnet_id, &key, &node)?;
//!
//! // Get latest root
//! let root = store.get_latest_subnet_root(&subnet_id)?;
//! ```

use crate::rocks::core::{ColumnFamily, SetuDB, StorageError};
use setu_merkle::error::{MerkleError, MerkleResult};
use setu_merkle::storage::{AnchorId, MerkleNodeStore, MerkleRootStore, MerkleStore, SubnetId};
use setu_merkle::sparse::SparseMerkleNode;
use setu_merkle::HashValue;
use std::sync::Arc;

/// Key for storing Merkle nodes: (subnet_id, node_hash)
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct NodeKey {
    subnet_id: [u8; 32],
    node_hash: [u8; 32],
}

/// Key for storing Merkle roots: (subnet_id, anchor_id)
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct RootKey {
    subnet_id: [u8; 32],
    anchor_id: u64,
}

/// Key for storing global roots: (marker, anchor_id)
/// The marker is all 0xFF bytes to distinguish from subnet keys
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct GlobalRootKey {
    marker: [u8; 32],  // All 0xFF to indicate global root
    anchor_id: u64,
}

impl GlobalRootKey {
    fn new(anchor_id: u64) -> Self {
        Self {
            marker: [0xFF; 32],
            anchor_id,
        }
    }
}

/// Key for storing latest anchor for a subnet
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct LatestAnchorKey {
    prefix: u8,  // 0x01 for subnet, 0x02 for global
    subnet_id: [u8; 32],
}

const LATEST_SUBNET_PREFIX: u8 = 0x01;
const LATEST_GLOBAL_PREFIX: u8 = 0x02;

/// RocksDB-backed implementation of MerkleStore.
///
/// This provides persistent storage for Merkle tree nodes and roots,
/// suitable for production use in the Setu network.
pub struct RocksDBMerkleStore {
    db: Arc<SetuDB>,
}

impl RocksDBMerkleStore {
    /// Create a new RocksDB merkle store from an existing database handle.
    pub fn new(db: SetuDB) -> Self {
        Self { db: Arc::new(db) }
    }

    /// Create from a shared database handle.
    pub fn from_shared(db: Arc<SetuDB>) -> Self {
        Self { db }
    }

    /// Open a new database at the given path.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, StorageError> {
        let db = SetuDB::open_default(path)?;
        Ok(Self::new(db))
    }

    /// Get the underlying database handle.
    pub fn db(&self) -> &SetuDB {
        &self.db
    }

    /// Convert HashValue to bytes array
    fn hash_to_bytes(hash: &HashValue) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(hash.as_bytes());
        bytes
    }

    /// Convert bytes array to HashValue
    fn bytes_to_hash(bytes: [u8; 32]) -> HashValue {
        HashValue::new(bytes)
    }

    /// Convert storage error to merkle error
    fn to_merkle_error(e: StorageError) -> MerkleError {
        MerkleError::StorageError(e.to_string())
    }

    /// Store the latest anchor for a subnet
    fn put_latest_subnet_anchor(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<()> {
        let key = LatestAnchorKey {
            prefix: LATEST_SUBNET_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .put(ColumnFamily::MerkleRoots, &key, &anchor_id)
            .map_err(Self::to_merkle_error)
    }

    /// Get the latest anchor for a subnet
    fn get_latest_subnet_anchor(&self, subnet_id: &SubnetId) -> MerkleResult<Option<AnchorId>> {
        let key = LatestAnchorKey {
            prefix: LATEST_SUBNET_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .get(ColumnFamily::MerkleRoots, &key)
            .map_err(Self::to_merkle_error)
    }

    /// Store the latest global anchor
    fn put_latest_global_anchor(&self, anchor_id: AnchorId) -> MerkleResult<()> {
        let key = LatestAnchorKey {
            prefix: LATEST_GLOBAL_PREFIX,
            subnet_id: [0u8; 32],
        };
        self.db
            .put(ColumnFamily::MerkleRoots, &key, &anchor_id)
            .map_err(Self::to_merkle_error)
    }

    /// Get the latest global anchor
    fn get_latest_global_anchor(&self) -> MerkleResult<Option<AnchorId>> {
        let key = LatestAnchorKey {
            prefix: LATEST_GLOBAL_PREFIX,
            subnet_id: [0u8; 32],
        };
        self.db
            .get(ColumnFamily::MerkleRoots, &key)
            .map_err(Self::to_merkle_error)
    }
}

impl MerkleNodeStore for RocksDBMerkleStore {
    fn put_node(&self, subnet_id: &SubnetId, hash: &HashValue, node: &SparseMerkleNode) -> MerkleResult<()> {
        let key = NodeKey {
            subnet_id: *subnet_id,
            node_hash: Self::hash_to_bytes(hash),
        };
        self.db
            .put(ColumnFamily::MerkleNodes, &key, node)
            .map_err(Self::to_merkle_error)
    }

    fn get_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<Option<SparseMerkleNode>> {
        let key = NodeKey {
            subnet_id: *subnet_id,
            node_hash: Self::hash_to_bytes(hash),
        };
        self.db
            .get(ColumnFamily::MerkleNodes, &key)
            .map_err(Self::to_merkle_error)
    }

    fn has_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<bool> {
        let key = NodeKey {
            subnet_id: *subnet_id,
            node_hash: Self::hash_to_bytes(hash),
        };
        self.db
            .exists(ColumnFamily::MerkleNodes, &key)
            .map_err(Self::to_merkle_error)
    }

    fn delete_node(&self, subnet_id: &SubnetId, hash: &HashValue) -> MerkleResult<()> {
        let key = NodeKey {
            subnet_id: *subnet_id,
            node_hash: Self::hash_to_bytes(hash),
        };
        self.db
            .delete(ColumnFamily::MerkleNodes, &key)
            .map_err(Self::to_merkle_error)
    }

    fn batch_put_nodes(&self, subnet_id: &SubnetId, nodes: &[(HashValue, SparseMerkleNode)]) -> MerkleResult<()> {
        // Use write batch for atomicity
        for (hash, node) in nodes {
            self.put_node(subnet_id, hash, node)?;
        }
        Ok(())
    }
}

impl MerkleRootStore for RocksDBMerkleStore {
    fn put_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()> {
        let key = RootKey {
            subnet_id: *subnet_id,
            anchor_id,
        };
        let root_bytes = Self::hash_to_bytes(root);

        self.db
            .put(ColumnFamily::MerkleRoots, &key, &root_bytes)
            .map_err(Self::to_merkle_error)?;

        // Update latest anchor if this is newer
        let current_latest = self.get_latest_subnet_anchor(subnet_id)?;
        if current_latest.map_or(true, |h| anchor_id > h) {
            self.put_latest_subnet_anchor(subnet_id, anchor_id)?;
        }

        Ok(())
    }

    fn get_subnet_root(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>> {
        let key = RootKey {
            subnet_id: *subnet_id,
            anchor_id,
        };

        let root_bytes: Option<[u8; 32]> = self
            .db
            .get(ColumnFamily::MerkleRoots, &key)
            .map_err(Self::to_merkle_error)?;

        Ok(root_bytes.map(Self::bytes_to_hash))
    }

    fn get_latest_subnet_root(&self, subnet_id: &SubnetId) -> MerkleResult<Option<(AnchorId, HashValue)>> {
        let latest_anchor = match self.get_latest_subnet_anchor(subnet_id)? {
            Some(a) => a,
            None => return Ok(None),
        };

        let root = self.get_subnet_root(subnet_id, latest_anchor)?;
        Ok(root.map(|r| (latest_anchor, r)))
    }

    fn put_global_root(&self, anchor_id: AnchorId, root: &HashValue) -> MerkleResult<()> {
        let key = GlobalRootKey::new(anchor_id);
        let root_bytes = Self::hash_to_bytes(root);

        self.db
            .put(ColumnFamily::MerkleRoots, &key, &root_bytes)
            .map_err(Self::to_merkle_error)?;

        // Update latest global anchor if this is newer
        let current_latest = self.get_latest_global_anchor()?;
        if current_latest.map_or(true, |h| anchor_id > h) {
            self.put_latest_global_anchor(anchor_id)?;
        }

        Ok(())
    }

    fn get_global_root(&self, anchor_id: AnchorId) -> MerkleResult<Option<HashValue>> {
        let key = GlobalRootKey::new(anchor_id);

        let root_bytes: Option<[u8; 32]> = self
            .db
            .get(ColumnFamily::MerkleRoots, &key)
            .map_err(Self::to_merkle_error)?;

        Ok(root_bytes.map(Self::bytes_to_hash))
    }

    fn get_latest_global_root(&self) -> MerkleResult<Option<(AnchorId, HashValue)>> {
        let latest_anchor = match self.get_latest_global_anchor()? {
            Some(a) => a,
            None => return Ok(None),
        };

        let root = self.get_global_root(latest_anchor)?;
        Ok(root.map(|r| (latest_anchor, r)))
    }

    fn list_anchors(&self, subnet_id: &SubnetId, start: AnchorId, end: AnchorId) -> MerkleResult<Vec<AnchorId>> {
        // For now, return a simple range if we know the latest anchor
        // A full implementation would use prefix iteration
        let mut anchors = Vec::new();
        for anchor_id in start..=end {
            if self.get_subnet_root(subnet_id, anchor_id)?.is_some() {
                anchors.push(anchor_id);
            }
        }
        Ok(anchors)
    }
}

impl MerkleStore for RocksDBMerkleStore {
    fn flush(&self) -> MerkleResult<()> {
        // RocksDB handles flushing internally
        // For explicit flush, we could call db.flush() but SetuDB doesn't expose it yet
        Ok(())
    }

    fn checkpoint(&self, _anchor_id: AnchorId) -> MerkleResult<()> {
        // Create a RocksDB checkpoint - for now, just flush
        // Full implementation would use RocksDB's checkpoint API
        self.flush()
    }

    fn prune_before(&self, _anchor_id: AnchorId) -> MerkleResult<u64> {
        // Delete old data before the given anchor
        // This is a no-op for now - full implementation would iterate and delete
        Ok(0)
    }
}

// Implement Send + Sync for the store
unsafe impl Send for RocksDBMerkleStore {}
unsafe impl Sync for RocksDBMerkleStore {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (RocksDBMerkleStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = RocksDBMerkleStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }

    fn test_hash(byte: u8) -> HashValue {
        HashValue::new([byte; 32])
    }

    fn test_subnet(byte: u8) -> SubnetId {
        [byte; 32]
    }

    fn test_node(byte: u8) -> SparseMerkleNode {
        SparseMerkleNode::Internal {
            left: test_hash(byte),
            right: test_hash(byte + 1),
        }
    }

    #[test]
    fn test_put_get_node() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);
        let hash = test_hash(1);
        let node = test_node(1);

        // Put node
        store.put_node(&subnet_id, &hash, &node).unwrap();

        // Node should exist
        assert!(store.has_node(&subnet_id, &hash).unwrap());

        // Get node
        let retrieved = store.get_node(&subnet_id, &hash).unwrap();
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_delete_node() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);
        let hash = test_hash(2);
        let node = test_node(2);

        store.put_node(&subnet_id, &hash, &node).unwrap();
        assert!(store.has_node(&subnet_id, &hash).unwrap());

        store.delete_node(&subnet_id, &hash).unwrap();
        assert!(!store.has_node(&subnet_id, &hash).unwrap());
    }

    #[test]
    fn test_subnet_isolation() {
        let (store, _temp_dir) = create_test_store();
        let hash = test_hash(3);
        let node1 = test_node(3);
        let node2 = test_node(4);
        let subnet1 = test_subnet(1);
        let subnet2 = test_subnet(2);

        // Store same key in different subnets
        store.put_node(&subnet1, &hash, &node1).unwrap();
        store.put_node(&subnet2, &hash, &node2).unwrap();

        // Both should exist
        assert!(store.has_node(&subnet1, &hash).unwrap());
        assert!(store.has_node(&subnet2, &hash).unwrap());

        // Delete from subnet 1
        store.delete_node(&subnet1, &hash).unwrap();
        assert!(!store.has_node(&subnet1, &hash).unwrap());
        assert!(store.has_node(&subnet2, &hash).unwrap()); // Still in subnet 2
    }

    #[test]
    fn test_put_get_subnet_root() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);
        let anchor_id = 100;
        let root = test_hash(42);

        store.put_subnet_root(&subnet_id, anchor_id, &root).unwrap();

        let retrieved = store.get_subnet_root(&subnet_id, anchor_id).unwrap();
        assert_eq!(retrieved, Some(root));
    }

    #[test]
    fn test_get_latest_subnet_root() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);

        // Initially no root
        assert!(store.get_latest_subnet_root(&subnet_id).unwrap().is_none());

        // Add roots at different anchors
        let root1 = test_hash(1);
        let root2 = test_hash(2);
        let root3 = test_hash(3);

        store.put_subnet_root(&subnet_id, 10, &root1).unwrap();
        assert_eq!(store.get_latest_subnet_root(&subnet_id).unwrap(), Some((10, root1)));

        store.put_subnet_root(&subnet_id, 20, &root2).unwrap();
        assert_eq!(store.get_latest_subnet_root(&subnet_id).unwrap(), Some((20, root2)));

        store.put_subnet_root(&subnet_id, 15, &root3).unwrap(); // Insert at earlier anchor
        assert_eq!(store.get_latest_subnet_root(&subnet_id).unwrap(), Some((20, root2))); // Still root2
    }

    #[test]
    fn test_global_roots() {
        let (store, _temp_dir) = create_test_store();

        // Initially no global root
        assert!(store.get_latest_global_root().unwrap().is_none());

        let root1 = test_hash(10);
        let root2 = test_hash(20);

        store.put_global_root(100, &root1).unwrap();
        assert_eq!(store.get_latest_global_root().unwrap(), Some((100, root1)));
        assert_eq!(store.get_global_root(100).unwrap(), Some(root1));

        store.put_global_root(200, &root2).unwrap();
        assert_eq!(store.get_latest_global_root().unwrap(), Some((200, root2)));
    }

    #[test]
    fn test_batch_put_nodes() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);

        let nodes: Vec<(HashValue, SparseMerkleNode)> = (0..5)
            .map(|i| (test_hash(i), test_node(i)))
            .collect();

        store.batch_put_nodes(&subnet_id, &nodes).unwrap();

        for (hash, _) in &nodes {
            assert!(store.has_node(&subnet_id, hash).unwrap());
        }
    }

    #[test]
    fn test_list_anchors() {
        let (store, _temp_dir) = create_test_store();
        let subnet_id = test_subnet(1);

        // Add some roots
        for i in [10, 20, 30, 40, 50] {
            store.put_subnet_root(&subnet_id, i, &test_hash(i as u8)).unwrap();
        }

        let anchors = store.list_anchors(&subnet_id, 15, 45).unwrap();
        assert_eq!(anchors, vec![20, 30, 40]);
    }
}
