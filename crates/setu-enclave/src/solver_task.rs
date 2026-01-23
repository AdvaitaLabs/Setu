//! Solver Task types for Validator → Solver communication.
//!
//! **DEPRECATED**: This module re-exports types from `setu_types::task`.
//! Please import directly from `setu_types::task` for new code.
//!
//! This module is kept for backward compatibility with existing code that
//! imports from `setu_enclave`.

// Re-export all types from setu_types::task for backward compatibility
pub use setu_types::task::{
    SolverTask, ResolvedInputs, OperationType, ResolvedObject,
    ReadSetEntry, MerkleProof,
    GasBudget, GasUsage,
};
