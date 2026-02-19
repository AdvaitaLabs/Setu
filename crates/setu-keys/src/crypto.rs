// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Core cryptographic types for Setu.
//!
//! This module provides the fundamental cryptographic primitives:
//! - `SignatureScheme`: Supported signature algorithms
//! - `SetuKeyPair`: Unified keypair type supporting multiple schemes
//! - `PublicKey`: Public key representation
//! - `Signature`: Digital signature
//! - `SetuAddress`: Account address derived from public key

use crate::error::KeyError;
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use ed25519_dalek::{SigningKey as Ed25519SigningKey, VerifyingKey as Ed25519VerifyingKey};
use k256::ecdsa::{
    signature::Signer as Secp256k1Signer, signature::Verifier as Secp256k1Verifier,
    Signature as Secp256k1Signature, SigningKey as Secp256k1SigningKey,
    VerifyingKey as Secp256k1VerifyingKey,
};
use p256::ecdsa::{
    Signature as Secp256r1Signature, SigningKey as Secp256r1SigningKey,
    VerifyingKey as Secp256r1VerifyingKey,
};

/// Type alias for Blake2b-256
type Blake2b256 = Blake2b<U32>;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::str::FromStr;
use zeroize::Zeroize;

/// Flag bytes for different signature schemes (used in serialization).
pub const ED25519_FLAG: u8 = 0x00;
pub const SECP256K1_FLAG: u8 = 0x01;
pub const SECP256R1_FLAG: u8 = 0x02;

/// Supported signature schemes in Setu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignatureScheme {
    ED25519,
    Secp256k1,
    Secp256r1,
}

impl SignatureScheme {
    /// Get the flag byte for this scheme.
    pub fn flag(&self) -> u8 {
        match self {
            SignatureScheme::ED25519 => ED25519_FLAG,
            SignatureScheme::Secp256k1 => SECP256K1_FLAG,
            SignatureScheme::Secp256r1 => SECP256R1_FLAG,
        }
    }

    /// Create scheme from flag byte.
    pub fn from_flag(flag: u8) -> Result<Self, KeyError> {
        match flag {
            ED25519_FLAG => Ok(SignatureScheme::ED25519),
            SECP256K1_FLAG => Ok(SignatureScheme::Secp256k1),
            SECP256R1_FLAG => Ok(SignatureScheme::Secp256r1),
            _ => Err(KeyError::UnsupportedScheme(format!(
                "Unknown flag: {}",
                flag
            ))),
        }
    }
}

impl Display for SignatureScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureScheme::ED25519 => write!(f, "ed25519"),
            SignatureScheme::Secp256k1 => write!(f, "secp256k1"),
            SignatureScheme::Secp256r1 => write!(f, "secp256r1"),
        }
    }
}

impl FromStr for SignatureScheme {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ed25519" => Ok(SignatureScheme::ED25519),
            "secp256k1" => Ok(SignatureScheme::Secp256k1),
            "secp256r1" | "p256" => Ok(SignatureScheme::Secp256r1),
            _ => Err(KeyError::UnsupportedScheme(s.to_string())),
        }
    }
}

impl Default for SignatureScheme {
    fn default() -> Self {
        SignatureScheme::ED25519
    }
}

/// Setu account address (32 bytes, derived from public key using Blake2b-256).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SetuAddress([u8; 32]);

impl SetuAddress {
    /// Create address from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to hex string with 0x prefix.
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.0))
    }
}

impl Display for SetuAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl FromStr for SetuAddress {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let bytes = hex::decode(s).map_err(|e| KeyError::Decoding(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(KeyError::Decoding(format!(
                "Invalid address length: expected 32, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl From<&PublicKey> for SetuAddress {
    fn from(pk: &PublicKey) -> Self {
        // Address = Blake2b-256(flag || public_key_bytes)
        let mut hasher = Blake2b256::new();
        hasher.update([pk.scheme().flag()]);
        hasher.update(pk.as_bytes());
        let hash = hasher.finalize();
        let mut addr = [0u8; 32];
        addr.copy_from_slice(&hash);
        Self(addr)
    }
}

impl From<PublicKey> for SetuAddress {
    fn from(pk: PublicKey) -> Self {
        (&pk).into()
    }
}

/// Public key representation supporting multiple schemes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicKey {
    Ed25519(Ed25519VerifyingKey),
    Secp256k1(Secp256k1VerifyingKey),
    Secp256r1(Secp256r1VerifyingKey),
}

impl PublicKey {
    /// Get the signature scheme.
    pub fn scheme(&self) -> SignatureScheme {
        match self {
            PublicKey::Ed25519(_) => SignatureScheme::ED25519,
            PublicKey::Secp256k1(_) => SignatureScheme::Secp256k1,
            PublicKey::Secp256r1(_) => SignatureScheme::Secp256r1,
        }
    }

    /// Get the raw public key bytes (without flag).
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            PublicKey::Ed25519(pk) => pk.as_bytes().to_vec(),
            PublicKey::Secp256k1(pk) => pk.to_sec1_bytes().to_vec(),
            PublicKey::Secp256r1(pk) => pk.to_sec1_bytes().to_vec(),
        }
    }

    /// Encode as Base64 with flag prefix.
    pub fn encode_base64(&self) -> String {
        let mut bytes = vec![self.scheme().flag()];
        bytes.extend(self.as_bytes());
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    }

    /// Decode from Base64 with flag prefix.
    pub fn decode_base64(s: &str) -> Result<Self, KeyError> {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
            .map_err(|e| KeyError::Decoding(e.to_string()))?;
        if bytes.is_empty() {
            return Err(KeyError::Decoding("Empty public key".to_string()));
        }
        let flag = bytes[0];
        let key_bytes = &bytes[1..];
        Self::from_bytes(SignatureScheme::from_flag(flag)?, key_bytes)
    }

    /// Create from raw bytes and scheme.
    pub fn from_bytes(scheme: SignatureScheme, bytes: &[u8]) -> Result<Self, KeyError> {
        match scheme {
            SignatureScheme::ED25519 => {
                if bytes.len() != 32 {
                    return Err(KeyError::InvalidKeyFormat(
                        "Ed25519 public key must be 32 bytes".to_string(),
                    ));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                let pk = Ed25519VerifyingKey::from_bytes(&arr)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
                Ok(PublicKey::Ed25519(pk))
            }
            SignatureScheme::Secp256k1 => {
                let pk = Secp256k1VerifyingKey::from_sec1_bytes(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
                Ok(PublicKey::Secp256k1(pk))
            }
            SignatureScheme::Secp256r1 => {
                let pk = Secp256r1VerifyingKey::from_sec1_bytes(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
                Ok(PublicKey::Secp256r1(pk))
            }
        }
    }

    /// Verify a signature against this public key.
    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), KeyError> {
        match (self, sig) {
            (PublicKey::Ed25519(pk), Signature::Ed25519(sig)) => {
                use ed25519_dalek::Verifier;
                pk.verify(msg, sig)
                    .map_err(|e| KeyError::SignatureVerification(e.to_string()))
            }
            (PublicKey::Secp256k1(pk), Signature::Secp256k1(sig)) => pk
                .verify(msg, sig)
                .map_err(|e| KeyError::SignatureVerification(e.to_string())),
            (PublicKey::Secp256r1(pk), Signature::Secp256r1(sig)) => pk
                .verify(msg, sig)
                .map_err(|e| KeyError::SignatureVerification(e.to_string())),
            _ => Err(KeyError::SignatureVerification(
                "Signature scheme mismatch".to_string(),
            )),
        }
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.encode_base64())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        PublicKey::decode_base64(&s).map_err(serde::de::Error::custom)
    }
}

/// Digital signature supporting multiple schemes.
#[derive(Debug, Clone)]
pub enum Signature {
    Ed25519(ed25519_dalek::Signature),
    Secp256k1(Secp256k1Signature),
    Secp256r1(Secp256r1Signature),
}

impl Signature {
    /// Get the signature scheme.
    pub fn scheme(&self) -> SignatureScheme {
        match self {
            Signature::Ed25519(_) => SignatureScheme::ED25519,
            Signature::Secp256k1(_) => SignatureScheme::Secp256k1,
            Signature::Secp256r1(_) => SignatureScheme::Secp256r1,
        }
    }

    /// Get the raw signature bytes.
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            Signature::Ed25519(sig) => sig.to_bytes().to_vec(),
            Signature::Secp256k1(sig) => sig.to_bytes().to_vec(),
            Signature::Secp256r1(sig) => sig.to_bytes().to_vec(),
        }
    }

    /// Encode as Base64 with flag prefix.
    pub fn encode_base64(&self) -> String {
        let mut bytes = vec![self.scheme().flag()];
        bytes.extend(self.as_bytes());
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    }

    /// Decode from Base64 with flag prefix.
    pub fn decode_base64(s: &str) -> Result<Self, KeyError> {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
            .map_err(|e| KeyError::Decoding(e.to_string()))?;
        if bytes.is_empty() {
            return Err(KeyError::Decoding("Empty signature".to_string()));
        }
        let flag = bytes[0];
        let sig_bytes = &bytes[1..];
        Self::from_bytes(SignatureScheme::from_flag(flag)?, sig_bytes)
    }

    /// Create from raw bytes and scheme.
    pub fn from_bytes(scheme: SignatureScheme, bytes: &[u8]) -> Result<Self, KeyError> {
        match scheme {
            SignatureScheme::ED25519 => {
                if bytes.len() != 64 {
                    return Err(KeyError::InvalidKeyFormat(
                        "Ed25519 signature must be 64 bytes".to_string(),
                    ));
                }
                let mut arr = [0u8; 64];
                arr.copy_from_slice(bytes);
                let sig = ed25519_dalek::Signature::from_bytes(&arr);
                Ok(Signature::Ed25519(sig))
            }
            SignatureScheme::Secp256k1 => {
                let sig = Secp256k1Signature::from_slice(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
                Ok(Signature::Secp256k1(sig))
            }
            SignatureScheme::Secp256r1 => {
                let sig = Secp256r1Signature::from_slice(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
                Ok(Signature::Secp256r1(sig))
            }
        }
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.encode_base64())
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Signature::decode_base64(&s).map_err(serde::de::Error::custom)
    }
}

/// Unified keypair type supporting multiple signature schemes.
///
/// The private key is zeroized on drop for security.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct SetuKeyPair {
    #[zeroize(skip)]
    scheme: SignatureScheme,
    secret_bytes: Vec<u8>,
}

impl SetuKeyPair {
    /// Create a new Ed25519 keypair.
    pub fn ed25519(signing_key: Ed25519SigningKey) -> Self {
        Self {
            scheme: SignatureScheme::ED25519,
            secret_bytes: signing_key.to_bytes().to_vec(),
        }
    }

    /// Create a new Secp256k1 keypair.
    pub fn secp256k1(signing_key: Secp256k1SigningKey) -> Self {
        Self {
            scheme: SignatureScheme::Secp256k1,
            secret_bytes: signing_key.to_bytes().to_vec(),
        }
    }

    /// Create a new Secp256r1 keypair.
    pub fn secp256r1(signing_key: Secp256r1SigningKey) -> Self {
        Self {
            scheme: SignatureScheme::Secp256r1,
            secret_bytes: signing_key.to_bytes().to_vec(),
        }
    }

    /// Get the signature scheme.
    pub fn scheme(&self) -> SignatureScheme {
        self.scheme
    }

    /// Get the public key.
    pub fn public(&self) -> PublicKey {
        match self.scheme {
            SignatureScheme::ED25519 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&self.secret_bytes);
                let sk = Ed25519SigningKey::from_bytes(&arr);
                PublicKey::Ed25519(sk.verifying_key())
            }
            SignatureScheme::Secp256k1 => {
                let sk = Secp256k1SigningKey::from_slice(&self.secret_bytes).unwrap();
                PublicKey::Secp256k1(*sk.verifying_key())
            }
            SignatureScheme::Secp256r1 => {
                let sk = Secp256r1SigningKey::from_slice(&self.secret_bytes).unwrap();
                PublicKey::Secp256r1(*sk.verifying_key())
            }
        }
    }

    /// Get the Setu address for this keypair.
    pub fn address(&self) -> SetuAddress {
        (&self.public()).into()
    }

    /// Sign a message.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        match self.scheme {
            SignatureScheme::ED25519 => {
                use ed25519_dalek::Signer;
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&self.secret_bytes);
                let sk = Ed25519SigningKey::from_bytes(&arr);
                let sig = sk.sign(msg);
                Signature::Ed25519(sig)
            }
            SignatureScheme::Secp256k1 => {
                let sk = Secp256k1SigningKey::from_slice(&self.secret_bytes).unwrap();
                let sig: Secp256k1Signature = sk.sign(msg);
                Signature::Secp256k1(sig)
            }
            SignatureScheme::Secp256r1 => {
                let sk = Secp256r1SigningKey::from_slice(&self.secret_bytes).unwrap();
                let sig: Secp256r1Signature = sk.sign(msg);
                Signature::Secp256r1(sig)
            }
        }
    }

    /// Encode as Base64 with flag prefix.
    pub fn encode_base64(&self) -> String {
        let mut bytes = vec![self.scheme.flag()];
        bytes.extend(&self.secret_bytes);
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    }

    /// Decode from Base64 with flag prefix.
    pub fn decode_base64(s: &str) -> Result<Self, KeyError> {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
            .map_err(|e| KeyError::Decoding(e.to_string()))?;
        if bytes.is_empty() {
            return Err(KeyError::Decoding("Empty keypair".to_string()));
        }
        let flag = bytes[0];
        let key_bytes = &bytes[1..];
        Self::from_bytes(SignatureScheme::from_flag(flag)?, key_bytes)
    }

    /// Create from raw secret bytes and scheme.
    pub fn from_bytes(scheme: SignatureScheme, bytes: &[u8]) -> Result<Self, KeyError> {
        // Validate the bytes by attempting to create a keypair
        match scheme {
            SignatureScheme::ED25519 => {
                if bytes.len() != 32 {
                    return Err(KeyError::InvalidKeyFormat(
                        "Ed25519 secret key must be 32 bytes".to_string(),
                    ));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                let _ = Ed25519SigningKey::from_bytes(&arr);
            }
            SignatureScheme::Secp256k1 => {
                let _ = Secp256k1SigningKey::from_slice(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
            }
            SignatureScheme::Secp256r1 => {
                let _ = Secp256r1SigningKey::from_slice(bytes)
                    .map_err(|e| KeyError::InvalidKeyFormat(e.to_string()))?;
            }
        }

        Ok(Self {
            scheme,
            secret_bytes: bytes.to_vec(),
        })
    }

    /// Generate a random keypair.
    pub fn generate(scheme: SignatureScheme) -> Self {
        let mut rng = rand::thread_rng();
        match scheme {
            SignatureScheme::ED25519 => {
                let sk = Ed25519SigningKey::generate(&mut rng);
                Self::ed25519(sk)
            }
            SignatureScheme::Secp256k1 => {
                let sk = Secp256k1SigningKey::random(&mut rng);
                Self::secp256k1(sk)
            }
            SignatureScheme::Secp256r1 => {
                let sk = Secp256r1SigningKey::random(&mut rng);
                Self::secp256r1(sk)
            }
        }
    }
}

impl Clone for SetuKeyPair {
    fn clone(&self) -> Self {
        Self {
            scheme: self.scheme,
            secret_bytes: self.secret_bytes.clone(),
        }
    }
}

impl std::fmt::Debug for SetuKeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetuKeyPair")
            .field("scheme", &self.scheme)
            .field("public_key", &self.public())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_sign_verify() {
        for scheme in [
            SignatureScheme::ED25519,
            SignatureScheme::Secp256k1,
            SignatureScheme::Secp256r1,
        ] {
            let kp = SetuKeyPair::generate(scheme);
            let msg = b"hello setu";
            let sig = kp.sign(msg);
            assert!(kp.public().verify(msg, &sig).is_ok());
        }
    }

    #[test]
    fn test_keypair_encoding() {
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        let encoded = kp.encode_base64();
        let decoded = SetuKeyPair::decode_base64(&encoded).unwrap();
        assert_eq!(kp.address(), decoded.address());
    }

    #[test]
    fn test_address_from_public_key() {
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        let addr = kp.address();
        let addr_str = addr.to_hex();
        assert!(addr_str.starts_with("0x"));
        assert_eq!(addr_str.len(), 66); // 0x + 64 hex chars
    }
}
