// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Builder pattern for Discovery service

use super::{
    metrics::DiscoveryMetrics, DiscoveryConfig, DiscoveryEventLoop, Handle, Server, SignedNodeInfo,
    State,
};
use anemo::NetworkRef;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

/// Discovery Service Builder
///
/// Provides a fluent interface for constructing the Discovery service
/// with optional configuration and metrics.
///
/// # Example
///
/// ```ignore
/// let (unstarted, server) = Builder::new()
///     .config(DiscoveryConfig::default())
///     .with_metrics(&prometheus_registry)
///     .build();
///
/// // Start the discovery after network is ready
/// let handle = unstarted.start(network);
/// ```
pub struct Builder {
    config: Option<DiscoveryConfig>,
    metrics: Option<DiscoveryMetrics>,
    our_info: Option<SignedNodeInfo>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    /// Create a new discovery builder
    pub fn new() -> Self {
        Self {
            config: None,
            metrics: None,
            our_info: None,
        }
    }

    /// Set the discovery configuration
    pub fn config(mut self, config: DiscoveryConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Enable metrics collection with the given registry
    pub fn with_metrics(mut self, registry: &prometheus::Registry) -> Self {
        self.metrics = Some(DiscoveryMetrics::new(registry));
        self
    }

    /// Set our own node info
    pub fn our_info(mut self, info: SignedNodeInfo) -> Self {
        self.our_info = Some(info);
        self
    }

    /// Build the discovery service
    ///
    /// Returns an `UnstartedDiscovery` that can be started once the network is ready,
    /// and a `Server` that should be registered with the network.
    pub fn build(self) -> (UnstartedDiscovery, Server) {
        let config = self.config.unwrap_or_default();
        let metrics = self.metrics.unwrap_or_else(DiscoveryMetrics::disabled);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let handle = Handle {
            _shutdown_handle: Arc::new(shutdown_tx),
        };

        let state = State {
            our_info: self.our_info,
            connected_peers: HashMap::new(),
            known_peers: HashMap::new(),
        };
        let state = Arc::new(RwLock::new(state));

        let server = Server {
            state: state.clone(),
            config: config.clone(),
        };

        (
            UnstartedDiscovery {
                handle,
                config,
                metrics,
                state,
                shutdown_rx,
            },
            server,
        )
    }
}

/// Unstarted discovery service
///
/// This struct holds the discovery configuration and state before the network
/// is available. Call `start()` once the network is ready to begin discovery.
pub struct UnstartedDiscovery {
    handle: Handle,
    config: DiscoveryConfig,
    #[allow(dead_code)]
    metrics: DiscoveryMetrics,
    state: Arc<RwLock<State>>,
    shutdown_rx: oneshot::Receiver<()>,
}

impl UnstartedDiscovery {
    /// Start the discovery service with the given network
    ///
    /// Returns a handle that keeps the discovery alive. When all handles are
    /// dropped, the discovery service will gracefully shut down.
    pub fn start(self, network: NetworkRef) -> (Handle, tokio::task::JoinHandle<()>) {
        let event_loop =
            DiscoveryEventLoop::new(self.config, self.state, network, self.shutdown_rx);

        let join_handle = tokio::spawn(async move {
            event_loop.run().await;
        });

        (self.handle, join_handle)
    }

    /// Get access to the shared state
    pub fn state(&self) -> Arc<RwLock<State>> {
        self.state.clone()
    }
}
