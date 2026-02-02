//! Configuration module for Setu Explorer

use std::net::SocketAddr;
use std::path::PathBuf;

/// Explorer configuration
#[derive(Debug, Clone)]
pub struct ExplorerConfig {
    /// HTTP server listen address
    pub listen_addr: SocketAddr,
    
    /// Storage mode
    pub storage_mode: StorageMode,
    
    /// Database path (for direct mode)
    pub db_path: Option<PathBuf>,
    
    /// Validator RPC URL (for RPC mode)
    pub validator_rpc_url: Option<String>,
    
    /// Enable CORS
    pub enable_cors: bool,
    
    /// Log level
    pub log_level: String,
}

/// Storage access mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageMode {
    /// Direct RocksDB access (read-only)
    DirectRocksDB,
    
    /// RPC access to validator
    #[allow(dead_code)]
    RPC,
}

impl ExplorerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let listen_addr = std::env::var("EXPLORER_LISTEN_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8081".to_string())
            .parse()
            .expect("Invalid EXPLORER_LISTEN_ADDR");
        
        let storage_mode = match std::env::var("EXPLORER_STORAGE_MODE")
            .unwrap_or_else(|_| "direct".to_string())
            .to_lowercase()
            .as_str()
        {
            "rpc" => StorageMode::RPC,
            _ => StorageMode::DirectRocksDB,
        };
        
        let db_path = std::env::var("EXPLORER_DB_PATH")
            .ok()
            .map(PathBuf::from);
        
        let validator_rpc_url = std::env::var("EXPLORER_VALIDATOR_RPC_URL").ok();
        
        let enable_cors = std::env::var("EXPLORER_ENABLE_CORS")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);
        
        let log_level = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "info".to_string());
        
        Self {
            listen_addr,
            storage_mode,
            db_path,
            validator_rpc_url,
            enable_cors,
            log_level,
        }
    }
    
    /// Validate configuration
    pub fn validate(&self) -> anyhow::Result<()> {
        match self.storage_mode {
            StorageMode::DirectRocksDB => {
                if self.db_path.is_none() {
                    anyhow::bail!("DB path is required for direct storage mode");
                }
            }
            StorageMode::RPC => {
                if self.validator_rpc_url.is_none() {
                    anyhow::bail!("Validator RPC URL is required for RPC mode");
                }
            }
        }
        
        Ok(())
    }
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8081".parse().unwrap(),
            storage_mode: StorageMode::DirectRocksDB,
            db_path: Some(PathBuf::from("./data/validator")),
            validator_rpc_url: None,
            enable_cors: true,
            log_level: "info".to_string(),
        }
    }
}

