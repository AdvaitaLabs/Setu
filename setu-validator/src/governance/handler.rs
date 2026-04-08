//! GovernanceHandler — API endpoints for governance operations.
//!
//! Endpoints:
//! - POST /governance/propose  — submit a governance proposal
//! - POST /governance/callback — receive Agent subnet decision
//! - GET  /governance/status/:proposal_id — query proposal status

use super::executor::{GovernanceExecutor, GovernanceExecutorError};
use super::service::{GovernanceService, GovernanceServiceError, PendingProposal};
use setu_types::governance::{GovernanceDecision, ProposalContent, SystemSubnetRegistration};
use setu_types::SubnetId;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{info, warn};

/// Request body for POST /governance/propose
#[derive(Debug, Deserialize)]
pub struct ProposeRequest {
    pub content: ProposalContent,
}

/// Response body for POST /governance/propose
#[derive(Debug, Serialize)]
pub struct ProposeResponse {
    pub success: bool,
    pub proposal_id: Option<String>,
    pub message: String,
}

/// Request body for POST /governance/callback
#[derive(Debug, Deserialize)]
pub struct CallbackRequest {
    pub proposal_id: String,
    pub callback_token: String,
    pub decision: GovernanceDecision,
}

/// Response body for POST /governance/callback
#[derive(Debug, Serialize)]
pub struct CallbackResponse {
    pub success: bool,
    pub message: String,
}

/// Response body for GET /governance/status/:proposal_id
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub found: bool,
    pub pending: bool,
    pub proposal_id: String,
    pub message: String,
}

/// Request body for POST /governance/register-system-subnet
#[derive(Debug, Deserialize)]
pub struct RegisterSystemSubnetRequest {
    /// SubnetId as hex string (with or without 0x prefix)
    pub subnet_id: String,
    /// Agent HTTP endpoint
    pub agent_endpoint: String,
    /// Optional: callback address override
    pub callback_addr: Option<String>,
    /// Optional: proposal timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Registrant identity
    pub registrant: String,
    /// Ed25519 public key hex (must match genesis validator's public_key)
    pub public_key: String,
    /// Ed25519 signature hex over the registration signing message
    pub signature: String,
}

/// Response body for POST /governance/register-system-subnet
#[derive(Debug, Serialize)]
pub struct RegisterSystemSubnetResponse {
    pub success: bool,
    pub event_id: Option<String>,
    pub message: String,
}

/// Error type for handler operations.
#[derive(Debug, thiserror::Error)]
pub enum GovernanceHandlerError {
    #[error("Executor error: {0}")]
    Executor(#[from] GovernanceExecutorError),
    #[error("Service error: {0}")]
    Service(#[from] GovernanceServiceError),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
}

/// GovernanceHandler orchestrates the propose/callback/status flow.
///
/// It is **not** an axum handler directly — it provides methods that the
/// ValidatorNetworkService can call from its axum route handlers.
/// This keeps the governance module independent of the specific web framework details.
pub struct GovernanceHandler;

impl GovernanceHandler {
    /// Handle a propose request.
    ///
    /// 1. Generate proposal_id and callback_token
    /// 2. Build Propose Event via GovernanceExecutor
    /// 3. Insert into GovernanceService pending_governance
    /// 4. Return (event, proposal_id, callback_token) for the caller to:
    ///    a. add_event_to_dag(event)
    ///    b. dispatch_to_agent(proposal_id, content, callback_token)
    ///
    /// The caller (ValidatorNetworkService) owns the VLC and DAG, so we return
    /// the Event for the caller to submit.
    pub fn prepare_propose(
        governance_service: &GovernanceService,
        content: ProposalContent,
        timestamp: u64,
        vlc_snapshot: setu_vlc::VLCSnapshot,
        validator_id: &str,
    ) -> Result<PreparedProposal, GovernanceHandlerError> {
        let proposal_id =
            governance_service.generate_proposal_id(&content.proposer, timestamp);
        let callback_token = GovernanceService::generate_callback_token();

        let event = GovernanceExecutor::execute_propose(
            proposal_id,
            content.clone(),
            timestamp,
            vlc_snapshot,
            None, // New proposal, no existing
            validator_id.to_string(),
        )?;

        let pending = PendingProposal {
            proposal_id,
            content,
            submitted_at: Instant::now(),
            callback_token,
        };

        governance_service.insert_pending(pending);

        info!(
            proposal_id = %hex::encode(proposal_id),
            "Governance proposal prepared"
        );

        Ok(PreparedProposal {
            event,
            proposal_id,
            callback_token,
        })
    }

    /// Handle a callback request from the Agent subnet.
    ///
    /// 1. Validate callback_token
    /// 2. Remove from pending
    /// 3. Build Execute Event via GovernanceExecutor
    /// 4. Return event for the caller to add_event_to_dag()
    pub fn prepare_execute(
        governance_service: &GovernanceService,
        proposal_id: [u8; 32],
        callback_token: [u8; 32],
        decision: GovernanceDecision,
        timestamp: u64,
        vlc_snapshot: setu_vlc::VLCSnapshot,
        // The caller reads this from GOVERNANCE SMT
        proposal: &setu_types::governance::GovernanceProposal,
        validator_id: &str,
        current_resource_params: Option<&setu_types::ResourceParams>,
    ) -> Result<setu_types::Event, GovernanceHandlerError> {
        // Validate callback token (constant-time comparison)
        if !governance_service.validate_callback_token(&proposal_id, &callback_token) {
            warn!(
                proposal_id = %hex::encode(proposal_id),
                "Invalid callback token for governance callback"
            );
            return Err(GovernanceHandlerError::Forbidden(
                "Invalid callback token".to_string(),
            ));
        }

        // Build Execute Event
        let event = GovernanceExecutor::execute_decision(
            proposal_id,
            decision,
            timestamp,
            vlc_snapshot,
            proposal,
            validator_id.to_string(),
            current_resource_params,
        )?;

        // Remove from pending (consume the one-time token)
        governance_service.remove_pending(&proposal_id);

        info!(
            proposal_id = %hex::encode(proposal_id),
            "Governance execute prepared from callback"
        );

        Ok(event)
    }

    /// Handle a timeout: build an Execute Event with rejection.
    pub fn prepare_timeout_execute(
        governance_service: &GovernanceService,
        proposal_id: [u8; 32],
        timestamp: u64,
        vlc_snapshot: setu_vlc::VLCSnapshot,
        proposal: &setu_types::governance::GovernanceProposal,
        validator_id: &str,
        current_resource_params: Option<&setu_types::ResourceParams>,
    ) -> Result<setu_types::Event, GovernanceHandlerError> {
        let decision = GovernanceDecision {
            approved: false,
            reasoning: format!(
                "Agent subnet timeout after {}s",
                governance_service.config().timeout.as_secs()
            ),
            conditions: vec![],
        };

        let event = GovernanceExecutor::execute_decision(
            proposal_id,
            decision,
            timestamp,
            vlc_snapshot,
            proposal,
            validator_id.to_string(),
            current_resource_params,
        )?;

        // Remove from pending
        governance_service.remove_pending(&proposal_id);

        info!(
            proposal_id = %hex::encode(proposal_id),
            "Governance timeout execute prepared"
        );

        Ok(event)
    }

    /// Handle a register-system-subnet request.
    ///
    /// 1. Parse and validate SubnetId from hex
    /// 2. Build RegisterSystemSubnet Event via GovernanceExecutor
    /// 3. Return event for the caller to `add_event_to_dag()`
    ///
    /// No pending state needed — this is a sync direct action.
    pub fn prepare_register_system_subnet(
        request: RegisterSystemSubnetRequest,
        timestamp: u64,
        vlc_snapshot: setu_vlc::VLCSnapshot,
        validator_id: &str,
        genesis_validators: &[setu_types::genesis::GenesisValidator],
    ) -> Result<setu_types::Event, GovernanceHandlerError> {
        // Parse SubnetId from hex
        let subnet_id = SubnetId::from_hex(&request.subnet_id)
            .map_err(|e| GovernanceHandlerError::InvalidRequest(
                format!("Invalid subnet_id hex: {}", e),
            ))?;

        let registration = SystemSubnetRegistration {
            subnet_id,
            agent_endpoint: request.agent_endpoint,
            callback_addr: request.callback_addr,
            timeout_secs: request.timeout_secs,
            registrant: request.registrant,
            public_key: request.public_key,
            signature: request.signature,
        };

        let event = GovernanceExecutor::execute_register_system_subnet(
            registration,
            timestamp,
            vlc_snapshot,
            validator_id.to_string(),
            genesis_validators,
        )?;

        info!(
            subnet_id = %subnet_id,
            "System subnet registration prepared"
        );

        Ok(event)
    }
}

/// Result of preparing a proposal — contains everything the caller needs.
pub struct PreparedProposal {
    pub event: setu_types::Event,
    pub proposal_id: [u8; 32],
    pub callback_token: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::service::GovernanceServiceConfig;
    use setu_types::governance::{
        GovernanceProposal, ProposalEffect, ProposalStatus, ProposalType,
    };
    use std::time::Duration;

    fn sample_vlc() -> setu_vlc::VLCSnapshot {
        setu_vlc::VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        }
    }

    fn sample_content() -> ProposalContent {
        ProposalContent {
            proposer: "alice".to_string(),
            proposal_type: ProposalType::ParameterChange,
            title: "Test Proposal".to_string(),
            description: "A test proposal".to_string(),
            action: ProposalEffect::UpdateParameter {
                key: "max_tps".to_string(),
                value: vec![1, 0, 0, 0],
            },
        }
    }

    fn sample_proposal(proposal_id: [u8; 32]) -> GovernanceProposal {
        GovernanceProposal {
            proposal_id,
            content: sample_content(),
            status: ProposalStatus::Pending,
            decision: None,
            created_at: 1000,
            decided_at: None,
        }
    }

    fn test_service() -> GovernanceService {
        GovernanceService::new(GovernanceServiceConfig {
            callback_addr: "127.0.0.1:8080".to_string(),
            timeout: Duration::from_secs(300),
            max_retries: 1,
            takeover_factor: 2,
        })
    }

    #[test]
    fn test_prepare_propose_ok() {
        let svc = test_service();
        let result =
            GovernanceHandler::prepare_propose(&svc, sample_content(), 1000, sample_vlc(), "test-validator");
        assert!(result.is_ok());
        let prepared = result.unwrap();
        assert!(!prepared.proposal_id.iter().all(|&b| b == 0));
        assert!(!prepared.callback_token.iter().all(|&b| b == 0));
        // Should be in pending
        assert_eq!(svc.pending_count(), 1);
    }

    #[test]
    fn test_prepare_execute_ok() {
        let svc = test_service();

        // First, prepare a proposal to get callback_token
        let prepared =
            GovernanceHandler::prepare_propose(&svc, sample_content(), 1000, sample_vlc(), "test-validator")
                .unwrap();

        let proposal = sample_proposal(prepared.proposal_id);
        let decision = GovernanceDecision {
            approved: true,
            reasoning: "ok".to_string(),
            conditions: vec![],
        };

        let result = GovernanceHandler::prepare_execute(
            &svc,
            prepared.proposal_id,
            prepared.callback_token,
            decision,
            2000,
            sample_vlc(),
            &proposal,
            "test-validator",
            None,
        );
        assert!(result.is_ok());
        // Should be removed from pending
        assert_eq!(svc.pending_count(), 0);
    }

    #[test]
    fn test_prepare_execute_wrong_token() {
        let svc = test_service();

        let prepared =
            GovernanceHandler::prepare_propose(&svc, sample_content(), 1000, sample_vlc(), "test-validator")
                .unwrap();

        let proposal = sample_proposal(prepared.proposal_id);
        let decision = GovernanceDecision {
            approved: true,
            reasoning: "ok".to_string(),
            conditions: vec![],
        };

        let result = GovernanceHandler::prepare_execute(
            &svc,
            prepared.proposal_id,
            [0xFFu8; 32], // wrong token
            decision,
            2000,
            sample_vlc(),
            &proposal,
            "test-validator",
            None,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceHandlerError::Forbidden(_)
        ));
        // Should still be in pending (not consumed)
        assert_eq!(svc.pending_count(), 1);
    }

    // ---- prepare_register_system_subnet tests ----

    fn sample_genesis_validators() -> Vec<setu_types::genesis::GenesisValidator> {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let pk_hex = hex::encode(signing_key.verifying_key().as_bytes());
        vec![setu_types::genesis::GenesisValidator {
            id: "validator-1".to_string(),
            address: "127.0.0.1".to_string(),
            p2p_port: 9000,
            public_key: Some(pk_hex),
        }]
    }

    fn signed_register_request() -> RegisterSystemSubnetRequest {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let pk_hex = hex::encode(signing_key.verifying_key().as_bytes());
        // Build a registration to compute signing message, then sign
        let mut reg = setu_types::governance::SystemSubnetRegistration {
            subnet_id: setu_types::SubnetId::new_system(0x20),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: Some("10.0.1.5:8080".to_string()),
            timeout_secs: Some(60),
            registrant: "validator-1".to_string(),
            public_key: pk_hex.clone(),
            signature: String::new(),
        };
        reg.sign(&[42u8; 32]).unwrap();
        RegisterSystemSubnetRequest {
            subnet_id: format!("0x{}", hex::encode(setu_types::SubnetId::new_system(0x20).as_bytes())),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: Some("10.0.1.5:8080".to_string()),
            timeout_secs: Some(60),
            registrant: "validator-1".to_string(),
            public_key: pk_hex,
            signature: reg.signature,
        }
    }

    #[test]
    fn test_prepare_register_system_subnet_ok() {
        let req = signed_register_request();
        let gv = sample_genesis_validators();
        let result = GovernanceHandler::prepare_register_system_subnet(
            req,
            1000,
            sample_vlc(),
            "test-validator",
            &gv,
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::EventType::Governance);
    }

    #[test]
    fn test_prepare_register_invalid_hex() {
        let req = RegisterSystemSubnetRequest {
            subnet_id: "not-a-hex".to_string(),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: None,
            timeout_secs: None,
            registrant: "validator-1".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };
        let gv = sample_genesis_validators();
        let result = GovernanceHandler::prepare_register_system_subnet(
            req,
            1000,
            sample_vlc(),
            "test-validator",
            &gv,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceHandlerError::InvalidRequest(_)
        ));
    }
}
