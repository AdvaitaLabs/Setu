//! Governance module — Agent subnet integration layer.
//!
//! Connects `setu-governance` pure logic to the Validator's DAG, HTTP API,
//! and async Agent subnet communication.

pub mod executor;
pub mod service;
pub mod handler;
