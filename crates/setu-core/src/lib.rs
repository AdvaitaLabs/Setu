//! Setu Core - Shared library for Validator and Solver
//!
//! This crate provides common functionality used by both
//! setu-validator and setu-solver.

pub mod config;
pub mod shard;

pub use config::NodeConfig;
pub use shard::{Shard, ShardId, ShardManager};
