//! Setu Runtime - Simple State Transition Execution
//! 
//! This crate provides a simplified runtime environment to validate the core mechanisms of Setu before introducing the Move VM. It implements basic state transition functions, supporting:
//! - Transfer operations
//! - Balance queries
//! - Object ownership transfers
//! 
//! In the future, it can smoothly transition to the Move VM without affecting other components。

pub mod executor;
pub mod state;
pub mod transaction;
pub mod error;

pub use executor::{RuntimeExecutor, ExecutionContext, ExecutionOutput, StateChange, StateChangeType};
pub use state::{StateStore, InMemoryStateStore};
pub use transaction::{Transaction, TransactionType, TransferTx, QueryTx};
pub use error::{RuntimeError, RuntimeResult};
