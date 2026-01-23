// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Setu cryptographic key management library.
//!
//! This crate provides:
//! - Key generation and derivation from BIP39 mnemonics
//! - Multiple signature schemes (Ed25519, Secp256k1, Secp256r1)
//! - Key storage (file-based and in-memory)
//! - Address derivation from public keys

// Suppress warning from zeroize macro
#![allow(unused_assignments)]

pub mod crypto;
pub mod error;
pub mod key_derive;
pub mod key_identity;
pub mod keypair_file;
pub mod keystore;
pub mod address_derive;

pub use crypto::{PublicKey, SetuAddress, SetuKeyPair, Signature, SignatureScheme};
pub use error::KeyError;
pub use key_derive::{derive_key_pair_from_path, generate_new_key};
pub use key_identity::KeyIdentity;
pub use keypair_file::{read_keypair_from_file, write_keypair_to_file, read_key};
pub use keystore::{AccountKeystore, FileBasedKeystore, InMemKeystore, Keystore};
pub use address_derive::{
    EthereumAddress, 
    derive_ethereum_address_from_secp256k1,
    derive_address_from_nostr_pubkey,
    verify_nostr_address_derivation,
    verify_ethereum_address_derivation,
};

// Convenience alias
pub use read_keypair_from_file as load_keypair;

// Re-export KeyPair type for convenience
pub use SetuKeyPair as KeyPair;
