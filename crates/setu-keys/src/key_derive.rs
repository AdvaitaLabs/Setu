// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Key derivation from BIP39 mnemonics.
//!
//! This module implements hierarchical deterministic (HD) key derivation
//! following BIP32/BIP44/SLIP-0010 standards.
//!
//! Setu uses coin type 99999 for its derivation paths:
//! - Ed25519: m/44'/99999'/0'/0'/{index}' (SLIP-0010, all hardened)
//! - Secp256k1: m/54'/99999'/0'/0/{index} (BIP-32/44, first 3 hardened)
//! - Secp256r1: m/74'/99999'/0'/0/{index} (BIP-32/44, first 3 hardened)

use crate::crypto::{SetuAddress, SetuKeyPair, SignatureScheme};
use crate::error::KeyError;
use bip32::{ChildNumber, DerivationPath, XPrv};
use bip39::{Language, Mnemonic};
use ed25519_dalek::SigningKey as Ed25519SigningKey;
use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use p256::ecdsa::SigningKey as Secp256r1SigningKey;
use slip10_ed25519::derive_ed25519_private_key;

/// Setu coin type for BIP44 derivation paths.
pub const DERIVATION_PATH_COIN_TYPE: u32 = 99999;

/// Purpose for Ed25519 (standard BIP44).
pub const DERIVATION_PATH_PURPOSE_ED25519: u32 = 44;

/// Purpose for Secp256k1 (Sui-style, distinguishes from Ed25519).
pub const DERIVATION_PATH_PURPOSE_SECP256K1: u32 = 54;

/// Purpose for Secp256r1 (Sui-style, distinguishes from other schemes).
pub const DERIVATION_PATH_PURPOSE_SECP256R1: u32 = 74;

/// Mnemonic word count options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordCount {
    Words12,
    Words15,
    Words18,
    Words21,
    Words24,
}

impl WordCount {
    /// Get the number of words.
    pub fn count(&self) -> usize {
        match self {
            WordCount::Words12 => 12,
            WordCount::Words15 => 15,
            WordCount::Words18 => 18,
            WordCount::Words21 => 21,
            WordCount::Words24 => 24,
        }
    }
}

impl Default for WordCount {
    fn default() -> Self {
        WordCount::Words12
    }
}

impl std::str::FromStr for WordCount {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "12" | "word12" | "words12" => Ok(WordCount::Words12),
            "15" | "word15" | "words15" => Ok(WordCount::Words15),
            "18" | "word18" | "words18" => Ok(WordCount::Words18),
            "21" | "word21" | "words21" => Ok(WordCount::Words21),
            "24" | "word24" | "words24" => Ok(WordCount::Words24),
            _ => Err(KeyError::InvalidMnemonic(format!(
                "Invalid word count: {}",
                s
            ))),
        }
    }
}

/// Generate a new random mnemonic phrase.
pub fn generate_mnemonic(word_count: WordCount) -> String {
    let mnemonic = Mnemonic::generate(word_count.count()).unwrap();
    mnemonic.to_string()
}

/// Derive a keypair from a mnemonic phrase and optional derivation path.
///
/// If no derivation path is provided, the default path for the scheme is used:
/// - Ed25519: m/44'/99999'/0'/0'/0'
/// - Secp256k1: m/54'/99999'/0'/0/0
/// - Secp256r1: m/74'/99999'/0'/0/0
pub fn derive_key_pair_from_path(
    seed: &[u8],
    derivation_path: Option<DerivationPath>,
    key_scheme: &SignatureScheme,
) -> Result<(SetuAddress, SetuKeyPair), KeyError> {
    let path = validate_path(key_scheme, derivation_path)?;

    match key_scheme {
        SignatureScheme::ED25519 => {
            let indexes = path.into_iter().map(|i| i.into()).collect::<Vec<u32>>();
            let derived = derive_ed25519_private_key(seed, &indexes);
            let sk = Ed25519SigningKey::from_bytes(&derived);
            let kp = SetuKeyPair::ed25519(sk);
            Ok((kp.address(), kp))
        }
        SignatureScheme::Secp256k1 => {
            let child_xprv = XPrv::derive_from_path(seed, &path)
                .map_err(|e| KeyError::KeyGeneration(e.to_string()))?;
            let sk =
                Secp256k1SigningKey::from_slice(child_xprv.private_key().to_bytes().as_slice())
                    .map_err(|e| KeyError::KeyGeneration(e.to_string()))?;
            let kp = SetuKeyPair::secp256k1(sk);
            Ok((kp.address(), kp))
        }
        SignatureScheme::Secp256r1 => {
            let child_xprv = XPrv::derive_from_path(seed, &path)
                .map_err(|e| KeyError::KeyGeneration(e.to_string()))?;
            let sk =
                Secp256r1SigningKey::from_slice(child_xprv.private_key().to_bytes().as_slice())
                    .map_err(|e| KeyError::KeyGeneration(e.to_string()))?;
            let kp = SetuKeyPair::secp256r1(sk);
            Ok((kp.address(), kp))
        }
    }
}

/// Validate and return the derivation path for the given scheme.
pub fn validate_path(
    key_scheme: &SignatureScheme,
    path: Option<DerivationPath>,
) -> Result<DerivationPath, KeyError> {
    match key_scheme {
        SignatureScheme::ED25519 => {
            match path {
                Some(p) => {
                    // Ed25519 requires all hardened derivation
                    if let &[purpose, coin_type, account, change, address] = p.as_ref() {
                        if Some(purpose)
                            == ChildNumber::new(DERIVATION_PATH_PURPOSE_ED25519, true).ok()
                            && Some(coin_type)
                                == ChildNumber::new(DERIVATION_PATH_COIN_TYPE, true).ok()
                            && account.is_hardened()
                            && change.is_hardened()
                            && address.is_hardened()
                        {
                            Ok(p)
                        } else {
                            Err(KeyError::InvalidDerivationPath(
                                "Ed25519 requires purpose=44', coin_type=99999', and all hardened levels".to_string(),
                            ))
                        }
                    } else {
                        Err(KeyError::InvalidDerivationPath(
                            "Derivation path must have exactly 5 levels".to_string(),
                        ))
                    }
                }
                None => {
                    // Default path: m/44'/99999'/0'/0'/0'
                    let path_str = format!(
                        "m/{}'/{}'/{}'/{}'/{}'",
                        DERIVATION_PATH_PURPOSE_ED25519, DERIVATION_PATH_COIN_TYPE, 0, 0, 0
                    );
                    path_str.parse().map_err(|_| {
                        KeyError::InvalidDerivationPath("Cannot parse default path".to_string())
                    })
                }
            }
        }
        SignatureScheme::Secp256k1 => {
            match path {
                Some(p) => {
                    // Secp256k1 requires first 3 levels hardened
                    if let &[purpose, coin_type, account, change, address] = p.as_ref() {
                        if Some(purpose)
                            == ChildNumber::new(DERIVATION_PATH_PURPOSE_SECP256K1, true).ok()
                            && Some(coin_type)
                                == ChildNumber::new(DERIVATION_PATH_COIN_TYPE, true).ok()
                            && account.is_hardened()
                            && !change.is_hardened()
                            && !address.is_hardened()
                        {
                            Ok(p)
                        } else {
                            Err(KeyError::InvalidDerivationPath(
                                "Secp256k1 requires purpose=54', coin_type=99999', account hardened, change/address not hardened".to_string(),
                            ))
                        }
                    } else {
                        Err(KeyError::InvalidDerivationPath(
                            "Derivation path must have exactly 5 levels".to_string(),
                        ))
                    }
                }
                None => {
                    // Default path: m/54'/99999'/0'/0/0
                    let path_str = format!(
                        "m/{}'/{}'/{}'/{}/{}",
                        DERIVATION_PATH_PURPOSE_SECP256K1, DERIVATION_PATH_COIN_TYPE, 0, 0, 0
                    );
                    path_str.parse().map_err(|_| {
                        KeyError::InvalidDerivationPath("Cannot parse default path".to_string())
                    })
                }
            }
        }
        SignatureScheme::Secp256r1 => {
            match path {
                Some(p) => {
                    // Secp256r1 requires first 3 levels hardened
                    if let &[purpose, coin_type, account, change, address] = p.as_ref() {
                        if Some(purpose)
                            == ChildNumber::new(DERIVATION_PATH_PURPOSE_SECP256R1, true).ok()
                            && Some(coin_type)
                                == ChildNumber::new(DERIVATION_PATH_COIN_TYPE, true).ok()
                            && account.is_hardened()
                            && !change.is_hardened()
                            && !address.is_hardened()
                        {
                            Ok(p)
                        } else {
                            Err(KeyError::InvalidDerivationPath(
                                "Secp256r1 requires purpose=74', coin_type=99999', account hardened, change/address not hardened".to_string(),
                            ))
                        }
                    } else {
                        Err(KeyError::InvalidDerivationPath(
                            "Derivation path must have exactly 5 levels".to_string(),
                        ))
                    }
                }
                None => {
                    // Default path: m/74'/99999'/0'/0/0
                    let path_str = format!(
                        "m/{}'/{}'/{}'/{}/{}",
                        DERIVATION_PATH_PURPOSE_SECP256R1, DERIVATION_PATH_COIN_TYPE, 0, 0, 0
                    );
                    path_str.parse().map_err(|_| {
                        KeyError::InvalidDerivationPath("Cannot parse default path".to_string())
                    })
                }
            }
        }
    }
}

/// Generate a new keypair with a random mnemonic.
///
/// Returns (address, keypair, scheme, mnemonic_phrase).
pub fn generate_new_key(
    key_scheme: SignatureScheme,
    derivation_path: Option<DerivationPath>,
    word_count: Option<WordCount>,
) -> Result<(SetuAddress, SetuKeyPair, SignatureScheme, String), KeyError> {
    let word_count = word_count.unwrap_or_default();
    let mnemonic = Mnemonic::generate(word_count.count())
        .map_err(|e| KeyError::InvalidMnemonic(e.to_string()))?;
    let phrase = mnemonic.to_string();
    let seed = mnemonic.to_seed("");

    let (address, kp) = derive_key_pair_from_path(&seed, derivation_path, &key_scheme)?;
    Ok((address, kp, key_scheme, phrase))
}

/// Derive a keypair from an existing mnemonic phrase.
pub fn derive_key_pair_from_mnemonic(
    phrase: &str,
    key_scheme: &SignatureScheme,
    derivation_path: Option<DerivationPath>,
) -> Result<(SetuAddress, SetuKeyPair), KeyError> {
    let mnemonic = Mnemonic::parse_in(Language::English, phrase)
        .map_err(|e| KeyError::InvalidMnemonic(e.to_string()))?;
    let seed = mnemonic.to_seed("");
    derive_key_pair_from_path(&seed, derivation_path, key_scheme)
}

/// Get the default derivation path for a given scheme and index.
pub fn default_derivation_path(
    scheme: &SignatureScheme,
    index: u32,
) -> Result<DerivationPath, KeyError> {
    let path_str = match scheme {
        SignatureScheme::ED25519 => format!(
            "m/{}'/{}'/{}'/{}'/{}'",
            DERIVATION_PATH_PURPOSE_ED25519, DERIVATION_PATH_COIN_TYPE, 0, 0, index
        ),
        SignatureScheme::Secp256k1 => format!(
            "m/{}'/{}'/{}'/{}/{}",
            DERIVATION_PATH_PURPOSE_SECP256K1, DERIVATION_PATH_COIN_TYPE, 0, 0, index
        ),
        SignatureScheme::Secp256r1 => format!(
            "m/{}'/{}'/{}'/{}/{}",
            DERIVATION_PATH_PURPOSE_SECP256R1, DERIVATION_PATH_COIN_TYPE, 0, 0, index
        ),
    };
    path_str
        .parse()
        .map_err(|_| KeyError::InvalidDerivationPath("Cannot parse derivation path".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_generate_mnemonic() {
        let phrase = generate_mnemonic(WordCount::Words12);
        let words: Vec<&str> = phrase.split_whitespace().collect();
        assert_eq!(words.len(), 12);
    }

    #[test]
    fn test_derive_ed25519() {
        let (addr, kp) =
            derive_key_pair_from_mnemonic(TEST_MNEMONIC, &SignatureScheme::ED25519, None).unwrap();
        assert_eq!(kp.scheme(), SignatureScheme::ED25519);
        assert_eq!(addr, kp.address());
    }

    #[test]
    fn test_derive_secp256k1() {
        let (addr, kp) =
            derive_key_pair_from_mnemonic(TEST_MNEMONIC, &SignatureScheme::Secp256k1, None)
                .unwrap();
        assert_eq!(kp.scheme(), SignatureScheme::Secp256k1);
        assert_eq!(addr, kp.address());
    }

    #[test]
    fn test_derive_secp256r1() {
        let (addr, kp) =
            derive_key_pair_from_mnemonic(TEST_MNEMONIC, &SignatureScheme::Secp256r1, None)
                .unwrap();
        assert_eq!(kp.scheme(), SignatureScheme::Secp256r1);
        assert_eq!(addr, kp.address());
    }

    #[test]
    fn test_deterministic_derivation() {
        // Same mnemonic should produce same keys
        let (addr1, kp1) =
            derive_key_pair_from_mnemonic(TEST_MNEMONIC, &SignatureScheme::ED25519, None).unwrap();
        let (addr2, kp2) =
            derive_key_pair_from_mnemonic(TEST_MNEMONIC, &SignatureScheme::ED25519, None).unwrap();
        assert_eq!(addr1, addr2);
        assert_eq!(kp1.public().as_bytes(), kp2.public().as_bytes());
    }

    #[test]
    fn test_different_indexes() {
        let path0 = default_derivation_path(&SignatureScheme::ED25519, 0).unwrap();
        let path1 = default_derivation_path(&SignatureScheme::ED25519, 1).unwrap();

        let mnemonic = Mnemonic::parse_in(Language::English, TEST_MNEMONIC).unwrap();
        let seed = mnemonic.to_seed("");

        let (addr0, _) =
            derive_key_pair_from_path(&seed, Some(path0), &SignatureScheme::ED25519).unwrap();
        let (addr1, _) =
            derive_key_pair_from_path(&seed, Some(path1), &SignatureScheme::ED25519).unwrap();

        // Different indexes should produce different addresses
        assert_ne!(addr0, addr1);
    }

    #[test]
    fn test_generate_new_key() {
        let (addr, kp, scheme, phrase) =
            generate_new_key(SignatureScheme::ED25519, None, None).unwrap();
        assert_eq!(scheme, SignatureScheme::ED25519);
        assert_eq!(addr, kp.address());
        assert!(!phrase.is_empty());

        // Should be able to recover same key from phrase
        let (addr2, kp2) =
            derive_key_pair_from_mnemonic(&phrase, &SignatureScheme::ED25519, None).unwrap();
        assert_eq!(addr, addr2);
        assert_eq!(kp.public().as_bytes(), kp2.public().as_bytes());
    }
}
