// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Keystore implementations for secure key storage and management.
//!
//! This module provides:
//! - `AccountKeystore` trait for key management operations
//! - `FileBasedKeystore` for persistent file storage
//! - `InMemKeystore` for in-memory storage (testing/ephemeral use)

use crate::crypto::{PublicKey, SetuAddress, SetuKeyPair, Signature, SignatureScheme};
use crate::error::KeyError;
use crate::key_derive::{derive_key_pair_from_mnemonic, generate_new_key, WordCount};
use crate::key_identity::KeyIdentity;
use async_trait::async_trait;
use bip32::DerivationPath;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

pub const ALIASES_FILE_EXTENSION: &str = "aliases";

/// Alias entry storing an alias name and its associated public key.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Alias {
    pub alias: String,
    pub public_key_base64: String,
}

/// Result of generating a new key.
pub struct GeneratedKey {
    pub address: SetuAddress,
    pub public_key: PublicKey,
    pub scheme: SignatureScheme,
    pub mnemonic: Option<String>,
}

/// Unified keystore enum supporting different storage backends.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Keystore {
    File(FileBasedKeystore),
    InMem(InMemKeystore),
}

impl Keystore {
    /// Create a new file-based keystore.
    pub fn file(path: &Path) -> Result<Self, KeyError> {
        Ok(Keystore::File(FileBasedKeystore::load_or_create(path)?))
    }

    /// Create a new in-memory keystore.
    pub fn in_memory() -> Self {
        Keystore::InMem(InMemKeystore::default())
    }
}

#[async_trait]
impl AccountKeystore for Keystore {
    async fn generate(
        &mut self,
        alias: Option<String>,
        scheme: SignatureScheme,
        derivation_path: Option<DerivationPath>,
        word_count: Option<WordCount>,
    ) -> Result<GeneratedKey, KeyError> {
        match self {
            Keystore::File(ks) => {
                ks.generate(alias, scheme, derivation_path, word_count)
                    .await
            }
            Keystore::InMem(ks) => {
                ks.generate(alias, scheme, derivation_path, word_count)
                    .await
            }
        }
    }

    async fn import(
        &mut self,
        alias: Option<String>,
        keypair: SetuKeyPair,
    ) -> Result<(), KeyError> {
        match self {
            Keystore::File(ks) => ks.import(alias, keypair).await,
            Keystore::InMem(ks) => ks.import(alias, keypair).await,
        }
    }

    async fn import_from_mnemonic(
        &mut self,
        phrase: &str,
        key_scheme: SignatureScheme,
        derivation_path: Option<DerivationPath>,
        alias: Option<String>,
    ) -> Result<SetuAddress, KeyError> {
        match self {
            Keystore::File(ks) => {
                ks.import_from_mnemonic(phrase, key_scheme, derivation_path, alias)
                    .await
            }
            Keystore::InMem(ks) => {
                ks.import_from_mnemonic(phrase, key_scheme, derivation_path, alias)
                    .await
            }
        }
    }

    async fn remove(&mut self, address: SetuAddress) -> Result<(), KeyError> {
        match self {
            Keystore::File(ks) => ks.remove(address).await,
            Keystore::InMem(ks) => ks.remove(address).await,
        }
    }

    fn entries(&self) -> Vec<PublicKey> {
        match self {
            Keystore::File(ks) => ks.entries(),
            Keystore::InMem(ks) => ks.entries(),
        }
    }

    fn export(&self, address: &SetuAddress) -> Result<&SetuKeyPair, KeyError> {
        match self {
            Keystore::File(ks) => ks.export(address),
            Keystore::InMem(ks) => ks.export(address),
        }
    }

    fn sign(&self, address: &SetuAddress, msg: &[u8]) -> Result<Signature, KeyError> {
        match self {
            Keystore::File(ks) => ks.sign(address, msg),
            Keystore::InMem(ks) => ks.sign(address, msg),
        }
    }

    fn aliases(&self) -> Vec<&Alias> {
        match self {
            Keystore::File(ks) => ks.aliases(),
            Keystore::InMem(ks) => ks.aliases(),
        }
    }

    fn addresses_with_alias(&self) -> Vec<(&SetuAddress, &Alias)> {
        match self {
            Keystore::File(ks) => ks.addresses_with_alias(),
            Keystore::InMem(ks) => ks.addresses_with_alias(),
        }
    }

    fn get_alias(&self, address: &SetuAddress) -> Result<String, KeyError> {
        match self {
            Keystore::File(ks) => ks.get_alias(address),
            Keystore::InMem(ks) => ks.get_alias(address),
        }
    }

    fn create_alias(&self, alias: Option<String>) -> Result<String, KeyError> {
        match self {
            Keystore::File(ks) => ks.create_alias(alias),
            Keystore::InMem(ks) => ks.create_alias(alias),
        }
    }

    async fn update_alias(
        &mut self,
        old_alias: &str,
        new_alias: Option<&str>,
    ) -> Result<String, KeyError> {
        match self {
            Keystore::File(ks) => ks.update_alias(old_alias, new_alias).await,
            Keystore::InMem(ks) => ks.update_alias(old_alias, new_alias).await,
        }
    }
}

/// Trait defining the interface for key storage and management.
#[async_trait]
pub trait AccountKeystore: Send + Sync {
    /// Generate a new keypair and add it to the keystore.
    async fn generate(
        &mut self,
        alias: Option<String>,
        scheme: SignatureScheme,
        derivation_path: Option<DerivationPath>,
        word_count: Option<WordCount>,
    ) -> Result<GeneratedKey, KeyError> {
        let (address, kp, scheme, phrase) = generate_new_key(scheme, derivation_path, word_count)?;
        let public_key = kp.public();
        self.import(alias, kp).await?;
        Ok(GeneratedKey {
            address,
            public_key,
            scheme,
            mnemonic: Some(phrase),
        })
    }

    /// Import a keypair into the keystore.
    async fn import(&mut self, alias: Option<String>, keypair: SetuKeyPair)
        -> Result<(), KeyError>;

    /// Import from a mnemonic phrase.
    async fn import_from_mnemonic(
        &mut self,
        phrase: &str,
        key_scheme: SignatureScheme,
        derivation_path: Option<DerivationPath>,
        alias: Option<String>,
    ) -> Result<SetuAddress, KeyError> {
        let (address, kp) = derive_key_pair_from_mnemonic(phrase, &key_scheme, derivation_path)?;
        self.import(alias, kp).await?;
        Ok(address)
    }

    /// Remove a keypair from the keystore.
    async fn remove(&mut self, address: SetuAddress) -> Result<(), KeyError>;

    /// Return all public keys in the keystore.
    fn entries(&self) -> Vec<PublicKey>;

    /// Export the keypair for the given address.
    fn export(&self, address: &SetuAddress) -> Result<&SetuKeyPair, KeyError>;

    /// Sign a message with the keypair for the given address.
    fn sign(&self, address: &SetuAddress, msg: &[u8]) -> Result<Signature, KeyError>;

    /// Return all addresses in the keystore.
    fn addresses(&self) -> Vec<SetuAddress> {
        self.entries().iter().map(|pk| pk.into()).collect()
    }

    /// Return all aliases.
    fn aliases(&self) -> Vec<&Alias>;

    /// Return addresses with their aliases.
    fn addresses_with_alias(&self) -> Vec<(&SetuAddress, &Alias)>;

    /// Get the alias for an address.
    fn get_alias(&self, address: &SetuAddress) -> Result<String, KeyError>;

    /// Check if an alias exists.
    fn alias_exists(&self, alias: &str) -> bool {
        self.aliases().iter().any(|a| a.alias == alias)
    }

    /// Create or validate an alias.
    fn create_alias(&self, alias: Option<String>) -> Result<String, KeyError>;

    /// Update an alias.
    async fn update_alias(
        &mut self,
        old_alias: &str,
        new_alias: Option<&str>,
    ) -> Result<String, KeyError>;

    /// Get address by identity (address or alias).
    fn get_by_identity(&self, key_identity: &KeyIdentity) -> Result<SetuAddress, KeyError> {
        match key_identity {
            KeyIdentity::Address(addr) => Ok(*addr),
            KeyIdentity::Alias(alias) => self
                .addresses_with_alias()
                .into_iter()
                .find(|(_, a)| a.alias == *alias)
                .map(|(addr, _)| *addr)
                .ok_or_else(|| KeyError::AliasNotFound(alias.clone())),
        }
    }
}

/// File-based keystore that persists keys to disk.
#[derive(Default)]
pub struct FileBasedKeystore {
    keys: BTreeMap<SetuAddress, SetuKeyPair>,
    aliases: BTreeMap<SetuAddress, Alias>,
    path: Option<PathBuf>,
}

impl Serialize for FileBasedKeystore {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            self.path
                .as_ref()
                .unwrap_or(&PathBuf::default())
                .to_str()
                .unwrap_or(""),
        )
    }
}

impl<'de> Deserialize<'de> for FileBasedKeystore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        FileBasedKeystore::load_or_create(&PathBuf::from(String::deserialize(deserializer)?))
            .map_err(D::Error::custom)
    }
}

#[async_trait]
impl AccountKeystore for FileBasedKeystore {
    async fn import(
        &mut self,
        alias: Option<String>,
        keypair: SetuKeyPair,
    ) -> Result<(), KeyError> {
        let address = keypair.address();
        let alias_name = self.create_alias(alias)?;
        self.aliases.insert(
            address,
            Alias {
                alias: alias_name,
                public_key_base64: keypair.public().encode_base64(),
            },
        );
        self.keys.insert(address, keypair);
        self.save().await?;
        Ok(())
    }

    async fn remove(&mut self, address: SetuAddress) -> Result<(), KeyError> {
        self.aliases.remove(&address);
        self.keys.remove(&address);
        self.save().await?;
        Ok(())
    }

    fn entries(&self) -> Vec<PublicKey> {
        self.keys.values().map(|kp| kp.public()).collect()
    }

    fn export(&self, address: &SetuAddress) -> Result<&SetuKeyPair, KeyError> {
        self.keys
            .get(address)
            .ok_or_else(|| KeyError::KeyNotFound(address.to_string()))
    }

    fn sign(&self, address: &SetuAddress, msg: &[u8]) -> Result<Signature, KeyError> {
        let kp = self.export(address)?;
        Ok(kp.sign(msg))
    }

    fn aliases(&self) -> Vec<&Alias> {
        self.aliases.values().collect()
    }

    fn addresses_with_alias(&self) -> Vec<(&SetuAddress, &Alias)> {
        self.aliases.iter().collect()
    }

    fn get_alias(&self, address: &SetuAddress) -> Result<String, KeyError> {
        self.aliases
            .get(address)
            .map(|a| a.alias.clone())
            .ok_or_else(|| KeyError::AliasNotFound(address.to_string()))
    }

    fn create_alias(&self, alias: Option<String>) -> Result<String, KeyError> {
        match alias {
            Some(a) if self.alias_exists(&a) => Err(KeyError::AliasExists(a)),
            Some(a) => Ok(a),
            None => Ok(generate_random_alias(
                &self.aliases().iter().map(|a| a.alias.clone()).collect(),
            )),
        }
    }

    async fn update_alias(
        &mut self,
        old_alias: &str,
        new_alias: Option<&str>,
    ) -> Result<String, KeyError> {
        if !self.alias_exists(old_alias) {
            return Err(KeyError::AliasNotFound(old_alias.to_string()));
        }
        let new_name = self.create_alias(new_alias.map(|s| s.to_string()))?;
        for a in self.aliases.values_mut() {
            if a.alias == old_alias {
                a.alias = new_name.clone();
                break;
            }
        }
        self.save_aliases().await?;
        Ok(new_name)
    }
}

impl FileBasedKeystore {
    /// Load an existing keystore or create a new one.
    pub fn load_or_create(path: &Path) -> Result<Self, KeyError> {
        let keys = if path.exists() {
            let reader = BufReader::new(fs::File::open(path)?);
            let kp_strings: Vec<String> = serde_json::from_reader(reader)
                .map_err(|e| KeyError::Serialization(e.to_string()))?;
            kp_strings
                .iter()
                .map(|s| {
                    let kp = SetuKeyPair::decode_base64(s)?;
                    Ok((kp.address(), kp))
                })
                .collect::<Result<BTreeMap<_, _>, KeyError>>()?
        } else {
            BTreeMap::new()
        };

        // Load aliases
        let mut aliases_path = path.to_path_buf();
        aliases_path.set_extension(ALIASES_FILE_EXTENSION);

        let aliases = if aliases_path.exists() {
            let reader = BufReader::new(fs::File::open(&aliases_path)?);
            let aliases: Vec<Alias> = serde_json::from_reader(reader)
                .map_err(|e| KeyError::Serialization(e.to_string()))?;
            aliases
                .into_iter()
                .map(|alias| {
                    let pk = PublicKey::decode_base64(&alias.public_key_base64)?;
                    Ok((pk.into(), alias))
                })
                .collect::<Result<BTreeMap<_, _>, KeyError>>()?
        } else if keys.is_empty() {
            BTreeMap::new()
        } else {
            // Generate aliases for existing keys without aliases
            let existing_aliases: HashSet<String> = HashSet::new();
            keys.iter()
                .enumerate()
                .map(|(i, (addr, kp))| {
                    let alias = generate_random_alias_with_index(&existing_aliases, i);
                    (
                        *addr,
                        Alias {
                            alias,
                            public_key_base64: kp.public().encode_base64(),
                        },
                    )
                })
                .collect()
        };

        Ok(Self {
            keys,
            aliases,
            path: Some(path.to_path_buf()),
        })
    }

    /// Save aliases to file.
    pub async fn save_aliases(&self) -> Result<(), KeyError> {
        if let Some(path) = &self.path {
            let aliases_store =
                serde_json::to_string_pretty(&self.aliases.values().collect::<Vec<_>>())
                    .map_err(|e| KeyError::Serialization(e.to_string()))?;
            let mut aliases_path = path.clone();
            aliases_path.set_extension(ALIASES_FILE_EXTENSION);
            tokio::fs::write(aliases_path, aliases_store).await?;
        }
        Ok(())
    }

    /// Save keystore to file.
    pub async fn save_keystore(&self) -> Result<(), KeyError> {
        if let Some(path) = &self.path {
            let store = serde_json::to_string_pretty(
                &self
                    .keys
                    .values()
                    .map(|k| k.encode_base64())
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| KeyError::Serialization(e.to_string()))?;
            tokio::fs::write(path, store).await?;
        }
        Ok(())
    }

    /// Save both keystore and aliases.
    pub async fn save(&self) -> Result<(), KeyError> {
        self.save_keystore().await?;
        self.save_aliases().await?;
        Ok(())
    }

    /// Get all keypairs.
    pub fn key_pairs(&self) -> Vec<&SetuKeyPair> {
        self.keys.values().collect()
    }
}

/// In-memory keystore for testing or ephemeral use.
#[derive(Default, Serialize, Deserialize)]
pub struct InMemKeystore {
    #[serde(skip)]
    keys: BTreeMap<SetuAddress, SetuKeyPair>,
    aliases: BTreeMap<SetuAddress, Alias>,
}

#[async_trait]
impl AccountKeystore for InMemKeystore {
    async fn import(
        &mut self,
        alias: Option<String>,
        keypair: SetuKeyPair,
    ) -> Result<(), KeyError> {
        let address = keypair.address();
        let alias_name = self.create_alias(alias)?;
        self.aliases.insert(
            address,
            Alias {
                alias: alias_name,
                public_key_base64: keypair.public().encode_base64(),
            },
        );
        self.keys.insert(address, keypair);
        Ok(())
    }

    async fn remove(&mut self, address: SetuAddress) -> Result<(), KeyError> {
        self.aliases.remove(&address);
        self.keys.remove(&address);
        Ok(())
    }

    fn entries(&self) -> Vec<PublicKey> {
        self.keys.values().map(|kp| kp.public()).collect()
    }

    fn export(&self, address: &SetuAddress) -> Result<&SetuKeyPair, KeyError> {
        self.keys
            .get(address)
            .ok_or_else(|| KeyError::KeyNotFound(address.to_string()))
    }

    fn sign(&self, address: &SetuAddress, msg: &[u8]) -> Result<Signature, KeyError> {
        let kp = self.export(address)?;
        Ok(kp.sign(msg))
    }

    fn aliases(&self) -> Vec<&Alias> {
        self.aliases.values().collect()
    }

    fn addresses_with_alias(&self) -> Vec<(&SetuAddress, &Alias)> {
        self.aliases.iter().collect()
    }

    fn get_alias(&self, address: &SetuAddress) -> Result<String, KeyError> {
        self.aliases
            .get(address)
            .map(|a| a.alias.clone())
            .ok_or_else(|| KeyError::AliasNotFound(address.to_string()))
    }

    fn create_alias(&self, alias: Option<String>) -> Result<String, KeyError> {
        match alias {
            Some(a) if self.alias_exists(&a) => Err(KeyError::AliasExists(a)),
            Some(a) => Ok(a),
            None => Ok(generate_random_alias(
                &self.aliases().iter().map(|a| a.alias.clone()).collect(),
            )),
        }
    }

    async fn update_alias(
        &mut self,
        old_alias: &str,
        new_alias: Option<&str>,
    ) -> Result<String, KeyError> {
        if !self.alias_exists(old_alias) {
            return Err(KeyError::AliasNotFound(old_alias.to_string()));
        }
        let new_name = self.create_alias(new_alias.map(|s| s.to_string()))?;
        for a in self.aliases.values_mut() {
            if a.alias == old_alias {
                a.alias = new_name.clone();
                break;
            }
        }
        Ok(new_name)
    }
}

/// Generate a random alias that doesn't conflict with existing aliases.
fn generate_random_alias(existing: &HashSet<String>) -> String {
    let adjectives = [
        "brave", "calm", "eager", "fair", "kind", "bold", "wise", "swift",
    ];
    let nouns = ["wolf", "bear", "hawk", "lion", "deer", "fox", "owl", "seal"];

    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();

    for _ in 0..100 {
        let adj = adjectives.choose(&mut rng).unwrap();
        let noun = nouns.choose(&mut rng).unwrap();
        let num: u16 = rand::Rng::gen_range(&mut rng, 0..1000);
        let alias = format!("{}-{}-{}", adj, noun, num);
        if !existing.contains(&alias) {
            return alias;
        }
    }

    // Fallback to a UUID-like string
    format!("key-{}", rand::Rng::gen::<u64>(&mut rand::thread_rng()))
}

fn generate_random_alias_with_index(existing: &HashSet<String>, _index: usize) -> String {
    let existing = existing.clone();
    generate_random_alias(&existing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inmem_keystore() {
        let mut ks = InMemKeystore::default();

        // Generate a key
        let result = ks
            .generate(
                Some("test-key".to_string()),
                SignatureScheme::ED25519,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.scheme, SignatureScheme::ED25519);
        assert!(result.mnemonic.is_some());

        // Check it's in the keystore
        assert_eq!(ks.addresses().len(), 1);
        assert!(ks.alias_exists("test-key"));

        // Sign a message
        let sig = ks.sign(&result.address, b"hello").unwrap();
        assert!(result.public_key.verify(b"hello", &sig).is_ok());

        // Remove the key
        ks.remove(result.address).await.unwrap();
        assert_eq!(ks.addresses().len(), 0);
    }

    #[tokio::test]
    async fn test_file_keystore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keystore.json");

        let mut ks = FileBasedKeystore::load_or_create(&path).unwrap();

        // Generate a key
        let result = ks
            .generate(None, SignatureScheme::ED25519, None, None)
            .await
            .unwrap();

        // Reload and verify
        let ks2 = FileBasedKeystore::load_or_create(&path).unwrap();
        assert_eq!(ks2.addresses().len(), 1);
        assert_eq!(ks2.addresses()[0], result.address);
    }
}
