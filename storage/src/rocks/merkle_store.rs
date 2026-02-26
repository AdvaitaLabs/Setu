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
use setu_merkle::storage::{
    AnchorId, B4Store, MerkleLeafStore, MerkleMetaStore, MerkleNodeStore, MerkleRootStore,
    MerkleStore, SubnetId,
};
use setu_merkle::sparse::SparseMerkleNode;
use setu_merkle::HashValue;
use rocksdb::WriteBatch;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

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

/// Key for storing leaf data: (subnet_id, object_id) - B4 scheme
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct LeafKey {
    subnet_id: [u8; 32],
    object_id: [u8; 32],
}

/// Key for subnet registry in MerkleMeta
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct SubnetRegistryKey {
    prefix: u8,  // 0x01 for subnet registry
    subnet_id: [u8; 32],
}

/// Key for last anchor per subnet in MerkleMeta
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct LastAnchorMetaKey {
    prefix: u8,  // 0x02 for last anchor
    subnet_id: [u8; 32],
}

/// Key for global metadata in MerkleMeta
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct GlobalMetaKey {
    prefix: u8,  // 0x03 for global meta
    key_len: u16,
    key_bytes: Vec<u8>,
}

const META_SUBNET_REGISTRY_PREFIX: u8 = 0x01;
const META_LAST_ANCHOR_PREFIX: u8 = 0x02;
const META_GLOBAL_PREFIX: u8 = 0x03;

/// RocksDB-backed implementation of MerkleStore.
///
/// This provides persistent storage for Merkle tree nodes and roots,
/// suitable for production use in the Setu network.
///
/// ## B4 Scheme Support
///
/// This store implements the B4Store trait for atomic batch persistence:
/// - `begin_batch()` creates a RocksDB WriteBatch
/// - `batch_*` methods accumulate operations into the batch
/// - `commit_batch()` writes all operations atomically
///
/// ## Subnet Cache (P1 optimization)
///
/// Maintains an in-memory cache of registered subnet IDs to avoid
/// repeated DB reads during commit operations.
pub struct RocksDBMerkleStore {
    db: Arc<SetuDB>,
    /// In-memory cache of registered subnet IDs (P1 optimization)
    registered_subnets_cache: Arc<RwLock<HashSet<SubnetId>>>,
}

impl RocksDBMerkleStore {
    /// Create a new RocksDB merkle store from an existing database handle.
    pub fn new(db: SetuDB) -> Self {
        let store = Self {
            db: Arc::new(db),
            registered_subnets_cache: Arc::new(RwLock::new(HashSet::new())),
        };
        // Load existing registered subnets into cache
        if let Ok(subnets) = store.list_registered_subnets() {
            let mut cache = store.registered_subnets_cache.write().unwrap();
            for subnet_id in subnets {
                cache.insert(subnet_id);
            }
        }
        store
    }

    /// Create from a shared database handle.
    pub fn from_shared(db: Arc<SetuDB>) -> Self {
        let store = Self {
            db,
            registered_subnets_cache: Arc::new(RwLock::new(HashSet::new())),
        };
        // Load existing registered subnets into cache
        if let Ok(subnets) = store.list_registered_subnets() {
            let mut cache = store.registered_subnets_cache.write().unwrap();
            for subnet_id in subnets {
                cache.insert(subnet_id);
            }
        }
        store
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
        // Use WriteBatch for true atomic batch operation
        let mut batch = self.db.batch();
        for (hash, node) in nodes {
            let key = NodeKey {
                subnet_id: *subnet_id,
                node_hash: Self::hash_to_bytes(hash),
            };
            self.db
                .batch_put(&mut batch, ColumnFamily::MerkleNodes, &key, node)
                .map_err(Self::to_merkle_error)?;
        }
        self.db.write_batch(batch).map_err(Self::to_merkle_error)
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

impl MerkleLeafStore for RocksDBMerkleStore {
    fn batch_put_leaves(
        &self,
        subnet_id: &SubnetId,
        leaves: &[(&HashValue, &[u8])],
    ) -> MerkleResult<()> {
        // Use WriteBatch for true atomic batch operation
        let mut batch = self.db.batch();
        for (object_id, value) in leaves {
            let key = LeafKey {
                subnet_id: *subnet_id,
                object_id: Self::hash_to_bytes(object_id),
            };
            self.db
                .batch_put(&mut batch, ColumnFamily::MerkleLeaves, &key, &value.to_vec())
                .map_err(Self::to_merkle_error)?;
        }
        self.db.write_batch(batch).map_err(Self::to_merkle_error)
    }

    fn batch_delete_leaves(
        &self,
        subnet_id: &SubnetId,
        object_ids: &[&HashValue],
    ) -> MerkleResult<()> {
        // Use WriteBatch for true atomic batch operation
        let mut batch = self.db.batch();
        for object_id in object_ids {
            let key = LeafKey {
                subnet_id: *subnet_id,
                object_id: Self::hash_to_bytes(object_id),
            };
            self.db
                .batch_delete(&mut batch, ColumnFamily::MerkleLeaves, &key)
                .map_err(Self::to_merkle_error)?;
        }
        self.db.write_batch(batch).map_err(Self::to_merkle_error)
    }

    fn load_all_leaves(&self, subnet_id: &SubnetId) -> MerkleResult<HashMap<HashValue, Vec<u8>>> {
        // Use prefix iteration to load all leaves for a subnet
        // The key format is (subnet_id, object_id), so we iterate with subnet_id prefix
        let mut result = HashMap::new();

        // Get an iterator with prefix matching subnet_id
        let prefix = subnet_id.as_slice();
        let iter = self
            .db
            .prefix_iterator(ColumnFamily::MerkleLeaves, prefix)
            .map_err(Self::to_merkle_error)?;

        for item in iter {
            let (key_bytes, value_bytes) = item.map_err(|e| {
                MerkleError::StorageError(format!("Iterator error: {}", e))
            })?;

            // Decode the key to extract object_id
            if key_bytes.len() >= 64 {
                // 32 bytes subnet_id + 32 bytes object_id
                let mut object_id_bytes = [0u8; 32];
                object_id_bytes.copy_from_slice(&key_bytes[32..64]);
                let object_id = Self::bytes_to_hash(object_id_bytes);
                result.insert(object_id, value_bytes.to_vec());
            }
        }

        Ok(result)
    }

    fn list_subnets(&self) -> MerkleResult<Vec<SubnetId>> {
        // Get subnets from the registry instead of scanning leaves
        self.list_registered_subnets()
    }

    fn get_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<Option<Vec<u8>>> {
        let key = LeafKey {
            subnet_id: *subnet_id,
            object_id: Self::hash_to_bytes(object_id),
        };
        self.db
            .get(ColumnFamily::MerkleLeaves, &key)
            .map_err(Self::to_merkle_error)
    }

    fn has_leaf(&self, subnet_id: &SubnetId, object_id: &HashValue) -> MerkleResult<bool> {
        let key = LeafKey {
            subnet_id: *subnet_id,
            object_id: Self::hash_to_bytes(object_id),
        };
        self.db
            .exists(ColumnFamily::MerkleLeaves, &key)
            .map_err(Self::to_merkle_error)
    }

    fn leaf_count(&self, subnet_id: &SubnetId) -> MerkleResult<usize> {
        // Count leaves by iterating (expensive, but accurate)
        let prefix = subnet_id.as_slice();
        let iter = self
            .db
            .prefix_iterator(ColumnFamily::MerkleLeaves, prefix)
            .map_err(Self::to_merkle_error)?;
        
        let mut count = 0;
        for item in iter {
            item.map_err(|e| MerkleError::StorageError(format!("Iterator error: {}", e)))?;
            count += 1;
        }
        Ok(count)
    }
}

impl MerkleMetaStore for RocksDBMerkleStore {
    fn register_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()> {
        let key = SubnetRegistryKey {
            prefix: META_SUBNET_REGISTRY_PREFIX,
            subnet_id: *subnet_id,
        };
        // Store empty value to indicate registration
        self.db
            .put(ColumnFamily::MerkleMeta, &key, &())
            .map_err(Self::to_merkle_error)
    }

    fn unregister_subnet(&self, subnet_id: &SubnetId) -> MerkleResult<()> {
        let key = SubnetRegistryKey {
            prefix: META_SUBNET_REGISTRY_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .delete(ColumnFamily::MerkleMeta, &key)
            .map_err(Self::to_merkle_error)
    }

    fn is_subnet_registered(&self, subnet_id: &SubnetId) -> MerkleResult<bool> {
        let key = SubnetRegistryKey {
            prefix: META_SUBNET_REGISTRY_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .exists(ColumnFamily::MerkleMeta, &key)
            .map_err(Self::to_merkle_error)
    }

    fn list_registered_subnets(&self) -> MerkleResult<Vec<SubnetId>> {
        // Iterate with prefix to find all registered subnets
        let prefix = &[META_SUBNET_REGISTRY_PREFIX];
        let iter = self
            .db
            .prefix_iterator(ColumnFamily::MerkleMeta, prefix)
            .map_err(Self::to_merkle_error)?;

        let mut subnets = Vec::new();
        for item in iter {
            let (key_bytes, _) = item.map_err(|e| {
                MerkleError::StorageError(format!("Iterator error: {}", e))
            })?;

            // Extract subnet_id from key (skip prefix byte)
            if key_bytes.len() >= 33 {
                let mut subnet_id = [0u8; 32];
                subnet_id.copy_from_slice(&key_bytes[1..33]);
                subnets.push(subnet_id);
            }
        }
        subnets.sort();
        Ok(subnets)
    }

    fn set_last_anchor(&self, subnet_id: &SubnetId, anchor_id: AnchorId) -> MerkleResult<()> {
        let key = LastAnchorMetaKey {
            prefix: META_LAST_ANCHOR_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .put(ColumnFamily::MerkleMeta, &key, &anchor_id)
            .map_err(Self::to_merkle_error)
    }

    fn get_last_anchor(&self, subnet_id: &SubnetId) -> MerkleResult<Option<AnchorId>> {
        let key = LastAnchorMetaKey {
            prefix: META_LAST_ANCHOR_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .get(ColumnFamily::MerkleMeta, &key)
            .map_err(Self::to_merkle_error)
    }

    fn set_meta(&self, key: &str, value: &[u8]) -> MerkleResult<()> {
        let meta_key = GlobalMetaKey {
            prefix: META_GLOBAL_PREFIX,
            key_len: key.len() as u16,
            key_bytes: key.as_bytes().to_vec(),
        };
        self.db
            .put(ColumnFamily::MerkleMeta, &meta_key, &value.to_vec())
            .map_err(Self::to_merkle_error)
    }

    fn get_meta(&self, key: &str) -> MerkleResult<Option<Vec<u8>>> {
        let meta_key = GlobalMetaKey {
            prefix: META_GLOBAL_PREFIX,
            key_len: key.len() as u16,
            key_bytes: key.as_bytes().to_vec(),
        };
        self.db
            .get(ColumnFamily::MerkleMeta, &meta_key)
            .map_err(Self::to_merkle_error)
    }
}

/// B4Store implementation for RocksDBMerkleStore.
///
/// This provides true atomic commit capability using RocksDB's WriteBatch.
/// All operations within a single `commit_batch()` call are guaranteed to
/// either all succeed or all fail atomically.
impl B4Store for RocksDBMerkleStore {
    fn begin_batch(&self) -> MerkleResult<Box<dyn std::any::Any + Send>> {
        Ok(Box::new(self.db.batch()))
    }

    fn commit_batch(&self, batch: Box<dyn std::any::Any + Send>) -> MerkleResult<()> {
        let batch = batch.downcast::<WriteBatch>()
            .map_err(|_| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        self.db.write_batch(*batch).map_err(Self::to_merkle_error)
    }

    fn batch_put_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        leaves: &[(&HashValue, &[u8])],
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        for (object_id, value) in leaves {
            let key = LeafKey {
                subnet_id: *subnet_id,
                object_id: Self::hash_to_bytes(object_id),
            };
            self.db
                .batch_put(batch, ColumnFamily::MerkleLeaves, &key, &value.to_vec())
                .map_err(Self::to_merkle_error)?;
        }
        Ok(())
    }

    fn batch_delete_leaves_to_batch(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        object_ids: &[&HashValue],
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        for object_id in object_ids {
            let key = LeafKey {
                subnet_id: *subnet_id,
                object_id: Self::hash_to_bytes(object_id),
            };
            self.db
                .batch_delete(batch, ColumnFamily::MerkleLeaves, &key)
                .map_err(Self::to_merkle_error)?;
        }
        Ok(())
    }

    fn batch_register_subnet(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
    ) -> MerkleResult<()> {
        // Check cache first to avoid redundant writes.
        //
        // Cache consistency note:
        // - If commit_batch() fails after cache update, cache and DB may be inconsistent.
        // - This is acceptable because:
        //   1. Node typically crashes/restarts on commit failure
        //   2. On restart, cache is reloaded from DB (see new()/from_shared())
        //   3. Redundant registration writes are idempotent
        {
            let cache = self.registered_subnets_cache.read().unwrap();
            if cache.contains(subnet_id) {
                return Ok(());
            }
        }

        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;

        // Add to batch
        let key = SubnetRegistryKey {
            prefix: META_SUBNET_REGISTRY_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .batch_put(batch, ColumnFamily::MerkleMeta, &key, &())
            .map_err(Self::to_merkle_error)?;

        // Update cache optimistically.
        // If commit fails, next commit retry will skip this (already in cache),
        // but the subnet registration is already in the new batch from the caller.
        {
            let mut cache = self.registered_subnets_cache.write().unwrap();
            cache.insert(*subnet_id);
        }

        Ok(())
    }

    fn batch_set_last_anchor(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        let key = LastAnchorMetaKey {
            prefix: META_LAST_ANCHOR_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .batch_put(batch, ColumnFamily::MerkleMeta, &key, &anchor_id)
            .map_err(Self::to_merkle_error)
    }

    fn batch_put_subnet_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        subnet_id: &SubnetId,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        let key = RootKey {
            subnet_id: *subnet_id,
            anchor_id,
        };
        let root_bytes = Self::hash_to_bytes(root);
        self.db
            .batch_put(batch, ColumnFamily::MerkleRoots, &key, &root_bytes)
            .map_err(Self::to_merkle_error)?;

        // Also update the latest anchor key
        let latest_key = LatestAnchorKey {
            prefix: LATEST_SUBNET_PREFIX,
            subnet_id: *subnet_id,
        };
        self.db
            .batch_put(batch, ColumnFamily::MerkleRoots, &latest_key, &anchor_id)
            .map_err(Self::to_merkle_error)
    }

    fn batch_put_global_root(
        &self,
        batch: &mut Box<dyn std::any::Any + Send>,
        anchor_id: AnchorId,
        root: &HashValue,
    ) -> MerkleResult<()> {
        let batch = batch.downcast_mut::<WriteBatch>()
            .ok_or_else(|| MerkleError::InvalidInput("Invalid batch type".to_string()))?;
        let key = GlobalRootKey::new(anchor_id);
        let root_bytes = Self::hash_to_bytes(root);
        self.db
            .batch_put(batch, ColumnFamily::MerkleRoots, &key, &root_bytes)
            .map_err(Self::to_merkle_error)?;

        // Also update the latest global anchor key
        let latest_key = LatestAnchorKey {
            prefix: LATEST_GLOBAL_PREFIX,
            subnet_id: [0u8; 32],
        };
        self.db
            .batch_put(batch, ColumnFamily::MerkleRoots, &latest_key, &anchor_id)
            .map_err(Self::to_merkle_error)
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
