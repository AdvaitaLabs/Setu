// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Generic message handler trait for the network layer
//!
//! This module provides a generic interface for handling incoming network messages,
//! allowing the application layer to define message types and processing logic
//! without coupling the network layer to specific message formats.

use anemo::{Request, Response};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;

/// Result type for message handling
pub type HandleResult = Result<Option<Bytes>, HandlerError>;

/// Error type for message handlers
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("Failed to deserialize message: {0}")]
    Deserialize(String),
    
    #[error("Failed to serialize response: {0}")]
    Serialize(String),
    
    #[error("Message handler error: {0}")]
    Handler(String),
    
    #[error("Storage error: {0}")]
    Storage(String),
}

/// Generic trait for handling incoming network messages
///
/// This trait abstracts the message handling logic, allowing different
/// applications to provide their own message types and processing logic.
///
/// # Example
///
/// ```ignore
/// struct MyMessageHandler {
///     // application-specific state
/// }
///
/// #[async_trait]
/// impl GenericMessageHandler for MyMessageHandler {
///     async fn handle(&self, route: &str, body: Bytes) -> HandleResult {
///         // Parse and handle the message based on route
///         match route {
///             "/my-protocol" => {
///                 let msg: MyMessage = bincode::deserialize(&body)?;
///                 // Process message...
///                 Ok(Some(response_bytes))
///             }
///             _ => Ok(None),
///         }
///     }
///     
///     fn routes(&self) -> Vec<&'static str> {
///         vec!["/my-protocol"]
///     }
/// }
/// ```
#[async_trait]
pub trait GenericMessageHandler: Send + Sync + 'static {
    /// Handle an incoming message
    ///
    /// # Arguments
    /// * `route` - The route path (e.g., "/setu")
    /// * `body` - The raw message bytes
    ///
    /// # Returns
    /// * `Ok(Some(bytes))` - Response bytes to send back
    /// * `Ok(None)` - No response needed (for one-way messages)
    /// * `Err(e)` - An error occurred during handling
    async fn handle(&self, route: &str, body: Bytes) -> HandleResult;
    
    /// Get the routes this handler should be registered for
    fn routes(&self) -> Vec<&'static str>;
}

/// Create an Anemo router from a generic message handler
///
/// This function wraps a `GenericMessageHandler` implementation in an Anemo-compatible
/// service and registers it for all routes returned by `handler.routes()`.
pub fn create_router_from_handler<H>(handler: Arc<H>) -> anemo::Router
where
    H: GenericMessageHandler,
{
    use std::convert::Infallible;
    use tower::util::BoxCloneService;
    
    let routes = handler.routes();
    let mut router = anemo::Router::new();
    
    for route in routes {
        let handler = handler.clone();
        let route_owned = route.to_string();
        
        let service = tower::service_fn(move |request: Request<Bytes>| {
            let handler = handler.clone();
            let route = route_owned.clone();
            async move {
                match handler.handle(&route, request.into_body()).await {
                    Ok(Some(response_bytes)) => {
                        Ok::<_, Infallible>(Response::new(response_bytes))
                    }
                    Ok(None) => {
                        Ok(Response::new(Bytes::new()))
                    }
                    Err(e) => {
                        tracing::warn!("Handler error: {}", e);
                        Ok(Response::new(Bytes::new()))
                    }
                }
            }
        });
        
        router = router.route(route, BoxCloneService::new(service));
    }
    
    router
}

/// Blanket implementation for Arc<H>
#[async_trait]
impl<H: GenericMessageHandler> GenericMessageHandler for Arc<H> {
    async fn handle(&self, route: &str, body: Bytes) -> HandleResult {
        (**self).handle(route, body).await
    }
    
    fn routes(&self) -> Vec<&'static str> {
        (**self).routes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    struct TestHandler;
    
    #[async_trait]
    impl GenericMessageHandler for TestHandler {
        async fn handle(&self, route: &str, body: Bytes) -> HandleResult {
            match route {
                "/test" => {
                    // Echo the message back
                    Ok(Some(body))
                }
                _ => Ok(None),
            }
        }
        
        fn routes(&self) -> Vec<&'static str> {
            vec!["/test"]
        }
    }
    
    #[test]
    fn test_handler_routes() {
        let handler = TestHandler;
        assert_eq!(handler.routes(), vec!["/test"]);
    }
    
    #[tokio::test]
    async fn test_handler_echo() {
        let handler = TestHandler;
        let input = Bytes::from("hello");
        
        let result = handler.handle("/test", input.clone()).await.unwrap();
        assert_eq!(result, Some(input));
    }
    
    #[tokio::test]
    async fn test_unknown_route() {
        let handler = TestHandler;
        let result = handler.handle("/unknown", Bytes::new()).await.unwrap();
        assert_eq!(result, None);
    }
}
