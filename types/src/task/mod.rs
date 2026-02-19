//! Task types for Validator → Solver communication.
//!
//! This module defines the data structures used when Validator sends
//! execution tasks to Solver nodes.
//!
//! ## Module Organization
//!
//! - `solver_task` - SolverTask and related types
//! - `attestation` - TEE attestation types
//! - `gas` - Gas budget and usage types
//!
//! ## Design Goals
//!
//! These are **pure data types** that both Validator and Solver can use
//! without Validator needing to depend on the TEE execution implementation.

mod attestation;
mod gas;
mod solver_task;

// Re-export all types
pub use attestation::{
    Attestation, AttestationData, AttestationError, AttestationResult, AttestationType,
    VerifiedAttestation,
};
pub use gas::{GasBudget, GasUsage};
pub use solver_task::{
    MerkleProof, OperationType, ReadSetEntry, ResolvedInputs, ResolvedObject, SolverTask,
};
