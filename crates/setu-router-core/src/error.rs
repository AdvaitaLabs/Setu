//! Error types for the router module

use thiserror::Error;

/// Router error types
#[derive(Debug, Error)]
pub enum RouterError {
    /// No solver is available for routing
    #[error("No solver available for routing")]
    NoSolverAvailable,

    /// Solver not found by ID
    #[error("Solver not found: {0}")]
    SolverNotFound(String),

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}
