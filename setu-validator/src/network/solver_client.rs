//! Solver HTTP Client types for Validator → Solver communication
//!
//! Re-exports types from setu-transport for Validator-side usage.
//! The actual client implementation is in setu_transport::http::SolverHttpClient.

// Re-export only the types actually used by tee_executor
pub use setu_transport::http::{
    ExecuteTaskRequest, ExecuteTaskResponse,
};
