//! TEE Attestation verification utilities.
//!
//! **DEPRECATED**: Core attestation types are now in `setu_types::task`.
//! This module only contains enclave-specific verification utilities.
//!
//! ## Re-exported Types
//!
//! The following types are re-exported from `setu_types::task`:
//! - `Attestation`, `AttestationType`, `AttestationData`
//! - `AttestationError`, `AttestationResult`, `VerifiedAttestation`
//!
//! ## Enclave-Specific Utilities (defined here)
//!
//! - `AttestationVerifier` trait - Interface for attestation verification
//! - `AllowlistVerifier` - Simple allowlist-based verifier implementation
//! - `NitroAttestationDocument`, `NitroPcrs` - AWS Nitro parsing types

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// Re-export core types from setu_types::task for backward compatibility
pub use setu_types::task::{
    Attestation, AttestationType, AttestationData,
    AttestationError, AttestationResult, VerifiedAttestation,
};

// ============================================
// Nitro-specific Types (enclave-specific)
// ============================================

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

// ============================================
// Attestation Verifier Trait (enclave-specific)
// ============================================

/// Attestation verifier trait
///
/// This trait defines the interface for verifying TEE attestations.
/// Different implementations can support different verification strategies.
pub trait AttestationVerifier: Send + Sync {
    /// Verify an attestation document
    fn verify(&self, attestation: &Attestation) -> AttestationResult<VerifiedAttestation>;
    
    /// Check if a measurement is in the allowlist
    fn is_measurement_allowed(&self, measurement: &[u8; 32]) -> bool;
}

// ============================================
// Allowlist Verifier Implementation
// ============================================

/// Simple allowlist-based verifier
///
/// This verifier maintains a set of allowed enclave measurements and
/// optionally allows mock attestations for testing.
pub struct AllowlistVerifier {
    /// Allowed measurements
    allowed_measurements: HashSet<[u8; 32]>,
    /// Whether to allow mock attestations
    allow_mock: bool,
}

impl AllowlistVerifier {
    /// Create a new AllowlistVerifier
    pub fn new(allow_mock: bool) -> Self {
        Self {
            allowed_measurements: HashSet::new(),
            allow_mock,
        }
    }
    
    /// Add a measurement to the allowlist
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
