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
pub mod setu_state_store;
pub mod state;
pub mod sui_vm;
pub mod transaction;
pub mod vm_object;

pub use error::{RuntimeError, RuntimeResult};
pub use executor::{
    ExecutionContext, ExecutionOutput, RuntimeExecutor, StateChange, StateChangeType,
};
pub use setu_state_store::SetuMerkleStateStore;
pub use state::{InMemoryStateStore, OverlayStateStore, StateStore};
pub use sui_vm::{
    compile_package_to_disassembly, execute_sui_entry_from_disassembly,
    execute_sui_entry_with_outcome, SuiVmArg, SuiVmExecutionOutcome, SuiVmWrite,
};
pub use transaction::{ProgramTx, QueryTx, Transaction, TransactionType, TransferTx};
pub use vm_object::{SuiVmStoredObject, SuiVmStoredValue};
