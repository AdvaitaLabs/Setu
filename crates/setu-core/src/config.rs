//! Configuration module for Setu nodes

use serde::{Deserialize, Serialize};

/// Node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Node ID
    pub node_id: String,

    /// Network configuration
    pub network: NetworkConfig,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Listen address
    pub listen_addr: String,

    /// Listen port
    pub port: u16,

    /// Peer addresses
    pub peers: Vec<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            network: NetworkConfig::default(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1".to_string(),
            port: 8000,
            peers: vec![],
        }
    }
}

impl NodeConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let mut config = NodeConfig::default();

        // Node ID
        if let Ok(node_id) = std::env::var("NODE_ID") {
            config.node_id = node_id;
        }

        // Network port
        if let Ok(port) = std::env::var("PORT") {
            if let Ok(port) = port.parse() {
                config.network.port = port;
            }
        }

        // Peers
        if let Ok(peers) = std::env::var("PEERS") {
            config.network.peers = peers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NodeConfig::default();
        assert!(!config.node_id.is_empty());
        assert_eq!(config.network.port, 8000);
    }
}
