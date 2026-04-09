//! Governance types for on-chain proposals and decisions.
//!
//! These types are shared across setu-governance (logic), setu-validator (integration),
//! and potentially setu-solver (future: voting, evidence).

use crate::subnet::SubnetId;
use serde::{Deserialize, Serialize};

// ========== Governance Payload (Event payload) ==========

/// Governance action payload — carried inside EventPayload::Governance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernancePayload {
    pub proposal_id: [u8; 32],
    pub action: GovernanceAction,
}

/// Discriminant for governance sub-actions within a single EventType::Governance.
///
/// Two semantic categories:
/// - **Async two-phase**: Propose → Agent evaluation → Execute (has pending state)
/// - **Sync direct**: RegisterSystemSubnet (no pending state, no Agent round-trip)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GovernanceAction {
    /// Submit a new proposal (async two-phase)
    Propose(ProposalContent),
    /// Record decision and execute effects (async two-phase)
    Execute(GovernanceDecision),
    /// Register/update a system subnet's Agent endpoint (sync direct)
    RegisterSystemSubnet(SystemSubnetRegistration),
}

/// Registration data for a system subnet's external Agent service.
///
/// Carried inside `GovernanceAction::RegisterSystemSubnet`.
/// Persisted in GOVERNANCE SMT as JSON (non-Coin object).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSubnetRegistration {
    /// Target system SubnetId (must be `is_system() && !is_root()`)
    pub subnet_id: SubnetId,
    /// Agent HTTP endpoint (e.g. "http://agent-server:8090")
    pub agent_endpoint: String,
    /// Optional: override callback address for this subnet
    pub callback_addr: Option<String>,
    /// Optional: proposal timeout override (seconds)
    pub timeout_secs: Option<u64>,
    /// Registrant identity (must be authorized — e.g. genesis validator)
    pub registrant: String,
    /// Ed25519 public key hex (must match genesis validator's public_key)
    #[serde(default)]
    pub public_key: String,
    /// Ed25519 signature hex over `signing_message()` (proves private key ownership)
    #[serde(default)]
    pub signature: String,
}

impl SystemSubnetRegistration {
    /// Deterministic signing message (domain-separated, same pattern as CfVote).
    ///
    /// Format: `SETU_REGISTER_SYSTEM_SUBNET_V1 || subnet_id(32B) || agent_endpoint || registrant`
    pub fn signing_message(&self) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(b"SETU_REGISTER_SYSTEM_SUBNET_V1");
        msg.extend_from_slice(&self.subnet_id.to_bytes());
        msg.extend_from_slice(self.agent_endpoint.as_bytes());
        msg.extend_from_slice(self.registrant.as_bytes());
        msg
    }

    /// Sign the registration with a private key (Ed25519).
    pub fn sign(&mut self, private_key: &[u8]) -> Result<(), String> {
        use ed25519_dalek::{Signer, SigningKey};

        if private_key.len() != 32 {
            return Err(format!("Invalid private key length: expected 32, got {}", private_key.len()));
        }

        let signing_key = SigningKey::from_bytes(
            private_key.try_into().map_err(|_| "Failed to convert key")?
        );

        let message = self.signing_message();
        let sig = signing_key.sign(&message);
        self.signature = hex::encode(sig.to_bytes());
        Ok(())
    }

    /// Verify the Ed25519 signature against the given public key bytes (32 bytes).
    pub fn verify_signature(&self, public_key_bytes: &[u8]) -> bool {
        use ed25519_dalek::{Verifier, VerifyingKey, Signature as Ed25519Signature};

        if self.signature.is_empty() || public_key_bytes.len() != 32 {
            return false;
        }

        let verifying_key = match VerifyingKey::from_bytes(
            public_key_bytes.try_into().unwrap_or(&[0u8; 32])
        ) {
            Ok(key) => key,
            Err(_) => return false,
        };

        let sig_bytes = match hex::decode(&self.signature) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let signature = match Ed25519Signature::from_slice(&sig_bytes) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        let message = self.signing_message();
        verifying_key.verify(&message, &signature).is_ok()
    }
}

/// Content of a governance proposal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalContent {
    /// Proposer address (hex)
    pub proposer: String,
    /// Category of the proposal
    pub proposal_type: ProposalType,
    /// Human-readable title
    pub title: String,
    /// Human-readable description
    pub description: String,
    /// Concrete effect to apply if approved
    pub action: ProposalEffect,
}

/// Category of governance proposal
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalType {
    ParameterChange,
    ValidatorSlash,
    DisputeResolution,
    SubnetPolicy,
    ResourceParamChange,
}

/// Concrete effect of a passed proposal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProposalEffect {
    UpdateParameter { key: String, value: Vec<u8> },
    SlashValidator { validator_id: String, amount: u64 },
    ResolveDispute { dispute_id: [u8; 32], resolution: Vec<u8> },
    UpdateResourceParam(crate::resource::ResourceParamChange),
}

/// Decision from evaluation (source-agnostic: could be Agent subnet, voting, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceDecision {
    pub approved: bool,
    pub reasoning: String,
    pub conditions: Vec<String>,
}

// ========== Governance Proposal (SMT state object) ==========

/// Stored in GOVERNANCE subnet SMT as JSON (non-Coin object → G6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceProposal {
    pub proposal_id: [u8; 32],
    pub content: ProposalContent,
    pub status: ProposalStatus,
    pub decision: Option<GovernanceDecision>,
    /// Timestamp from ExecutionContext (G1: deterministic)
    pub created_at: u64,
    pub decided_at: Option<u64>,
}

/// Lifecycle status of a governance proposal
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalStatus {
    /// Submitted, awaiting evaluation
    Pending,
    /// Approved, effects applied
    Approved,
    /// Rejected
    Rejected,
    /// Execution of effects failed
    Failed,
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_content() -> ProposalContent {
        ProposalContent {
            proposer: "alice".into(),
            proposal_type: ProposalType::ParameterChange,
            title: "Increase block size".into(),
            description: "Set max_block_size to 2MB".into(),
            action: ProposalEffect::UpdateParameter {
                key: "max_block_size".into(),
                value: vec![0, 0, 0, 2],
            },
        }
    }

    fn sample_decision(approved: bool) -> GovernanceDecision {
        GovernanceDecision {
            approved,
            reasoning: "Looks good".into(),
            conditions: vec![],
        }
    }

    #[test]
    fn test_governance_payload_serde() {
        let payload = GovernancePayload {
            proposal_id: [1u8; 32],
            action: GovernanceAction::Propose(sample_content()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let decoded: GovernancePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.proposal_id, [1u8; 32]);
    }

    #[test]
    fn test_governance_proposal_serde() {
        let proposal = GovernanceProposal {
            proposal_id: [2u8; 32],
            content: sample_content(),
            status: ProposalStatus::Pending,
            decision: None,
            created_at: 1000,
            decided_at: None,
        };
        let json = serde_json::to_string(&proposal).unwrap();
        let decoded: GovernanceProposal = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, ProposalStatus::Pending);
        assert_eq!(decoded.proposal_id, [2u8; 32]);
    }

    #[test]
    fn test_proposal_effect_update_resource_param_serde() {
        use crate::resource::{ResourceParamChange, ResourceGovernanceMode};
        let effect = ProposalEffect::UpdateResourceParam(
            ResourceParamChange::SetPowerCostPerEvent(2),
        );
        let json = serde_json::to_string(&effect).unwrap();
        assert!(json.contains("UpdateResourceParam"));
        assert!(json.contains("SetPowerCostPerEvent"));
        let decoded: ProposalEffect = serde_json::from_str(&json).unwrap();
        match decoded {
            ProposalEffect::UpdateResourceParam(ResourceParamChange::SetPowerCostPerEvent(v)) => {
                assert_eq!(v, 2);
            }
            _ => panic!("Expected UpdateResourceParam"),
        }

        // Test mode variant serialization
        let mode_effect = ProposalEffect::UpdateResourceParam(
            ResourceParamChange::SetPowerMode(ResourceGovernanceMode::Enabled),
        );
        let mode_json = serde_json::to_string(&mode_effect).unwrap();
        assert!(mode_json.contains("Enabled"));
        let _: ProposalEffect = serde_json::from_str(&mode_json).unwrap();
    }

    #[test]
    fn test_governance_decision_execute_serde() {
        let payload = GovernancePayload {
            proposal_id: [3u8; 32],
            action: GovernanceAction::Execute(sample_decision(true)),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let decoded: GovernancePayload = serde_json::from_str(&json).unwrap();
        match decoded.action {
            GovernanceAction::Execute(d) => assert!(d.approved),
            _ => panic!("Expected Execute"),
        }
    }

    #[test]
    fn test_system_subnet_registration_serde() {
        let reg = SystemSubnetRegistration {
            subnet_id: crate::SubnetId::new_system(0x20),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: Some("10.0.1.5:8080".to_string()),
            timeout_secs: Some(60),
            registrant: "validator-1".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };
        let json = serde_json::to_string(&reg).unwrap();
        let decoded: SystemSubnetRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.subnet_id, crate::SubnetId::new_system(0x20));
        assert_eq!(decoded.agent_endpoint, "http://oracle:8091");
        assert!(decoded.callback_addr.is_some());
        assert_eq!(decoded.timeout_secs, Some(60));
    }

    #[test]
    fn test_system_subnet_registration_sign_verify() {
        use ed25519_dalek::SigningKey;

        let signing_key = SigningKey::from_bytes(&[42u8; 32]);
        let pk_hex = hex::encode(signing_key.verifying_key().as_bytes());

        let mut reg = SystemSubnetRegistration {
            subnet_id: crate::SubnetId::new_system(0x20),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: None,
            timeout_secs: None,
            registrant: "validator-1".to_string(),
            public_key: pk_hex,
            signature: String::new(),
        };

        // Sign
        reg.sign(&[42u8; 32]).unwrap();
        assert!(!reg.signature.is_empty());

        // Verify with correct key
        let pk_bytes = signing_key.verifying_key().as_bytes().to_vec();
        assert!(reg.verify_signature(&pk_bytes));

        // Verify with wrong key fails
        assert!(!reg.verify_signature(&[0u8; 32]));

        // Tamper with endpoint → verification fails
        reg.agent_endpoint = "http://evil:9999".to_string();
        assert!(!reg.verify_signature(&pk_bytes));
    }

    #[test]
    fn test_governance_action_register_serde() {
        let reg = SystemSubnetRegistration {
            subnet_id: crate::SubnetId::new_system(0x20),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: None,
            timeout_secs: None,
            registrant: "validator-1".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };
        let payload = GovernancePayload {
            proposal_id: crate::SubnetId::new_system(0x20).to_bytes(),
            action: GovernanceAction::RegisterSystemSubnet(reg),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let decoded: GovernancePayload = serde_json::from_str(&json).unwrap();
        match decoded.action {
            GovernanceAction::RegisterSystemSubnet(r) => {
                assert_eq!(r.subnet_id, crate::SubnetId::new_system(0x20));
                assert_eq!(r.agent_endpoint, "http://oracle:8091");
            }
            _ => panic!("Expected RegisterSystemSubnet"),
        }
    }
}
