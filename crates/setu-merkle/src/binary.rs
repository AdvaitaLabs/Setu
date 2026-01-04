//! Binary Merkle Tree implementation.
//!
//! A simple and efficient binary Merkle tree for ordered data commitments.
//! Used for event list commitments in DAG folding and TEE execution proofs.
//!
//! # Features
//!
//! - O(log n) proof generation and verification
//! - Domain separation with leaf/inner node prefixes
//! - Support for inclusion proofs
//! - Efficient batch construction
//!
//! # Example
//!
//! ```
//! use setu_merkle::binary::BinaryMerkleTree;
//!
//! let leaves = vec![b"leaf0".to_vec(), b"leaf1".to_vec()];
//! let tree = BinaryMerkleTree::build(&leaves);
//! let root = tree.root();
//!
//! let proof = tree.get_proof(0).unwrap();
//! assert!(proof.verify(&root, &leaves[0], 0).is_ok());
//! ```

use serde::{Deserialize, Serialize};

use crate::error::{MerkleError, MerkleResult};
use crate::hash::{hash_internal, hash_leaf, HashValue};

/// A node in the binary Merkle tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    /// An empty node (placeholder for padding)
    Empty,
    /// A node with a hash value
    Hash(HashValue),
}

impl Node {
    /// Get the hash value of this node
    pub fn hash(&self) -> HashValue {
        match self {
            Node::Empty => HashValue::zero(),
            Node::Hash(h) => *h,
        }
    }

    /// Check if this node is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, Node::Empty)
    }
}

impl Default for Node {
    fn default() -> Self {
        Node::Empty
    }
}

impl From<HashValue> for Node {
    fn from(hash: HashValue) -> Self {
        if hash.is_zero() {
            Node::Empty
        } else {
            Node::Hash(hash)
        }
    }
}

/// A proof of inclusion for a leaf in the binary Merkle tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryMerkleProof {
    /// The sibling hashes on the path from leaf to root
    siblings: Vec<Node>,
}

impl BinaryMerkleProof {
    /// Create a new proof from sibling nodes
    pub fn new(siblings: Vec<Node>) -> Self {
        Self { siblings }
    }

    /// Get the depth of this proof (number of levels)
    pub fn depth(&self) -> usize {
        self.siblings.len()
    }

    /// Verify the proof given a root hash and leaf data.
    ///
    /// # Arguments
    ///
    /// * `root` - The expected root hash
    /// * `leaf` - The leaf data
    /// * `leaf_index` - The index of the leaf in the tree
    ///
    /// # Returns
    ///
    /// Ok(()) if the proof is valid, Err otherwise
    pub fn verify(&self, root: &HashValue, leaf: &[u8], leaf_index: usize) -> MerkleResult<()> {
        let computed_root = self.compute_root(leaf, leaf_index)?;
        if &computed_root == root {
            Ok(())
        } else {
            Err(MerkleError::InvalidProof(format!(
                "Root mismatch: expected {}, got {}",
                root, computed_root
            )))
        }
    }

    /// Compute the root hash from the proof and leaf data.
    ///
    /// # Arguments
    ///
    /// * `leaf` - The leaf data
    /// * `leaf_index` - The index of the leaf in the tree
    ///
    /// # Returns
    ///
    /// The computed root hash
    pub fn compute_root(&self, leaf: &[u8], leaf_index: usize) -> MerkleResult<HashValue> {
        // Check if leaf_index is valid for this proof depth
        if leaf_index >> self.siblings.len() != 0 {
            return Err(MerkleError::InvalidProof(format!(
                "Leaf index {} too large for proof depth {}",
                leaf_index,
                self.siblings.len()
            )));
        }

        let mut current = hash_leaf(leaf);
        let mut index = leaf_index;

        for sibling in &self.siblings {
            let sibling_hash = sibling.hash();
            current = if index % 2 == 0 {
                // Current node is left child
                hash_internal(&current, &sibling_hash)
            } else {
                // Current node is right child
                hash_internal(&sibling_hash, &current)
            };
            index /= 2;
        }

        Ok(current)
    }

    /// Check if this proof is for the rightmost leaf in the tree.
    ///
    /// This is useful for append-only trees where we need to verify
    /// that we're extending from the rightmost position.
    pub fn is_rightmost(&self, leaf_index: usize) -> bool {
        let mut index = leaf_index;
        for sibling in &self.siblings {
            if index % 2 == 0 && !sibling.is_empty() {
                return false;
            }
            index /= 2;
        }
        true
    }
}

/// A binary Merkle tree for ordered data commitments.
///
/// The tree is built from a list of leaf data. Each leaf is hashed with a
/// domain-separation prefix, and internal nodes are computed by hashing
/// their children with a different prefix.
///
/// Empty subtrees are represented by zero hashes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinaryMerkleTree {
    /// All nodes in the tree, stored level by level starting with leaves
    nodes: Vec<Node>,
    /// Number of leaves in the tree
    n_leaves: usize,
}

impl BinaryMerkleTree {
    /// Build a Merkle tree from leaf data.
    ///
    /// # Arguments
    ///
    /// * `leaves` - The leaf data to include in the tree
    ///
    /// # Returns
    ///
    /// A new BinaryMerkleTree
    pub fn build<T: AsRef<[u8]>>(leaves: &[T]) -> Self {
        if leaves.is_empty() {
            return Self {
                nodes: vec![],
                n_leaves: 0,
            };
        }

        // Hash all leaves
        let leaf_hashes: Vec<Node> = leaves
            .iter()
            .map(|leaf| Node::Hash(hash_leaf(leaf.as_ref())))
            .collect();

        Self::build_from_leaf_nodes(leaf_hashes)
    }

    /// Build a Merkle tree from pre-computed leaf hashes.
    pub fn build_from_hashes(leaf_hashes: Vec<HashValue>) -> Self {
        let nodes: Vec<Node> = leaf_hashes.into_iter().map(Node::from).collect();
        Self::build_from_leaf_nodes(nodes)
    }

    /// Build tree from leaf nodes
    fn build_from_leaf_nodes(leaf_nodes: Vec<Node>) -> Self {
        let n_leaves = leaf_nodes.len();
        if n_leaves == 0 {
            return Self {
                nodes: vec![],
                n_leaves: 0,
            };
        }

        // Calculate total number of nodes needed
        let total_nodes = Self::calculate_total_nodes(n_leaves);
        let mut nodes = Vec::with_capacity(total_nodes);
        nodes.extend(leaf_nodes);

        let mut level_nodes = n_leaves;
        let mut prev_level_start = 0;

        // Build each level
        while level_nodes > 1 {
            // Pad to even number if necessary
            if level_nodes % 2 == 1 {
                nodes.push(Node::Empty);
                level_nodes += 1;
            }

            let new_level_start = prev_level_start + level_nodes;

            // Compute parent nodes
            for i in (prev_level_start..new_level_start).step_by(2) {
                let left = &nodes[i];
                let right = &nodes[i + 1];
                let parent = Self::compute_parent(left, right);
                nodes.push(parent);
            }

            prev_level_start = new_level_start;
            level_nodes /= 2;
        }

        Self { nodes, n_leaves }
    }

    /// Compute parent node from two children
    fn compute_parent(left: &Node, right: &Node) -> Node {
        match (left, right) {
            (Node::Empty, Node::Empty) => Node::Empty,
            _ => Node::Hash(hash_internal(&left.hash(), &right.hash())),
        }
    }

    /// Calculate total number of nodes needed for n leaves
    fn calculate_total_nodes(n_leaves: usize) -> usize {
        if n_leaves == 0 {
            return 0;
        }
        let mut total = 0;
        let mut level_size = n_leaves;
        while level_size > 1 {
            level_size = (level_size + 1) / 2 * 2; // Round up to even
            total += level_size;
            level_size /= 2;
        }
        total + 1 // Add root
    }

    /// Get the root hash of the tree.
    pub fn root(&self) -> HashValue {
        self.nodes.last().map(|n| n.hash()).unwrap_or(HashValue::zero())
    }

    /// Get the number of leaves in the tree.
    pub fn num_leaves(&self) -> usize {
        self.n_leaves
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.n_leaves == 0
    }

    /// Get a proof of inclusion for the leaf at the given index.
    ///
    /// # Arguments
    ///
    /// * `leaf_index` - The index of the leaf to prove
    ///
    /// # Returns
    ///
    /// A BinaryMerkleProof if the index is valid
    pub fn get_proof(&self, leaf_index: usize) -> MerkleResult<BinaryMerkleProof> {
        if leaf_index >= self.n_leaves {
            return Err(MerkleError::IndexOutOfBounds {
                index: leaf_index,
                size: self.n_leaves,
            });
        }

        if self.n_leaves == 0 {
            return Err(MerkleError::EmptyTree);
        }

        let depth = self.tree_depth();
        let mut siblings = Vec::with_capacity(depth);
        let mut index = leaf_index;
        let mut level_start = 0;
        let mut level_size = self.n_leaves;

        while level_size > 1 {
            // Pad level size to even
            let padded_size = (level_size + 1) / 2 * 2;

            // Find sibling index
            let sibling_index = if index % 2 == 0 {
                level_start + index + 1
            } else {
                level_start + index - 1
            };

            // Get sibling node (might be empty if we're at the edge)
            let sibling = if sibling_index < level_start + padded_size {
                self.nodes.get(sibling_index).cloned().unwrap_or(Node::Empty)
            } else {
                Node::Empty
            };

            siblings.push(sibling);

            // Move to parent level
            level_start += padded_size;
            level_size = padded_size / 2;
            index /= 2;
        }

        Ok(BinaryMerkleProof::new(siblings))
    }

    /// Get the depth of the tree (number of levels from leaf to root)
    fn tree_depth(&self) -> usize {
        if self.n_leaves <= 1 {
            0
        } else {
            (self.n_leaves as f64).log2().ceil() as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_tree() {
        let tree = BinaryMerkleTree::build::<Vec<u8>>(&[]);
        assert!(tree.is_empty());
        assert_eq!(tree.root(), HashValue::zero());
    }

    #[test]
    fn test_single_leaf() {
        let leaves = vec![b"only leaf".to_vec()];
        let tree = BinaryMerkleTree::build(&leaves);

        assert_eq!(tree.num_leaves(), 1);
        assert_ne!(tree.root(), HashValue::zero());

        let proof = tree.get_proof(0).unwrap();
        assert!(proof.verify(&tree.root(), &leaves[0], 0).is_ok());
    }

    #[test]
    fn test_two_leaves() {
        let leaves = vec![b"leaf0".to_vec(), b"leaf1".to_vec()];
        let tree = BinaryMerkleTree::build(&leaves);

        let root = tree.root();
        assert_ne!(root, HashValue::zero());

        // Verify both proofs
        for i in 0..2 {
            let proof = tree.get_proof(i).unwrap();
            assert!(proof.verify(&root, &leaves[i], i).is_ok());
        }
    }

    #[test]
    fn test_four_leaves() {
        let leaves: Vec<Vec<u8>> = (0..4).map(|i| format!("leaf{}", i).into_bytes()).collect();
        let tree = BinaryMerkleTree::build(&leaves);

        let root = tree.root();

        for i in 0..4 {
            let proof = tree.get_proof(i).unwrap();
            assert!(proof.verify(&root, &leaves[i], i).is_ok());
        }
    }

    #[test]
    fn test_odd_number_of_leaves() {
        let leaves: Vec<Vec<u8>> = (0..5).map(|i| format!("leaf{}", i).into_bytes()).collect();
        let tree = BinaryMerkleTree::build(&leaves);

        let root = tree.root();

        for i in 0..5 {
            let proof = tree.get_proof(i).unwrap();
            assert!(
                proof.verify(&root, &leaves[i], i).is_ok(),
                "Proof failed for leaf {}",
                i
            );
        }
    }

    #[test]
    fn test_proof_for_invalid_index() {
        let leaves = vec![b"leaf0".to_vec(), b"leaf1".to_vec()];
        let tree = BinaryMerkleTree::build(&leaves);

        let result = tree.get_proof(5);
        assert!(matches!(result, Err(MerkleError::IndexOutOfBounds { .. })));
    }

    #[test]
    fn test_wrong_leaf_data() {
        let leaves = vec![b"leaf0".to_vec(), b"leaf1".to_vec()];
        let tree = BinaryMerkleTree::build(&leaves);
        let root = tree.root();

        let proof = tree.get_proof(0).unwrap();
        let result = proof.verify(&root, b"wrong data", 0);
        assert!(matches!(result, Err(MerkleError::InvalidProof(_))));
    }

    #[test]
    fn test_wrong_index() {
        let leaves = vec![b"leaf0".to_vec(), b"leaf1".to_vec()];
        let tree = BinaryMerkleTree::build(&leaves);
        let root = tree.root();

        let proof = tree.get_proof(0).unwrap();
        // Verify with wrong index should fail
        let result = proof.verify(&root, &leaves[0], 1);
        assert!(matches!(result, Err(MerkleError::InvalidProof(_))));
    }

    #[test]
    fn test_is_rightmost() {
        let leaves: Vec<Vec<u8>> = (0..4).map(|i| format!("leaf{}", i).into_bytes()).collect();
        let tree = BinaryMerkleTree::build(&leaves);

        // Only the last leaf should be rightmost
        for i in 0..3 {
            let proof = tree.get_proof(i).unwrap();
            assert!(!proof.is_rightmost(i), "Leaf {} should not be rightmost", i);
        }

        let proof = tree.get_proof(3).unwrap();
        assert!(proof.is_rightmost(3), "Leaf 3 should be rightmost");
    }

    #[test]
    fn test_deterministic() {
        let leaves = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];
        let tree1 = BinaryMerkleTree::build(&leaves);
        let tree2 = BinaryMerkleTree::build(&leaves);

        assert_eq!(tree1.root(), tree2.root());
    }
}
