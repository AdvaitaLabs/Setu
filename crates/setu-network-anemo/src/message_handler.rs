// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Message handler for incoming Setu protocol messages
//!
//! This module provides a Router handler that processes incoming `SetuMessage`
//! requests and returns appropriate responses.

use crate::state_sync::StateSyncStore;
use anemo::{Request, Response, Router};
use bytes::Bytes;
use setu_protocol::{NetworkEvent, SetuMessage, SerializedEvent};
use setu_types::Event;
use std::{convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tower::util::BoxCloneService;
use tracing::{debug, warn};

/// Create a router with the Setu message handler
///
/// This registers a handler at "/setu" that processes incoming `SetuMessage` 
/// requests and returns responses for request-response patterns like `RequestEvents`.
pub fn create_setu_router<S>(
    store: Arc<S>,
    local_node_id: String,
    event_tx: mpsc::Sender<NetworkEvent>,
) -> Router
where
    S: StateSyncStore,
{
    let handler = SetuMessageHandler {
        store,
        local_node_id,
        event_tx,
    };
    
    Router::new().route("/setu", handler.into_service())
}

/// Handler for Setu protocol messages
struct SetuMessageHandler<S> {
    store: Arc<S>,
    local_node_id: String,
    event_tx: mpsc::Sender<NetworkEvent>,
}

impl<S> SetuMessageHandler<S>
where
    S: StateSyncStore,
{
    fn into_service(self) -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
        let handler = Arc::new(self);
        
        let service = tower::service_fn(move |request: Request<Bytes>| {
            let handler = handler.clone();
            async move {
                let response = handler.handle_request(request).await;
                Ok::<_, Infallible>(response)
            }
        });
        
        tower::util::BoxCloneService::new(service)
    }
    
    async fn handle_request(&self, request: Request<Bytes>) -> Response<Bytes> {
        // Deserialize the incoming message
        let message: SetuMessage = match bincode::deserialize(request.body()) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Failed to deserialize message: {}", e);
                return Response::new(Bytes::new());
            }
        };
        
        debug!("Received message: {:?}", std::mem::discriminant(&message));
        
        // Handle the message based on type
        let response_message = match message {
            SetuMessage::RequestEvents { event_ids, requester_id } => {
                debug!(
                    "Processing RequestEvents from {}: {} event(s) requested",
                    requester_id,
                    event_ids.len()
                );
                
                // Fetch events from store (returns SerializedEvent)
                match self.store.get_events_by_ids(&event_ids).await {
                    Ok(serialized_events) => {
                        // Convert SerializedEvent to Event by deserializing the data field
                        let events: Vec<Event> = serialized_events
                            .into_iter()
                            .filter_map(|se| {
                                // Deserialize the data field which contains the full Event
                                bincode::deserialize(&se.data).ok()
                            })
                            .collect();
                        
                        debug!("Found {} events to return", events.len());
                        Some(SetuMessage::EventsResponse {
                            events,
                            responder_id: self.local_node_id.clone(),
                        })
                    }
                    Err(e) => {
                        warn!("Failed to get events: {:?}", e);
                        Some(SetuMessage::EventsResponse {
                            events: Vec::new(),
                            responder_id: self.local_node_id.clone(),
                        })
                    }
                }
            }
            
            SetuMessage::Ping { timestamp, nonce } => {
                debug!("Processing Ping request");
                Some(SetuMessage::Pong { timestamp, nonce })
            }
            
            SetuMessage::EventBroadcast { event, sender_id } => {
                debug!(
                    "Processing EventBroadcast from {}: event_id={}",
                    sender_id,
                    event.id
                );
                
                // Notify application layer
                let _ = self.event_tx.try_send(NetworkEvent::EventReceived {
                    peer_id: sender_id.clone(),
                    event: event.clone(),
                });
                
                // Convert Event to SerializedEvent for storage
                let serialized = SerializedEvent {
                    seq: 0, // Will be assigned by store
                    id: event.id.clone(),
                    data: bincode::serialize(&event).unwrap_or_default(),
                };
                
                // Store received event
                if let Err(e) = self.store.store_events(vec![serialized]).await {
                    warn!("Failed to store broadcast event: {:?}", e);
                } else {
                    debug!("Stored event from broadcast");
                }
                
                // EventBroadcast doesn't require a response
                None
            }
            
            // Consensus messages - notify application layer
            SetuMessage::CFProposal { cf, proposer_id } => {
                debug!(
                    "Received CFProposal from {}: cf_id={}",
                    proposer_id,
                    cf.id
                );
                // Notify application layer
                let _ = self.event_tx.try_send(NetworkEvent::CFProposal {
                    peer_id: proposer_id,
                    cf,
                });
                None
            }
            
            SetuMessage::CFVote { vote } => {
                debug!(
                    "Received CFVote: cf_id={}, voter={}",
                    vote.cf_id,
                    vote.validator_id
                );
                // Notify application layer
                let _ = self.event_tx.try_send(NetworkEvent::VoteReceived {
                    peer_id: vote.validator_id.clone(),
                    vote,
                });
                None
            }
            
            SetuMessage::CFFinalized { cf, sender_id } => {
                debug!(
                    "Received CFFinalized from {}: cf_id={}",
                    sender_id,
                    cf.id
                );
                // Notify application layer
                let _ = self.event_tx.try_send(NetworkEvent::CFFinalized {
                    peer_id: sender_id,
                    cf,
                });
                None
            }
            
            // Response messages should not be received as requests
            SetuMessage::EventsResponse { .. } | SetuMessage::Pong { .. } => {
                warn!("Received response message as request - ignoring");
                None
            }
        };
        
        // Serialize and return response
        match response_message {
            Some(msg) => {
                match bincode::serialize(&msg) {
                    Ok(bytes) => Response::new(Bytes::from(bytes)),
                    Err(e) => {
                        warn!("Failed to serialize response: {}", e);
                        Response::new(Bytes::new())
                    }
                }
            }
            None => Response::new(Bytes::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_sync::InMemoryStateSyncStore;
    use setu_types::VLCSnapshot;
    
    fn create_test_handler(store: Arc<InMemoryStateSyncStore>) -> (SetuMessageHandler<InMemoryStateSyncStore>, mpsc::Receiver<NetworkEvent>) {
        let (event_tx, event_rx) = mpsc::channel(100);
        let handler = SetuMessageHandler {
            store,
            local_node_id: "test_node".to_string(),
            event_tx,
        };
        (handler, event_rx)
    }
    
    #[tokio::test]
    async fn test_request_events_handler() {
        let store = Arc::new(InMemoryStateSyncStore::new());
        
        // Create a test event and serialize it
        let event1 = Event::genesis("test_creator".to_string(), VLCSnapshot::default());
        let event1_data = bincode::serialize(&event1).unwrap();
        
        // Add serialized event to the store
        let serialized_event = SerializedEvent {
            seq: 1,
            id: event1.id.clone(),
            data: event1_data,
        };
        store.add_event(serialized_event).await;
        
        // Create handler
        let (handler, _event_rx) = create_test_handler(store);
        
        // Create request
        let request_msg = SetuMessage::RequestEvents {
            event_ids: vec![event1.id.clone()],
            requester_id: "requester".to_string(),
        };
        let request_bytes = bincode::serialize(&request_msg).unwrap();
        let request = Request::new(Bytes::from(request_bytes));
        
        // Handle request
        let response = handler.handle_request(request).await;
        
        // Verify response
        let response_msg: SetuMessage = bincode::deserialize(response.body()).unwrap();
        match response_msg {
            SetuMessage::EventsResponse { events, responder_id } => {
                assert_eq!(responder_id, "test_node");
                assert_eq!(events.len(), 1);
            }
            _ => panic!("Expected EventsResponse"),
        }
    }
    
    #[tokio::test]
    async fn test_ping_handler() {
        let store = Arc::new(InMemoryStateSyncStore::new());
        let (handler, _event_rx) = create_test_handler(store);
        
        let request_msg = SetuMessage::Ping {
            timestamp: 12345,
            nonce: 99,
        };
        let request_bytes = bincode::serialize(&request_msg).unwrap();
        let request = Request::new(Bytes::from(request_bytes));
        
        let response = handler.handle_request(request).await;
        
        let response_msg: SetuMessage = bincode::deserialize(response.body()).unwrap();
        match response_msg {
            SetuMessage::Pong { timestamp, nonce } => {
                assert_eq!(timestamp, 12345);
                assert_eq!(nonce, 99);
            }
            _ => panic!("Expected Pong"),
        }
    }
    
    #[tokio::test]
    async fn test_event_broadcast_handler() {
        use setu_types::Event;
        
        let store = Arc::new(InMemoryStateSyncStore::new());
        let (handler, mut event_rx) = create_test_handler(store.clone());
        
        // Create a test event to broadcast
        let event = Event::genesis("sender_node".to_string(), VLCSnapshot::default());
        let event_id = event.id.clone();
        
        let request_msg = SetuMessage::EventBroadcast {
            event,
            sender_id: "sender_node".to_string(),
        };
        let request_bytes = bincode::serialize(&request_msg).unwrap();
        let request = Request::new(Bytes::from(request_bytes));
        
        // Handle request (no response expected for broadcast)
        let response = handler.handle_request(request).await;
        assert!(response.body().is_empty(), "EventBroadcast should not return response");
        
        // Verify event was stored
        let stored_events = store.get_events_by_ids(&[event_id.clone()]).await.unwrap();
        assert_eq!(stored_events.len(), 1);
        assert_eq!(stored_events[0].id, event_id);
        
        // Verify seq was auto-assigned (not 0)
        assert!(stored_events[0].seq > 0, "seq should be auto-assigned");
        
        // Verify NetworkEvent was sent
        let network_event = event_rx.try_recv().unwrap();
        match network_event {
            NetworkEvent::EventReceived { peer_id, event } => {
                assert_eq!(peer_id, "sender_node");
                assert_eq!(event.id, event_id);
            }
            _ => panic!("Expected EventReceived"),
        }
    }
}
