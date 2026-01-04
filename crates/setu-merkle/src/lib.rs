//! # setu-merkle
//!
//! Merkle tree implementations for the Setu network.
//!
//! This crate provides two types of Merkle trees:
//!
//! - [`binary::BinaryMerkleTree`]: A simple binary Merkle tree for ordered data commitments
//! - [`sparse::SparseMerkleTree`]: A 256-bit sparse Merkle tree for key-value storage
//!
//! ## Design Philosophy
//!
//! Setu uses a hybrid Merkle architecture:
//!
//! - **Binary Merkle Tree**: Used for event commitments in DAG folding, where data is an
//!   ordered list and we need efficient inclusion proofs.
//!
//! - **Sparse Merkle Tree**: Used for object state storage (Coin, Profile, Credential, etc.),
//!   where keys are 256-bit identifiers and we need efficient non-inclusion proofs.

pub mod binary;
pub mod error;
pub mod hash;
pub mod sparse;

pub use binary::{BinaryMerkleProof, BinaryMerkleTree};
pub use error::{MerkleError, MerkleResult};
pub use hash::HashValue;
pub use sparse::{SparseMerkleProof, SparseMerkleTree};

/// The length of hash digests used in merkle trees (32 bytes = 256 bits)
pub const HASH_LENGTH: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_tree_basic() {
        let leaves: Vec<Vec<u8>> = vec![
            b"leaf0".to_vec(),
            b"leaf1".to_vec(),
            b"leaf2".to_vec(),
            b"leaf3".to_vec(),
        ];

        let tree = BinaryMerkleTree::build(&leaves);
        let root = tree.root();

        // Verify all proofs
        for (i, leaf) in leaves.iter().enumerate() {
            let proof = tree.get_proof(i).unwrap();
            assert!(proof.verify(&root, leaf, i).is_ok());
        }
    }

    #[test]
    fn test_sparse_tree_basic() {
        let mut tree = SparseMerkleTree::new();

        let key1 = HashValue::from_slice(&[1u8; 32]).unwrap();
        let key2 = HashValue::from_slice(&[2u8; 32]).unwrap();

        tree.insert(key1, b"value1".to_vec());
        tree.insert(key2, b"value2".to_vec());

        assert_eq!(tree.get(&key1), Some(&b"value1".to_vec()));
        assert_eq!(tree.get(&key2), Some(&b"value2".to_vec()));

        let root = tree.root();
        assert_ne!(root, HashValue::zero());
    }
}
