//! Effect executor — translates ProposalEffect + Decision into abstract GovernanceEffects.
//!
//! Returns abstract descriptions (not StateChanges). The integration layer
//! (GovernanceExecutor in setu-validator) materializes these into concrete
//! StateChanges with correct serialization (BCS for coins, JSON for metadata).

use setu_types::{GovernanceDecision, ProposalEffect, ProposalStatus, ResourceParamChange};

/// Abstract description of a governance side-effect.
///
/// The integration layer (setu-validator) converts these into `StateChange`
/// entries with proper serialization and `target_subnet` routing.
#[derive(Debug, Clone)]
pub enum GovernanceEffect {
    /// Update proposal status in GOVERNANCE subnet
    UpdateProposal {
        proposal_id: [u8; 32],
        new_status: ProposalStatus,
        decision: Option<GovernanceDecision>,
    },
    /// Slash a validator's coin (materializes to ROOT subnet StateChange)
    SlashValidator {
        validator_id: String,
        amount: u64,
    },
    /// Update a system parameter
    UpdateParameter {
        key: String,
        value: Vec<u8>,
    },
    /// Resolve a dispute
    ResolveDispute {
        dispute_id: [u8; 32],
        resolution: Vec<u8>,
    },
    /// Update a resource governance parameter
    UpdateResourceParam {
        change: ResourceParamChange,
    },
}

pub struct EffectExecutor;

impl EffectExecutor {
    /// Generate abstract effects from a proposal effect and decision.
    ///
    /// If rejected, only the proposal status update is emitted (no side-effects).
    pub fn execute(
        proposal_id: [u8; 32],
        effect: &ProposalEffect,
        decision: &GovernanceDecision,
    ) -> Vec<GovernanceEffect> {
        let new_status = if decision.approved {
            ProposalStatus::Approved
        } else {
            ProposalStatus::Rejected
        };

        let mut effects = vec![GovernanceEffect::UpdateProposal {
            proposal_id,
            new_status,
            decision: Some(decision.clone()),
        }];

        if decision.approved {
            match effect {
                ProposalEffect::SlashValidator { validator_id, amount } => {
                    effects.push(GovernanceEffect::SlashValidator {
                        validator_id: validator_id.clone(),
                        amount: *amount,
                    });
                }
                ProposalEffect::UpdateParameter { key, value } => {
                    effects.push(GovernanceEffect::UpdateParameter {
                        key: key.clone(),
                        value: value.clone(),
                    });
                }
                ProposalEffect::ResolveDispute { dispute_id, resolution } => {
                    effects.push(GovernanceEffect::ResolveDispute {
                        dispute_id: *dispute_id,
                        resolution: resolution.clone(),
                    });
                }
                ProposalEffect::UpdateResourceParam(change) => {
                    effects.push(GovernanceEffect::UpdateResourceParam {
                        change: change.clone(),
                    });
                }
            }
        }

        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(approved: bool) -> GovernanceDecision {
        GovernanceDecision {
            approved,
            reasoning: "test".into(),
            conditions: vec![],
        }
    }

    #[test]
    fn test_effect_executor_slash() {
        let effect = ProposalEffect::SlashValidator {
            validator_id: "v1".into(),
            amount: 100,
        };
        let effects = EffectExecutor::execute([1u8; 32], &effect, &decision(true));
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[0], GovernanceEffect::UpdateProposal { new_status: ProposalStatus::Approved, .. }));
        assert!(matches!(&effects[1], GovernanceEffect::SlashValidator { validator_id, amount: 100 } if validator_id == "v1"));
    }

    #[test]
    fn test_effect_executor_update_param() {
        let effect = ProposalEffect::UpdateParameter {
            key: "max_tps".into(),
            value: vec![1, 2, 3],
        };
        let effects = EffectExecutor::execute([2u8; 32], &effect, &decision(true));
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[1], GovernanceEffect::UpdateParameter { key, .. } if key == "max_tps"));
    }

    #[test]
    fn test_effect_executor_update_resource_param() {
        use setu_types::{ResourceGovernanceMode};
        let effect = ProposalEffect::UpdateResourceParam(
            ResourceParamChange::SetPowerCostPerEvent(5),
        );
        let effects = EffectExecutor::execute([4u8; 32], &effect, &decision(true));
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[1], GovernanceEffect::UpdateResourceParam { change }
            if matches!(change, ResourceParamChange::SetPowerCostPerEvent(5))));

        // Verify mode variant works too
        let mode_effect = ProposalEffect::UpdateResourceParam(
            ResourceParamChange::SetPowerMode(ResourceGovernanceMode::Enabled),
        );
        let mode_effects = EffectExecutor::execute([5u8; 32], &mode_effect, &decision(true));
        assert_eq!(mode_effects.len(), 2);
    }

    #[test]
    fn test_effect_executor_rejected_no_side_effects() {
        let effect = ProposalEffect::SlashValidator {
            validator_id: "v1".into(),
            amount: 100,
        };
        let effects = EffectExecutor::execute([3u8; 32], &effect, &decision(false));
        assert_eq!(effects.len(), 1); // Only UpdateProposal, no SlashValidator
        assert!(matches!(&effects[0], GovernanceEffect::UpdateProposal { new_status: ProposalStatus::Rejected, .. }));
    }
}
