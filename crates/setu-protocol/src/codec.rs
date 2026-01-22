// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Generic message codec trait for protocol encoding/decoding
//!
//! This module provides a generic interface for message serialization
//! that can be used across different consensus implementations.

use bytes::Bytes;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::ProtocolError;

/// Generic message codec trait
///
/// This trait defines the interface for encoding and decoding protocol
/// messages. It allows different implementations (bincode, protobuf, etc.)
/// while keeping the API consistent.
///
/// # Example
///
/// ```rust,ignore
/// use setu_protocol::{GenericCodec, BincodeCodec};
///
/// let codec = BincodeCodec;
/// let message = MyMessage { data: 42 };
/// let bytes = codec.encode(&message)?;
/// let decoded: MyMessage = codec.decode(&bytes)?;
/// ```
pub trait GenericCodec: Send + Sync {
    /// Encode a message to bytes
    fn encode<T: Serialize>(&self, msg: &T) -> Result<Bytes, ProtocolError>;
    
    /// Decode bytes to a message
    fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, ProtocolError>;
    
    /// Encode a message to a Vec<u8>
    fn encode_to_vec<T: Serialize>(&self, msg: &T) -> Result<Vec<u8>, ProtocolError>;
}

/// Default bincode-based codec implementation
///
/// This is the default codec used in Setu for its compact binary format
/// and good performance characteristics.
#[derive(Debug, Clone, Copy, Default)]
pub struct BincodeCodec;

impl GenericCodec for BincodeCodec {
    fn encode<T: Serialize>(&self, msg: &T) -> Result<Bytes, ProtocolError> {
        let bytes = bincode::serialize(msg)
            .map_err(|e| ProtocolError::Encode(e.to_string()))?;
        Ok(Bytes::from(bytes))
    }
    
    fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, ProtocolError> {
        bincode::deserialize(bytes)
            .map_err(|e| ProtocolError::Decode(e.to_string()))
    }
    
    fn encode_to_vec<T: Serialize>(&self, msg: &T) -> Result<Vec<u8>, ProtocolError> {
        bincode::serialize(msg)
            .map_err(|e| ProtocolError::Encode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestMessage {
        value: u64,
        text: String,
    }

    #[test]
    fn test_bincode_codec_roundtrip() {
        let codec = BincodeCodec;
        let msg = TestMessage {
            value: 42,
            text: "hello".to_string(),
        };

        let encoded = codec.encode(&msg).unwrap();
        let decoded: TestMessage = codec.decode(&encoded).unwrap();

        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_encode_to_vec() {
        let codec = BincodeCodec;
        let msg = TestMessage {
            value: 123,
            text: "test".to_string(),
        };

        let vec = codec.encode_to_vec(&msg).unwrap();
        let decoded: TestMessage = codec.decode(&vec).unwrap();

        assert_eq!(msg, decoded);
    }
}
