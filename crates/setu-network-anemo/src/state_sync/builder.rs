// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Builder pattern for StateSync service

use super::{
    Handle, Server, StateSyncConfig, StateSyncEventLoop, StateSyncMessage, StateSyncStore, SyncState,
    metrics::StateSyncMetrics,
};
use anemo::NetworkRef;
use std::sync::Arc;
use tokio::sync::mpsc;

/// State Sync Service Builder
///
/// Provides a fluent interface for constructing the StateSync service
/// with a required store and optional configuration.
///
/// # Example
///
/// ```ignore
/// let store = Arc::new(InMemoryStateSyncStore::new());
/// let (unstarted, server) = Builder::new()
///     .store(store)
///     .config(StateSyncConfig::default())
///     .with_metrics(&prometheus_registry)
///     .build();
///
/// // Start the sync after network is ready
/// let handle = unstarted.start(network);
/// ```
pub struct Builder<S> {
    store: Option<Arc<S>>,
    config: Option<StateSyncConfig>,
    metrics: Option<StateSyncMetrics>,
}

impl Builder<()> {
    /// Create a new state sync builder
    pub fn new() -> Self {
        Self {
            store: None,
            config: None,
            metrics: None,
        }
    }
}

impl Default for Builder<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Builder<S> {
    /// Set the storage backend (must be wrapped in Arc)
    pub fn store<NewStore>(self, store: Arc<NewStore>) -> Builder<NewStore> {
        Builder {
            store: Some(store),
            config: self.config,
            metrics: self.metrics,
        }
    }

    /// Set the state sync configuration
    pub fn config(mut self, config: StateSyncConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Enable metrics collection
    pub fn with_metrics(mut self, registry: &prometheus::Registry) -> Self {
        self.metrics = Some(StateSyncMetrics::new(registry));
        self
    }
}

impl<S> Builder<S>
where
    S: StateSyncStore,
{
    /// Build the state sync service
    ///
    /// Returns an `UnstartedStateSync` that can be started once the network is ready,
    /// and a `Server` that should be registered with the network.
    pub fn build(self) -> (UnstartedStateSync<S>, Server<Arc<S>>) {
        let store = self.store.expect("Store is required");
        let config = self.config.unwrap_or_default();
        let metrics = self.metrics.unwrap_or_else(StateSyncMetrics::disabled);

        let (tx, rx) = mpsc::channel(1000);
        
        let handle = Handle { sender: tx };

        let state = Arc::new(SyncState::new());

        let server = Server {
            state: state.clone(),
            store: store.clone(),
            config: config.clone(),
        };

        (
            UnstartedStateSync {
                handle,
                config,
                metrics,
                state,
                store,
                message_rx: rx,
            },
            server,
        )
    }
}

/// Unstarted state sync service
pub struct UnstartedStateSync<S> {
    handle: Handle,
    config: StateSyncConfig,
    #[allow(dead_code)]
    metrics: StateSyncMetrics,
    state: Arc<SyncState>,
    store: Arc<S>,
    message_rx: mpsc::Receiver<StateSyncMessage>,
}

impl<S> UnstartedStateSync<S>
where
    S: StateSyncStore,
{
    /// Start the state sync service
    pub fn start(self, network: NetworkRef) -> (Handle, tokio::task::JoinHandle<()>) {
        let event_loop = StateSyncEventLoop::new(
            self.config,
            self.state,
            self.store,
            network,
            self.message_rx,
        );

        let join_handle = tokio::spawn(async move {
            event_loop.run().await;
        });

        (self.handle, join_handle)
    }

    /// Get access to the shared state
    pub fn state(&self) -> Arc<SyncState> {
        self.state.clone()
    }
}
