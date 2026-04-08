//! GovernanceExecutor — builds Governance Events with StateChanges.
//!
//! Does NOT write SMT (G3 compliant, Deferred Commit).
//! Caller (GovernanceHandler) is responsible for `add_event_to_dag()`.
//! Follows the InfraExecutor pattern: receive vlc_snapshot, return Event.

use setu_governance::{
    GovernanceEffect, GovernanceError, ProposalStateMachine, ProposalValidator,
};
use setu_types::event::{Event, EventPayload, EventType, ExecutionResult, StateChange};
use setu_types::governance::{
    GovernanceAction, GovernanceDecision, GovernancePayload, GovernanceProposal, ProposalContent,
    SystemSubnetRegistration,
};
use setu_types::genesis::GenesisValidator;
use setu_types::SubnetId;
use setu_vlc::VLCSnapshot;

/// Builds Governance Events. Pure construction — no I/O, no SMT writes.
pub struct GovernanceExecutor;

impl GovernanceExecutor {
    /// Build a Propose Event.
    ///
    /// `existing` is looked up from the committed GOVERNANCE SMT by the caller.
    /// Returns an Event ready for `add_event_to_dag()`.
    pub fn execute_propose(
        proposal_id: [u8; 32],
        content: ProposalContent,
        timestamp: u64,
        vlc_snapshot: VLCSnapshot,
        existing: Option<&GovernanceProposal>,
        creator: String,
    ) -> Result<Event, GovernanceExecutorError> {
        // Validate via setu-governance pure logic
        ProposalValidator::validate_propose(&content, existing)?;

        // Create proposal object
        let proposal = ProposalStateMachine::create(proposal_id, content.clone(), timestamp);

        // Build state_changes: create proposal in GOVERNANCE subnet (G11: oid:{hex})
        let key = format!("oid:{}", hex::encode(proposal_id));
        let new_value =
            serde_json::to_vec(&proposal).map_err(|e| GovernanceExecutorError::Serialization(e.to_string()))?;

        let state_changes = vec![StateChange::new(key, None, Some(new_value))];

        // Build Event
        let payload = GovernancePayload {
            proposal_id,
            action: GovernanceAction::Propose(content),
        };

        let mut event = Event::new(
            EventType::Governance,
            vec![], // empty parent_ids (same as InfraExecutor)
            vlc_snapshot,
            creator,
        );
        event.payload = EventPayload::Governance(payload);
        event.set_execution_result(ExecutionResult::success().with_changes(state_changes));

        Ok(event)
    }

    /// Build an Execute Event with a decision.
    ///
    /// `proposal` is read from the committed GOVERNANCE SMT by the caller.
    /// May produce cross-subnet StateChanges via `target_subnet`.
    pub fn execute_decision(
        proposal_id: [u8; 32],
        decision: GovernanceDecision,
        timestamp: u64,
        vlc_snapshot: VLCSnapshot,
        proposal: &GovernanceProposal,
        creator: String,
    ) -> Result<Event, GovernanceExecutorError> {
        // Validate via setu-governance pure logic
        ProposalValidator::validate_execute(Some(proposal), &decision)?;

        // Transition state machine → updated proposal + abstract effects
        let (updated_proposal, effects) =
            ProposalStateMachine::transition(proposal, &decision, timestamp)?;

        // Materialize abstract effects → concrete StateChanges
        let state_changes = Self::materialize_effects(proposal_id, proposal, &updated_proposal, &effects)?;

        // Build Event
        let payload = GovernancePayload {
            proposal_id,
            action: GovernanceAction::Execute(decision),
        };

        let mut event = Event::new(
            EventType::Governance,
            vec![],
            vlc_snapshot,
            creator,
        );
        event.payload = EventPayload::Governance(payload);
        event.set_execution_result(ExecutionResult::success().with_changes(state_changes));

        Ok(event)
    }

    /// Build a RegisterSystemSubnet Event.
    ///
    /// Validates that `subnet_id` is a system subnet and not ROOT.
    /// State key: `oid:{hex(subnet_id)}` in GOVERNANCE SMT.
    /// proposal_id in GovernancePayload = `subnet_id.to_bytes()`.
    pub fn execute_register_system_subnet(
        registration: SystemSubnetRegistration,
        timestamp: u64,
        vlc_snapshot: VLCSnapshot,
        creator: String,
        genesis_validators: &[GenesisValidator],
    ) -> Result<Event, GovernanceExecutorError> {
        // Validate: must be system subnet, not ROOT
        if !registration.subnet_id.is_system() || registration.subnet_id.is_root() {
            return Err(GovernanceExecutorError::InvalidRegistration(
                "SubnetId must be a system subnet (is_system() && !is_root())".to_string(),
            ));
        }

        // Validate: agent_endpoint must be http:// or https://
        if !registration.agent_endpoint.starts_with("http://")
            && !registration.agent_endpoint.starts_with("https://")
        {
            return Err(GovernanceExecutorError::InvalidRegistration(
                "agent_endpoint must start with http:// or https://".to_string(),
            ));
        }

        // Auth: registrant must be a genesis validator
        let genesis_val = genesis_validators.iter()
            .find(|v| v.id == registration.registrant)
            .ok_or_else(|| GovernanceExecutorError::Unauthorized(
                format!("'{}' is not a genesis validator", registration.registrant),
            ))?;

        // Auth: public_key must match genesis validator's public_key
        let genesis_pk = genesis_val.public_key.as_ref()
            .ok_or_else(|| GovernanceExecutorError::Unauthorized(
                format!("Genesis validator '{}' has no public key configured", registration.registrant),
            ))?;
        if registration.public_key != *genesis_pk {
            return Err(GovernanceExecutorError::Unauthorized(
                "Public key does not match genesis validator config".to_string(),
            ));
        }

        // Auth: verify Ed25519 signature
        let pk_bytes = hex::decode(&registration.public_key)
            .map_err(|e| GovernanceExecutorError::Unauthorized(
                format!("Invalid public_key hex: {}", e),
            ))?;
        if !registration.verify_signature(&pk_bytes) {
            return Err(GovernanceExecutorError::Unauthorized(
                "Invalid signature".to_string(),
            ));
        }

        // State key: oid:{hex(subnet_id)} — G11 compliant
        let key = format!("oid:{}", hex::encode(registration.subnet_id.as_bytes()));
        let new_value = serde_json::to_vec(&registration)
            .map_err(|e| GovernanceExecutorError::Serialization(e.to_string()))?;

        let state_changes = vec![StateChange::new(key, None, Some(new_value))];

        // proposal_id = subnet_id.to_bytes() (R3-ISSUE-1)
        let payload = GovernancePayload {
            proposal_id: registration.subnet_id.to_bytes(),
            action: GovernanceAction::RegisterSystemSubnet(registration),
        };

        let mut event = Event::new(
            EventType::Governance,
            vec![],
            vlc_snapshot,
            creator,
        );
        event.payload = EventPayload::Governance(payload);
        event.set_execution_result(ExecutionResult::success().with_changes(state_changes));

        Ok(event)
    }

    /// Materialize GovernanceEffect → StateChange.
    ///
    /// This is the integration layer's responsibility:
    /// - UpdateProposal → JSON to GOVERNANCE subnet (target_subnet=None)
    /// - SlashValidator → target_subnet=Some(ROOT)
    /// - Others → target_subnet=Some(ROOT)
    ///
    /// `original_proposal` is the pre-transition proposal from SMT (used as old_value).
    /// `updated_proposal` is the post-transition proposal (used as new_value).
    fn materialize_effects(
        proposal_id: [u8; 32],
        original_proposal: &GovernanceProposal,
        updated_proposal: &GovernanceProposal,
        effects: &[GovernanceEffect],
    ) -> Result<Vec<StateChange>, GovernanceExecutorError> {
        let mut changes = Vec::new();

        for effect in effects {
            match effect {
                GovernanceEffect::UpdateProposal { .. } => {
                    // Serialize updated proposal as JSON (non-Coin object → G6 JSON zone)
                    let key = format!("oid:{}", hex::encode(proposal_id));
                    // old_value: serialize the original proposal directly from SMT.
                    // Using the original avoids fragile reconstruction from updated_proposal
                    // and guarantees byte-match with apply_committed_events conflict detection.
                    let old_value = serde_json::to_vec(original_proposal)
                        .map_err(|e| GovernanceExecutorError::Serialization(e.to_string()))?;
                    let new_value = serde_json::to_vec(updated_proposal)
                        .map_err(|e| GovernanceExecutorError::Serialization(e.to_string()))?;

                    changes.push(StateChange::update(key, old_value, new_value));
                }
                GovernanceEffect::SlashValidator {
                    validator_id,
                    amount,
                } => {
                    // Cross-subnet: write to ROOT subnet
                    // In Phase 1, we record the intent as a JSON marker.
                    // Full coin-level slash requires reading the validator's coin from ROOT SMT,
                    // which will be implemented when the slash mechanism is production-ready.
                    let key = format!(
                        "oid:{}",
                        hex::encode(blake3::hash(format!("slash:{}:{}", validator_id, amount).as_bytes()).as_bytes())
                    );
                    let value = serde_json::to_vec(&serde_json::json!({
                        "type": "slash",
                        "validator_id": validator_id,
                        "amount": amount,
                    }))
                    .map_err(|e| GovernanceExecutorError::Serialization(e.to_string()))?;

                    changes.push(StateChange::insert(key, value).with_target_subnet(SubnetId::ROOT));
                }
                GovernanceEffect::UpdateParameter { key, value } => {
                    let state_key = format!(
                        "oid:{}",
                        hex::encode(blake3::hash(format!("param:{}", key).as_bytes()).as_bytes())
                    );
                    changes.push(
                        StateChange::insert(state_key, value.clone())
                            .with_target_subnet(SubnetId::ROOT),
                    );
                }
                GovernanceEffect::ResolveDispute {
                    dispute_id,
                    resolution,
                } => {
                    let key = format!("oid:{}", hex::encode(dispute_id));
                    changes.push(
                        StateChange::insert(key, resolution.clone())
                            .with_target_subnet(SubnetId::ROOT),
                    );
                }
            }
        }

        Ok(changes)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GovernanceExecutorError {
    #[error("Governance validation error: {0}")]
    Validation(#[from] GovernanceError),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Invalid registration: {0}")]
    InvalidRegistration(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::governance::{ProposalEffect, ProposalStatus, ProposalType};

    fn sample_vlc() -> VLCSnapshot {
        let vlc = setu_vlc::VectorClock::new();
        VLCSnapshot {
            vector_clock: vlc,
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

    fn sample_proposal_pending() -> GovernanceProposal {
        GovernanceProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            status: ProposalStatus::Pending,
            decision: None,
            created_at: 1000,
            decided_at: None,
        }
    }

    fn sample_decision(approved: bool) -> GovernanceDecision {
        GovernanceDecision {
            approved,
            reasoning: "test reasoning".to_string(),
            conditions: vec![],
        }
    }

    // ---- execute_propose tests ----

    #[test]
    fn test_execute_propose_ok() {
        let result = GovernanceExecutor::execute_propose(
            [1u8; 32],
            sample_content(),
            1000,
            sample_vlc(),
            None, // no existing
            "test-validator".to_string(),
        );
        let event = result.unwrap();
        assert_eq!(event.event_type, EventType::Governance);
        assert!(event.execution_result.is_some());
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 1);
        assert!(er.state_changes[0].old_value.is_none());
        assert!(er.state_changes[0].new_value.is_some());
    }

    #[test]
    fn test_execute_propose_duplicate() {
        let existing = sample_proposal_pending();
        let result = GovernanceExecutor::execute_propose(
            [1u8; 32],
            sample_content(),
            1000,
            sample_vlc(),
            Some(&existing),
            "test-validator".to_string(),
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceExecutorError::Validation(_)
        ));
    }

    #[test]
    fn test_execute_propose_empty_title() {
        let mut content = sample_content();
        content.title = String::new();
        let result =
            GovernanceExecutor::execute_propose([1u8; 32], content, 1000, sample_vlc(), None, "test-validator".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_propose_state_change_format() {
        let result = GovernanceExecutor::execute_propose(
            [1u8; 32],
            sample_content(),
            1000,
            sample_vlc(),
            None,
            "test-validator".to_string(),
        )
        .unwrap();
        let sc = &result.execution_result.as_ref().unwrap().state_changes[0];
        // G11: key must start with "oid:"
        assert!(sc.key.starts_with("oid:"));
        // old_value must be None for creation
        assert!(sc.old_value.is_none());
        // new_value must be valid JSON
        let parsed: serde_json::Value =
            serde_json::from_slice(sc.new_value.as_ref().unwrap()).unwrap();
        assert!(parsed.get("proposal_id").is_some());
        assert_eq!(parsed["status"], "Pending");
        // target_subnet must be None (writes to event's own GOVERNANCE subnet)
        assert!(sc.target_subnet.is_none());
    }

    #[test]
    fn test_execute_propose_subnet_id() {
        let result = GovernanceExecutor::execute_propose(
            [1u8; 32],
            sample_content(),
            1000,
            sample_vlc(),
            None,
            "test-validator".to_string(),
        )
        .unwrap();
        assert_eq!(result.get_subnet_id(), SubnetId::GOVERNANCE);
    }

    // ---- execute_decision tests ----

    #[test]
    fn test_execute_decision_approve() {
        let proposal = sample_proposal_pending();
        let result = GovernanceExecutor::execute_decision(
            [1u8; 32],
            sample_decision(true),
            2000,
            sample_vlc(),
            &proposal,
            "test-validator".to_string(),
        );
        let event = result.unwrap();
        assert_eq!(event.event_type, EventType::Governance);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        // Should have UpdateProposal + UpdateParameter effects
        assert!(er.state_changes.len() >= 2);
    }

    #[test]
    fn test_execute_decision_reject() {
        let proposal = sample_proposal_pending();
        let result = GovernanceExecutor::execute_decision(
            [1u8; 32],
            sample_decision(false),
            2000,
            sample_vlc(),
            &proposal,
            "test-validator".to_string(),
        );
        let event = result.unwrap();
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        // Rejected: only UpdateProposal, no side-effect changes
        assert_eq!(er.state_changes.len(), 1);
        // Verify it's the proposal update (no target_subnet)
        assert!(er.state_changes[0].target_subnet.is_none());
    }

    #[test]
    fn test_execute_decision_slash_cross_subnet() {
        let mut proposal = sample_proposal_pending();
        proposal.content.action = ProposalEffect::SlashValidator {
            validator_id: "val1".to_string(),
            amount: 100,
        };
        let result = GovernanceExecutor::execute_decision(
            [1u8; 32],
            sample_decision(true),
            2000,
            sample_vlc(),
            &proposal,
            "test-validator".to_string(),
        );
        let event = result.unwrap();
        let er = event.execution_result.as_ref().unwrap();
        // Should have UpdateProposal (GOVERNANCE) + SlashValidator (ROOT)
        assert!(er.state_changes.len() >= 2);
        let cross_subnet = er
            .state_changes
            .iter()
            .find(|sc| sc.target_subnet.is_some());
        assert!(cross_subnet.is_some());
        assert_eq!(cross_subnet.unwrap().target_subnet, Some(SubnetId::ROOT));
    }

    #[test]
    fn test_execute_decision_not_pending() {
        let mut proposal = sample_proposal_pending();
        proposal.status = ProposalStatus::Approved;
        let result = GovernanceExecutor::execute_decision(
            [1u8; 32],
            sample_decision(true),
            2000,
            sample_vlc(),
            &proposal,
            "test-validator".to_string(),
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceExecutorError::Validation(_)
        ));
    }

    #[test]
    fn test_materialize_effects_update_proposal() {
        let original = sample_proposal_pending();
        let updated = GovernanceProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            status: ProposalStatus::Approved,
            decision: Some(sample_decision(true)),
            created_at: 1000,
            decided_at: Some(2000),
        };
        let effects = vec![GovernanceEffect::UpdateProposal {
            proposal_id: [1u8; 32],
            new_status: ProposalStatus::Approved,
            decision: Some(sample_decision(true)),
        }];
        let changes =
            GovernanceExecutor::materialize_effects([1u8; 32], &original, &updated, &effects).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].key.starts_with("oid:"));
        assert!(changes[0].target_subnet.is_none());
    }

    #[test]
    fn test_materialize_effects_slash() {
        let original = sample_proposal_pending();
        let updated = GovernanceProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            status: ProposalStatus::Approved,
            decision: Some(sample_decision(true)),
            created_at: 1000,
            decided_at: Some(2000),
        };
        let effects = vec![GovernanceEffect::SlashValidator {
            validator_id: "val1".to_string(),
            amount: 100,
        }];
        let changes =
            GovernanceExecutor::materialize_effects([1u8; 32], &original, &updated, &effects).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].target_subnet, Some(SubnetId::ROOT));
    }

    /// R8-ISSUE-1 verification: Propose.new_value must == Execute.old_value byte-for-byte.
    /// apply_committed_events does byte-level comparison for conflict detection.
    #[test]
    fn test_propose_new_value_matches_execute_old_value() {
        let proposal_id = [1u8; 32];
        let content = sample_content();
        let timestamp = 1000u64;

        // Phase 1: Propose Event
        let propose_event = GovernanceExecutor::execute_propose(
            proposal_id,
            content.clone(),
            timestamp,
            sample_vlc(),
            None,
            "test-validator".to_string(),
        )
        .unwrap();
        let propose_new_value = propose_event
            .execution_result
            .as_ref()
            .unwrap()
            .state_changes[0]
            .new_value
            .as_ref()
            .unwrap()
            .clone();

        // Simulate what SMT stores: the propose_new_value bytes.
        // The proposal object as it would be read back from SMT for execute_decision.
        let stored_proposal: GovernanceProposal =
            serde_json::from_slice(&propose_new_value).unwrap();

        // Phase 2: Execute Event uses the stored proposal
        let execute_event = GovernanceExecutor::execute_decision(
            proposal_id,
            sample_decision(true),
            2000,
            sample_vlc(),
            &stored_proposal,
            "test-validator".to_string(),
        )
        .unwrap();

        // Find the UpdateProposal state change (target_subnet == None)
        let update_sc = execute_event
            .execution_result
            .as_ref()
            .unwrap()
            .state_changes
            .iter()
            .find(|sc| sc.target_subnet.is_none())
            .expect("Must have UpdateProposal state change");

        let execute_old_value = update_sc.old_value.as_ref().unwrap();

        // CRITICAL: these must be byte-identical for conflict detection to pass
        assert_eq!(
            &propose_new_value, execute_old_value,
            "Propose.new_value and Execute.old_value must match byte-for-byte!\n\
             Propose: {}\nExecute: {}",
            String::from_utf8_lossy(&propose_new_value),
            String::from_utf8_lossy(execute_old_value),
        );
    }

    // ---- execute_register_system_subnet tests ----

    fn sample_genesis_validators() -> Vec<GenesisValidator> {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let pk_hex = hex::encode(signing_key.verifying_key().as_bytes());
        vec![GenesisValidator {
            id: "validator-1".to_string(),
            address: "127.0.0.1".to_string(),
            p2p_port: 9000,
            public_key: Some(pk_hex),
        }]
    }

    fn sample_system_registration() -> SystemSubnetRegistration {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let pk_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let mut reg = SystemSubnetRegistration {
            subnet_id: SubnetId::new_system(0x20),
            agent_endpoint: "http://oracle:8091".to_string(),
            callback_addr: Some("10.0.1.5:8080".to_string()),
            timeout_secs: Some(60),
            registrant: "validator-1".to_string(),
            public_key: pk_hex,
            signature: String::new(),
        };
        reg.sign(&[42u8; 32]).unwrap();
        reg
    }

    #[test]
    fn test_execute_register_system_subnet_ok() {
        let reg = sample_system_registration();
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg,
            1000,
            sample_vlc(),
            "test-validator".to_string(),
            &gv,
        );
        let event = result.unwrap();
        assert_eq!(event.event_type, EventType::Governance);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 1);

        // G11: key must use oid:{hex(subnet_id)} format
        let sc = &er.state_changes[0];
        let expected_key = format!("oid:{}", hex::encode(SubnetId::new_system(0x20).as_bytes()));
        assert_eq!(sc.key, expected_key);
        assert!(sc.old_value.is_none()); // creation
        assert!(sc.new_value.is_some());

        // Verify new_value is valid SystemSubnetRegistration JSON
        let parsed: SystemSubnetRegistration =
            serde_json::from_slice(sc.new_value.as_ref().unwrap()).unwrap();
        assert_eq!(parsed.agent_endpoint, "http://oracle:8091");

        // Verify payload
        if let EventPayload::Governance(payload) = &event.payload {
            assert_eq!(payload.proposal_id, SubnetId::new_system(0x20).to_bytes());
            assert!(matches!(
                payload.action,
                GovernanceAction::RegisterSystemSubnet(_)
            ));
        } else {
            panic!("Expected Governance payload");
        }
    }

    #[test]
    fn test_execute_register_non_system_rejected() {
        let mut reg = sample_system_registration();
        reg.subnet_id = SubnetId::from_str_id("app-subnet"); // APP type
        // Re-sign with changed subnet_id
        reg.sign(&[42u8; 32]).unwrap();
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg,
            1000,
            sample_vlc(),
            "test-validator".to_string(),
            &gv,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceExecutorError::InvalidRegistration(_)
        ));
    }

    #[test]
    fn test_execute_register_root_rejected() {
        let mut reg = sample_system_registration();
        reg.subnet_id = SubnetId::ROOT;
        reg.sign(&[42u8; 32]).unwrap();
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg,
            1000,
            sample_vlc(),
            "test-validator".to_string(),
            &gv,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GovernanceExecutorError::InvalidRegistration(_)
        ));
    }

    #[test]
    fn test_execute_register_invalid_endpoint() {
        let mut reg = sample_system_registration();
        reg.agent_endpoint = "ftp://bad-protocol:8091".to_string();
        reg.sign(&[42u8; 32]).unwrap();
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg,
            1000,
            sample_vlc(),
            "test-validator".to_string(),
            &gv,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GovernanceExecutorError::InvalidRegistration(_)));
        assert!(err.to_string().contains("http://"));
    }

    #[test]
    fn test_execute_register_non_genesis_registrant_rejected() {
        let mut reg = sample_system_registration();
        reg.registrant = "unknown-node".to_string();
        reg.sign(&[42u8; 32]).unwrap();
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg, 1000, sample_vlc(), "test-validator".to_string(), &gv,
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GovernanceExecutorError::Unauthorized(_)));
    }

    #[test]
    fn test_execute_register_wrong_public_key_rejected() {
        let mut reg = sample_system_registration();
        // Use a different key that doesn't match genesis
        let other_key = ed25519_dalek::SigningKey::from_bytes(&[99u8; 32]);
        reg.public_key = hex::encode(other_key.verifying_key().as_bytes());
        reg.sign(&[99u8; 32]).unwrap(); // Valid signature but wrong key
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg, 1000, sample_vlc(), "test-validator".to_string(), &gv,
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GovernanceExecutorError::Unauthorized(_)));
    }

    #[test]
    fn test_execute_register_invalid_signature_rejected() {
        let mut reg = sample_system_registration();
        reg.signature = hex::encode([0xFFu8; 64]); // Garbage signature
        let gv = sample_genesis_validators();
        let result = GovernanceExecutor::execute_register_system_subnet(
            reg, 1000, sample_vlc(), "test-validator".to_string(), &gv,
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GovernanceExecutorError::Unauthorized(_)));
    }
}
