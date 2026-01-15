//! Setu RPC - Network communication layer
//!
//! This module provides RPC interfaces for communication between:
//! - Router -> Solver (transfer dispatch)
//! - Solver -> Validator (event submission)
//! - Solver -> Validator (registration)
//! - CLI -> Validator (registration commands)
//! - Wallet/DApp -> Validator (user queries and operations)
//!
//! Uses Anemo for high-performance P2P RPC communication.

pub mod router;
pub mod solver;
pub mod validator;
pub mod registration;
pub mod user;        // User RPC for wallet/DApp integration
pub mod error;
pub mod messages;

pub use error::{RpcError, Result};
pub use messages::*;
pub use registration::*;
pub use user::*;

