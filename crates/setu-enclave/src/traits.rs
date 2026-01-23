//! EnclaveRuntime trait and related types.
//!
//! This module defines the core interface that all enclave implementations must satisfy.

use async_trait::async_trait;
use setu_types::task::Attestation;
use crate::stf::{StfInput, StfOutput, StfResult};

/// Configuration for enclave initialization
#[derive(Debug, Clone)]
pub struct EnclaveConfig {
    /// Unique identifier for this enclave instance
    pub enclave_id: String,
    /// Solver ID that owns this enclave
    pub solver_id: String,
    /// Maximum execution time in milliseconds
    pub max_execution_time_ms: u64,
    /// Maximum memory usage in bytes
    pub max_memory_bytes: u64,
    /// Whether to enable detailed logging (may leak info in production)
    pub enable_debug_logging: bool,
}

impl Default for EnclaveConfig {
    fn default() -> Self {
        Self {
            enclave_id: uuid::Uuid::new_v4().to_string(),
            solver_id: "default-solver".to_string(),
            max_execution_time_ms: 30_000, // 30 seconds
            max_memory_bytes: 1024 * 1024 * 1024, // 1 GB
            enable_debug_logging: false,
        }
    }
}

impl EnclaveConfig {
    pub fn new(solver_id: String) -> Self {
        Self {
            solver_id,
            ..Default::default()
        }
    }
    
    pub fn with_solver_id(mut self, solver_id: String) -> Self {
        self.solver_id = solver_id;
        self
    }
}

/// Information about the enclave
#[derive(Debug, Clone)]
pub struct EnclaveInfo {
    /// Unique enclave instance ID
    pub enclave_id: String,
    /// TEE platform type
    pub platform: EnclavePlatform,
    /// Enclave measurement (PCR0 for Nitro, MRENCLAVE for SGX)
    pub measurement: [u8; 32],
    /// Software version running inside enclave
    pub version: String,
    /// Whether this is a simulated enclave
    pub is_simulated: bool,
}

/// Supported TEE platforms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnclavePlatform {
    /// Simulated enclave (no real TEE)
    Mock,
    /// AWS Nitro Enclaves
    AwsNitro,
    /// Intel SGX (future)
    IntelSgx,
    /// AMD SEV (future)
    AmdSev,
}

impl std::fmt::Display for EnclavePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnclavePlatform::Mock => write!(f, "Mock"),
            EnclavePlatform::AwsNitro => write!(f, "AWS-Nitro"),
            EnclavePlatform::IntelSgx => write!(f, "Intel-SGX"),
            EnclavePlatform::AmdSev => write!(f, "AMD-SEV"),
        }
    }
}

/// Core trait for TEE enclave implementations.
///
/// This trait defines the interface for executing Stateless Transition Functions
/// inside a TEE and generating/verifying attestations.
#[async_trait]
pub trait EnclaveRuntime: Send + Sync {
    /// Execute the Stateless Transition Function.
    ///
    /// This is the core computation that:
    /// 1. Takes pre-state root and events as input
    /// 2. Executes state transitions
    /// 3. Produces post-state root, state diff, and attestation
    ///
    /// The execution is:
    /// - **Stateless**: No persistent state inside enclave
    /// - **Deterministic**: Same inputs produce same outputs
    /// - **Attested**: Output is cryptographically signed
    async fn execute_stf(&self, input: StfInput) -> StfResult<StfOutput>;
    
    /// Generate an attestation for given user data.
    ///
    /// The attestation proves that:
    /// 1. The computation ran inside a valid TEE
    /// 2. The enclave code has the expected measurement
    /// 3. The output data was produced by this enclave
    async fn generate_attestation(&self, user_data: [u8; 32]) -> StfResult<Attestation>;
    
    /// Verify an attestation (typically called by validators).
    ///
    /// Checks:
    /// 1. Attestation signature is valid
    /// 2. Enclave measurement matches expected value
    /// 3. Attestation is not expired
    async fn verify_attestation(&self, attestation: &Attestation) -> StfResult<bool>;
    
    /// Get information about this enclave.
    fn info(&self) -> EnclaveInfo;
    
    /// Get the enclave's measurement (used for verification).
    fn measurement(&self) -> [u8; 32];
    
    /// Check if this is a simulated/mock enclave.
    fn is_simulated(&self) -> bool;
}

/// Extension trait for batch operations
#[async_trait]
pub trait EnclaveRuntimeExt: EnclaveRuntime {
    /// Execute multiple STF inputs in sequence.
    ///
    /// This is useful for processing batches of events efficiently.
    async fn execute_batch(&self, inputs: Vec<StfInput>) -> StfResult<Vec<StfOutput>> {
        let mut outputs = Vec::with_capacity(inputs.len());
        for input in inputs {
            outputs.push(self.execute_stf(input).await?);
        }
        Ok(outputs)
    }
}

// Blanket implementation for all EnclaveRuntime implementations
impl<T: EnclaveRuntime> EnclaveRuntimeExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_enclave_config_default() {
        let config = EnclaveConfig::default();
        assert_eq!(config.max_execution_time_ms, 30_000);
        assert_eq!(config.max_memory_bytes, 1024 * 1024 * 1024);
    }
    
    #[test]
    fn test_enclave_platform_display() {
        assert_eq!(format!("{}", EnclavePlatform::Mock), "Mock");
        assert_eq!(format!("{}", EnclavePlatform::AwsNitro), "AWS-Nitro");
    }
}
