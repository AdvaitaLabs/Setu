//! Setu Runtime - Simple State Transition Execution
//!
//! This crate provides a simplified runtime environment to validate the core mechanisms of Setu before introducing the Move VM. It implements basic state transition functions, supporting:
//! - Transfer operations
//! - Balance queries
//! - Object ownership transfers
//!
//! In the future, it can smoothly transition to the Move VM without affecting other components。

pub mod error;
pub mod executor;
pub mod state;
pub mod transaction;

pub use error::{RuntimeError, RuntimeResult};
pub use executor::{
    ExecutionContext, ExecutionOutput, RuntimeExecutor, StateChange, StateChangeType,
};
pub use state::{InMemoryStateStore, StateStore};
pub use transaction::{
    BinaryOp, CompareOp, Instruction, ProgramTx, ProgramValue, QueryTx, Transaction,
    TransactionType, TransferTx,
};
