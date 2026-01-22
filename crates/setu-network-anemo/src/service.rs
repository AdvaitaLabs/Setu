// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Network service implementation using Anemo
//!
//! This module provides the main network service that integrates:
//! - Discovery: Peer discovery and connection management
//! - Transport: QUIC-based communication
//!
//! Use `with_handler()` to create a network service with a custom message handler.
//! The message handler implementation should be provided by the application layer.
//!
//! The service follows Sui's patterns using the builder pattern for construction.

use crate::{
    config::NetworkConfig,
    discovery::{self, DiscoveryConfig, NodeType},
    error::Result,
    generic_handler::GenericMessageHandler,
    metrics::NetworkMetrics,
    peer_manager::AnemoPeerManager,
    transport::AnemoTransport,
    AnemoError,
};
use anemo::PeerId;
use bytes::Bytes;
use crate::node_info::NodeInfo;
use std::sync::Arc;
use tracing::{debug, info};


/// Anemo-based network service for Setu
///
/// This is the main entry point for network functionality. It integrates
/// discovery and transport into a unified service.
///
/// Message handling is delegated to the application layer via `GenericMessageHandler`.
pub struct AnemoNetworkService {
    /// Anemo transport
    transport: Arc<AnemoTransport>,

    /// Peer manager
    peer_manager: Arc<AnemoPeerManager>,

    /// Discovery handle (for controlling discovery)
    #[allow(dead_code)]
    discovery_handle: Option<discovery::Handle>,

    /// Local node information
    local_node_info: NodeInfo,

    /// Node type (validator or solver)
    node_type: NodeType,

    /// Metrics (optional)
    #[allow(dead_code)]
    metrics: Option<NetworkMetrics>,
}

impl AnemoNetworkService {
    /// Create a new network service with a custom message handler
    ///
    /// This is the standard constructor. It allows the application layer to 
    /// provide its own message handler implementation.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use setu_network_anemo::{AnemoNetworkService, GenericMessageHandler};
    /// use setu_validator::network_adapter::SetuMessageHandler;
    ///
    /// let handler = Arc::new(SetuMessageHandler::new(store, node_id, event_tx));
    /// let service = AnemoNetworkService::with_handler(config, node_info, handler).await?;
    /// ```
    pub async fn with_handler<H>(
        config: NetworkConfig,
        local_node_info: NodeInfo,
        handler: Arc<H>,
    ) -> Result<Self>
    where
        H: GenericMessageHandler,
    {
        info!("Creating Anemo network service with handler for node {}", local_node_info.id);

        // Create router from the generic handler
        let router = crate::generic_handler::create_router_from_handler(handler);

        // Create transport with the router
        let transport = Arc::new(AnemoTransport::with_router(&config.anemo, router).await?);

        // Create peer manager
        let peer_manager = Arc::new(AnemoPeerManager::new(transport.clone())?);

        // Build and start discovery
        let discovery_config = DiscoveryConfig::default();
        let (unstarted_discovery, _discovery_server) = discovery::Builder::new()
            .config(discovery_config)
            .build();
        let network_ref = transport.network().downgrade();
        let (discovery_handle, _join_handle) = unstarted_discovery.start(network_ref);

        info!("Anemo network service created for node {}", local_node_info.id);

        Ok(Self {
            transport,
            peer_manager,
            discovery_handle: Some(discovery_handle),
            local_node_info,
            node_type: NodeType::Validator,
            metrics: None,
        })
    }

    /// Set the node type
    pub fn set_node_type(&mut self, node_type: NodeType) {
        self.node_type = node_type;
    }

    /// Get local peer ID
    pub fn peer_id(&self) -> PeerId {
        self.transport.peer_id()
    }

    /// Get local address
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.transport.local_addr()
    }

    /// Connect to a peer
    pub async fn connect_to_peer(&self, node_info: NodeInfo) -> Result<PeerId> {
        let addr = format!("{}:{}", node_info.address, node_info.port);
        let socket_addr: std::net::SocketAddr = addr
            .parse()
            .map_err(|e| AnemoError::InvalidConfig(format!("Invalid address: {}", e)))?;
        let peer_id = self.transport.connect(socket_addr).await?;
        
        // Add to peer manager
        self.peer_manager.add_peer(node_info.clone(), peer_id).await?;

        Ok(peer_id)
    }

    /// Disconnect from a peer
    pub async fn disconnect_from_peer(&self, peer_id_str: &str) -> Result<()> {
        let peer_id = self.parse_peer_id(peer_id_str)?;
        self.peer_manager.disconnect_from_peer(&peer_id).await?;
        Ok(())
    }

    /// Send raw bytes to a peer via RPC
    ///
    /// This is the generic send method. Message serialization should be
    /// handled by the application layer.
    pub async fn send_to_peer(&self, peer_id: PeerId, route: &str, data: Bytes) -> Result<Bytes> {
        debug!("Sending {} bytes to peer {} on route {}", data.len(), peer_id, route);
        
        let mut request = anemo::Request::new(data);
        *request.route_mut() = route.into();
        let response = self.transport.rpc(peer_id, request).await?;
        
        Ok(response.into_body())
    }

    /// Broadcast raw bytes to all connected peers
    ///
    /// Message serialization should be handled by the application layer.
    /// Returns (success_count, total_peers) for partial failure tracking.
    pub async fn broadcast(&self, route: &str, data: Bytes) -> Result<(usize, usize)> {
        debug!("Broadcasting {} bytes on route {}", data.len(), route);
        
        let peers = self.peer_manager.get_connected_peers();
        let total = peers.len();
        let mut success = 0;
        
        for peer_info in peers {
            let mut request = anemo::Request::new(data.clone());
            *request.route_mut() = route.into();
            match self.transport.rpc(peer_info.peer_id, request).await {
                Ok(_) => success += 1,
                Err(e) => tracing::warn!("Failed to send to peer {}: {}", peer_info.peer_id, e),
            }
        }

        Ok((success, total))
    }

    /// Get connected peer count
    pub fn get_peer_count(&self) -> usize {
        self.peer_manager.get_connected_peers().len()
    }

    /// Get all connected peers
    pub fn get_connected_peers(&self) -> Vec<String> {
        self.peer_manager
            .get_connected_peers()
            .into_iter()
            .map(|p| format!("{}", p.peer_id))
            .collect()
    }

    /// Get transport reference for advanced use
    pub fn transport(&self) -> &Arc<AnemoTransport> {
        &self.transport
    }

    /// Get peer manager reference
    pub fn peer_manager(&self) -> &Arc<AnemoPeerManager> {
        &self.peer_manager
    }

    /// Get local node information
    pub fn local_node_info(&self) -> &NodeInfo {
        &self.local_node_info
    }

    /// Shutdown the network service
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down network service");
        
        // Handles are dropped, which signals shutdown
        // Transport shutdown
        self.transport.shutdown().await?;
        
        info!("Network service shutdown complete");
        Ok(())
    }

    /// Helper: Parse peer ID from string
    fn parse_peer_id(&self, peer_id_str: &str) -> Result<PeerId> {
        let bytes = hex::decode(peer_id_str)
            .map_err(|e| AnemoError::InvalidConfig(format!("Invalid peer ID: {}", e)))?;

        if bytes.len() != 32 {
            return Err(AnemoError::InvalidConfig(
                "Peer ID must be 32 bytes".to_string(),
            ));
        }

        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(PeerId(array))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AnemoConfig;
    use crate::generic_handler::{GenericMessageHandler, HandleResult};
    use async_trait::async_trait;
    use bytes::Bytes;

    /// A minimal test handler for unit tests
    struct TestHandler;

    #[async_trait]
    impl GenericMessageHandler for TestHandler {
        async fn handle(&self, _route: &str, _body: Bytes) -> HandleResult {
            Ok(Some(Bytes::new()))
        }

        fn routes(&self) -> Vec<&'static str> {
            vec!["/test"]
        }
    }

    #[tokio::test]
    async fn test_network_service_creation() {
        let config = NetworkConfig {
            anemo: AnemoConfig {
                listen_addr: "127.0.0.1:0".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let node_info = NodeInfo::new_validator(
            "test-node".to_string(),
            "127.0.0.1".to_string(),
            9000,
        );

        let handler = Arc::new(TestHandler);
        let service = AnemoNetworkService::with_handler(config, node_info, handler)
            .await
            .unwrap();

        assert_eq!(service.get_peer_count(), 0);
    }
}

