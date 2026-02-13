//! PoCW economic calculations for FluxTransfer transactions
//!
//! Provides flux burn and power drain computation, gated by PoCWConfig::enabled.

pub mod flux_burn;
pub mod power;
pub mod processor;
pub mod fold_observer;
