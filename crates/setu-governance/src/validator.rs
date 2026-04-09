//! Proposal validation — pure functions, state passed in by caller.

use setu_types::{GovernanceProposal, GovernanceDecision, ProposalContent, ProposalStatus};
use crate::error::GovernanceError;

pub struct ProposalValidator;

impl ProposalValidator {
    /// Validate a new proposal submission.
    ///
    /// `existing` is looked up by the caller from the GOVERNANCE SMT
    /// (this crate has no storage access).
    pub fn validate_propose(
        content: &ProposalContent,
        existing: Option<&GovernanceProposal>,
    ) -> Result<(), GovernanceError> {
        if existing.is_some() {
            return Err(GovernanceError::DuplicateProposal);
        }
        Self::validate_content(content)?;
        Ok(())
    }

    /// Validate an execute (decision) action against a proposal.
    pub fn validate_execute(
        proposal: Option<&GovernanceProposal>,
        _decision: &GovernanceDecision,
    ) -> Result<(), GovernanceError> {
        let proposal = proposal.ok_or(GovernanceError::ProposalNotFound)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(GovernanceError::InvalidStatus {
                expected: ProposalStatus::Pending,
                actual: proposal.status.clone(),
            });
        }
        Ok(())
    }

    fn validate_content(content: &ProposalContent) -> Result<(), GovernanceError> {
        if content.title.is_empty() {
            return Err(GovernanceError::InvalidContent("title cannot be empty".into()));
        }
        if content.proposer.is_empty() {
            return Err(GovernanceError::InvalidContent("proposer cannot be empty".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{ProposalType, ProposalEffect};

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

    fn sample_proposal(status: ProposalStatus) -> GovernanceProposal {
        GovernanceProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            status,
            decision: None,
            created_at: 1000,
            decided_at: None,
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
    fn test_validate_propose_ok() {
        let result = ProposalValidator::validate_propose(&sample_content(), None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_propose_duplicate() {
        let existing = sample_proposal(ProposalStatus::Pending);
        let result = ProposalValidator::validate_propose(&sample_content(), Some(&existing));
        assert!(matches!(result, Err(GovernanceError::DuplicateProposal)));
    }

    #[test]
    fn test_validate_propose_empty_title() {
        let mut content = sample_content();
        content.title = String::new();
        let result = ProposalValidator::validate_propose(&content, None);
        assert!(matches!(result, Err(GovernanceError::InvalidContent(_))));
    }

    #[test]
    fn test_validate_execute_ok() {
        let proposal = sample_proposal(ProposalStatus::Pending);
        let decision = sample_decision(true);
        let result = ProposalValidator::validate_execute(Some(&proposal), &decision);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_execute_not_pending() {
        let proposal = sample_proposal(ProposalStatus::Approved);
        let decision = sample_decision(true);
        let result = ProposalValidator::validate_execute(Some(&proposal), &decision);
        assert!(matches!(result, Err(GovernanceError::InvalidStatus { .. })));
    }

    #[test]
    fn test_validate_execute_not_found() {
        let decision = sample_decision(true);
        let result = ProposalValidator::validate_execute(None, &decision);
        assert!(matches!(result, Err(GovernanceError::ProposalNotFound)));
    }
}
