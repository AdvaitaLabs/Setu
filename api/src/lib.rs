//! Setu HTTP API Layer
//!
//! This module provides HTTP API handlers for the Setu Validator.
//! It acts as the interface between external clients (wallets, DApps, CLI)
//! and the core validator logic.

pub mod handlers;
pub mod types;

pub use handlers::*;
pub use types::*;

