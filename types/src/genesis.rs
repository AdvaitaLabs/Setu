//! Genesis configuration types
//!
//! Defines the structure of genesis.json and provides utilities
//! to build Genesis Events with proper state changes.

use serde::{Deserialize, Serialize};

/// Genesis configuration loaded from genesis.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisConfig {
    /// Chain identifier (e.g., "setu-devnet", "setu-mainnet")
    pub chain_id: String,

    /// Genesis timestamp (ISO 8601, informational)
    #[serde(default)]
    pub timestamp: Option<String>,

    /// Seed accounts to create at genesis
    pub accounts: Vec<GenesisAccount>,

    /// Subnet for the genesis coins (default: "ROOT")
    #[serde(default = "default_subnet_id")]
    pub subnet_id: String,

    /// Initial validator set (static configuration for multi-validator)
    #[serde(default)]
    pub validators: Vec<GenesisValidator>,
}

/// A single account entry in genesis.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Hex address ("0x" + 64 hex chars) derived from a real public key
    pub address: String,

    /// Optional human-readable label (not used for address derivation)
    #[serde(default)]
    pub name: Option<String>,

    /// Initial balance in the smallest unit
    pub balance: u64,

    /// Number of coin objects to create for this account (default: 1)
    ///
    /// When > 1, the total balance is split evenly across N coin objects.
    /// This pre-shards the account's coins at genesis, enabling higher
    /// per-sender parallelism in the multi-coin object model.
    ///
    /// Example: balance=1000000000, coins_per_account=5
    /// → 5 coins × 200000000 each (last coin absorbs rounding remainder)
    #[serde(default = "default_coins_per_account")]
    pub coins_per_account: u32,
}

/// A validator entry in genesis.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Validator ID (must match NODE_ID env var on the corresponding node)
    pub id: String,
    /// P2P address (hostname or IP)
    pub address: String,
    /// P2P port
    pub p2p_port: u16,
    /// Ed25519 public key hex (optional; when empty, signature verification is skipped)
    #[serde(default)]
    pub public_key: Option<String>,
}

fn default_subnet_id() -> String {
    "ROOT".to_string()
}

fn default_coins_per_account() -> u32 {
    1
}

impl GenesisConfig {
    /// Load genesis configuration from a JSON file
    pub fn load(path: &str) -> Result<Self, GenesisError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| GenesisError::IoError(path.to_string(), e.to_string()))?;
        let config: Self = serde_json::from_str(&content)
            .map_err(|e| GenesisError::ParseError(path.to_string(), e.to_string()))?;

        if config.accounts.is_empty() {
            return Err(GenesisError::NoAccounts);
        }

        Ok(config)
    }
}

/// Errors during genesis processing
#[derive(Debug, thiserror::Error)]
pub enum GenesisError {
    #[error("Failed to read genesis file '{0}': {1}")]
    IoError(String, String),

    #[error("Failed to parse genesis file '{0}': {1}")]
    ParseError(String, String),

    #[error("Genesis config has no accounts")]
    NoAccounts,
}
