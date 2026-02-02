//! Message Router
//!
//! Routes incoming network events to appropriate handlers in the consensus layer.
//! Events and CFs are stored in-memory (DAG) until finalized, then persisted.

use consensus::ConsensusEngine;
use crate::protocol::NetworkEvent;
use setu_storage::{EventStoreBackend, AnchorStoreBackend};
use setu_types::{ConsensusFrame, Event, Vote};
use crate::persistence::FinalizationPersister;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Trait for handling network events
///
/// This trait allows decoupling the network layer from the consensus layer.
/// Implementations receive network events and route them to appropriate handlers.
#[async_trait::async_trait]
pub trait NetworkEventHandler: Send + Sync {
    /// Handle an incoming event from the network
    async fn handle_event(&self, peer_id: String, event: Event);
    
    /// Handle an incoming CF proposal
    async fn handle_cf_proposal(&self, peer_id: String, cf: ConsensusFrame);
    
    /// Handle an incoming vote
    async fn handle_vote(&self, peer_id: String, vote: Vote);
    
    /// Handle CF finalized notification
    async fn handle_cf_finalized(&self, peer_id: String, cf: ConsensusFrame);
    
    /// Handle peer connected event
    async fn handle_peer_connected(&self, peer_id: String);
    
    /// Handle peer disconnected event
    async fn handle_peer_disconnected(&self, peer_id: String);
}

/// Message router that directs network events to the consensus engine
/// 
/// The router handles:
/// 1. Adding events to the DAG via ConsensusEngine (in-memory until finalized)
/// 2. Routing CF proposals and votes to consensus
/// 3. Persisting finalized data (Anchor + Events) when CF reaches quorum
/// 
/// Note: Events are NOT persisted immediately. Persistence happens when CF is finalized,
/// which batch-persists the Anchor and all Events included in the CF.
pub struct MessageRouter {
    /// The consensus engine to route messages to
    engine: Arc<ConsensusEngine>,
    /// Event store for persisting finalized events
    event_store: Arc<dyn EventStoreBackend>,
    /// Anchor store for persisting finalized anchors
    anchor_store: Arc<dyn AnchorStoreBackend>,

}

impl MessageRouter {
    /// Create a new message router with storage for persistence
    pub fn new(
        engine: Arc<ConsensusEngine>,
        event_store: Arc<dyn EventStoreBackend>,
        anchor_store: Arc<dyn AnchorStoreBackend>,
    ) -> Self {
        Self {
            engine,
            event_store,
            anchor_store,
        }
    }
    
    /// Start the message router event loop
    ///
    /// This spawns a task that consumes network events and routes them
    /// to the appropriate handlers.
    pub fn start(
        self: Arc<Self>,
        mut event_rx: mpsc::Receiver<NetworkEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Message router started");
            
            while let Some(event) = event_rx.recv().await {
                self.route_event(event).await;
            }
            
            info!("Message router stopped");
        })
    }
    
    /// Route a single network event to the appropriate handler
    async fn route_event(&self, event: NetworkEvent) {
        match event {
            NetworkEvent::EventReceived { peer_id, event } => {
                self.handle_event(peer_id, event).await;
            }
            NetworkEvent::CFProposal { peer_id, cf } => {
                self.handle_cf_proposal(peer_id, cf).await;
            }
            NetworkEvent::VoteReceived { peer_id, vote } => {
                self.handle_vote(peer_id, vote).await;
            }
            NetworkEvent::CFFinalized { peer_id, cf } => {
                self.handle_cf_finalized(peer_id, cf).await;
            }
            NetworkEvent::PeerConnected { peer_id, node_info } => {
                debug!(peer = %peer_id, node = ?node_info, "Peer connected");
                self.handle_peer_connected(peer_id).await;
            }
            NetworkEvent::PeerDisconnected { peer_id } => {
                debug!(peer = %peer_id, "Peer disconnected");
                self.handle_peer_disconnected(peer_id).await;
            }
        }
    }
}

/// Implement FinalizationPersister for MessageRouter
/// 
/// This provides the shared persist_finalized_anchor() implementation
#[async_trait::async_trait]
impl FinalizationPersister for MessageRouter {
    fn engine(&self) -> &Arc<ConsensusEngine> {
        &self.engine
    }
    
    fn event_store(&self) -> &Arc<dyn EventStoreBackend> {
        &self.event_store
    }
    
    fn anchor_store(&self) -> &Arc<dyn AnchorStoreBackend> {
        &self.anchor_store
    }
}

#[async_trait::async_trait]
impl NetworkEventHandler for MessageRouter {
    async fn handle_event(&self, peer_id: String, event: Event) {
        debug!(
            event_id = %event.id,
            from = %peer_id,
            "Routing event to consensus engine"
        );
        
        // Step 1: Verify event ID matches content (anti-tampering)
        if !event.verify_id() {
            warn!(
                event_id = %event.id,
                from = %peer_id,
                "Event ID verification failed - possible tampering, rejecting"
            );
            return;
        }
        
        // Step 2: Add to consensus DAG (in-memory only)
        // Events are persisted later when CF is finalized
        // Note: receive_event_from_network is idempotent - duplicate events return Ok
        match self.engine.receive_event_from_network(event.clone()).await {
            Ok(event_id) => {
                debug!(
                    event_id = %event_id,
                    "Event successfully added to DAG (pending finalization)"
                );
            }
            Err(e) => {
                let error_str = e.to_string();
                
                // Check if this is a MissingParent error
                if error_str.contains("Missing parent") {
                    warn!(
                        event_id = %event.id,
                        from = %peer_id,
                        parents = ?event.parent_ids,
                        "Event has missing parents, attempting sync"
                    );
                    
                    // Attempt to fetch missing parent events from the broadcaster
                    if let Some(broadcaster) = self.engine.get_broadcaster().await {
                        match broadcaster.request_events(&event.parent_ids).await {
                            Ok(fetched_events) => {
                                debug!(
                                    count = fetched_events.len(),
                                    "Fetched missing parent events"
                                );
                                
                                // Add fetched parents to DAG
                                for parent_event in fetched_events {
                                    if let Err(e) = self.engine.receive_event_from_network(parent_event).await {
                                        debug!(error = %e, "Failed to add fetched parent (may already exist)");
                                    }
                                }
                                
                                // Retry adding the original event
                                match self.engine.receive_event_from_network(event).await {
                                    Ok(event_id) => {
                                        debug!(
                                            event_id = %event_id,
                                            "Event added after parent sync"
                                        );
                                    }
                                    Err(retry_err) => {
                                        warn!(
                                            error = %retry_err,
                                            "Event still failed after parent sync"
                                        );
                                    }
                                }
                            }
                            Err(fetch_err) => {
                                warn!(
                                    error = %fetch_err,
                                    "Failed to fetch missing parent events"
                                );
                            }
                        }
                    } else {
                        warn!("No broadcaster available for parent sync");
                    }
                } else {
                    warn!(
                        event_id = %event.id,
                        error = %e,
                        "Failed to add event to DAG"
                    );
                }
            }
        }
    }
    
    async fn handle_cf_proposal(&self, peer_id: String, cf: ConsensusFrame) {
        info!(
            cf_id = %cf.id,
            proposer = %cf.proposer,
            from = %peer_id,
            "Routing CF proposal to consensus engine"
        );
        
        let cf_id = cf.id.clone();
        match self.engine.receive_cf(cf).await {
            Ok((finalized, anchor)) => {
                if finalized {
                    info!(
                        cf_id = %cf_id,
                        anchor = ?anchor.as_ref().map(|a| &a.id),
                        "CF finalized after local vote"
                    );
                    
                    // Persist finalized data (same logic as handle_vote)
                    if let Some(anchor) = anchor {
                        if let Err(e) = self.persist_finalized_anchor(&anchor).await {
                            warn!(
                                anchor_id = %anchor.id,
                                error = %e,
                                "Failed to persist finalized anchor (will retry)"
                            );
                        }
                    }
                } else {
                    debug!(cf_id = %cf_id, "CF proposal processed, awaiting votes");
                }
            }
            Err(e) => {
                warn!(
                    cf_id = %cf_id,
                    error = %e,
                    "Failed to process CF proposal"
                );
            }
        }
    }
    
    async fn handle_vote(&self, peer_id: String, vote: Vote) {
        debug!(
            cf_id = %vote.cf_id,
            voter = %vote.validator_id,
            from = %peer_id,
            "Routing vote to consensus engine"
        );
        
        match self.engine.receive_vote(vote.clone()).await {
            Ok((finalized, anchor)) => {
                if finalized {
                    info!(
                        cf_id = %vote.cf_id,
                        anchor = ?anchor.as_ref().map(|a| &a.id),
                        "CF finalized after vote"
                    );
                    
                    // Persist finalized data
                    if let Some(anchor) = anchor {
                        if let Err(e) = self.persist_finalized_anchor(&anchor).await {
                            warn!(
                                anchor_id = %anchor.id,
                                error = %e,
                                "Failed to persist finalized anchor (will retry)"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    cf_id = %vote.cf_id,
                    error = %e,
                    "Failed to process vote"
                );
            }
        }
    }
    
    async fn handle_cf_finalized(&self, peer_id: String, cf: ConsensusFrame) {
        info!(
            cf_id = %cf.id,
            from = %peer_id,
            "Received CF finalized notification"
        );
        // The engine already handles finalization through vote quorum
        // This notification is for state sync purposes
    }
    
    async fn handle_peer_connected(&self, peer_id: String) {
        debug!(peer = %peer_id, "New peer connected - may trigger sync");
        // Could trigger state sync with new peer here
    }
    
    async fn handle_peer_disconnected(&self, peer_id: String) {
        debug!(peer = %peer_id, "Peer disconnected");
        // Could update peer tracking state here
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consensus::{ConsensusEngine, ValidatorSet};
    use setu_types::{ConsensusConfig, ValidatorInfo, NodeInfo, VLCSnapshot};
    use setu_vlc::VectorClock;
    
    fn create_test_stores() -> (Arc<EventStore>, Arc<AnchorStore>) {
        let event_store = Arc::new(EventStore::new());
        let anchor_store = Arc::new(AnchorStore::new());
        (event_store, anchor_store)
    }
    
    fn create_test_engine() -> Arc<ConsensusEngine> {
        let config = ConsensusConfig::default();
        let mut validator_set = ValidatorSet::new();
        let node_info = NodeInfo::new_validator("v1".to_string(), "127.0.0.1".to_string(), 8080);
        validator_set.add_validator(ValidatorInfo::new(node_info, false));
        Arc::new(ConsensusEngine::new(
            config,
            "v1".to_string(),
            validator_set,
        ))
    }
    
    fn create_test_event() -> Event {
        Event::genesis(
            "test-creator".to_string(),
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
        )
    }
    
    #[tokio::test]
    async fn test_router_creation() {
        let engine = create_test_engine();
        let (event_store, anchor_store) = create_test_stores();
        let _router = MessageRouter::new(engine, event_store, anchor_store);
        // Router created successfully
    }
    
    #[tokio::test]
    async fn test_handle_event_adds_to_dag() {
        let engine = create_test_engine();
        let (event_store, anchor_store) = create_test_stores();
        let router = MessageRouter::new(engine.clone(), event_store, anchor_store);
        
        let event = create_test_event();
        let event_id = event.id.clone();
        
        // Handle the event - adds to DAG only, not persisted
        router.handle_event("peer-1".to_string(), event).await;
        
        // Verify it was added to the DAG
        let events = engine.get_events_by_ids(&[event_id.clone()]).await;
        assert_eq!(events.len(), 1, "Event should be in DAG");
        assert_eq!(events[0].id, event_id);
    }
}
