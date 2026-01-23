// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Address derivation utilities for different use cases.
//!
//! This module provides:
//! - Ethereum-style address derivation from secp256k1 public keys (for Validator/Solver)
//! - Address derivation from Nostr public keys (for Users)

use crate::error::KeyError;
use sha2::{Sha256, Digest as Sha2Digest};
use sha3::{Keccak256, Digest as Sha3Digest};

/// Ethereum-style address (20 bytes, 0x-prefixed hex string)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthereumAddress(pub [u8; 20]);

impl EthereumAddress {
    /// Create from raw bytes
    pub fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Get raw bytes
    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Convert to hex string with 0x prefix
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.0))
    }

    /// Parse from hex string (with or without 0x prefix)
    pub fn from_hex(s: &str) -> Result<Self, KeyError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let bytes = hex::decode(s).map_err(|e| KeyError::Decoding(e.to_string()))?;
        if bytes.len() != 20 {
            return Err(KeyError::Decoding(format!(
                "Invalid Ethereum address length: expected 20, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl std::fmt::Display for EthereumAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Derive Ethereum-style address from secp256k1 public key.
///
/// Algorithm:
/// 1. Take the uncompressed public key (65 bytes: 0x04 || x || y)
/// 2. Remove the 0x04 prefix (64 bytes)
/// 3. Compute Keccak256 hash
/// 4. Take the last 20 bytes as the address
///
/// This is the standard Ethereum address derivation.
pub fn derive_ethereum_address_from_secp256k1(public_key: &[u8]) -> Result<EthereumAddress, KeyError> {
    // Validate public key format
    if public_key.len() != 65 {
        return Err(KeyError::InvalidKeyFormat(format!(
            "Expected 65-byte uncompressed secp256k1 public key, got {} bytes",
            public_key.len()
        )));
    }
    
    if public_key[0] != 0x04 {
        return Err(KeyError::InvalidKeyFormat(
            "Public key must start with 0x04 (uncompressed format)".to_string()
        ));
    }
    
    // Remove the 0x04 prefix
    let key_without_prefix = &public_key[1..];
    
    // Compute Keccak256 hash
    let hash = Keccak256::digest(key_without_prefix);
    
    // Take the last 20 bytes
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    
    Ok(EthereumAddress(address))
}

/// Derive address from Nostr public key (32-byte Schnorr x-only public key).
///
/// Algorithm:
/// 1. Take the Nostr public key (32 bytes, x-only Schnorr)
/// 2. Compute SHA256 hash
/// 3. Take the first 20 bytes as the address
///
/// This creates an Ethereum-compatible address format from Nostr keys.
pub fn derive_address_from_nostr_pubkey(nostr_pubkey: &[u8]) -> Result<EthereumAddress, KeyError> {
    // Validate Nostr public key format
    if nostr_pubkey.len() != 32 {
        return Err(KeyError::InvalidKeyFormat(format!(
            "Expected 32-byte Nostr public key, got {} bytes",
            nostr_pubkey.len()
        )));
    }
    
    // Compute SHA256 hash
    let hash = Sha256::digest(nostr_pubkey);
    
    // Take the first 20 bytes
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[0..20]);
    
    Ok(EthereumAddress(address))
}

/// Verify that an address was correctly derived from a Nostr public key.
pub fn verify_nostr_address_derivation(
    address: &str,
    nostr_pubkey: &[u8],
) -> Result<bool, KeyError> {
    let expected_address = derive_address_from_nostr_pubkey(nostr_pubkey)?;
    let provided_address = EthereumAddress::from_hex(address)?;
    Ok(expected_address == provided_address)
}

/// Verify that an address was correctly derived from a secp256k1 public key.
pub fn verify_ethereum_address_derivation(
    address: &str,
    public_key: &[u8],
) -> Result<bool, KeyError> {
    let expected_address = derive_ethereum_address_from_secp256k1(public_key)?;
    let provided_address = EthereumAddress::from_hex(address)?;
    Ok(expected_address == provided_address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{SigningKey, VerifyingKey};
    use k256::elliptic_curve::sec1::ToEncodedPoint;

    #[test]
    fn test_ethereum_address_derivation() {
        // Generate a secp256k1 keypair
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = VerifyingKey::from(&signing_key);
        
        // Get uncompressed public key (65 bytes)
        let public_key = verifying_key.to_encoded_point(false);
        let public_key_bytes = public_key.as_bytes();
        
        // Derive address
        let address = derive_ethereum_address_from_secp256k1(public_key_bytes).unwrap();
        
        // Verify format
        assert_eq!(address.as_bytes().len(), 20);
        let hex = address.to_hex();
        assert!(hex.starts_with("0x"));
        assert_eq!(hex.len(), 42); // 0x + 40 hex chars
        
        // Verify derivation
        assert!(verify_ethereum_address_derivation(&hex, public_key_bytes).unwrap());
    }

    #[test]
    fn test_nostr_address_derivation() {
        // Mock Nostr public key (32 bytes)
        let nostr_pubkey = [0x42u8; 32];
        
        // Derive address
        let address = derive_address_from_nostr_pubkey(&nostr_pubkey).unwrap();
        
        // Verify format
        assert_eq!(address.as_bytes().len(), 20);
        let hex = address.to_hex();
        assert!(hex.starts_with("0x"));
        assert_eq!(hex.len(), 42);
        
        // Verify derivation
        assert!(verify_nostr_address_derivation(&hex, &nostr_pubkey).unwrap());
    }

    #[test]
    fn test_deterministic_derivation() {
        // Same Nostr pubkey should always produce same address
        let nostr_pubkey = [0x42u8; 32];
        let addr1 = derive_address_from_nostr_pubkey(&nostr_pubkey).unwrap();
        let addr2 = derive_address_from_nostr_pubkey(&nostr_pubkey).unwrap();
        assert_eq!(addr1, addr2);
    }

    #[test]
    fn test_different_keys_different_addresses() {
        let nostr_pubkey1 = [0x42u8; 32];
        let nostr_pubkey2 = [0x43u8; 32];
        
        let addr1 = derive_address_from_nostr_pubkey(&nostr_pubkey1).unwrap();
        let addr2 = derive_address_from_nostr_pubkey(&nostr_pubkey2).unwrap();
        
        assert_ne!(addr1, addr2);
    }

    #[test]
    fn test_invalid_nostr_pubkey_length() {
        let invalid_pubkey = [0x42u8; 31]; // Wrong length
        let result = derive_address_from_nostr_pubkey(&invalid_pubkey);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_secp256k1_pubkey_length() {
        let invalid_pubkey = [0x04u8; 64]; // Wrong length
        let result = derive_ethereum_address_from_secp256k1(&invalid_pubkey);
        assert!(result.is_err());
    }

    #[test]
    fn test_ethereum_address_hex_parsing() {
        let addr = EthereumAddress([0x42u8; 20]);
        let hex = addr.to_hex();
        let parsed = EthereumAddress::from_hex(&hex).unwrap();
        assert_eq!(addr, parsed);
        
        // Test without 0x prefix
        let hex_no_prefix = &hex[2..];
        let parsed2 = EthereumAddress::from_hex(hex_no_prefix).unwrap();
        assert_eq!(addr, parsed2);
    }
}

