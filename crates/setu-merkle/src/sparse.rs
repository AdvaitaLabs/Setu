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
//! - **BLAKE3 hashing** for 3-5x performance improvement over SHA256
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
use std::collections::HashMap;
use std::sync::Arc;

// C1 Optimization: Use im::HashMap for O(1) clone via structural sharing
use im::HashMap as ImHashMap;

use crate::error::{MerkleError, MerkleResult};
use crate::hash::{blake3_hash, hash_sparse_internal, hash_sparse_leaf, HashValue};
use crate::HASH_LENGTH;

/// Placeholder hash for empty subtrees.
/// This is the hash of an empty node, computed as BLAKE3("SPARSE_EMPTY").
fn empty_hash() -> HashValue {
    lazy_static::initialize(&EMPTY_HASH);
    *EMPTY_HASH
}

lazy_static::lazy_static! {
    static ref EMPTY_HASH: HashValue = {
        let mut bytes = [0u8; HASH_LENGTH];
        bytes.copy_from_slice(blake3::hash(b"SPARSE_EMPTY").as_bytes());
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
                hash_sparse_leaf(key, value_hash)
            }
            SparseMerkleNode::Internal { left, right } => {
                hash_sparse_internal(left, right)
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
        hash_sparse_leaf(&self.key, &self.value_hash)
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

    /// Get the sibling hashes (for proof conversion)
    pub fn sibling_hashes(&self) -> &[HashValue] {
        &self.siblings
    }

    /// Get the leaf node if this is an inclusion proof
    pub fn leaf(&self) -> Option<&SparseMerkleLeafNode> {
        self.leaf.as_ref()
    }

    /// Check if this is an inclusion proof (leaf exists)
    pub fn is_inclusion(&self) -> bool {
        self.leaf.is_some()
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

    /// Compute root hash from a leaf hash traversing up the path.
    ///
    /// Siblings are stored top-down (from root level towards leaf).
    /// We need to traverse in reverse order (bottom-up) to compute the root.
    fn compute_root_from_leaf(&self, key: &HashValue, leaf_hash: &HashValue) -> MerkleResult<HashValue> {
        let mut current = *leaf_hash;
        
        // Traverse from bottom (leaf) to top (root)
        // siblings are stored top-down, so we iterate in reverse
        for (i, sibling) in self.siblings.iter().enumerate().rev() {
            // Bit index corresponds to the depth level
            // siblings[0] is at depth 0, siblings[n-1] is at depth n-1
            let bit = key.bit(i);
            
            current = if bit {
                // Current node is right child, sibling is left
                hash_internal(sibling, &current)
            } else {
                // Current node is left child, sibling is right
                hash_internal(&current, sibling)
            };
        }
        
        Ok(current)
    }
}

/// Hash a value for storage in the tree (using BLAKE3)
#[inline]
fn hash_value(value: &[u8]) -> HashValue {
    blake3_hash(value)
}

/// Hash two children to create internal node hash (using BLAKE3)
#[inline]
fn hash_internal(left: &HashValue, right: &HashValue) -> HashValue {
    hash_sparse_internal(left, right)
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
    ///
    /// This method traverses the tree structure and collects all sibling hashes
    /// needed to verify the proof from leaf to root.
    pub fn get_proof(&self, key: &HashValue) -> SparseMerkleProof {
        if self.leaves.is_empty() {
            return SparseMerkleProof::new(vec![], None);
        }

        // Collect all leaf nodes with their hashes
        let leaf_nodes: Vec<(HashValue, HashValue)> = self.leaves
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

        // Find the target leaf (if exists)
        let target_leaf = self.leaves.get(key).map(|v| {
            SparseMerkleLeafNode {
                key: *key,
                value_hash: hash_value(v),
            }
        });

        let key_exists = target_leaf.is_some();

        // For non-inclusion proof, find the closest neighbor leaf
        let proof_leaf = if key_exists {
            target_leaf
        } else {
            // Find leaf with longest common prefix
            leaf_nodes.iter()
                .max_by_key(|(k, _)| k.common_prefix_bits(key))
                .map(|(k, _)| {
                    let v = self.leaves.get(k).unwrap();
                    SparseMerkleLeafNode {
                        key: *k,
                        value_hash: hash_value(v),
                    }
                })
        };

        // Determine which key to use for path (target or neighbor)
        let path_key = if key_exists {
            *key
        } else if let Some(ref leaf) = proof_leaf {
            leaf.key
        } else {
            *key
        };

        // Build siblings by traversing the tree structure
        let mut siblings = Vec::new();
        self.build_proof_path(&path_key, &leaf_nodes, 0, &mut siblings);

        SparseMerkleProof::new(siblings, proof_leaf)
    }

    /// Build proof path by recursively computing subtree hashes.
    /// Returns the hash of the current subtree.
    /// Collects siblings along the path to the target key.
    fn build_proof_path(
        &self,
        target_key: &HashValue,
        leaves: &[(HashValue, HashValue)],
        depth: usize,
        siblings: &mut Vec<HashValue>,
    ) -> HashValue {
        if leaves.is_empty() {
            return empty_hash();
        }

        if leaves.len() == 1 {
            return leaves[0].1;
        }

        if depth >= 256 {
            return leaves[0].1;
        }

        // Partition leaves by bit at current depth
        let (left_leaves, right_leaves): (Vec<_>, Vec<_>) = leaves
            .iter()
            .cloned()
            .partition(|(k, _)| !k.bit(depth));

        let target_goes_left = !target_key.bit(depth);

        if target_goes_left {
            // Target is in left subtree, right subtree is sibling
            let right_hash = self.compute_subtree_hash(&right_leaves, depth + 1);
            siblings.push(right_hash);
            self.build_proof_path(target_key, &left_leaves, depth + 1, siblings)
        } else {
            // Target is in right subtree, left subtree is sibling
            let left_hash = self.compute_subtree_hash(&left_leaves, depth + 1);
            siblings.push(left_hash);
            self.build_proof_path(target_key, &right_leaves, depth + 1, siblings)
        }
    }

    /// Compute the hash of a subtree.
    fn compute_subtree_hash(&self, leaves: &[(HashValue, HashValue)], depth: usize) -> HashValue {
        if leaves.is_empty() {
            return empty_hash();
        }

        if leaves.len() == 1 {
            return leaves[0].1;
        }

        if depth >= 256 {
            return leaves[0].1;
        }

        // Partition by bit at current depth
        let (left_leaves, right_leaves): (Vec<_>, Vec<_>) = leaves
            .iter()
            .cloned()
            .partition(|(k, _)| !k.bit(depth));

        let left_hash = self.compute_subtree_hash(&left_leaves, depth + 1);
        let right_hash = self.compute_subtree_hash(&right_leaves, depth + 1);

        hash_internal(&left_hash, &right_hash)
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

    #[test]
    fn test_inclusion_proof_single_leaf() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);
        let value = b"hello".to_vec();

        tree.insert(key, value.clone());
        let root = tree.root();

        let proof = tree.get_proof(&key);
        assert!(proof.verify_inclusion(&root, &key, &value).is_ok());
    }

    #[test]
    fn test_inclusion_proof_multiple_leaves() {
        let mut tree = SparseMerkleTree::new();

        // Insert multiple values
        for i in 0..5u8 {
            tree.insert(test_key(i), format!("value{}", i).into_bytes());
        }

        let root = tree.root();

        // Verify proof for each key
        for i in 0..5u8 {
            let key = test_key(i);
            let value = format!("value{}", i).into_bytes();
            let proof = tree.get_proof(&key);
            
            assert!(
                proof.verify_inclusion(&root, &key, &value).is_ok(),
                "Inclusion proof failed for key {}",
                i
            );
        }
    }

    #[test]
    fn test_non_inclusion_proof() {
        let mut tree = SparseMerkleTree::new();

        // Insert some values
        tree.insert(test_key(1), b"value1".to_vec());
        tree.insert(test_key(3), b"value3".to_vec());

        let root = tree.root();

        // Try to prove non-inclusion of key 2
        let non_existent_key = test_key(2);
        let proof = tree.get_proof(&non_existent_key);

        assert!(
            proof.verify_non_inclusion(&root, &non_existent_key).is_ok(),
            "Non-inclusion proof should succeed for non-existent key"
        );
    }

    #[test]
    fn test_proof_wrong_value_fails() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);

        tree.insert(key, b"correct".to_vec());
        let root = tree.root();

        let proof = tree.get_proof(&key);
        
        // Verifying with wrong value should fail
        let result = proof.verify_inclusion(&root, &key, b"wrong");
        assert!(result.is_err());
    }

    #[test]
    fn test_proof_wrong_root_fails() {
        let mut tree = SparseMerkleTree::new();
        let key = test_key(1);
        let value = b"value".to_vec();

        tree.insert(key, value.clone());
        let wrong_root = HashValue::new([0xAB; 32]);

        let proof = tree.get_proof(&key);
        let result = proof.verify_inclusion(&wrong_root, &key, &value);
        assert!(result.is_err());
    }
}

// ============================================================================
// Incremental Sparse Merkle Tree Implementation
// ============================================================================

/// A tree node reference - either stored by hash or in-memory.
#[derive(Clone, Debug)]
enum TreeNode {
    /// Empty subtree
    Empty,
    /// Leaf node with key and value hash
    Leaf {
        key: HashValue,
        value_hash: HashValue,
        node_hash: HashValue,
    },
    /// Internal node with left and right child hashes
    Internal {
        left: HashValue,
        right: HashValue,
        node_hash: HashValue,
    },
}

impl TreeNode {
    fn hash(&self) -> HashValue {
        match self {
            TreeNode::Empty => empty_hash(),
            TreeNode::Leaf { node_hash, .. } => *node_hash,
            TreeNode::Internal { node_hash, .. } => *node_hash,
        }
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        matches!(self, TreeNode::Empty)
    }

    fn new_leaf(key: HashValue, value_hash: HashValue) -> Self {
        let node_hash = hash_sparse_leaf(&key, &value_hash);
        TreeNode::Leaf { key, value_hash, node_hash }
    }

    fn new_internal(left: HashValue, right: HashValue) -> Self {
        let node_hash = hash_internal(&left, &right);
        TreeNode::Internal { left, right, node_hash }
    }
}

/// An incremental sparse Merkle tree with O(log n) updates.
///
/// This implementation stores the tree structure explicitly and supports
/// efficient incremental updates without rebuilding the entire tree.
///
/// # Performance
///
/// - Insert/Update: O(log n) - only updates nodes along the path
/// - Delete: O(log n) - only updates nodes along the path  
/// - Get: O(1) - direct lookup in leaves map
/// - Proof generation: O(log n)
///
/// # Example
///
/// ```
/// use setu_merkle::sparse::IncrementalSparseMerkleTree;
/// use setu_merkle::HashValue;
///
/// let mut tree = IncrementalSparseMerkleTree::new();
///
/// let key = HashValue::from_slice(&[1u8; 32]).unwrap();
/// tree.insert(key, b"value".to_vec());
///
/// assert_eq!(tree.get(&key), Some(&b"value".to_vec()));
/// ```
#[derive(Debug)]
pub struct IncrementalSparseMerkleTree {
    /// Root hash of the tree
    root_hash: HashValue,
    /// Key-value pairs (leaves) - C1: Uses im::HashMap for O(1) clone
    /// Values wrapped in Arc for efficient sharing during CoW operations
    leaves: ImHashMap<HashValue, Arc<Vec<u8>>>,
    /// Node storage: hash -> TreeNode - C1: Uses im::HashMap for O(1) clone
    nodes: ImHashMap<HashValue, TreeNode>,
    /// Dirty leaves tracking: keys that have been inserted/updated since last commit
    /// Uses std::HashSet - no need for CoW (reset on clone)
    dirty_leaves: std::collections::HashSet<HashValue>,
    /// Deleted leaves tracking: keys that have been deleted since last commit
    /// Uses std::HashSet - no need for CoW (reset on clone)
    deleted_leaves: std::collections::HashSet<HashValue>,
}

/// Leaf changes for batch persistence (B4 scheme)
#[derive(Debug, Clone, Default)]
pub struct LeafChanges {
    /// New or updated leaves: (key, value)
    pub upserts: Vec<(HashValue, Vec<u8>)>,
    /// Deleted leaf keys
    pub deletes: Vec<HashValue>,
}

impl Clone for IncrementalSparseMerkleTree {
    /// C1 Optimization: O(1) clone via im::HashMap structural sharing.
    /// 
    /// Before C1: O(N) deep copy of all leaves and nodes
    /// After C1:  O(1) reference count increment (data shared until modified)
    /// 
    /// Note: dirty_leaves and deleted_leaves are reset (not cloned) because
    /// clones are used for temporary calculations that don't need persistence.
    fn clone(&self) -> Self {
        Self {
            root_hash: self.root_hash,
            leaves: self.leaves.clone(),  // O(1) - im::HashMap structural sharing
            nodes: self.nodes.clone(),    // O(1) - im::HashMap structural sharing
            // Do NOT clone dirty tracking - clones are for temporary calculations
            dirty_leaves: std::collections::HashSet::new(),
            deleted_leaves: std::collections::HashSet::new(),
        }
    }
}

impl Default for IncrementalSparseMerkleTree {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalSparseMerkleTree {
    /// Create a new empty tree.
    pub fn new() -> Self {
        Self {
            root_hash: empty_hash(),
            leaves: ImHashMap::new(),  // C1: im::HashMap
            nodes: ImHashMap::new(),   // C1: im::HashMap
            dirty_leaves: std::collections::HashSet::new(),
            deleted_leaves: std::collections::HashSet::new(),
        }
    }

    /// Create a tree from existing leaves (for crash recovery).
    ///
    /// This rebuilds the tree structure from persisted leaf data.
    /// The dirty tracking is NOT set - recovered data is already persisted.
    /// 
    /// Accepts std::HashMap for compatibility with persistence layer.
    pub fn from_leaves(leaves: HashMap<HashValue, Vec<u8>>) -> Self {
        let mut tree = Self {
            root_hash: empty_hash(),
            leaves: ImHashMap::new(),  // C1: im::HashMap
            nodes: ImHashMap::new(),   // C1: im::HashMap
            dirty_leaves: std::collections::HashSet::new(),
            deleted_leaves: std::collections::HashSet::new(),
        };
        
        // Insert all leaves (this rebuilds the tree structure)
        for (key, value) in leaves {
            tree.insert_without_tracking(key, value);
        }
        
        tree
    }

    /// Get the root hash.
    pub fn root(&self) -> HashValue {
        self.root_hash
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Get the number of leaves.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Get a value by key.
    /// 
    /// C1: Returns reference to inner Vec<u8> through Arc deref.
    pub fn get(&self, key: &HashValue) -> Option<&Vec<u8>> {
        self.leaves.get(key).map(|arc| arc.as_ref())
    }

    /// Check if a key exists.
    pub fn contains(&self, key: &HashValue) -> bool {
        self.leaves.contains_key(key)
    }

    /// Insert or update a key-value pair.
    ///
    /// This performs an O(log n) incremental update, only modifying
    /// nodes along the path from the leaf to the root.
    /// 
    /// C1: Value is wrapped in Arc for efficient sharing during CoW clone.
    pub fn insert(&mut self, key: HashValue, value: Vec<u8>) -> Option<Vec<u8>> {
        // Track dirty leaf for persistence (B4 scheme)
        self.dirty_leaves.insert(key);
        // Remove from deleted if it was previously marked as deleted
        self.deleted_leaves.remove(&key);
        
        // C1: Wrap value in Arc for efficient CoW sharing
        let arc_value = Arc::new(value);
        let old_arc = self.leaves.insert(key, arc_value.clone());
        
        // Compute value hash and create leaf node
        let value_hash = hash_value(&arc_value);
        let new_leaf = TreeNode::new_leaf(key, value_hash);
        
        // Update the tree incrementally
        self.root_hash = self.insert_at_node(self.root_hash, &key, new_leaf, 0);
        
        // C1: Unwrap Arc to return owned Vec<u8> for API compatibility
        old_arc.map(|arc| match Arc::try_unwrap(arc) {
            Ok(v) => v,
            Err(arc) => arc.as_ref().clone(),
        })
    }

    /// Insert without dirty tracking (used for recovery from persisted data).
    /// 
    /// C1: Value is wrapped in Arc for efficient sharing during CoW clone.
    fn insert_without_tracking(&mut self, key: HashValue, value: Vec<u8>) -> Option<Vec<u8>> {
        // C1: Wrap value in Arc for efficient CoW sharing
        let arc_value = Arc::new(value);
        let old_arc = self.leaves.insert(key, arc_value.clone());
        
        // Compute value hash and create leaf node
        let value_hash = hash_value(&arc_value);
        let new_leaf = TreeNode::new_leaf(key, value_hash);
        
        // Update the tree incrementally
        self.root_hash = self.insert_at_node(self.root_hash, &key, new_leaf, 0);
        
        // C1: Unwrap Arc to return owned Vec<u8> for API compatibility
        old_arc.map(|arc| match Arc::try_unwrap(arc) {
            Ok(v) => v,
            Err(arc) => arc.as_ref().clone(),
        })
    }

    /// Remove a key from the tree.
    ///
    /// This performs an O(log n) incremental update.
    /// 
    /// C1: Unwraps Arc to return owned Vec<u8> for API compatibility.
    pub fn remove(&mut self, key: &HashValue) -> Option<Vec<u8>> {
        let old_arc = self.leaves.remove(key)?;
        
        // Track deleted leaf for persistence (B4 scheme)
        self.dirty_leaves.remove(key);
        self.deleted_leaves.insert(*key);
        
        // Update the tree incrementally
        self.root_hash = self.remove_at_node(self.root_hash, key, 0);
        
        // C1: Unwrap Arc to return owned Vec<u8>
        Some(match Arc::try_unwrap(old_arc) {
            Ok(v) => v,
            Err(arc) => arc.as_ref().clone(),
        })
    }

    /// Batch insert multiple key-value pairs.
    ///
    /// For large batches, this is more efficient than individual inserts
    /// as it can share path computations.
    pub fn batch_insert(&mut self, entries: Vec<(HashValue, Vec<u8>)>) {
        for (key, value) in entries {
            self.insert(key, value);
        }
    }

    /// Insert a leaf node at the given position, returning the new subtree root.
    fn insert_at_node(
        &mut self,
        current_hash: HashValue,
        key: &HashValue,
        new_leaf: TreeNode,
        depth: usize,
    ) -> HashValue {
        if depth >= 256 {
            // Reached max depth, just return the leaf
            let hash = new_leaf.hash();
            self.nodes.insert(hash, new_leaf);
            return hash;
        }

        // Get current node (if exists)
        let current_node = if current_hash == empty_hash() {
            TreeNode::Empty
        } else {
            self.nodes.get(&current_hash).cloned().unwrap_or(TreeNode::Empty)
        };

        match current_node {
            TreeNode::Empty => {
                // Empty spot, just insert the leaf
                let hash = new_leaf.hash();
                self.nodes.insert(hash, new_leaf);
                hash
            }
            TreeNode::Leaf { key: existing_key, value_hash: existing_vh, .. } => {
                if existing_key == *key {
                    // Same key, replace the value
                    let hash = new_leaf.hash();
                    self.nodes.insert(hash, new_leaf);
                    hash
                } else {
                    // Different key, need to split
                    self.split_leaf(
                        &existing_key, existing_vh,
                        key, new_leaf,
                        depth,
                    )
                }
            }
            TreeNode::Internal { left, right, .. } => {
                // Traverse down the appropriate branch
                let key_bit = key.bit(depth);
                
                let (new_left, new_right) = if key_bit {
                    // Go right
                    let new_right = self.insert_at_node(right, key, new_leaf, depth + 1);
                    (left, new_right)
                } else {
                    // Go left
                    let new_left = self.insert_at_node(left, key, new_leaf, depth + 1);
                    (new_left, right)
                };
                
                // Create new internal node
                let new_internal = TreeNode::new_internal(new_left, new_right);
                let hash = new_internal.hash();
                self.nodes.insert(hash, new_internal);
                hash
            }
        }
    }

    /// Split a leaf node when inserting a new key at the same position.
    fn split_leaf(
        &mut self,
        existing_key: &HashValue,
        existing_vh: HashValue,
        new_key: &HashValue,
        new_leaf: TreeNode,
        depth: usize,
    ) -> HashValue {
        if depth >= 256 {
            // Shouldn't happen with proper keys
            return new_leaf.hash();
        }

        let existing_bit = existing_key.bit(depth);
        let new_bit = new_key.bit(depth);

        if existing_bit == new_bit {
            // Same bit, need to go deeper
            let subtree = self.split_leaf(existing_key, existing_vh, new_key, new_leaf, depth + 1);
            
            let (left, right) = if existing_bit {
                (empty_hash(), subtree)
            } else {
                (subtree, empty_hash())
            };
            
            let internal = TreeNode::new_internal(left, right);
            let hash = internal.hash();
            self.nodes.insert(hash, internal);
            hash
        } else {
            // Different bits, create internal node with both leaves
            let existing_leaf = TreeNode::new_leaf(*existing_key, existing_vh);
            let existing_hash = existing_leaf.hash();
            self.nodes.insert(existing_hash, existing_leaf);
            
            let new_hash = new_leaf.hash();
            self.nodes.insert(new_hash, new_leaf);
            
            let (left, right) = if new_bit {
                (existing_hash, new_hash)
            } else {
                (new_hash, existing_hash)
            };
            
            let internal = TreeNode::new_internal(left, right);
            let hash = internal.hash();
            self.nodes.insert(hash, internal);
            hash
        }
    }

    /// Remove a key at the given node, returning the new subtree root.
    fn remove_at_node(
        &mut self,
        current_hash: HashValue,
        key: &HashValue,
        depth: usize,
    ) -> HashValue {
        if current_hash == empty_hash() {
            return empty_hash();
        }

        let current_node = match self.nodes.get(&current_hash) {
            Some(node) => node.clone(),
            None => return empty_hash(),
        };

        match current_node {
            TreeNode::Empty => empty_hash(),
            TreeNode::Leaf { key: leaf_key, .. } => {
                if leaf_key == *key {
                    empty_hash()
                } else {
                    // Not the key we're looking for
                    current_hash
                }
            }
            TreeNode::Internal { left, right, .. } => {
                let key_bit = key.bit(depth);
                
                let (new_left, new_right) = if key_bit {
                    let new_right = self.remove_at_node(right, key, depth + 1);
                    (left, new_right)
                } else {
                    let new_left = self.remove_at_node(left, key, depth + 1);
                    (new_left, right)
                };
                
                // Check if we can collapse
                let left_empty = new_left == empty_hash();
                let right_empty = new_right == empty_hash();
                
                if left_empty && right_empty {
                    empty_hash()
                } else if left_empty {
                    // Only right child, check if it's a leaf
                    if let Some(TreeNode::Leaf { .. }) = self.nodes.get(&new_right) {
                        new_right
                    } else {
                        let internal = TreeNode::new_internal(new_left, new_right);
                        let hash = internal.hash();
                        self.nodes.insert(hash, internal);
                        hash
                    }
                } else if right_empty {
                    // Only left child, check if it's a leaf
                    if let Some(TreeNode::Leaf { .. }) = self.nodes.get(&new_left) {
                        new_left
                    } else {
                        let internal = TreeNode::new_internal(new_left, new_right);
                        let hash = internal.hash();
                        self.nodes.insert(hash, internal);
                        hash
                    }
                } else {
                    let internal = TreeNode::new_internal(new_left, new_right);
                    let hash = internal.hash();
                    self.nodes.insert(hash, internal);
                    hash
                }
            }
        }
    }

    /// Generate a proof for a key.
    pub fn get_proof(&self, key: &HashValue) -> SparseMerkleProof {
        let mut siblings = Vec::new();
        let mut current_hash = self.root_hash;
        
        for depth in 0..256 {
            if current_hash == empty_hash() {
                break;
            }
            
            let node = match self.nodes.get(&current_hash) {
                Some(n) => n,
                None => break,
            };
            
            match node {
                TreeNode::Empty => break,
                TreeNode::Leaf { key: leaf_key, value_hash, .. } => {
                    // Found a leaf
                    let proof_leaf = SparseMerkleLeafNode {
                        key: *leaf_key,
                        value_hash: *value_hash,
                    };
                    return SparseMerkleProof::new(siblings, Some(proof_leaf));
                }
                TreeNode::Internal { left, right, .. } => {
                    let key_bit = key.bit(depth);
                    if key_bit {
                        siblings.push(*left);
                        current_hash = *right;
                    } else {
                        siblings.push(*right);
                        current_hash = *left;
                    }
                }
            }
        }
        
        // Empty tree or no leaf found
        SparseMerkleProof::new(siblings, None)
    }

    /// Get the number of nodes in storage.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get the number of leaf entries.
    pub fn leaf_count(&self) -> usize {
        self.leaves.len()
    }

    /// Clear unused nodes (garbage collection).
    ///
    /// This removes nodes that are no longer reachable from the current root.
    pub fn gc(&mut self) {
        let mut reachable = std::collections::HashSet::new();
        self.mark_reachable(self.root_hash, &mut reachable);
        
        self.nodes.retain(|hash, _| reachable.contains(hash));
    }

    /// Iterate over all leaf entries (key, value).
    ///
    /// This is useful for rebuilding indexes at startup.
    /// 
    /// # Performance
    /// - O(n) where n is the number of leaves
    /// - Just iterates over the in-memory HashMap, very fast
    /// 
    /// C1: Maps Arc<Vec<u8>> to &Vec<u8> for API compatibility.
    pub fn iter_leaves(&self) -> impl Iterator<Item = (&HashValue, &Vec<u8>)> {
        self.leaves.iter().map(|(k, arc)| (k, arc.as_ref()))
    }

    /// Get all leaves as a vector of (key, value) pairs.
    ///
    /// Useful when you need ownership of the data.
    /// 
    /// C1: Clones inner Vec<u8> from Arc for ownership transfer.
    pub fn all_leaves(&self) -> Vec<(HashValue, Vec<u8>)> {
        self.leaves.iter().map(|(k, arc)| (*k, arc.as_ref().clone())).collect()
    }

    // =========================================================================
    // B4 Scheme: Dirty Data Tracking for Batch Persistence
    // =========================================================================

    /// Take all pending changes and clear the tracking (called during commit).
    ///
    /// Returns `LeafChanges` containing:
    /// - `upserts`: All leaves that were inserted or updated since last commit
    /// - `deletes`: All keys that were deleted since last commit
    ///
    /// After calling this, `has_pending_changes()` will return false.
    /// 
    /// C1: Clones inner Vec<u8> from Arc for persistence (actual data needed).
    pub fn take_changes(&mut self) -> LeafChanges {
        let upserts: Vec<_> = self.dirty_leaves
            .drain()
            .filter_map(|k| self.leaves.get(&k).map(|arc| (k, arc.as_ref().clone())))
            .collect();
        let deletes: Vec<_> = self.deleted_leaves.drain().collect();
        
        LeafChanges { upserts, deletes }
    }

    /// Check if there are any uncommitted changes.
    ///
    /// Returns true if there are dirty or deleted leaves that haven't been
    /// persisted yet.
    pub fn has_pending_changes(&self) -> bool {
        !self.dirty_leaves.is_empty() || !self.deleted_leaves.is_empty()
    }

    /// Get the number of pending upserts.
    pub fn pending_upsert_count(&self) -> usize {
        self.dirty_leaves.len()
    }

    /// Get the number of pending deletes.
    pub fn pending_delete_count(&self) -> usize {
        self.deleted_leaves.len()
    }

    fn mark_reachable(&self, hash: HashValue, reachable: &mut std::collections::HashSet<HashValue>) {
        if hash == empty_hash() || reachable.contains(&hash) {
            return;
        }
        
        reachable.insert(hash);
        
        if let Some(node) = self.nodes.get(&hash) {
            if let TreeNode::Internal { left, right, .. } = node {
                self.mark_reachable(*left, reachable);
                self.mark_reachable(*right, reachable);
            }
        }
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;

    fn test_key(byte: u8) -> HashValue {
        HashValue::new([byte; 32])
    }

    #[test]
    fn test_incremental_empty_tree() {
        let tree = IncrementalSparseMerkleTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.root(), empty_hash());
    }

    #[test]
    fn test_incremental_single_insert() {
        let mut tree = IncrementalSparseMerkleTree::new();
        let key = test_key(1);
        let value = b"hello".to_vec();

        tree.insert(key, value.clone());

        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&key), Some(&value));
        assert_ne!(tree.root(), empty_hash());
    }

    #[test]
    fn test_incremental_multiple_inserts() {
        let mut tree = IncrementalSparseMerkleTree::new();

        for i in 0..10u8 {
            tree.insert(test_key(i), format!("value{}", i).into_bytes());
        }

        assert_eq!(tree.len(), 10);

        for i in 0..10u8 {
            let expected = format!("value{}", i).into_bytes();
            assert_eq!(tree.get(&test_key(i)), Some(&expected));
        }
    }

    #[test]
    fn test_incremental_update() {
        let mut tree = IncrementalSparseMerkleTree::new();
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
    fn test_incremental_remove() {
        let mut tree = IncrementalSparseMerkleTree::new();
        let key = test_key(1);

        tree.insert(key, b"value".to_vec());
        assert!(!tree.is_empty());

        let removed = tree.remove(&key);
        assert_eq!(removed, Some(b"value".to_vec()));
        assert!(tree.is_empty());
        assert_eq!(tree.root(), empty_hash());
    }

    #[test]
    fn test_incremental_deterministic() {
        let mut tree1 = IncrementalSparseMerkleTree::new();
        let mut tree2 = IncrementalSparseMerkleTree::new();

        // Insert in different order
        tree1.insert(test_key(1), b"a".to_vec());
        tree1.insert(test_key(2), b"b".to_vec());

        tree2.insert(test_key(2), b"b".to_vec());
        tree2.insert(test_key(1), b"a".to_vec());

        assert_eq!(tree1.root(), tree2.root());
    }

    #[test]
    fn test_incremental_proof_inclusion() {
        let mut tree = IncrementalSparseMerkleTree::new();

        for i in 0..5u8 {
            tree.insert(test_key(i), format!("value{}", i).into_bytes());
        }

        let root = tree.root();

        for i in 0..5u8 {
            let key = test_key(i);
            let value = format!("value{}", i).into_bytes();
            let proof = tree.get_proof(&key);
            
            assert!(
                proof.verify_inclusion(&root, &key, &value).is_ok(),
                "Inclusion proof failed for key {}",
                i
            );
        }
    }

    #[test]
    fn test_incremental_gc() {
        let mut tree = IncrementalSparseMerkleTree::new();

        // Insert some values
        for i in 0..5u8 {
            tree.insert(test_key(i), format!("value{}", i).into_bytes());
        }
        
        let nodes_before = tree.node_count();

        // Update values (creates new nodes, old ones become garbage)
        for i in 0..5u8 {
            tree.insert(test_key(i), format!("updated{}", i).into_bytes());
        }

        let nodes_after_update = tree.node_count();
        assert!(nodes_after_update > nodes_before);

        // Run GC
        tree.gc();
        let nodes_after_gc = tree.node_count();
        
        assert!(nodes_after_gc <= nodes_after_update);
    }

    #[test]
    fn test_incremental_matches_original() {
        // Verify that incremental tree produces same root as original
        let mut inc_tree = IncrementalSparseMerkleTree::new();
        let mut orig_tree = SparseMerkleTree::new();

        let entries: Vec<(HashValue, Vec<u8>)> = (0..10u8)
            .map(|i| (test_key(i), format!("value{}", i).into_bytes()))
            .collect();

        for (key, value) in &entries {
            inc_tree.insert(*key, value.clone());
            orig_tree.insert(*key, value.clone());
        }

        assert_eq!(inc_tree.root(), orig_tree.root());
    }

    // =========================================================================
    // C1 Optimization Tests: Clone Independence and Performance
    // =========================================================================

    #[test]
    fn test_c1_clone_independence() {
        // Verify that clone creates an independent copy (CoW behavior)
        let mut tree = IncrementalSparseMerkleTree::new();
        let key = test_key(1);
        tree.insert(key, b"original".to_vec());
        let original_root = tree.root();
        
        // Clone the tree
        let mut cloned = tree.clone();
        
        // Modify the clone
        cloned.insert(key, b"modified".to_vec());
        
        // Original should be unchanged
        assert_eq!(tree.get(&key), Some(&b"original".to_vec()));
        assert_eq!(tree.root(), original_root);
        
        // Clone should have the new value
        assert_eq!(cloned.get(&key), Some(&b"modified".to_vec()));
        assert_ne!(cloned.root(), original_root);
    }

    #[test]
    fn test_c1_clone_dirty_tracking_reset() {
        // Verify that clone resets dirty tracking (for temporary calculations)
        let mut tree = IncrementalSparseMerkleTree::new();
        tree.insert(test_key(1), b"value".to_vec());
        
        // Original has dirty leaves
        assert!(tree.has_pending_changes());
        
        // Clone should NOT have dirty leaves
        let cloned = tree.clone();
        assert!(!cloned.has_pending_changes());
    }

    #[test]
    fn test_c1_clone_performance() {
        // Verify that clone is fast with im::HashMap (O(1) instead of O(N))
        // Insert 10K entries
        let mut tree = IncrementalSparseMerkleTree::new();
        for i in 0..10_000u32 {
            let mut key_bytes = [0u8; 32];
            key_bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let key = HashValue::new(key_bytes);
            tree.insert(key, vec![i as u8; 100]);
        }
        
        // Clone 100 times and measure - with im::HashMap this should be very fast
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = tree.clone();
        }
        let elapsed = start.elapsed();
        
        // 100 clones should complete in < 50ms with im::HashMap
        // (vs ~1000ms+ with std::HashMap for 10K entries)
        assert!(
            elapsed.as_millis() < 50,
            "C1 Clone too slow: {:?} (expected < 50ms for 100 clones of 10K entries)",
            elapsed
        );
        
        println!("C1 Clone performance: 100 clones of 10K entries in {:?}", elapsed);
    }

    #[test]
    fn test_c1_clone_then_modify_independence() {
        // Comprehensive test: clone, modify both, verify independence
        let mut tree = IncrementalSparseMerkleTree::new();
        
        // Initial entries
        for i in 0..5u8 {
            tree.insert(test_key(i), format!("original{}", i).into_bytes());
        }
        
        // Clone
        let mut cloned = tree.clone();
        
        // Modify original: update existing + add new
        tree.insert(test_key(0), b"updated_original".to_vec());
        tree.insert(test_key(100), b"new_in_original".to_vec());
        
        // Modify clone: update existing + remove
        cloned.insert(test_key(1), b"updated_clone".to_vec());
        cloned.remove(&test_key(2));
        
        // Verify original
        assert_eq!(tree.get(&test_key(0)), Some(&b"updated_original".to_vec()));
        assert_eq!(tree.get(&test_key(1)), Some(&b"original1".to_vec())); // unchanged
        assert_eq!(tree.get(&test_key(2)), Some(&b"original2".to_vec())); // not removed
        assert_eq!(tree.get(&test_key(100)), Some(&b"new_in_original".to_vec()));
        
        // Verify clone
        assert_eq!(cloned.get(&test_key(0)), Some(&b"original0".to_vec())); // unchanged
        assert_eq!(cloned.get(&test_key(1)), Some(&b"updated_clone".to_vec()));
        assert_eq!(cloned.get(&test_key(2)), None); // removed
        assert_eq!(cloned.get(&test_key(100)), None); // not added
    }

    #[test]
    fn test_c1_insert_return_old_value() {
        // Verify insert returns the old value correctly with Arc unwrap
        let mut tree = IncrementalSparseMerkleTree::new();
        let key = test_key(1);
        
        // First insert - no old value
        let old1 = tree.insert(key, b"first".to_vec());
        assert_eq!(old1, None);
        
        // Second insert - should return old value
        let old2 = tree.insert(key, b"second".to_vec());
        assert_eq!(old2, Some(b"first".to_vec()));
        
        // Third insert - should return second value
        let old3 = tree.insert(key, b"third".to_vec());
        assert_eq!(old3, Some(b"second".to_vec()));
        
        // Current value should be third
        assert_eq!(tree.get(&key), Some(&b"third".to_vec()));
    }

    #[test]
    fn test_c1_take_changes_clones_data() {
        // Verify take_changes returns actual data (not Arc references)
        let mut tree = IncrementalSparseMerkleTree::new();
        tree.insert(test_key(1), b"value1".to_vec());
        tree.insert(test_key(2), b"value2".to_vec());
        
        let changes = tree.take_changes();
        assert_eq!(changes.upserts.len(), 2);
        
        // Verify the data is correct (actual Vec<u8>, not Arc)
        for (key, value) in &changes.upserts {
            if *key == test_key(1) {
                assert_eq!(value, &b"value1".to_vec());
            } else if *key == test_key(2) {
                assert_eq!(value, &b"value2".to_vec());
            }
        }
        
        // Tree should still have the data
        assert_eq!(tree.get(&test_key(1)), Some(&b"value1".to_vec()));
        assert_eq!(tree.get(&test_key(2)), Some(&b"value2".to_vec()));
        
        // But no more pending changes
        assert!(!tree.has_pending_changes());
    }

    #[test]
    fn test_c1_from_leaves_recovery() {
        // Verify from_leaves works correctly for crash recovery
        let mut original = IncrementalSparseMerkleTree::new();
        for i in 0..10u8 {
            original.insert(test_key(i), format!("value{}", i).into_bytes());
        }
        let original_root = original.root();
        
        // Simulate crash recovery: get all leaves as std::HashMap
        let leaves: HashMap<HashValue, Vec<u8>> = original.all_leaves().into_iter().collect();
        
        // Recover from leaves
        let recovered = IncrementalSparseMerkleTree::from_leaves(leaves);
        
        // Should have same root
        assert_eq!(recovered.root(), original_root);
        
        // Should have same data
        for i in 0..10u8 {
            let expected = format!("value{}", i).into_bytes();
            assert_eq!(recovered.get(&test_key(i)), Some(&expected));
        }
        
        // Recovered tree should NOT have pending changes
        assert!(!recovered.has_pending_changes());
    }

    #[test]
    fn test_c1_iter_leaves_type_compatibility() {
        // Verify iter_leaves returns correct types
        let mut tree = IncrementalSparseMerkleTree::new();
        tree.insert(test_key(1), b"value1".to_vec());
        tree.insert(test_key(2), b"value2".to_vec());
        
        // iter_leaves should return (&HashValue, &Vec<u8>)
        let mut count = 0;
        for (key, value) in tree.iter_leaves() {
            // key should be &HashValue
            let _: &HashValue = key;
            // value should be &Vec<u8>
            let _: &Vec<u8> = value;
            count += 1;
        }
        assert_eq!(count, 2);
    }
}
