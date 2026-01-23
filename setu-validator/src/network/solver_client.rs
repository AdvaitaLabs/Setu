//! Solver HTTP Client types for Validator → Solver communication
//!
//! Re-exports types from setu-transport for Validator-side usage.
//! The actual client implementation is in setu_transport::http::SolverHttpClient.

// Re-export all HTTP types from setu-transport
pub use setu_transport::http::{
    ExecuteTaskRequest, ExecuteTaskResponse,
    TeeExecutionResultDto, StateChangeDto, AttestationDto,
    SolverHttpClient, SolverHttpClientConfig,
};
