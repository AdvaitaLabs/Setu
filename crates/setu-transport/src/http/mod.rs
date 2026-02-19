//! HTTP transport module
//!
//! Provides HTTP client/server abstractions for Validator ↔ Solver communication.

pub mod client;
pub mod middleware;
pub mod server;
pub mod types;

pub use client::{SolverHttpClient, SolverHttpClientConfig};
pub use server::{create_router, start_server, HttpServerConfig, SolverHttpHandler};
pub use types::{
    AttestationDto, EnclaveInfoDto, ExecuteTaskRequest, ExecuteTaskResponse, HealthResponse,
    SolverInfoResponse, StateChangeDto, TeeExecutionResultDto,
};
