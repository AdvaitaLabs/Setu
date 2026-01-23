//! # Setu Enclave
//!
//! TEE (Trusted Execution Environment) abstraction layer for Setu.
//!
//! This crate provides a unified interface for executing Stateless Transition Functions (STF)
//! inside a TEE, with support for multiple backends:
//!
//! - **MockEnclave**: Simulated TEE for development, testing, and MVP
//! - **NitroEnclave**: AWS Nitro Enclaves for production deployment
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          Setu Enclave                                   │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │                     EnclaveRuntime Trait                         │   │
//! │  │  • execute_stf()     - Run Stateless Transition Function        │   │
//! │  │  • generate_attestation() - Create TEE attestation              │   │
//! │  │  • verify_attestation()   - Verify attestation (for validators) │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                              │                                         │
//! │              ┌───────────────┼───────────────┐                         │
//! │              ▼                               ▼                         │
//! │  ┌───────────────────────┐      ┌───────────────────────┐             │
//! │  │     MockEnclave       │      │    NitroEnclave       │             │
//! │  │   (feature: mock)     │      │   (feature: nitro)    │             │
//! │  │                       │      │                       │             │
//! │  │  • No real TEE        │      │  • AWS Nitro TEE      │             │
//! │  │  • Simulated proofs   │      │  • Real attestation   │             │
//! │  │  • Fast execution     │      │  • PCR measurements   │             │
//! │  │  • For dev/test       │      │  • For production     │             │
//! │  └───────────────────────┘      └───────────────────────┘             │
//! │                                                                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Stateless Transition Function (STF)
//!
//! The STF is the core computation that runs inside the enclave:
//!
//! ```text
//! STF: (pre_state_root, events) → (post_state_root, state_diff, attestation)
//! ```
//!
//! Key properties:
//! - **Stateless**: No persistent state inside enclave
//! - **Deterministic**: Same inputs always produce same outputs
//! - **Verifiable**: Outputs are cryptographically attested
//!
//! ## Usage
//!
//! ```rust,ignore
//! use setu_enclave::{EnclaveRuntime, MockEnclave, StfInput, StfOutput};
//!
//! // Create enclave (mock for development)
//! let enclave = MockEnclave::new("solver-1");
//!
//! // Prepare STF input
//! let input = StfInput {
//!     pre_state_root: [0u8; 32],
//!     events: vec![event1, event2],
//!     read_set: vec![...],
//! };
//!
//! // Execute STF
//! let output = enclave.execute_stf(input).await?;
//!
//! // Output contains:
//! // - post_state_root
//! // - state_diff (Vec<StateChange>)
//! // - attestation (for validator verification)
//! ```

pub mod attestation;
pub mod solver_task;
pub mod stf;
pub mod traits;

#[cfg(feature = "mock")]
pub mod mock;

#[cfg(feature = "nitro")]
pub mod nitro;

// Re-export enclave-specific verification utilities
pub use attestation::{
    AttestationVerifier, AllowlistVerifier,
    NitroAttestationDocument, NitroPcrs,
};
pub use stf::{
    StfInput, StfOutput, StfError, StfResult,
    WriteSetEntry, StateDiff, ExecutionStats,
    Hash, TaskId,
};
pub use traits::{EnclaveRuntime, EnclaveConfig, EnclaveInfo, EnclavePlatform};

// === Re-export from setu-types (canonical source) ===
// This allows setu-solver to still import from setu-enclave for backward compatibility
pub use setu_types::task::{
    // Attestation types
    Attestation, AttestationType, AttestationData,
    AttestationError, AttestationResult, VerifiedAttestation,
    // Solver task types
    SolverTask, ResolvedInputs, OperationType, ResolvedObject,
    ReadSetEntry, MerkleProof,
    // Gas types
    GasBudget, GasUsage,
};

// Re-export implementations based on features
#[cfg(feature = "mock")]
pub use mock::MockEnclave;

#[cfg(feature = "nitro")]
pub use nitro::NitroEnclave;

/// Create the default enclave based on enabled features
#[cfg(feature = "mock")]
pub fn create_default_enclave(solver_id: &str) -> MockEnclave {
    MockEnclave::default_with_solver_id(solver_id.to_string())
}

#[cfg(all(feature = "nitro", not(feature = "mock")))]
pub fn create_default_enclave(solver_id: &str) -> NitroEnclave {
    use nitro::NitroConfig;
    let mut config = NitroConfig::default();
    config.base.solver_id = solver_id.to_string();
    NitroEnclave::new(config)
}
