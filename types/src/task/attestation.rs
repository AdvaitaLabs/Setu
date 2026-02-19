//! TEE Attestation types.
//!
//! Attestations provide cryptographic proof that computation was performed
//! inside a trusted execution environment with specific measurements.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Attestation errors
#[derive(Debug, Error)]
pub enum AttestationError {
    #[error("Signature verification failed")]
    InvalidSignature,

    #[error("Certificate chain validation failed: {0}")]
    InvalidCertificateChain(String),

    #[error("Enclave measurement not in allowlist: {measurement}")]
    UnknownMeasurement { measurement: String },

    #[error("User data mismatch: expected {expected}, got {actual}")]
    UserDataMismatch { expected: String, actual: String },

    #[error("Task ID mismatch in attestation")]
    TaskIdMismatch,

    #[error("Input hash mismatch in attestation")]
    InputHashMismatch,

    #[error("Pre-state root mismatch in attestation")]
    PreStateRootMismatch,

    #[error("Attestation expired")]
    Expired,

    #[error("Unsupported attestation type: {0}")]
    UnsupportedType(String),

    #[error("Document parsing failed: {0}")]
    ParseError(String),
}

pub type AttestationResult<T> = Result<T, AttestationError>;

/// Data bound in attestation for task identity verification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttestationData {
    /// Unique task identifier (for replay protection)
    pub task_id: [u8; 32],

    /// Hash of all inputs (for tampering protection)
    pub input_hash: [u8; 32],

    /// State root before execution (for consistency verification)
    pub pre_state_root: [u8; 32],

    /// State root after execution (result commitment)
    pub post_state_root: [u8; 32],
}

impl AttestationData {
    pub fn new(
        task_id: [u8; 32],
        input_hash: [u8; 32],
        pre_state_root: [u8; 32],
        post_state_root: [u8; 32],
    ) -> Self {
        Self {
            task_id,
            input_hash,
            pre_state_root,
            post_state_root,
        }
    }

    /// Compute the user_data hash from this attestation data
    pub fn to_user_data(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(&self.task_id);
        hasher.update(&self.input_hash);
        hasher.update(&self.pre_state_root);
        hasher.update(&self.post_state_root);

        hasher.finalize().into()
    }

    /// Verify that this AttestationData matches the given user_data
    pub fn verify(&self, user_data: &[u8; 32]) -> bool {
        self.to_user_data() == *user_data
    }
}

/// Attestation type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationType {
    /// Simulated attestation for development/testing
    Mock,
    /// AWS Nitro Enclave attestation
    AwsNitro,
    /// Intel SGX attestation (future)
    IntelSgx,
    /// AMD SEV attestation (future)
    AmdSev,
}

impl std::fmt::Display for AttestationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttestationType::Mock => write!(f, "mock"),
            AttestationType::AwsNitro => write!(f, "aws_nitro"),
            AttestationType::IntelSgx => write!(f, "intel_sgx"),
            AttestationType::AmdSev => write!(f, "amd_sev"),
        }
    }
}

/// TEE attestation containing proof of enclave execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Type of attestation
    pub attestation_type: AttestationType,

    /// Enclave measurement (PCR0 for Nitro, MRENCLAVE for SGX)
    pub measurement: [u8; 32],

    /// User data (hash of AttestationData)
    pub user_data: [u8; 32],

    /// Structured attestation data (task binding information)
    pub attestation_data: Option<AttestationData>,

    /// Raw attestation document (format depends on type)
    pub document: Vec<u8>,

    /// Timestamp when attestation was generated (Unix epoch seconds)
    pub timestamp: u64,

    /// Optional: solver ID that generated this attestation
    pub solver_id: Option<String>,

    // === Fields for easy DTO conversion ===
    /// Enclave ID (for DTO compatibility)
    pub enclave_id: String,
    /// Task ID binding (for DTO compatibility)
    pub task_id_binding: [u8; 32],
    /// Input hash (for DTO compatibility)
    pub input_hash: [u8; 32],
    /// Pre-state root (for DTO compatibility)
    pub pre_state_root: [u8; 32],
    /// Post-state root (for DTO compatibility)
    pub post_state_root: [u8; 32],
    /// Signature (for DTO compatibility)
    pub signature: Vec<u8>,
}

impl Attestation {
    /// Create a new attestation
    pub fn new(
        attestation_type: AttestationType,
        measurement: [u8; 32],
        user_data: [u8; 32],
        document: Vec<u8>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            attestation_type,
            measurement,
            user_data,
            attestation_data: None,
            document: document.clone(),
            timestamp,
            solver_id: None,
            enclave_id: format!("{:?}", attestation_type),
            task_id_binding: [0u8; 32],
            input_hash: [0u8; 32],
            pre_state_root: [0u8; 32],
            post_state_root: [0u8; 32],
            signature: document,
        }
    }

    /// Create from AttestationData (recommended)
    pub fn from_data(
        attestation_type: AttestationType,
        measurement: [u8; 32],
        data: AttestationData,
        document: Vec<u8>,
    ) -> Self {
        let user_data = data.to_user_data();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            attestation_type,
            measurement,
            user_data,
            enclave_id: format!("{:?}", attestation_type),
            task_id_binding: data.task_id,
            input_hash: data.input_hash,
            pre_state_root: data.pre_state_root,
            post_state_root: data.post_state_root,
            attestation_data: Some(data),
            signature: document.clone(),
            document,
            timestamp,
            solver_id: None,
        }
    }

    /// Create a mock attestation for testing
    pub fn mock(user_data: [u8; 32]) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(b"mock_enclave_v1");
        let measurement: [u8; 32] = hasher.finalize().into();

        let document = b"MOCK_ATTESTATION_DOCUMENT".to_vec();

        Self::new(AttestationType::Mock, measurement, user_data, document)
    }

    /// Create a mock attestation with AttestationData binding
    pub fn mock_with_data(data: AttestationData) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(b"mock_enclave_v1");
        let measurement: [u8; 32] = hasher.finalize().into();

        let document = b"MOCK_ATTESTATION_DOCUMENT".to_vec();

        Self::from_data(AttestationType::Mock, measurement, data, document)
    }

    /// Set the solver ID (builder pattern)
    pub fn with_solver_id(mut self, solver_id: String) -> Self {
        self.solver_id = Some(solver_id);
        self
    }

    /// Check if this is a mock attestation
    pub fn is_mock(&self) -> bool {
        self.attestation_type == AttestationType::Mock
    }

    /// Verify that the attestation data matches the user_data hash
    pub fn verify_data(&self) -> bool {
        match &self.attestation_data {
            Some(data) => data.verify(&self.user_data),
            None => false,
        }
    }

    /// Get measurement as hex string
    pub fn measurement_hex(&self) -> String {
        hex::encode(self.measurement)
    }

    /// Get user data as hex string  
    pub fn user_data_hex(&self) -> String {
        hex::encode(self.user_data)
    }

    /// Compute hash of this attestation for signing/verification
    pub fn hash(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(&[self.attestation_type as u8]);
        hasher.update(self.measurement);
        hasher.update(self.user_data);
        hasher.update(&self.timestamp.to_le_bytes());

        hasher.finalize().into()
    }

    /// Get the task_id if attestation_data is present
    pub fn task_id(&self) -> Option<&[u8; 32]> {
        self.attestation_data.as_ref().map(|d| &d.task_id)
    }

    /// Set attestation data (for binding after creation)
    pub fn with_attestation_data(mut self, data: AttestationData) -> Self {
        self.attestation_data = Some(data);
        self
    }
}

/// Result of successful attestation verification
#[derive(Debug, Clone)]
pub struct VerifiedAttestation {
    /// Verified enclave measurement
    pub measurement: [u8; 32],
    /// Verified user data
    pub user_data: [u8; 32],
    /// Attestation type
    pub attestation_type: AttestationType,
    /// Verification timestamp
    pub verified_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attestation_data_hash() {
        let data = AttestationData::new([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);

        let user_data = data.to_user_data();
        assert!(data.verify(&user_data));
    }

    #[test]
    fn test_mock_attestation() {
        let att = Attestation::mock([0u8; 32]);
        assert!(att.is_mock());
    }
}
