//! Governance proposal logic — pure rules, no I/O, no storage.
//!
//! This crate depends on `setu-types` only and is consumed by both
//! `setu-validator` (integration layer) and `setu-solver` (future).

pub mod error;
pub mod validator;
pub mod effect;
pub mod state_machine;

pub use error::GovernanceError;
pub use validator::ProposalValidator;
pub use effect::{EffectExecutor, GovernanceEffect};
pub use state_machine::ProposalStateMachine;
