// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Configuration for Anemo-based network

use serde::{Deserialize, Serialize};

/// Configuration for Anemo network
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnemoConfig {
    /// Listen address (e.g., "0.0.0.0:9000")
    pub listen_addr: String,

    /// Server name for TLS
    pub server_name: String,

    /// Ed25519 private key (32 bytes)
    /// If None, a random key will be generated
    pub private_key: Option<[u8; 32]>,

    /// QUIC configuration
    pub quic: QuicConfig,

    /// Connection limits
    pub connection_limits: ConnectionLimits,

    /// Timeout settings
    pub timeouts: TimeoutConfig,
}

/// QUIC protocol configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuicConfig {
    /// Maximum concurrent bidirectional streams
    pub max_concurrent_bidi_streams: u64,

    /// Maximum concurrent unidirectional streams
    pub max_concurrent_uni_streams: u64,

    /// Stream receive window size
    pub stream_receive_window: u64,

    /// Connection receive window size
    pub receive_window: u64,

    /// Send window size
    pub send_window: u64,

    /// Max idle timeout in milliseconds
    pub max_idle_timeout_ms: u64,

    /// Keep-alive interval in milliseconds
    pub keep_alive_interval_ms: Option<u64>,

    /// Socket send buffer size
    pub socket_send_buffer_size: Option<usize>,

    /// Socket receive buffer size
    pub socket_receive_buffer_size: Option<usize>,
}

/// Connection limits configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionLimits {
    /// Maximum number of concurrent connections
    pub max_concurrent_connections: Option<usize>,

    /// Maximum number of concurrent outgoing connection attempts
    pub max_concurrent_outstanding_connecting_connections: usize,

    /// Channel capacity for connection manager
    pub connection_manager_channel_capacity: usize,

    /// Peer event broadcast channel capacity
    pub peer_event_broadcast_channel_capacity: usize,
}

/// Timeout configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeoutConfig {
    /// Timeout for establishing a connection
    pub connection_timeout_ms: u64,

    /// Timeout for inbound requests
    pub inbound_request_timeout_ms: u64,

    /// Timeout for outbound requests
    pub outbound_request_timeout_ms: u64,

    /// Interval for connectivity checks
    pub connectivity_check_interval_ms: u64,

    /// Connection backoff duration
    pub connection_backoff_ms: u64,

    /// Maximum connection backoff duration
    pub max_connection_backoff_ms: u64,
}

impl Default for AnemoConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:9000".to_string(),
            server_name: crate::DEFAULT_SERVER_NAME.to_string(),
            private_key: None,
            quic: QuicConfig::default(),
            connection_limits: ConnectionLimits::default(),
            timeouts: TimeoutConfig::default(),
        }
    }
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            max_concurrent_bidi_streams: 100,
            max_concurrent_uni_streams: 100,
            stream_receive_window: 1_250_000,      // 1.25MB
            receive_window: 0,                      // Unlimited
            send_window: 10_000_000,                // 10MB
            max_idle_timeout_ms: 30_000,            // 30 seconds
            keep_alive_interval_ms: Some(5_000),    // 5 seconds
            // Use smaller buffer sizes for better compatibility
            socket_send_buffer_size: Some(2 << 20), // 2MB (was 20MB)
            socket_receive_buffer_size: Some(2 << 20), // 2MB (was 20MB)
        }
    }
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_concurrent_connections: Some(100),
            max_concurrent_outstanding_connecting_connections: 10,
            connection_manager_channel_capacity: 128,
            peer_event_broadcast_channel_capacity: 128,
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connection_timeout_ms: 30_000,          // 30 seconds
            inbound_request_timeout_ms: 30_000,     // 30 seconds
            outbound_request_timeout_ms: 30_000,    // 30 seconds
            connectivity_check_interval_ms: 5_000,   // 5 seconds
            connection_backoff_ms: 100,             // 100ms
            max_connection_backoff_ms: 60_000,      // 60 seconds
        }
    }
}

impl AnemoConfig {
    /// Convert to Anemo's Config type
    pub fn to_anemo_config(&self) -> anemo::Config {
        let mut config = anemo::Config::default();

        // QUIC configuration
        let mut quic_config = anemo::QuicConfig::default();
        quic_config.max_concurrent_bidi_streams = Some(self.quic.max_concurrent_bidi_streams);
        quic_config.max_concurrent_uni_streams = Some(self.quic.max_concurrent_uni_streams);
        quic_config.stream_receive_window = Some(self.quic.stream_receive_window);
        quic_config.receive_window = Some(self.quic.receive_window);
        quic_config.send_window = Some(self.quic.send_window);
        quic_config.max_idle_timeout_ms = Some(self.quic.max_idle_timeout_ms);
        quic_config.keep_alive_interval_ms = self.quic.keep_alive_interval_ms;
        quic_config.socket_send_buffer_size = self.quic.socket_send_buffer_size;
        quic_config.socket_receive_buffer_size = self.quic.socket_receive_buffer_size;
        config.quic = Some(quic_config);

        // Connection limits
        config.max_concurrent_connections = self.connection_limits.max_concurrent_connections;
        config.max_concurrent_outstanding_connecting_connections = 
            Some(self.connection_limits.max_concurrent_outstanding_connecting_connections);
        config.connection_manager_channel_capacity = 
            Some(self.connection_limits.connection_manager_channel_capacity);
        config.peer_event_broadcast_channel_capacity = 
            Some(self.connection_limits.peer_event_broadcast_channel_capacity);

        // Timeouts
        config.connectivity_check_interval_ms = Some(self.timeouts.connectivity_check_interval_ms);
        config.connection_backoff_ms = Some(self.timeouts.connection_backoff_ms);
        config.max_connection_backoff_ms = Some(self.timeouts.max_connection_backoff_ms);
        config.inbound_request_timeout_ms = Some(self.timeouts.inbound_request_timeout_ms);
        config.outbound_request_timeout_ms = Some(self.timeouts.outbound_request_timeout_ms);

        config
    }
}

/// Network configuration for Setu
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    /// Anemo-specific configuration
    pub anemo: AnemoConfig,

    /// Maximum number of peers
    pub max_peers: usize,

    /// Enable metrics collection
    pub enable_metrics: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            anemo: AnemoConfig::default(),
            max_peers: 100,
            enable_metrics: true,
        }
    }
}
