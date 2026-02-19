//! Network service module
//!
//! This module is organized for maintainability:
//! - `service.rs` - Core ValidatorNetworkService struct and management
//! - `transfer_handler.rs` - Transfer submission and routing
//! - `tee_executor.rs` - Parallel TEE execution (performance critical)
//! - `event_handler.rs` - Event processing, verification, DAG, state queries
//! - `types.rs` - Shared types and utilities
//! - `registration.rs` - Registration handler implementation
//! - `solver_client.rs` - Solver HTTP client types

mod event_handler;
mod registration;
mod service;
mod solver_client;
mod tee_executor;
mod transfer_handler;
mod types;

pub use registration::ValidatorRegistrationHandler;
pub use service::*;
pub use types::*;

// Internal modules - not re-exported as they are implementation details
// pub use solver_client::*;
// pub use tee_executor::TeeExecutor;
// pub use transfer_handler::TransferHandler;
// pub use event_handler::EventHandler;
