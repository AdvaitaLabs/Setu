//! Subnet Aggregation Tree implementation.
//!
//! A Binary Merkle Tree that aggregates all subnet state roots into a single
//! global state root. This is used in Setu's DAG consensus to commit to the
//! state of all subnets at each anchor.
//!
//! # Design
//!
//! The tree is constructed from `(SubnetId, SubnetStateRoot)` pairs:
//! - Leaves are sorted by SubnetId for deterministic ordering
//! - ROOT Subnet (SubnetId = 0) is always at the first position
//! - Each leaf is hashed as: `hash(AGGREGATION_LEAF || subnet_id || state_root)`
//! - Internal nodes use standard binary merkle tree construction
//!
//! # Example
//!
//! ```
//! use setu_merkle::aggregation::{SubnetAggregationTree, SubnetStateEntry};
//! use setu_merkle::HashValue;
//!
//! let entries = vec![
//!     SubnetStateEntry::new([0u8; 32], HashValue::from_slice(&[1u8; 32]).unwrap()),
//!     SubnetStateEntry::new([2u8; 32], HashValue::from_slice(&[2u8; 32]).unwrap()),
//! ];
//!
//! let tree = SubnetAggregationTree::build(entries);
//! let global_root = tree.root();
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::binary::BinaryMerkleTree;
use crate::error::{MerkleError, MerkleResult};
use crate::hash::HashValue;
use crate::HASH_LENGTH;

/// Domain separation prefix for subnet aggregation leaf nodes.
const AGGREGATION_LEAF_PREFIX: &[u8] = &[0x10];

/// SubnetId type (32 bytes).
pub type SubnetId = [u8; 32];

/// ROOT subnet identifier (all zeros).
pub const ROOT_SUBNET: SubnetId = [0u8; 32];

/// An entry representing a subnet's state root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetStateEntry {
    /// The subnet identifier
    pub subnet_id: SubnetId,
    /// The state root of this subnet's Object State SMT
    pub state_root: HashValue,
    /// Optional: number of objects in the subnet
    pub object_count: Option<u64>,
    /// Optional: last updated anchor ID
    pub last_anchor_id: Option<u64>,
}

impl SubnetStateEntry {
    /// Create a new subnet state entry.
    pub fn new(subnet_id: SubnetId, state_root: HashValue) -> Self {
        Self {
            subnet_id,
            state_root,
            object_count: None,
            last_anchor_id: None,
        }
    }

    /// Create a new subnet state entry with metadata.
    pub fn with_metadata(
        subnet_id: SubnetId,
        state_root: HashValue,
        object_count: u64,
        last_anchor_id: u64,
    ) -> Self {
        Self {
            subnet_id,
            state_root,
            object_count: Some(object_count),
            last_anchor_id: Some(last_anchor_id),
        }
    }

    /// Compute the leaf hash for this entry.
    ///
    /// Leaf hash = SHA256(AGGREGATION_LEAF_PREFIX || subnet_id || state_root)
    pub fn leaf_hash(&self) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(AGGREGATION_LEAF_PREFIX);
        hasher.update(&self.subnet_id);
        hasher.update(self.state_root.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; HASH_LENGTH];
        bytes.copy_from_slice(&result);
        HashValue::new(bytes)
    }

    /// Check if this is the ROOT subnet.
    pub fn is_root(&self) -> bool {
        self.subnet_id == ROOT_SUBNET
    }
}

/// A proof of inclusion for a subnet's state root in the aggregation tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetAggregationProof {
    /// The subnet entry being proven
    pub entry: SubnetStateEntry,
    /// The index of this subnet in the sorted list
    pub index: usize,
    /// Sibling hashes from leaf to root
    pub siblings: Vec<HashValue>,
}

impl SubnetAggregationProof {
    /// Verify that the subnet entry is included in the aggregation tree.
    pub fn verify(&self, global_root: &HashValue) -> MerkleResult<()> {
        let leaf_hash = self.entry.leaf_hash();
        let computed_root = self.compute_root(&leaf_hash)?;

        if &computed_root == global_root {
            Ok(())
        } else {
            Err(MerkleError::InvalidProof(format!(
                "Root mismatch: expected {}, computed {}",
                global_root, computed_root
            )))
        }
    }

    /// Compute the root from the leaf hash.
    fn compute_root(&self, leaf_hash: &HashValue) -> MerkleResult<HashValue> {
        let mut current = *leaf_hash;
        let mut index = self.index;

        for sibling in &self.siblings {
            current = if index % 2 == 0 {
                // Current is left child
                hash_internal(&current, sibling)
            } else {
                // Current is right child
                hash_internal(sibling, &current)
            };
            index /= 2;
        }

        Ok(current)
    }
}

/// Hash two children to create an internal node.
fn hash_internal(left: &HashValue, right: &HashValue) -> HashValue {
    use crate::hash::prefix;
    let mut hasher = Sha256::new();
    hasher.update(prefix::INTERNAL);
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue::new(bytes)
}

/// Subnet Aggregation Tree.
///
/// Aggregates all subnet state roots into a single global state root.
/// Uses a binary merkle tree internally with deterministic ordering by SubnetId.
#[derive(Clone, Debug)]
pub struct SubnetAggregationTree {
    /// Sorted entries (ROOT subnet first, then by SubnetId)
    entries: Vec<SubnetStateEntry>,
    /// The underlying binary merkle tree
    tree: BinaryMerkleTree,
    /// Cached global root
    global_root: HashValue,
}

impl SubnetAggregationTree {
    /// Build an aggregation tree from subnet state entries.
    ///
    /// Entries are automatically sorted by SubnetId with ROOT first.
    pub fn build(mut entries: Vec<SubnetStateEntry>) -> Self {
        if entries.is_empty() {
            return Self {
                entries: vec![],
                tree: BinaryMerkleTree::build::<Vec<u8>>(&[]),
                global_root: HashValue::zero(),
            };
        }

        // Sort entries: ROOT first, then by SubnetId
        entries.sort_by(|a, b| {
            // ROOT subnet always comes first
            match (a.is_root(), b.is_root()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.subnet_id.cmp(&b.subnet_id),
            }
        });

        // Build leaf hashes
        let leaf_hashes: Vec<HashValue> = entries.iter().map(|e| e.leaf_hash()).collect();

        // Build binary merkle tree
        let tree = BinaryMerkleTree::build_from_hashes(leaf_hashes);
        let global_root = tree.root();

        Self {
            entries,
            tree,
            global_root,
        }
    }

    /// Get the global state root (root of the aggregation tree).
    pub fn root(&self) -> HashValue {
        self.global_root
    }

    /// Get the number of subnets in the tree.
    pub fn num_subnets(&self) -> usize {
        self.entries.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the sorted list of entries.
    pub fn entries(&self) -> &[SubnetStateEntry] {
        &self.entries
    }

    /// Find a subnet's entry by SubnetId.
    pub fn get_subnet(&self, subnet_id: &SubnetId) -> Option<&SubnetStateEntry> {
        self.entries.iter().find(|e| &e.subnet_id == subnet_id)
    }

    /// Get the state root for a specific subnet.
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<HashValue> {
        self.get_subnet(subnet_id).map(|e| e.state_root)
    }

    /// Get the ROOT subnet's state root.
    pub fn root_subnet_state(&self) -> Option<HashValue> {
        self.get_subnet_root(&ROOT_SUBNET)
    }

    /// Generate a proof of inclusion for a subnet.
    pub fn get_proof(&self, subnet_id: &SubnetId) -> MerkleResult<SubnetAggregationProof> {
        // Find the index of the subnet
        let index = self
            .entries
            .iter()
            .position(|e| &e.subnet_id == subnet_id)
            .ok_or(MerkleError::KeyNotFound)?;

        let entry = self.entries[index].clone();

        // Get the binary merkle proof
        let binary_proof = self.tree.get_proof(index)?;

        // Extract sibling hashes
        let siblings: Vec<HashValue> = binary_proof
            .siblings()
            .iter()
            .map(|n| n.hash())
            .collect();

        Ok(SubnetAggregationProof {
            entry,
            index,
            siblings,
        })
    }

    /// Verify a proof against this tree's root.
    pub fn verify_proof(&self, proof: &SubnetAggregationProof) -> MerkleResult<()> {
        proof.verify(&self.global_root)
    }

    /// Create a new tree with an updated subnet state.
    ///
    /// Returns a new tree with the updated entry. If the subnet doesn't exist,
    /// it will be added.
    pub fn update_subnet(&self, entry: SubnetStateEntry) -> Self {
        let mut new_entries: Vec<SubnetStateEntry> = self
            .entries
            .iter()
            .filter(|e| e.subnet_id != entry.subnet_id)
            .cloned()
            .collect();
        new_entries.push(entry);
        Self::build(new_entries)
    }

    /// Create a new tree with a subnet removed.
    pub fn remove_subnet(&self, subnet_id: &SubnetId) -> Self {
        let new_entries: Vec<SubnetStateEntry> = self
            .entries
            .iter()
            .filter(|e| &e.subnet_id != subnet_id)
            .cloned()
            .collect();
        Self::build(new_entries)
    }

    /// Get a map of SubnetId -> StateRoot for convenience.
    pub fn to_map(&self) -> std::collections::HashMap<SubnetId, HashValue> {
        self.entries
            .iter()
            .map(|e| (e.subnet_id, e.state_root))
            .collect()
    }
}

impl Default for SubnetAggregationTree {
    fn default() -> Self {
        Self::build(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_subnet_id(byte: u8) -> SubnetId {
        let mut id = [0u8; 32];
        id[0] = byte;
        id
    }

    fn make_state_root(byte: u8) -> HashValue {
        HashValue::new([byte; 32])
    }

    #[test]
    fn test_empty_tree() {
        let tree = SubnetAggregationTree::build(vec![]);
        assert!(tree.is_empty());
        assert_eq!(tree.root(), HashValue::zero());
    }

    #[test]
    fn test_single_subnet() {
        let entry = SubnetStateEntry::new(ROOT_SUBNET, make_state_root(1));
        let tree = SubnetAggregationTree::build(vec![entry.clone()]);

        assert_eq!(tree.num_subnets(), 1);
        assert!(tree.get_subnet(&ROOT_SUBNET).is_some());
        assert_ne!(tree.root(), HashValue::zero());
    }

    #[test]
    fn test_root_subnet_first() {
        let entries = vec![
            SubnetStateEntry::new(make_subnet_id(5), make_state_root(5)),
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(2), make_state_root(2)),
        ];

        let tree = SubnetAggregationTree::build(entries);

        // ROOT should be first
        assert!(tree.entries()[0].is_root());
        assert_eq!(tree.entries()[0].subnet_id, ROOT_SUBNET);
    }

    #[test]
    fn test_deterministic_ordering() {
        let entries1 = vec![
            SubnetStateEntry::new(make_subnet_id(3), make_state_root(3)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
        ];

        let entries2 = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
            SubnetStateEntry::new(make_subnet_id(3), make_state_root(3)),
        ];

        let tree1 = SubnetAggregationTree::build(entries1);
        let tree2 = SubnetAggregationTree::build(entries2);

        // Same entries in different order should produce same root
        assert_eq!(tree1.root(), tree2.root());
    }

    #[test]
    fn test_inclusion_proof() {
        let entries = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
            SubnetStateEntry::new(make_subnet_id(2), make_state_root(2)),
            SubnetStateEntry::new(make_subnet_id(3), make_state_root(3)),
        ];

        let tree = SubnetAggregationTree::build(entries);

        // Test proof for each subnet
        for entry in tree.entries() {
            let proof = tree.get_proof(&entry.subnet_id).unwrap();
            assert!(proof.verify(&tree.root()).is_ok());
        }
    }

    #[test]
    fn test_proof_wrong_root_fails() {
        let entries = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
        ];

        let tree = SubnetAggregationTree::build(entries);
        let proof = tree.get_proof(&ROOT_SUBNET).unwrap();

        let wrong_root = HashValue::new([0xAB; 32]);
        assert!(proof.verify(&wrong_root).is_err());
    }

    #[test]
    fn test_update_subnet() {
        let entries = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
        ];

        let tree = SubnetAggregationTree::build(entries);
        let old_root = tree.root();

        // Update subnet 1's state
        let new_entry = SubnetStateEntry::new(make_subnet_id(1), make_state_root(100));
        let new_tree = tree.update_subnet(new_entry);

        // Root should change
        assert_ne!(new_tree.root(), old_root);
        // Still have 2 subnets
        assert_eq!(new_tree.num_subnets(), 2);
        // New state root should be updated
        assert_eq!(
            new_tree.get_subnet_root(&make_subnet_id(1)),
            Some(make_state_root(100))
        );
    }

    #[test]
    fn test_add_new_subnet() {
        let entries = vec![SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0))];

        let tree = SubnetAggregationTree::build(entries);

        // Add a new subnet
        let new_entry = SubnetStateEntry::new(make_subnet_id(5), make_state_root(5));
        let new_tree = tree.update_subnet(new_entry);

        assert_eq!(new_tree.num_subnets(), 2);
        assert!(new_tree.get_subnet(&make_subnet_id(5)).is_some());
    }

    #[test]
    fn test_remove_subnet() {
        let entries = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
            SubnetStateEntry::new(make_subnet_id(2), make_state_root(2)),
        ];

        let tree = SubnetAggregationTree::build(entries);
        let new_tree = tree.remove_subnet(&make_subnet_id(1));

        assert_eq!(new_tree.num_subnets(), 2);
        assert!(new_tree.get_subnet(&make_subnet_id(1)).is_none());
    }

    #[test]
    fn test_leaf_hash_deterministic() {
        let entry1 = SubnetStateEntry::new(make_subnet_id(1), make_state_root(1));
        let entry2 = SubnetStateEntry::new(make_subnet_id(1), make_state_root(1));

        assert_eq!(entry1.leaf_hash(), entry2.leaf_hash());
    }

    #[test]
    fn test_different_entries_different_hashes() {
        let entry1 = SubnetStateEntry::new(make_subnet_id(1), make_state_root(1));
        let entry2 = SubnetStateEntry::new(make_subnet_id(2), make_state_root(1));
        let entry3 = SubnetStateEntry::new(make_subnet_id(1), make_state_root(2));

        assert_ne!(entry1.leaf_hash(), entry2.leaf_hash());
        assert_ne!(entry1.leaf_hash(), entry3.leaf_hash());
    }

    #[test]
    fn test_to_map() {
        let entries = vec![
            SubnetStateEntry::new(ROOT_SUBNET, make_state_root(0)),
            SubnetStateEntry::new(make_subnet_id(1), make_state_root(1)),
        ];

        let tree = SubnetAggregationTree::build(entries);
        let map = tree.to_map();

        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&ROOT_SUBNET), Some(&make_state_root(0)));
        assert_eq!(map.get(&make_subnet_id(1)), Some(&make_state_root(1)));
    }
}
