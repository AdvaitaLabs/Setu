// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Peer management for Anemo network

use crate::{error::Result, node_info::NodeInfo, transport::AnemoTransport};
use anemo::{types::PeerEvent, PeerId};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Information about a peer
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// Node information
    pub node_info: NodeInfo,
    /// Anemo peer ID
    pub peer_id: PeerId,
    /// Connection state
    pub connected: bool,
}

/// Manages peers in the Anemo network
pub struct AnemoPeerManager {
    /// Anemo transport
    transport: Arc<AnemoTransport>,
    /// Known peers
    peers: Arc<DashMap<PeerId, PeerInfo>>,
}

impl AnemoPeerManager {
    /// Create a new peer manager and spawn background event handler
    pub fn new(transport: Arc<AnemoTransport>) -> Result<Self> {
        let (mut event_rx, connected_peers) = transport.subscribe()?;

        let peers = Arc::new(DashMap::new());

        // Add already connected peers
        for peer_id in connected_peers {
            peers.insert(
                peer_id,
                PeerInfo {
                    node_info: NodeInfo::new_validator(
                        format!("peer-{}", peer_id),
                        "unknown".to_string(),
                        0,
                    ),
                    peer_id,
                    connected: true,
                },
            );
        }

        // Spawn background task to handle peer events
        let peers_clone = Arc::clone(&peers);
        tokio::spawn(async move {
            info!("Starting peer event handler");
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        Self::handle_peer_event_static(&peers_clone, event).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Peer event receiver lagged, skipped {} events", skipped);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Peer event channel closed, stopping peer manager");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            transport,
            peers,
        })
    }

    /// Handle peer events (static version for use in spawned task)
    async fn handle_peer_event_static(peers: &DashMap<PeerId, PeerInfo>, event: PeerEvent) {
        match event {
            PeerEvent::NewPeer(peer_id) => {
                info!("New peer connected: {}", peer_id);
                peers.entry(peer_id).and_modify(|info| {
                    info.connected = true;
                }).or_insert_with(|| PeerInfo {
                    node_info: NodeInfo::new_validator(
                        format!("peer-{}", peer_id),
                        "unknown".to_string(),
                        0,
                    ),
                    peer_id,
                    connected: true,
                });
            }
            PeerEvent::LostPeer(peer_id, reason) => {
                info!("Peer disconnected: {} (reason: {:?})", peer_id, reason);
                peers.entry(peer_id).and_modify(|info| {
                    info.connected = false;
                });
            }
        }
    }

    /// Add a peer with node information
    pub async fn add_peer(&self, node_info: NodeInfo, peer_id: PeerId) -> Result<()> {
        debug!("Adding peer {} with node info", peer_id);

        let peer_info = PeerInfo {
            node_info,
            peer_id,
            connected: false,
        };

        self.peers.insert(peer_id, peer_info);
        Ok(())
    }

    /// Remove a peer
    pub async fn remove_peer(&self, peer_id: &PeerId) -> Result<()> {
        debug!("Removing peer {}", peer_id);
        self.peers.remove(peer_id);
        Ok(())
    }

    /// Get peer information
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        self.peers.get(peer_id).map(|entry| entry.value().clone())
    }

    /// Get all connected peers
    pub fn get_connected_peers(&self) -> Vec<PeerInfo> {
        self.peers
            .iter()
            .filter(|entry| entry.value().connected)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get all peers (connected and disconnected)
    pub fn get_all_peers(&self) -> Vec<PeerInfo> {
        self.peers
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get peer count
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get connected peer count
    pub fn connected_peer_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|entry| entry.value().connected)
            .count()
    }

    /// Connect to a peer
    pub async fn connect_to_peer(&self, addr: std::net::SocketAddr) -> Result<PeerId> {
        self.transport.connect(addr).await
    }

    /// Disconnect from a peer
    pub async fn disconnect_from_peer(&self, peer_id: &PeerId) -> Result<()> {
        self.transport.disconnect(*peer_id)?;
        self.peers.entry(*peer_id).and_modify(|info| {
            info.connected = false;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AnemoConfig;

    #[tokio::test]
    async fn test_peer_manager_creation() {
        let config = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };

        let transport = Arc::new(AnemoTransport::new(&config).await.unwrap());
        let peer_manager = AnemoPeerManager::new(transport).unwrap();

        assert_eq!(peer_manager.peer_count(), 0);
    }

    #[tokio::test]
    async fn test_peer_tracking() {
        let config1 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport1 = Arc::new(AnemoTransport::new(&config1).await.unwrap());
        let peer_manager = AnemoPeerManager::new(transport1.clone()).unwrap();

        // No need to start peer manager, it's automatically started in new()

        let config2 = AnemoConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let transport2 = Arc::new(AnemoTransport::new(&config2).await.unwrap());

        // Connect
        let peer_id = transport1.connect(transport2.local_addr()).await.unwrap();

        // Give time for event processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify peer is tracked
        assert!(peer_manager.get_peer(&peer_id).is_some());
    }
}

impl Clone for AnemoPeerManager {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            peers: self.peers.clone(),
        }
    }
}
