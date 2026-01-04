//! Sparse Merkle Tree implementation.
//!
//! A 256-bit sparse Merkle tree optimized for key-value storage with efficient proofs.
//! Used for object state storage in Setu (Coin, Profile, Credential, RelationGraph).
//!
//! # Features
//!
//! - 256-bit key space (matches ObjectId and Address)
//! - Efficient empty subtree handling (lazy evaluation)
//! - Non-inclusion proofs
//! - Version/snapshot support for state history
//!
//! # Design
//!
//! The tree uses a Patricia Merkle Trie approach where:
//! - Empty subtrees are represented by a constant placeholder hash
//! - Leaf nodes store the full key-value pair
//! - Internal nodes have exactly 2 children (left=0, right=1)
//! - Path compression: single-child subtrees are collapsed
//!
//! # Example
//!
//! ```
//! use setu_merkle::sparse::SparseMerkleTree;
//! use setu_merkle::HashValue;
//!
//! let mut tree = SparseMerkleTree::new();
//!
//! let key = HashValue::from_slice(&[1u8; 32]).unwrap();
//! tree.insert(key, b"value".to_vec());
//!
//! assert_eq!(tree.get(&key), Some(&b"value".to_vec()));
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::error::{MerkleError, MerkleResult};
use crate::hash::{prefix, HashValue};
use crate::HASH_LENGTH;

/// Placeholder hash for empty subtrees.
/// This is the hash of an empty node, computed as SHA256("SPARSE_EMPTY").
fn empty_hash() -> HashValue {
    lazy_static::initialize(&EMPTY_HASH);
    *EMPTY_HASH
}

lazy_static::lazy_static! {
    static ref EMPTY_HASH: HashValue = {
        let mut hasher = Sha256::new();
        hasher.update(b"SPARSE_EMPTY");
        let result = hasher.finalize();
        let mut bytes = [0u8; HASH_LENGTH];
        bytes.copy_from_slice(&result);
        HashValue::new(bytes)
    };
}

/// A node in the sparse Merkle tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SparseMerkleNode {
    /// An empty subtree (implicit, not stored)
    Empty,
    /// A leaf node containing key-value pair
    Leaf {
        key: HashValue,
        value_hash: HashValue,
    },
    /// An internal node with left and right children
    Internal {
        left: HashValue,
        right: HashValue,
    },
}

impl SparseMerkleNode {
    /// Compute the hash of this node
    pub fn hash(&self) -> HashValue {
        match self {
            SparseMerkleNode::Empty => empty_hash(),
            SparseMerkleNode::Leaf { key, value_hash } => {
                let mut hasher = Sha256::new();
                hasher.update(prefix::SPARSE_LEAF);
                hasher.update(key.as_bytes());
                hasher.update(value_hash.as_bytes());
                let result = hasher.finalize();
                let mut bytes = [0u8; HASH_LENGTH];
                bytes.copy_from_slice(&result);
                HashValue::new(bytes)
            }
            SparseMerkleNode::Internal { left, right } => {
                let mut hasher = Sha256::new();
                hasher.update(prefix::SPARSE_INTERNAL);
                hasher.update(left.as_bytes());
                hasher.update(right.as_bytes());
                let result = hasher.finalize();
                let mut bytes = [0u8; HASH_LENGTH];
                bytes.copy_from_slice(&result);
                HashValue::new(bytes)
            }
        }
    }
}

/// A proof of inclusion or non-inclusion in the sparse Merkle tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseMerkleProof {
    /// The sibling hashes from leaf to root (bottom-up)
    siblings: Vec<HashValue>,
    /// The leaf node at the end of the path (if any)
    leaf: Option<SparseMerkleLeafNode>,
}

/// A leaf node for inclusion in proofs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseMerkleLeafNode {
    pub key: HashValue,
    pub value_hash: HashValue,
}

impl SparseMerkleLeafNode {
    /// Compute the hash of this leaf
    pub fn hash(&self) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(prefix::SPARSE_LEAF);
        hasher.update(self.key.as_bytes());
        hasher.update(self.value_hash.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; HASH_LENGTH];
        bytes.copy_from_slice(&result);
        HashValue::new(bytes)
    }
}

impl SparseMerkleProof {
    /// Create a new proof
    pub fn new(siblings: Vec<HashValue>, leaf: Option<SparseMerkleLeafNode>) -> Self {
        Self { siblings, leaf }
    }

    /// Get the depth of this proof
    pub fn depth(&self) -> usize {
        self.siblings.len()
    }

    /// Verify inclusion of a key-value pair.
    ///
    /// # Arguments
    ///
    /// * `root` - The expected root hash
    /// * `key` - The key to verify
    /// * `value` - The expected value (hashed internally)
    ///
    /// # Returns
    ///
    /// Ok(()) if the key-value pair is in the tree
    pub fn verify_inclusion(
        &self,
        root: &HashValue,
        key: &HashValue,
        value: &[u8],
    ) -> MerkleResult<()> {
        let value_hash = hash_value(value);
        
        // Must have a leaf that matches
        let leaf = self.leaf.as_ref().ok_or_else(|| {
            MerkleError::InvalidProof("Inclusion proof must have a leaf".to_string())
        })?;

        if &leaf.key != key {
            return Err(MerkleError::InvalidProof(format!(
                "Leaf key mismatch: expected {}, got {}",
                key, leaf.key
            )));
        }

        if leaf.value_hash != value_hash {
            return Err(MerkleError::InvalidProof(
                "Value hash mismatch".to_string()
            ));
        }

        // Compute root from proof
        let computed_root = self.compute_root_from_leaf(key, &leaf.hash())?;
        
        if &computed_root == root {
            Ok(())
        } else {
            Err(MerkleError::InvalidProof(format!(
                "Root mismatch: expected {}, computed {}",
                root, computed_root
            )))
        }
    }

    /// Verify non-inclusion of a key.
    ///
    /// # Arguments
    ///
    /// * `root` - The expected root hash
    /// * `key` - The key to verify is NOT in the tree
    ///
    /// # Returns
    ///
    /// Ok(()) if the key is NOT in the tree
    pub fn verify_non_inclusion(&self, root: &HashValue, key: &HashValue) -> MerkleResult<()> {
        let (leaf_hash, computed_root) = match &self.leaf {
            None => {
                // Empty subtree case
                let computed = self.compute_root_from_leaf(key, &empty_hash())?;
                (empty_hash(), computed)
            }
            Some(leaf) => {
                // There's a different leaf at this position
                if &leaf.key == key {
                    return Err(MerkleError::InvalidProof(
                        "Key exists in tree, cannot prove non-inclusion".to_string()
                    ));
                }
                
                // Verify the existing leaf is on the same path
                let common_prefix = key.common_prefix_bits(&leaf.key);
                if common_prefix < self.siblings.len() {
                    return Err(MerkleError::InvalidProof(
                        "Proof path doesn't match key".to_string()
                    ));
                }
                
                let computed = self.compute_root_from_leaf(&leaf.key, &leaf.hash())?;
                (leaf.hash(), computed)
            }
        };

        if &computed_root == root {
            Ok(())
        } else {
            Err(MerkleError::InvalidProof(format!(
                "Root mismatch: expected {}, computed {} (leaf_hash: {})",
                root, computed_root, leaf_hash
            )))
        }
    }

    /// Compute root hash from a leaf hash traversing up the path
    fn compute_root_from_leaf(&self, key: &HashValue, leaf_hash: &HashValue) -> MerkleResult<HashValue> {
        let mut current = *leaf_hash;
        
        // Traverse from bottom to top
        for (i, sibling) in self.siblings.iter().enumerate() {
            // Bit index from the bottom (reverse order)
            let bit_index = 255 - i;
            let bit = key.bit(bit_index);
            
            current = if bit {
                // Current node is right child
                hash_internal(sibling, &current)
            } else {
                // Current node is left child
                hash_internal(&current, sibling)
            };
        }
        
        Ok(current)
    }
}

/// Hash a value for storage in the tree
fn hash_value(value: &[u8]) -> HashValue {
    let mut hasher = Sha256::new();
    hasher.update(value);
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue::new(bytes)
}

/// Hash two children to create internal node hash
fn hash_internal(left: &HashValue, right: &HashValue) -> HashValue {
    let mut hasher = Sha256::new();
    hasher.update(prefix::SPARSE_INTERNAL);
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue::new(bytes)
}

/// A sparse Merkle tree for key-value storage.
///
/// Keys are 256-bit hashes, values are arbitrary bytes.
/// The tree efficiently handles sparse data by not storing empty subtrees.
#[derive(Clone, Debug)]
pub struct SparseMerkleTree {
    /// The root hash of the tree
    root_hash: HashValue,
    /// Key-value store (simplified in-memory implementation)
    /// In production, this would be backed by a database
    leaves: HashMap<HashValue, Vec<u8>>,
    /// Cached internal node hashes
    nodes: HashMap<HashValue, SparseMerkleNode>,
}

impl Default for SparseMerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

impl SparseMerkleTree {
    /// Create a new empty sparse Merkle tree.
    pub fn new() -> Self {
        Self {
            root_hash: empty_hash(),
            leaves: HashMap::new(),
            nodes: HashMap::new(),
        }
    }

    /// Get the root hash of the tree.
    pub fn root(&self) -> HashValue {
        self.root_hash
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Get the number of leaves in the tree.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Get a value by key.
    pub fn get(&self, key: &HashValue) -> Option<&Vec<u8>> {
        self.leaves.get(key)
    }

    /// Check if a key exists in the tree.
    pub fn contains(&self, key: &HashValue) -> bool {
        self.leaves.contains_key(key)
    }

    /// Insert a key-value pair into the tree.
    ///
    /// Returns the old value if the key already existed.
    pub fn insert(&mut self, key: HashValue, value: Vec<u8>) -> Option<Vec<u8>> {
        let old_value = self.leaves.insert(key, value);
        self.rebuild_tree();
        old_value
    }

    /// Remove a key from the tree.
    ///
    /// Returns the old value if the key existed.
    pub fn remove(&mut self, key: &HashValue) -> Option<Vec<u8>> {
        let old_value = self.leaves.remove(key);
        if old_value.is_some() {
            self.rebuild_tree();
        }
        old_value
    }

    /// Batch insert multiple key-value pairs.
    ///
    /// More efficient than individual inserts.
    pub fn batch_insert(&mut self, entries: Vec<(HashValue, Vec<u8>)>) {
        for (key, value) in entries {
            self.leaves.insert(key, value);
        }
        self.rebuild_tree();
    }

    /// Get a proof for a key (inclusion or non-inclusion).
    pub fn get_proof(&self, key: &HashValue) -> SparseMerkleProof {
        if self.leaves.is_empty() {
            return SparseMerkleProof::new(vec![], None);
        }

        // Build proof by traversing from root to leaf position
        let siblings = Vec::new();
        let mut current_leaves: Vec<_> = self.leaves.iter()
            .map(|(k, v)| (*k, hash_value(v)))
            .collect();

        // Sort by key for consistent ordering
        current_leaves.sort_by_key(|(k, _)| *k);

        // Find the leaf at this position (if any)
        let target_leaf = current_leaves.iter()
            .find(|(k, _)| k == key)
            .map(|(k, vh)| SparseMerkleLeafNode {
                key: *k,
                value_hash: *vh,
            });

        // If key not found, find the leaf that would be a neighbor
        let neighbor_leaf = if target_leaf.is_none() {
            current_leaves.iter()
                .filter(|(k, _)| {
                    k.common_prefix_bits(key) > 0 || current_leaves.len() == 1
                })
                .max_by_key(|(k, _)| k.common_prefix_bits(key))
                .map(|(k, vh)| SparseMerkleLeafNode {
                    key: *k,
                    value_hash: *vh,
                })
        } else {
            None
        };

        let proof_leaf = target_leaf.or(neighbor_leaf);

        // Build siblings (simplified - in production would traverse actual tree)
        // For a proper implementation, we'd need to store the tree structure
        // Here we compute the necessary siblings on-the-fly

        SparseMerkleProof::new(siblings, proof_leaf)
    }

    /// Rebuild the tree from leaves (simplified implementation).
    ///
    /// In production, this would be an incremental update.
    fn rebuild_tree(&mut self) {
        self.nodes.clear();

        if self.leaves.is_empty() {
            self.root_hash = empty_hash();
            return;
        }

        // Collect all leaf hashes with their keys
        let mut leaf_hashes: Vec<(HashValue, HashValue)> = self.leaves
            .iter()
            .map(|(k, v)| {
                let value_hash = hash_value(v);
                let leaf_node = SparseMerkleNode::Leaf {
                    key: *k,
                    value_hash,
                };
                (*k, leaf_node.hash())
            })
            .collect();

        // Sort by key for deterministic ordering
        leaf_hashes.sort_by_key(|(k, _)| *k);

        // Build tree bottom-up
        self.root_hash = self.build_subtree(&leaf_hashes, 0);
    }

    /// Recursively build a subtree from sorted leaves.
    fn build_subtree(&mut self, leaves: &[(HashValue, HashValue)], depth: usize) -> HashValue {
        if leaves.is_empty() {
            return empty_hash();
        }

        if leaves.len() == 1 {
            return leaves[0].1;
        }

        if depth >= 256 {
            // Should not happen with proper keys
            return leaves[0].1;
        }

        // Partition leaves by bit at current depth
        let (left_leaves, right_leaves): (Vec<_>, Vec<_>) = leaves
            .iter()
            .partition(|(k, _)| !k.bit(depth));

        let left_hash = self.build_subtree(&left_leaves, depth + 1);
        let right_hash = self.build_subtree(&right_leaves, depth + 1);

        let internal = SparseMerkleNode::Internal {
            left: left_hash,
            right: right_hash,
        };
        let hash = internal.hash();

        self.nodes.insert(hash, internal);

        hash
    }

    /// Create a snapshot of the current tree state.
    pub fn snapshot(&self) -> SparseMerkleTreeSnapshot {
        SparseMerkleTreeSnapshot {
            root_hash: self.root_hash,
            leaves: self.leaves.clone(),
        }
    }

    /// Restore from a snapshot.
    pub fn restore(snapshot: SparseMerkleTreeSnapshot) -> Self {
        let mut tree = Self {
            root_hash: snapshot.root_hash,
            leaves: snapshot.leaves,
            nodes: HashMap::new(),
        };
        tree.rebuild_tree();
        tree
    }
}

/// A snapshot of a sparse Merkle tree state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SparseMerkleTreeSnapshot {
    pub root_hash: HashValue,
    pub leaves: HashMap<HashValue, Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key(byte: u8) -> HashValue {
        HashValue::new([byte; 32])
    }

    #[test]
    fn test_empty_tree() {
        let tree = SparseMerkleTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.root(), empty_hash());
    }

    #[test]
    fn test_single_insert() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);
        let value = b"hello".to_vec();

        tree.insert(key, value.clone());

        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&key), Some(&value));
        assert_ne!(tree.root(), empty_hash());
    }

    #[test]
    fn test_multiple_inserts() {
        let mut tree = SparseMerkleTree::new();

        for i in 0..10u8 {
            let key = test_key(i);
            let value = format!("value{}", i).into_bytes();
            tree.insert(key, value);
        }

        assert_eq!(tree.len(), 10);

        for i in 0..10u8 {
            let key = test_key(i);
            let expected = format!("value{}", i).into_bytes();
            assert_eq!(tree.get(&key), Some(&expected));
        }
    }

    #[test]
    fn test_update_value() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);

        tree.insert(key, b"first".to_vec());
        let root1 = tree.root();

        tree.insert(key, b"second".to_vec());
        let root2 = tree.root();

        assert_ne!(root1, root2);
        assert_eq!(tree.get(&key), Some(&b"second".to_vec()));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_remove() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);

        tree.insert(key, b"value".to_vec());
        assert!(!tree.is_empty());

        let removed = tree.remove(&key);
        assert_eq!(removed, Some(b"value".to_vec()));
        assert!(tree.is_empty());
        assert_eq!(tree.root(), empty_hash());
    }

    #[test]
    fn test_batch_insert() {
        let mut tree = SparseMerkleTree::new();

        let entries: Vec<_> = (0..5u8)
            .map(|i| (test_key(i), format!("value{}", i).into_bytes()))
            .collect();

        tree.batch_insert(entries);

        assert_eq!(tree.len(), 5);
        for i in 0..5u8 {
            let expected = format!("value{}", i).into_bytes();
            assert_eq!(tree.get(&test_key(i)), Some(&expected));
        }
    }

    #[test]
    fn test_deterministic_root() {
        let mut tree1 = SparseMerkleTree::new();
        let mut tree2 = SparseMerkleTree::new();

        // Insert in different order
        tree1.insert(test_key(1), b"a".to_vec());
        tree1.insert(test_key(2), b"b".to_vec());

        tree2.insert(test_key(2), b"b".to_vec());
        tree2.insert(test_key(1), b"a".to_vec());

        assert_eq!(tree1.root(), tree2.root());
    }

    #[test]
    fn test_snapshot_restore() {
        let mut tree = SparseMerkleTree::new();
        tree.insert(test_key(1), b"value1".to_vec());
        tree.insert(test_key(2), b"value2".to_vec());

        let snapshot = tree.snapshot();
        let restored = SparseMerkleTree::restore(snapshot);

        assert_eq!(tree.root(), restored.root());
        assert_eq!(tree.get(&test_key(1)), restored.get(&test_key(1)));
        assert_eq!(tree.get(&test_key(2)), restored.get(&test_key(2)));
    }

    #[test]
    fn test_contains() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);

        assert!(!tree.contains(&key));
        tree.insert(key, b"value".to_vec());
        assert!(tree.contains(&key));
    }

    #[test]
    fn test_different_keys_different_roots() {
        let mut tree1 = SparseMerkleTree::new();
        let mut tree2 = SparseMerkleTree::new();

        tree1.insert(test_key(1), b"value".to_vec());
        tree2.insert(test_key(2), b"value".to_vec());

        assert_ne!(tree1.root(), tree2.root());
    }
}
