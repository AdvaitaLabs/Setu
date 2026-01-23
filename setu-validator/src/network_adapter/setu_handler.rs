//! Setu Message Handler Implementation
//!
//! This module implements the `GenericMessageHandler` trait from the network layer,
//! providing Setu-specific message handling logic. This moves the business logic
//! from the network layer to the application layer.

use crate::protocol::{NetworkEvent, SetuMessage, SerializedEvent};
use async_trait::async_trait;
use bytes::Bytes;
use setu_network_anemo::{GenericMessageHandler, HandleResult, HandlerError};
use setu_types::Event;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Route path for Setu protocol messages
pub const SETU_ROUTE: &str = "/setu";

/// Storage trait required by the message handler
///
/// This trait abstracts the storage operations needed for message handling,
/// allowing different storage backends to be used.
///
/// ## Three-Layer Query Requirement
/// 
/// Implementations MUST support three-layer query (DAG → EventStore) to ensure
/// events can be found even after GC. Use `ConsensusEngine::get_events_by_ids_three_layer()`
/// or equivalent logic:
/// 
/// ```ignore
/// // Step 1: Query DAG (hot data)
/// let mut results = Vec::new();
/// let dag = dag_manager.dag().read().await;
/// for id in event_ids {
///     if let Some(event) = dag.get_event(id) {
///         results.push(event.clone());
///     } else {
///         store_query_ids.push(id.clone());
///     }
/// }
/// 
/// // Step 2: Query EventStore for misses (cold data)
/// if !store_query_ids.is_empty() {
///     let store_events = event_store.get_events_batch(&store_query_ids).await;
///     results.extend(store_events);
/// }
/// ```
#[async_trait]
pub trait MessageHandlerStore: Send + Sync + 'static {
    /// Get events by their IDs
    /// 
    /// **IMPORTANT**: Implementation must use three-layer query (DAG → EventStore)
    /// to find events that may have been GC'd from the active DAG.
    async fn get_events_by_ids(&self, event_ids: &[String]) -> Result<Vec<SerializedEvent>, String>;
    
    /// Store events
    async fn store_events(&self, events: Vec<SerializedEvent>) -> Result<(), String>;
}

/// Setu protocol message handler
///
/// This handler implements the `GenericMessageHandler` trait, processing
/// incoming Setu protocol messages and producing responses.
pub struct SetuMessageHandler<S> {
    store: Arc<S>,
    local_node_id: String,
    event_tx: mpsc::Sender<NetworkEvent>,
}

impl<S> SetuMessageHandler<S>
where
    S: MessageHandlerStore,
{
    /// Create a new Setu message handler
    pub fn new(
        store: Arc<S>,
        local_node_id: String,
        event_tx: mpsc::Sender<NetworkEvent>,
    ) -> Self {
        Self {
            store,
            local_node_id,
            event_tx,
        }
    }
    
    async fn handle_message(&self, message: SetuMessage) -> Result<Option<SetuMessage>, HandlerError> {
        match message {
            SetuMessage::RequestEvents { event_ids, requester_id } => {
                debug!(
                    "Processing RequestEvents from {}: {} event(s) requested",
                    requester_id,
                    event_ids.len()
                );
                
                match self.store.get_events_by_ids(&event_ids).await {
                    Ok(serialized_events) => {
                        // Convert SerializedEvent to Event by deserializing the data field
                        let events: Vec<Event> = serialized_events
                            .into_iter()
                            .filter_map(|se| bincode::deserialize(&se.data).ok())
                            .collect();
                        
                        debug!("Found {} events to return", events.len());
                        Ok(Some(SetuMessage::EventsResponse {
                            events,
                            responder_id: self.local_node_id.clone(),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to get events: {}", e);
                        Ok(Some(SetuMessage::EventsResponse {
                            events: Vec::new(),
                            responder_id: self.local_node_id.clone(),
                        }))
                    }
                }
            }
            
            SetuMessage::Ping { timestamp, nonce } => {
                debug!("Processing Ping request");
                Ok(Some(SetuMessage::Pong { timestamp, nonce }))
            }
            
            SetuMessage::EventBroadcast { event, sender_id } => {
                debug!(
                    "Processing EventBroadcast from {}: event_id={}",
                    sender_id,
                    event.id
                );
                
                // Notify application layer with backpressure (await instead of try_send)
                // This ensures events are not dropped during high load
                if let Err(e) = self.event_tx.send(NetworkEvent::EventReceived {
                    peer_id: sender_id.clone(),
                    event: event.clone(),
                }).await {
                    // Channel closed indicates system shutdown
                    warn!(
                        event_id = %event.id,
                        error = %e,
                        "Event channel closed - system may be shutting down"
                    );
                }
                
                // TODO: Optimize storage strategy
                // Current behavior: Store unconfirmed events for State Sync
                // Future improvement: Only store confirmed events (after CF finalization)
                // Keeping current behavior for backward compatibility with State Sync protocol
                //
                // Note: This may store events that fail consensus validation.
                // State Sync should filter based on finalized anchors.
                let serialized = SerializedEvent {
                    seq: 0, // Will be assigned by store
                    id: event.id.clone(),
                    data: bincode::serialize(&event).unwrap_or_default(),
                };
                
                if let Err(e) = self.store.store_events(vec![serialized]).await {
                    warn!("Failed to store broadcast event: {}", e);
                } else {
                    debug!("Stored unconfirmed event for state sync");
                }
                
                Ok(None) // EventBroadcast doesn't require a response
            }
            
            SetuMessage::CFProposal { cf, proposer_id } => {
                debug!("Received CFProposal from {}: cf_id={}", proposer_id, cf.id);
                // Use send().await for backpressure instead of try_send to prevent dropping CF proposals
                // CF proposals are critical for consensus - dropping them causes consensus to stall
                if let Err(e) = self.event_tx.send(NetworkEvent::CFProposal {
                    peer_id: proposer_id.clone(),
                    cf: cf.clone(),
                }).await {
                    error!(
                        cf_id = %cf.id,
                        error = %e,
                        "CF channel closed - system may be shutting down"
                    );
                }
                Ok(None)
            }
            
            SetuMessage::CFVote { vote } => {
                debug!("Received CFVote: cf_id={}, voter={}", vote.cf_id, vote.validator_id);
                // Use send().await for backpressure instead of try_send to prevent dropping votes
                // Votes are critical for reaching quorum - dropping them prevents finalization
                if let Err(e) = self.event_tx.send(NetworkEvent::VoteReceived {
                    peer_id: vote.validator_id.clone(),
                    vote: vote.clone(),
                }).await {
                    error!(
                        cf_id = %vote.cf_id,
                        voter = %vote.validator_id,
                        error = %e,
                        "Vote channel closed - system may be shutting down"
                    );
                }
                Ok(None)
            }
            
            SetuMessage::CFFinalized { cf, sender_id } => {
                debug!("Received CFFinalized from {}: cf_id={}", sender_id, cf.id);
                // Use send().await for backpressure instead of try_send for consistency
                // CFFinalized notifications should not be dropped as they help with state sync
                if let Err(e) = self.event_tx.send(NetworkEvent::CFFinalized {
                    peer_id: sender_id.clone(),
                    cf: cf.clone(),
                }).await {
                    warn!(
                        cf_id = %cf.id,
                        error = %e,
                        "Finalized channel closed - system may be shutting down"
                    );
                }
                Ok(None)
            }
            
            // Response messages should not be received as requests
            SetuMessage::EventsResponse { .. } | SetuMessage::Pong { .. } => {
                warn!("Received response message as request - ignoring");
                Ok(None)
            }
        }
    }
}

#[async_trait]
impl<S> GenericMessageHandler for SetuMessageHandler<S>
where
    S: MessageHandlerStore,
{
    async fn handle(&self, route: &str, body: Bytes) -> HandleResult {
        if route != SETU_ROUTE {
            return Ok(None);
        }
        
        // Deserialize the incoming message
        let message: SetuMessage = bincode::deserialize(&body)
            .map_err(|e| HandlerError::Deserialize(e.to_string()))?;
        
        debug!("Received message: {:?}", std::mem::discriminant(&message));
        
        // Handle the message
        match self.handle_message(message).await? {
            Some(response) => {
                let bytes = bincode::serialize(&response)
                    .map_err(|e| HandlerError::Serialize(e.to_string()))?;
                Ok(Some(Bytes::from(bytes)))
            }
            None => Ok(None),
        }
    }
    
    fn routes(&self) -> Vec<&'static str> {
        vec![SETU_ROUTE]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::NetworkEvent;
    use setu_types::VLCSnapshot;
    use std::collections::HashMap;
    use tokio::sync::RwLock;
    
    /// Mock store for testing
    struct MockStore {
        events: RwLock<HashMap<String, SerializedEvent>>,
    }
    
    impl MockStore {
        fn new() -> Self {
            Self {
                events: RwLock::new(HashMap::new()),
            }
        }
        
        async fn add_event(&self, event: SerializedEvent) {
            self.events.write().await.insert(event.id.clone(), event);
        }
    }
    
    #[async_trait]
    impl MessageHandlerStore for MockStore {
        async fn get_events_by_ids(&self, event_ids: &[String]) -> Result<Vec<SerializedEvent>, String> {
            let events = self.events.read().await;
            Ok(event_ids
                .iter()
                .filter_map(|id| events.get(id).cloned())
                .collect())
        }
        
        async fn store_events(&self, events: Vec<SerializedEvent>) -> Result<(), String> {
            let mut store = self.events.write().await;
            for event in events {
                store.insert(event.id.clone(), event);
            }
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_ping_handler() {
        let store = Arc::new(MockStore::new());
        let (event_tx, _event_rx) = mpsc::channel(100);
        let handler = SetuMessageHandler::new(store, "test_node".to_string(), event_tx);
        
        let request = SetuMessage::Ping { timestamp: 12345, nonce: 99 };
        let request_bytes = Bytes::from(bincode::serialize(&request).unwrap());
        
        let result = handler.handle(SETU_ROUTE, request_bytes).await.unwrap();
        assert!(result.is_some());
        
        let response: SetuMessage = bincode::deserialize(&result.unwrap()).unwrap();
        match response {
            SetuMessage::Pong { timestamp, nonce } => {
                assert_eq!(timestamp, 12345);
                assert_eq!(nonce, 99);
            }
            _ => panic!("Expected Pong response"),
        }
    }
    
    #[tokio::test]
    async fn test_event_broadcast_handler() {
        let store = Arc::new(MockStore::new());
        let (event_tx, mut event_rx) = mpsc::channel(100);
        let handler = SetuMessageHandler::new(store, "test_node".to_string(), event_tx);
        
        let event = Event::genesis("sender".to_string(), VLCSnapshot::default());
        let event_id = event.id.clone();
        
        let request = SetuMessage::EventBroadcast {
            event,
            sender_id: "sender".to_string(),
        };
        let request_bytes = Bytes::from(bincode::serialize(&request).unwrap());
        
        let result = handler.handle(SETU_ROUTE, request_bytes).await.unwrap();
        assert!(result.is_none()); // Broadcast has no response
        
        // Verify network event was sent
        let network_event = event_rx.try_recv().unwrap();
        match network_event {
            NetworkEvent::EventReceived { peer_id, event } => {
                assert_eq!(peer_id, "sender");
                assert_eq!(event.id, event_id);
            }
            _ => panic!("Expected EventReceived"),
        }
    }
    
    #[tokio::test]
    async fn test_request_events_handler() {
        let store = Arc::new(MockStore::new());
        let (event_tx, _) = mpsc::channel(100);
        
        // Add a test event to the store
        let event = Event::genesis("creator".to_string(), VLCSnapshot::default());
        let event_id = event.id.clone();
        let serialized = SerializedEvent {
            seq: 1,
            id: event_id.clone(),
            data: bincode::serialize(&event).unwrap(),
        };
        store.add_event(serialized).await;
        
        let handler = SetuMessageHandler::new(store, "test_node".to_string(), event_tx);
        
        let request = SetuMessage::RequestEvents {
            event_ids: vec![event_id.clone()],
            requester_id: "requester".to_string(),
        };
        let request_bytes = Bytes::from(bincode::serialize(&request).unwrap());
        
        let result = handler.handle(SETU_ROUTE, request_bytes).await.unwrap();
        assert!(result.is_some());
        
        let response: SetuMessage = bincode::deserialize(&result.unwrap()).unwrap();
        match response {
            SetuMessage::EventsResponse { events, responder_id } => {
                assert_eq!(responder_id, "test_node");
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].id, event_id);
            }
            _ => panic!("Expected EventsResponse"),
        }
    }
}
