//! HTTP transport module
//!
//! Provides HTTP client/server abstractions for Validator ↔ Solver communication.

pub mod types;
pub mod client;
pub mod server;
pub mod middleware;

pub use types::{
    ExecuteTaskRequest, ExecuteTaskResponse,
    ExecuteBatchRequest, ExecuteBatchResponse,
    TeeExecutionResultDto, StateChangeDto, AttestationDto,
    HealthResponse, SolverInfoResponse, EnclaveInfoDto,
};
pub use client::{SolverHttpClient, SolverHttpClientConfig};
pub use server::{SolverHttpHandler, create_router, start_server, HttpServerConfig};
