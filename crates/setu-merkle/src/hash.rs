//! Hash utilities and types for merkle trees.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::{MerkleError, MerkleResult, HASH_LENGTH};

/// A 256-bit hash value used as keys and node hashes in merkle trees.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct HashValue([u8; HASH_LENGTH]);

impl HashValue {
    /// The zero hash (all zeros)
    pub const ZERO: HashValue = HashValue([0u8; HASH_LENGTH]);

    /// Create a new HashValue from a fixed-size array
    pub fn new(bytes: [u8; HASH_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Create a HashValue from a slice
    pub fn from_slice(bytes: &[u8]) -> MerkleResult<Self> {
        if bytes.len() != HASH_LENGTH {
            return Err(MerkleError::InvalidHashLength {
                expected: HASH_LENGTH,
                got: bytes.len(),
            });
        }
        let mut arr = [0u8; HASH_LENGTH];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    /// Create a HashValue from hex string
    pub fn from_hex(hex_str: &str) -> MerkleResult<Self> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(hex_str)
            .map_err(|e| MerkleError::InvalidInput(format!("Invalid hex: {}", e)))?;
        Self::from_slice(&bytes)
    }

    /// Returns the zero hash
    pub fn zero() -> Self {
        Self::ZERO
    }

    /// Check if this is the zero hash
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; HASH_LENGTH]
    }

    /// Get the underlying bytes
    pub fn as_bytes(&self) -> &[u8; HASH_LENGTH] {
        &self.0
    }

    /// Convert to a Vec<u8>
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    /// Get the nibble (half-byte) at the given index (0-63)
    /// 
    /// Nibbles are used in sparse merkle trees to traverse the tree.
    /// Index 0 is the most significant nibble.
    pub fn nibble(&self, index: usize) -> u8 {
        assert!(index < HASH_LENGTH * 2, "nibble index out of bounds");
        let byte = self.0[index / 2];
        if index % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        }
    }

    /// Get the bit at the given index (0-255)
    /// 
    /// Index 0 is the most significant bit.
    pub fn bit(&self, index: usize) -> bool {
        assert!(index < HASH_LENGTH * 8, "bit index out of bounds");
        let byte = self.0[index / 8];
        let bit_pos = 7 - (index % 8);
        (byte >> bit_pos) & 1 == 1
    }

    /// Compute the common prefix length with another hash (in bits)
    pub fn common_prefix_bits(&self, other: &HashValue) -> usize {
        for i in 0..HASH_LENGTH {
            if self.0[i] != other.0[i] {
                let xor = self.0[i] ^ other.0[i];
                return i * 8 + xor.leading_zeros() as usize;
            }
        }
        HASH_LENGTH * 8
    }
}

impl fmt::Display for HashValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for HashValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HashValue({})", self)
    }
}

impl AsRef<[u8]> for HashValue {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; HASH_LENGTH]> for HashValue {
    fn from(bytes: [u8; HASH_LENGTH]) -> Self {
        Self(bytes)
    }
}

/// Domain separation prefixes for hashing
pub mod prefix {
    /// Prefix for leaf nodes in binary merkle tree
    pub const LEAF: &[u8] = &[0x00];
    /// Prefix for internal nodes in binary merkle tree
    pub const INTERNAL: &[u8] = &[0x01];
    /// Prefix for sparse merkle tree leaf nodes
    pub const SPARSE_LEAF: &[u8] = &[0x02];
    /// Prefix for sparse merkle tree internal nodes
    pub const SPARSE_INTERNAL: &[u8] = &[0x03];
}

/// Hash data using SHA-256
pub fn sha256(data: &[u8]) -> HashValue {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue(bytes)
}

/// Hash data with a domain separation prefix
pub fn sha256_with_prefix(prefix: &[u8], data: &[u8]) -> HashValue {
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(data);
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue(bytes)
}

/// Hash two child hashes to create parent hash (for binary merkle tree)
pub fn hash_internal(left: &HashValue, right: &HashValue) -> HashValue {
    let mut hasher = Sha256::new();
    hasher.update(prefix::INTERNAL);
    hasher.update(left.as_bytes());
    hasher.update(right.as_bytes());
    let result = hasher.finalize();
    let mut bytes = [0u8; HASH_LENGTH];
    bytes.copy_from_slice(&result);
    HashValue(bytes)
}

/// Hash leaf data (for binary merkle tree)
pub fn hash_leaf(data: &[u8]) -> HashValue {
    sha256_with_prefix(prefix::LEAF, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_value_nibble() {
        let hash = HashValue::new([0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        
        assert_eq!(hash.nibble(0), 0x0A);
        assert_eq!(hash.nibble(1), 0x0B);
        assert_eq!(hash.nibble(2), 0x0C);
        assert_eq!(hash.nibble(3), 0x0D);
    }

    #[test]
    fn test_hash_value_bit() {
        let hash = HashValue::new([0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                   0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        
        assert!(hash.bit(0));   // MSB of first byte
        assert!(!hash.bit(1));
        assert!(hash.bit(255)); // LSB of last byte
    }

    #[test]
    fn test_common_prefix_bits() {
        let h1 = HashValue::new([0xFF; 32]);
        let h2 = HashValue::new([0xFF; 32]);
        assert_eq!(h1.common_prefix_bits(&h2), 256);

        let h3 = HashValue::new([0x80; 32]);
        let h4 = HashValue::new([0x00; 32]);
        assert_eq!(h3.common_prefix_bits(&h4), 0);

        let mut arr1 = [0xFF; 32];
        let mut arr2 = [0xFF; 32];
        arr1[0] = 0xF0;
        arr2[0] = 0xF8;
        let h5 = HashValue::new(arr1);
        let h6 = HashValue::new(arr2);
        assert_eq!(h5.common_prefix_bits(&h6), 4);
    }
}
