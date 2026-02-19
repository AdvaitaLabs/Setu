// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! # Setu Protocol
//!
//! This crate provides generic protocol utilities for Setu network communication.
//! It serves as the foundation for any consensus implementation by providing:
//!
//! - Protocol versioning support
//! - Generic message encoding/decoding traits
//! - Common error types
//!
//! ## Design Goals
//!
//! 1. **Consensus-agnostic**: Core modules have no specific message types
//! 2. **Protocol Versioning**: Support backward-compatible protocol evolution
//! 3. **Extensible Codecs**: Pluggable serialization formats
//!
//! ## Module Organization
//!
//! ### Core (Generic Tools - no setu-types dependency)
//! - [`version`] - Protocol version management
//! - [`codec`] - Generic message codec trait and implementations
//! - [`error`] - Common protocol error types
//!
//! ## Usage
//!
//! ```rust,ignore
//! use setu_protocol::{GenericCodec, BincodeCodec, ProtocolVersion};
//!
//! // Check protocol version compatibility
//! let local = ProtocolVersion::CURRENT;
//! let remote = ProtocolVersion::new(1, 0, 0);
//! assert!(local.is_compatible_with(&remote));
//!
//! // Encode/decode with generic codec
//! let codec = BincodeCodec;
//! let bytes = codec.encode(&my_message)?;
//! let decoded: MyMessage = codec.decode(&bytes)?;
//! ```

// =============================================================================
// Core Modules (Generic Protocol Tools - NO setu-types dependency)
// =============================================================================

pub mod codec;
pub mod error;
pub mod rpc;
pub mod version;

// =============================================================================
// Solver HTTP Communication (Validator ↔ Solver)
// =============================================================================

pub mod solver_http;

// Re-export core types
pub use codec::{BincodeCodec, GenericCodec};
pub use error::ProtocolError;
pub use version::ProtocolVersion;

// Re-export RPC serialization types (these are generic wrappers)
pub use rpc::{
    SerializedConsensusFrame, SerializedEvent, SerializedVote, SyncConsensusFramesRequest,
    SyncConsensusFramesResponse, SyncEventsRequest, SyncEventsResponse,
};

// Re-export Solver HTTP types
pub use solver_http::{
    AttestationDto, ExecuteTaskRequest, ExecuteTaskResponse, StateChangeDto, TeeExecutionResultDto,
};
