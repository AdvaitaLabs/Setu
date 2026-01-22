// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Transport layer implementation using Anemo

use crate::{config::AnemoConfig, error::Result, AnemoError};
use anemo::{Network, PeerId, Router};
use bytes::Bytes;
use std::net::SocketAddr;
use tracing::{debug, info};

/// Anemo-based transport implementation
#[derive(Clone)]
pub struct AnemoTransport {
    /// Anemo network instance
    network: Network,
}

impl AnemoTransport {
    /// Create a new AnemoTransport with default echo service
    pub async fn new(config: &AnemoConfig) -> Result<Self> {
        Self::with_router(config, Router::new()).await
    }

    /// Create a new AnemoTransport with a custom router
    pub async fn with_router(config: &AnemoConfig, router: Router) -> Result<Self> {
        info!("Initializing Anemo transport on {}", config.listen_addr);

        // Parse listen address
        let listen_addr: SocketAddr = config
            .listen_addr
            .parse()
            .map_err(|e| AnemoError::InvalidConfig(format!("Invalid listen address: {}", e)))?;

        // Convert to Anemo config
        let anemo_config = config.to_anemo_config();

        // Generate or use provided private key
        let private_key = config.private_key.unwrap_or_else(|| {
            let mut rng = rand::thread_rng();
            let mut key = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rng, &mut key);
            key
        });

        // Build the network with the router
        let network = Network::bind(listen_addr)
            .server_name(&config.server_name)
            .private_key(private_key)
            .config(anemo_config)
            .start(router)?;

        info!(
            "Anemo network started on {} with PeerId: {}",
            network.local_addr(),
            network.peer_id()
        );

        Ok(Self { network })
    }

    /// Get the underlying Anemo network
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// Get local peer ID
    pub fn peer_id(&self) -> PeerId {
        self.network.peer_id()
    }

    /// Get local address
    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// Connect to a peer
    pub async fn connect(&self, addr: SocketAddr) -> Result<PeerId> {
        debug!("Connecting to peer at {}", addr);
        let peer_id = self.network.connect(addr).await?;
        info!("Connected to peer {} at {}", peer_id, addr);
        Ok(peer_id)
    }

    /// Connect to a peer with known peer ID
    pub async fn connect_with_peer_id(&self, addr: SocketAddr, peer_id: PeerId) -> Result<PeerId> {
        debug!("Connecting to peer {} at {}", peer_id, addr);
        let connected_peer_id = self.network.connect_with_peer_id(addr, peer_id).await?;
        info!("Connected to peer {} at {}", connected_peer_id, addr);
        Ok(connected_peer_id)
    }

    /// Disconnect from a peer
    pub fn disconnect(&self, peer_id: PeerId) -> Result<()> {
        debug!("Disconnecting from peer {}", peer_id);
        self.network.disconnect(peer_id)?;
        info!("Disconnected from peer {}", peer_id);
        Ok(())
    }

    /// Get list of connected peers
    pub fn peers(&self) -> Vec<PeerId> {
        self.network.peers()
    }

    /// Send an RPC request to a peer
    pub async fn rpc(
        &self,
        peer_id: PeerId,
        request: anemo::Request<Bytes>,
    ) -> Result<anemo::Response<Bytes>> {
        debug!("Sending RPC to peer {}", peer_id);
        let response = self.network.rpc(peer_id, request).await?;
        debug!("Received RPC response from peer {}", peer_id);
        Ok(response)
    }

    /// Subscribe to peer events (connect/disconnect)
    pub fn subscribe(
        &self,
    ) -> Result<(
        tokio::sync::broadcast::Receiver<anemo::types::PeerEvent>,
        Vec<PeerId>,
    )> {
        Ok(self.network.subscribe()?)
    }

    /// Shutdown the network
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Anemo network");
        self.network.shutdown().await?;
        Ok(())
    }
}

impl Drop for AnemoTransport {
    fn drop(&mut self) {
        // Network will be shut down automatically when dropped
        debug!("AnemoTransport dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_anemo_transport_creation() {
        let config = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };

        let transport = AnemoTransport::new(&config).await.unwrap();
        assert!(transport.local_addr().port() > 0);
    }

    #[tokio::test]
    async fn test_peer_connection() {
        let config1 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport1 = AnemoTransport::new(&config1).await.unwrap();

        let config2 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport2 = AnemoTransport::new(&config2).await.unwrap();

        // Connect transport1 to transport2
        let peer_id = transport1.connect(transport2.local_addr()).await.unwrap();
        assert_eq!(peer_id, transport2.peer_id());

        // Verify connection
        let peers = transport1.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0], transport2.peer_id());
    }

    #[tokio::test]
    async fn test_rpc_communication() {
        let config1 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport1 = AnemoTransport::new(&config1).await.unwrap();

        let config2 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport2 = AnemoTransport::new(&config2).await.unwrap();

        // Connect
        let peer_id = transport1.connect(transport2.local_addr()).await.unwrap();

        // Send RPC
        let message = Bytes::from("Hello, Anemo!");
        let request = anemo::Request::new(message.clone());
        let response = transport1.rpc(peer_id, request).await.unwrap();

        assert_eq!(response.into_body(), message);
    }
}
