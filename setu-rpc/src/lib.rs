//! Setu RPC - Network communication layer
//!
//! This module provides RPC interfaces for communication between:
//! - Router -> Solver
//! - Solver -> Validator
//! - Solver -> Router (registration)
//!
//! Uses Anemo for high-performance P2P RPC communication.

pub mod router;
pub mod solver;
pub mod validator;
pub mod error;

pub use error::{RpcError, Result};

