//! Proposal state machine — lifecycle transitions for governance proposals.

use setu_types::{
    GovernanceDecision, GovernanceProposal, ProposalContent, ProposalStatus,
};
use crate::effect::{EffectExecutor, GovernanceEffect};
use crate::error::GovernanceError;

pub struct ProposalStateMachine;

impl ProposalStateMachine {
    /// Create a new proposal in Pending status.
    pub fn create(
        proposal_id: [u8; 32],
        content: ProposalContent,
        timestamp: u64,
    ) -> GovernanceProposal {
        GovernanceProposal {
            proposal_id,
            content,
            status: ProposalStatus::Pending,
            decision: None,
            created_at: timestamp,
            decided_at: None,
        }
    }

    /// Transition a proposal based on a decision.
    ///
    /// Returns the updated proposal and the abstract effects to apply.
    pub fn transition(
        proposal: &GovernanceProposal,
        decision: &GovernanceDecision,
        timestamp: u64,
    ) -> Result<(GovernanceProposal, Vec<GovernanceEffect>), GovernanceError> {
        if proposal.status != ProposalStatus::Pending {
            return Err(GovernanceError::InvalidStatus {
                expected: ProposalStatus::Pending,
                actual: proposal.status.clone(),
            });
        }

        let effects = EffectExecutor::execute(
            proposal.proposal_id,
            &proposal.content.action,
            decision,
        );

        let new_status = if decision.approved {
            ProposalStatus::Approved
        } else {
            ProposalStatus::Rejected
        };

        let updated = GovernanceProposal {
            proposal_id: proposal.proposal_id,
            content: proposal.content.clone(),
            status: new_status,
            decision: Some(decision.clone()),
            created_at: proposal.created_at,
            decided_at: Some(timestamp),
        };

        Ok((updated, effects))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{ProposalType, ProposalEffect};

    fn sample_content() -> ProposalContent {
        ProposalContent {
            proposer: "alice".into(),
            proposal_type: ProposalType::ValidatorSlash,
            title: "Slash bad validator".into(),
            description: "Validator v1 misbehaved".into(),
            action: ProposalEffect::SlashValidator {
                validator_id: "v1".into(),
                amount: 500,
            },
        }
    }

    fn sample_decision(approved: bool) -> GovernanceDecision {
        GovernanceDecision {
            approved,
            reasoning: "Reviewed".into(),
            conditions: vec![],
        }
    }

    #[test]
    fn test_state_machine_create() {
        let proposal = ProposalStateMachine::create([1u8; 32], sample_content(), 1000);
        assert_eq!(proposal.status, ProposalStatus::Pending);
        assert!(proposal.decision.is_none());
        assert_eq!(proposal.created_at, 1000);
        assert!(proposal.decided_at.is_none());
    }

    #[test]
    fn test_state_machine_approve() {
        let proposal = ProposalStateMachine::create([1u8; 32], sample_content(), 1000);
        let decision = sample_decision(true);
        let (updated, effects) = ProposalStateMachine::transition(&proposal, &decision, 2000).unwrap();
        assert_eq!(updated.status, ProposalStatus::Approved);
        assert!(updated.decision.is_some());
        assert_eq!(updated.decided_at, Some(2000));
        // UpdateProposal + SlashValidator
        assert_eq!(effects.len(), 2);
    }

    #[test]
    fn test_state_machine_reject() {
        let proposal = ProposalStateMachine::create([1u8; 32], sample_content(), 1000);
        let decision = sample_decision(false);
        let (updated, effects) = ProposalStateMachine::transition(&proposal, &decision, 2000).unwrap();
        assert_eq!(updated.status, ProposalStatus::Rejected);
        // Only UpdateProposal (no side-effects for rejected)
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn test_state_machine_transition_already_decided() {
        let mut proposal = ProposalStateMachine::create([1u8; 32], sample_content(), 1000);
        proposal.status = ProposalStatus::Approved;
        let decision = sample_decision(true);
        let result = ProposalStateMachine::transition(&proposal, &decision, 2000);
        assert!(matches!(result, Err(GovernanceError::InvalidStatus { .. })));
    }
}
