//! Runtime error types

use thiserror::Error;
use setu_types::ObjectId;

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Object not found: {0}")]
    ObjectNotFound(ObjectId),
    
    #[error("Insufficient balance for {address}: required {required}, available {available}")]
    InsufficientBalance { address: String, required: u64, available: u64 },
    
    #[error("Invalid ownership: object {object_id} is not owned by {address}")]
    InvalidOwnership { object_id: ObjectId, address: String },
    
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),
    
    #[error("State error: {0}")]
    StateError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    #[error("Account frozen: {0}")]
    AccountFrozen(String),
    
    #[error("Unknown error: {0}")]
    Unknown(String),

    #[error("Move VM is not enabled")]
    VMNotEnabled,

    #[error("Move VM initialization error: {0}")]
    VMInitError(String),

    #[error("Move VM execution error: {0}")]
    VMExecutionError(String),

    // ── B6b Programmable Transaction Block errors ──────────────────────────
    // See docs/feat/move-vm-phase9-ptb-exec/design.md §4.

    /// PTB validation: an `Argument` refers to an out-of-range input/result index
    /// (e.g. `Argument::Result(5)` when only 3 commands have run).
    #[error("PTB argument out of bounds: {0}")]
    PtbArgumentOutOfBounds(String),

    /// PTB borrow-stack violation (§4.6): the same Argument slot was consumed twice.
    #[error("PTB argument already consumed: {0}")]
    PtbArgumentAlreadyConsumed(String),

    /// §4.8: the target/source Argument's TypeTag is not `Coin<T>`, or sources/target
    /// `T` mismatched, or a MoveCall-result slot (no tracked TypeTag) was passed to
    /// a Coin command.
    #[error("PTB invalid coin layout: {0}")]
    PtbInvalidCoinLayout(String),

    /// §2 non-goals + §5.2: B6b TransferObjects is Coin-only.
    #[error("PTB unsupported transfer type (B6b is Coin-only): {0}")]
    PtbUnsupportedTransferType(String),

    /// §5.1: parsing `Vec<String>` to `Vec<TypeTag>` failed.
    #[error("PTB invalid type tag: {0}")]
    PtbInvalidTypeTag(String),

    /// B6c · per-PTB gas budget exhausted. `used` carries the
    /// `instructions_executed()` value captured BEFORE the body bailed,
    /// so the wrapper can build a `success=false` output that still
    /// reports work done up to the failure point. Only this variant is
    /// translated into `Ok(MoveExecutionOutput { success: false, .. })`
    /// by `execute_ptb`; all other `RuntimeError` variants continue to
    /// propagate as `Err` so wire-validation / borrow-stack tests keep
    /// their existing contracts.
    #[error("PTB out of gas: used {used}")]
    OutOfGas { used: u64 },
}
