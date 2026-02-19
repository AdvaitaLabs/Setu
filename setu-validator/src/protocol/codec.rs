// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Message codec for encoding/decoding protocol messages
//!
//! This module provides serialization utilities for `SetuMessage` and other
//! protocol types. It uses bincode for efficient binary serialization.

use bytes::Bytes;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use super::SetuMessage;

/// Errors that can occur during message encoding/decoding
#[derive(Debug, Error)]
pub enum MessageCodecError {
    #[error("Failed to serialize message: {0}")]
    SerializationError(String),

    #[error("Failed to deserialize message: {0}")]
    DeserializationError(String),
}

/// Message codec for serialization/deserialization
///
/// Provides methods for encoding `SetuMessage` to bytes and decoding
/// bytes back to `SetuMessage`. Also provides generic methods for
/// encoding/decoding any serializable type.
pub struct MessageCodec;

impl MessageCodec {
    /// Encode a SetuMessage to bytes
    pub fn encode(message: &SetuMessage) -> Result<Bytes, MessageCodecError> {
        Self::encode_generic(message)
    }

    /// Decode bytes to a SetuMessage
    pub fn decode(bytes: &[u8]) -> Result<SetuMessage, MessageCodecError> {
        Self::decode_generic(bytes)
    }

    /// Encode any serializable type to bytes
    pub fn encode_generic<T: Serialize>(message: &T) -> Result<Bytes, MessageCodecError> {
        let bytes = bincode::serialize(message)
            .map_err(|e| MessageCodecError::SerializationError(e.to_string()))?;
        Ok(Bytes::from(bytes))
    }

    /// Decode bytes to any deserializable type
    pub fn decode_generic<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MessageCodecError> {
        bincode::deserialize(bytes)
            .map_err(|e| MessageCodecError::DeserializationError(e.to_string()))
    }

    /// Encode to a Vec<u8> instead of Bytes (useful for some APIs)
    pub fn encode_to_vec<T: Serialize>(message: &T) -> Result<Vec<u8>, MessageCodecError> {
        bincode::serialize(message)
            .map_err(|e| MessageCodecError::SerializationError(e.to_string()))
    }
}

/// Trait for types that can be encoded/decoded using MessageCodec
///
/// This trait can be implemented by types that need custom serialization
/// logic or want to provide a more ergonomic API.
pub trait Encodable: Serialize + Sized {
    /// Encode this value to bytes
    fn encode(&self) -> Result<Bytes, MessageCodecError> {
        MessageCodec::encode_generic(self)
    }

    /// Encode this value to a Vec<u8>
    fn encode_to_vec(&self) -> Result<Vec<u8>, MessageCodecError> {
        MessageCodec::encode_to_vec(self)
    }
}

/// Trait for types that can be decoded from bytes
pub trait Decodable: DeserializeOwned + Sized {
    /// Decode bytes to this type
    fn decode(bytes: &[u8]) -> Result<Self, MessageCodecError> {
        MessageCodec::decode_generic(bytes)
    }
}

// Implement for SetuMessage
impl Encodable for SetuMessage {}
impl Decodable for SetuMessage {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::sync::SyncEventsRequest;
    use setu_types::{Event, VLCSnapshot};

    #[test]
    fn test_setu_message_roundtrip() {
        let event = Event::genesis("test_creator".to_string(), VLCSnapshot::default());
        let msg = SetuMessage::EventBroadcast {
            event,
            sender_id: "sender1".to_string(),
        };

        let encoded = MessageCodec::encode(&msg).unwrap();
        let decoded = MessageCodec::decode(&encoded).unwrap();

        assert!(matches!(decoded, SetuMessage::EventBroadcast { .. }));
    }

    #[test]
    fn test_generic_roundtrip() {
        let req = SyncEventsRequest {
            start_seq: 100,
            limit: 50,
        };

        let encoded = MessageCodec::encode_generic(&req).unwrap();
        let decoded: SyncEventsRequest = MessageCodec::decode_generic(&encoded).unwrap();

        assert_eq!(decoded.start_seq, 100);
        assert_eq!(decoded.limit, 50);
    }

    #[test]
    fn test_encodable_trait() {
        let msg = SetuMessage::Ping {
            timestamp: 123,
            nonce: 456,
        };

        let encoded = msg.encode().unwrap();
        let decoded = SetuMessage::decode(&encoded).unwrap();

        assert!(matches!(
            decoded,
            SetuMessage::Ping {
                timestamp: 123,
                nonce: 456
            }
        ));
    }
}
