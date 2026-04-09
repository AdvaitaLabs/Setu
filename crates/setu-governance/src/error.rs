//! Governance error types.

use setu_types::ProposalStatus;

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    #[error("Duplicate proposal: proposal with this ID already exists")]
    DuplicateProposal,

    #[error("Proposal not found")]
    ProposalNotFound,

    #[error("Invalid proposal status: expected {expected:?}, got {actual:?}")]
    InvalidStatus {
        expected: ProposalStatus,
        actual: ProposalStatus,
    },

    #[error("Invalid proposal content: {0}")]
    InvalidContent(String),

    #[error("Effect execution failed: {0}")]
    EffectExecutionFailed(String),
}
