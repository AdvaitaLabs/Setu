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

mod solver_task;
mod attestation;
mod gas;

// Re-export all types
pub use solver_task::{
    SolverTask, ResolvedInputs, OperationType, ResolvedObject,
    ReadSetEntry, MerkleProof, ResolvedDynamicField,
};
pub use attestation::{
    Attestation, AttestationType, AttestationData,
    AttestationError, AttestationResult, VerifiedAttestation,
};
pub use gas::{GasBudget, GasUsage};
