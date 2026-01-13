//! TEE Attestation types and utilities.
//!
//! Attestations provide cryptographic proof that computation was performed
//! inside a trusted execution environment with specific measurements.
//!
//! ## Attestation Types
//!
//! - **Mock**: Simulated attestation for development/testing
//! - **AWS Nitro**: Real attestation from AWS Nitro Enclaves
//! - **Intel SGX**: (Future) Intel SGX attestation
//! - **AMD SEV**: (Future) AMD SEV attestation
//!
//! ## Verification Flow
//!
//! ```text
//! Validator receives StfOutput with Attestation
//!        │
//!        ▼
//! ┌──────────────────────────────────────────┐
//! │  1. Check attestation_type               │
//! │  2. Verify signature/document            │
//! │  3. Extract enclave measurement (PCR)    │
//! │  4. Check measurement against allowlist  │
//! │  5. Verify user_data matches output hash │
//! └──────────────────────────────────────────┘
//!        │
//!        ▼
//!   Accept or reject
//! ```

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
    
    #[error("Attestation expired")]
    Expired,
    
    #[error("Unsupported attestation type: {0}")]
    UnsupportedType(String),
    
    #[error("Document parsing failed: {0}")]
    ParseError(String),
}

pub type AttestationResult<T> = Result<T, AttestationError>;

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
    
    /// User data (typically hash of StfOutput)
    pub user_data: [u8; 32],
    
    /// Raw attestation document (format depends on type)
    pub document: Vec<u8>,
    
    /// Timestamp when attestation was generated (Unix epoch seconds)
    pub timestamp: u64,
    
    /// Optional: solver ID that generated this attestation
    pub solver_id: Option<String>,
}

impl Attestation {
    /// Create a new attestation
    pub fn new(
        attestation_type: AttestationType,
        measurement: [u8; 32],
        user_data: [u8; 32],
        document: Vec<u8>,
    ) -> Self {
        Self {
            attestation_type,
            measurement,
            user_data,
            document,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            solver_id: None,
        }
    }
    
    /// Create a mock attestation for testing
    pub fn mock(user_data: [u8; 32]) -> Self {
        use sha2::{Sha256, Digest};
        
        // Generate a mock measurement
        let mut hasher = Sha256::new();
        hasher.update(b"mock_enclave_v1");
        let measurement: [u8; 32] = hasher.finalize().into();
        
        // Mock document is just a placeholder
        let document = b"MOCK_ATTESTATION_DOCUMENT".to_vec();
        
        Self::new(AttestationType::Mock, measurement, user_data, document)
    }
    
    /// Set solver ID
    pub fn with_solver_id(mut self, solver_id: String) -> Self {
        self.solver_id = Some(solver_id);
        self
    }
    
    /// Get measurement as hex string
    pub fn measurement_hex(&self) -> String {
        hex::encode(self.measurement)
    }
    
    /// Get user data as hex string
    pub fn user_data_hex(&self) -> String {
        hex::encode(self.user_data)
    }
    
    /// Check if this is a mock attestation
    pub fn is_mock(&self) -> bool {
        self.attestation_type == AttestationType::Mock
    }
    
    /// Compute hash of this attestation for signing/verification
    pub fn hash(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        
        let mut hasher = Sha256::new();
        hasher.update(&[self.attestation_type as u8]);
        hasher.update(self.measurement);
        hasher.update(self.user_data);
        hasher.update(&self.timestamp.to_le_bytes());
        
        hasher.finalize().into()
    }
}

/// AWS Nitro attestation document (parsed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NitroAttestationDocument {
    /// Module ID (enclave image ID)
    pub module_id: String,
    
    /// PCR values (Platform Configuration Registers)
    /// PCR0: Enclave image hash
    /// PCR1: Linux kernel and boot ramdisk hash
    /// PCR2: Application hash
    pub pcrs: NitroPcrs,
    
    /// Certificate chain
    pub certificate: Vec<u8>,
    
    /// CA bundle for verification
    pub cabundle: Vec<Vec<u8>>,
    
    /// Optional public key
    pub public_key: Option<Vec<u8>>,
    
    /// User-provided data (nonce)
    pub user_data: Option<Vec<u8>>,
    
    /// Timestamp
    pub timestamp: u64,
}

/// Nitro PCR values
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NitroPcrs {
    pub pcr0: Option<Vec<u8>>,
    pub pcr1: Option<Vec<u8>>,
    pub pcr2: Option<Vec<u8>>,
    pub pcr3: Option<Vec<u8>>,
    pub pcr4: Option<Vec<u8>>,
    pub pcr8: Option<Vec<u8>>,
}

impl NitroPcrs {
    /// Get PCR0 as 32-byte array (enclave measurement)
    pub fn get_measurement(&self) -> Option<[u8; 32]> {
        self.pcr0.as_ref().and_then(|pcr| {
            if pcr.len() >= 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&pcr[..32]);
                Some(arr)
            } else {
                None
            }
        })
    }
}

/// Attestation verifier trait
pub trait AttestationVerifier: Send + Sync {
    /// Verify an attestation document
    fn verify(&self, attestation: &Attestation) -> AttestationResult<VerifiedAttestation>;
    
    /// Check if a measurement is in the allowlist
    fn is_measurement_allowed(&self, measurement: &[u8; 32]) -> bool;
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

/// Simple allowlist-based verifier
pub struct AllowlistVerifier {
    /// Allowed measurements
    allowed_measurements: std::collections::HashSet<[u8; 32]>,
    /// Whether to allow mock attestations
    allow_mock: bool,
}

impl AllowlistVerifier {
    pub fn new(allow_mock: bool) -> Self {
        Self {
            allowed_measurements: std::collections::HashSet::new(),
            allow_mock,
        }
    }
    
    pub fn add_measurement(&mut self, measurement: [u8; 32]) {
        self.allowed_measurements.insert(measurement);
    }
    
    /// Create a verifier that allows all mock attestations (for testing)
    pub fn allow_all_mock() -> Self {
        Self::new(true)
    }
}

impl AttestationVerifier for AllowlistVerifier {
    fn verify(&self, attestation: &Attestation) -> AttestationResult<VerifiedAttestation> {
        // Handle mock attestations
        if attestation.is_mock() {
            if self.allow_mock {
                return Ok(VerifiedAttestation {
                    measurement: attestation.measurement,
                    user_data: attestation.user_data,
                    attestation_type: attestation.attestation_type,
                    verified_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                });
            } else {
                return Err(AttestationError::UnsupportedType("mock".to_string()));
            }
        }
        
        // Check measurement allowlist
        if !self.is_measurement_allowed(&attestation.measurement) {
            return Err(AttestationError::UnknownMeasurement {
                measurement: attestation.measurement_hex(),
            });
        }
        
        // For non-mock attestations, we'd need to verify the document
        // This is a placeholder - real implementation would parse and verify
        match attestation.attestation_type {
            AttestationType::AwsNitro => {
                // TODO: Implement Nitro document verification
                Ok(VerifiedAttestation {
                    measurement: attestation.measurement,
                    user_data: attestation.user_data,
                    attestation_type: attestation.attestation_type,
                    verified_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                })
            }
            _ => Err(AttestationError::UnsupportedType(
                attestation.attestation_type.to_string(),
            )),
        }
    }
    
    fn is_measurement_allowed(&self, measurement: &[u8; 32]) -> bool {
        self.allowed_measurements.contains(measurement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_mock_attestation() {
        let user_data = [42u8; 32];
        let attestation = Attestation::mock(user_data);
        
        assert!(attestation.is_mock());
        assert_eq!(attestation.user_data, user_data);
        assert!(!attestation.measurement_hex().is_empty());
    }
    
    #[test]
    fn test_attestation_hash() {
        let user_data = [1u8; 32];
        let attestation = Attestation::mock(user_data);
        
        let hash1 = attestation.hash();
        let hash2 = attestation.hash();
        
        assert_eq!(hash1, hash2);
    }
    
    #[test]
    fn test_allowlist_verifier_mock() {
        let verifier = AllowlistVerifier::allow_all_mock();
        let attestation = Attestation::mock([0u8; 32]);
        
        let result = verifier.verify(&attestation);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_allowlist_verifier_rejects_mock() {
        let verifier = AllowlistVerifier::new(false);
        let attestation = Attestation::mock([0u8; 32]);
        
        let result = verifier.verify(&attestation);
        assert!(result.is_err());
    }
}
