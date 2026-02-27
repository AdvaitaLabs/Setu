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
}

/// A single account entry in genesis.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Human-readable name (will be hashed to Address)
    pub name: String,

    /// Initial balance in the smallest unit
    pub balance: u64,
}

fn default_subnet_id() -> String {
    "ROOT".to_string()
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
