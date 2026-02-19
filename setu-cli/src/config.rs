//! Configuration management

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default router address
    pub router_address: String,

    /// Default router port
    pub router_port: u16,

    /// Client ID
    pub client_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            router_address: "127.0.0.1".to_string(),
            router_port: 8080,
            client_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl Config {
    /// Load config from file
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save config to file
    pub fn save(&self, path: &str) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn default_path() -> PathBuf {
        let home = dirs::home_dir().expect("Failed to get home directory");
        home.join(".setu").join("config.toml")
    }

    /// Initialize config directory and file
    pub fn init() -> Result<PathBuf> {
        let config_path = Self::default_path();

        // Create directory if not exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create default config if not exists
        if !config_path.exists() {
            let config = Config::default();
            config.save(config_path.to_str().unwrap())?;
        }

        Ok(config_path)
    }
}
