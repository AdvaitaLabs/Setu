//! Setu Transport Layer
//!
//! This crate provides unified transport abstractions for Validator ↔ Solver communication.
//!
//! # Features
//!
//! - `http` (default): HTTP client/server using axum + reqwest
//! - `websocket` (planned): WebSocket support for bidirectional streaming
//! - `grpc` (planned): gRPC support for high-performance RPC
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    setu-transport                           │
//! ├─────────────────┬─────────────────┬───────────────────────┤
//! │   http/         │   websocket/    │       grpc/           │
//! │   - types.rs    │   (planned)     │       (planned)       │
//! │   - client.rs   │                 │                       │
//! │   - server.rs   │                 │                       │
//! │   - middleware/ │                 │                       │
//! └─────────────────┴─────────────────┴───────────────────────┘
//! ```
//!
//! # Usage
//!
//! ## HTTP Client (Validator side)
//!
//! ```ignore
//! use setu_transport::http::{SolverHttpClient, ExecuteTaskRequest};
//!
//! let client = SolverHttpClient::new("http://solver:8080");
//! let response = client.execute_task(request).await?;
//! ```
//!
//! ## HTTP Server (Solver side)
//!
//! ```ignore
//! use setu_transport::http::{create_router, ExecuteTaskHandler};
//!
//! let router = create_router(handler);
//! axum::serve(listener, router).await?;
//! ```

pub mod error;

#[cfg(feature = "http")]
pub mod http;

pub use error::{TransportError, TransportResult};

#[cfg(feature = "http")]
pub use http::types::{
    AttestationDto, ExecuteTaskRequest, ExecuteTaskResponse, StateChangeDto, TeeExecutionResultDto,
};
