// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Protocol error types
//!
//! This module defines common error types used across the protocol layer.
//! These errors are generic and can be used by any consensus implementation.

use std::time::Duration;
use thiserror::Error;

/// Protocol-level errors
///
/// These errors represent failures in protocol operations such as
/// encoding, decoding, network communication, and version negotiation.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Failed to encode a message
    #[error("Encode error: {0}")]
    Encode(String),

    /// Failed to decode a message
    #[error("Decode error: {0}")]
    Decode(String),

    /// Connection failed
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Operation timed out
    #[error("Timeout after {0:?}")]
    Timeout(Duration),

    /// Route not found for message
    #[error("Route not found: {0}")]
    RouteNotFound(String),

    /// Peer is unavailable
    #[error("Peer unavailable: {0}")]
    PeerUnavailable(String),

    /// Protocol version mismatch
    #[error("Version mismatch: local={local}, remote={remote}")]
    VersionMismatch { local: String, remote: String },

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl ProtocolError {
    /// Create a new encode error
    pub fn encode(msg: impl Into<String>) -> Self {
        Self::Encode(msg.into())
    }

    /// Create a new decode error
    pub fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    /// Create a new connection error
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::Connection(msg.into())
    }

    /// Create a new timeout error
    pub fn timeout(duration: Duration) -> Self {
        Self::Timeout(duration)
    }

    /// Create a new internal error
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// Check if this is a transient error that might succeed on retry
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Timeout(_) | Self::Connection(_) | Self::PeerUnavailable(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ProtocolError::encode("test error");
        assert_eq!(err.to_string(), "Encode error: test error");
    }

    #[test]
    fn test_is_transient() {
        assert!(ProtocolError::timeout(Duration::from_secs(5)).is_transient());
        assert!(ProtocolError::connection("lost").is_transient());
        assert!(!ProtocolError::encode("invalid").is_transient());
    }
}
