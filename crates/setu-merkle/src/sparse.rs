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
        let node_hash = {
            let mut hasher = Sha256::new();
            hasher.update(prefix::SPARSE_LEAF);
            hasher.update(key.as_bytes());
            hasher.update(value_hash.as_bytes());
            let result = hasher.finalize();
            let mut bytes = [0u8; HASH_LENGTH];
            bytes.copy_from_slice(&result);
            HashValue::new(bytes)
        };
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
#[derive(Clone, Debug)]
pub struct IncrementalSparseMerkleTree {
    /// Root hash of the tree
    root_hash: HashValue,
    /// Key-value pairs (leaves)
    leaves: HashMap<HashValue, Vec<u8>>,
    /// Node storage: hash -> TreeNode
    nodes: HashMap<HashValue, TreeNode>,
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
            leaves: HashMap::new(),
            nodes: HashMap::new(),
        }
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
    pub fn get(&self, key: &HashValue) -> Option<&Vec<u8>> {
        self.leaves.get(key)
    }

    /// Check if a key exists.
    pub fn contains(&self, key: &HashValue) -> bool {
        self.leaves.contains_key(key)
    }

    /// Insert or update a key-value pair.
    ///
    /// This performs an O(log n) incremental update, only modifying
    /// nodes along the path from the leaf to the root.
    pub fn insert(&mut self, key: HashValue, value: Vec<u8>) -> Option<Vec<u8>> {
        let old_value = self.leaves.insert(key, value.clone());
        
        // Compute value hash and create leaf node
        let value_hash = hash_value(&value);
        let new_leaf = TreeNode::new_leaf(key, value_hash);
        
        // Update the tree incrementally
        self.root_hash = self.insert_at_node(self.root_hash, &key, new_leaf, 0);
        
        old_value
    }

    /// Remove a key from the tree.
    ///
    /// This performs an O(log n) incremental update.
    pub fn remove(&mut self, key: &HashValue) -> Option<Vec<u8>> {
        let old_value = self.leaves.remove(key)?;
        
        // Update the tree incrementally
        self.root_hash = self.remove_at_node(self.root_hash, key, 0);
        
        Some(old_value)
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

    /// Clear unused nodes (garbage collection).
    ///
    /// This removes nodes that are no longer reachable from the current root.
    pub fn gc(&mut self) {
        let mut reachable = std::collections::HashSet::new();
        self.mark_reachable(self.root_hash, &mut reachable);
        
        self.nodes.retain(|hash, _| reachable.contains(hash));
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
}
