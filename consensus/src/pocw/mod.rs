//! PoCW economic calculations for FluxTransfer and TaskSubmit events.
//!
//! Provides flux burn, power drain, and scoring computation,
//! gated by PoCWConfig::enabled.

pub mod emission;
pub mod flux_burn;
pub mod power;
pub mod processor;
pub mod scoring;
pub mod fold_observer;
