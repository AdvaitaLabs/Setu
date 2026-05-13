//! SolverTask Preparation Module (solver-tee3 Architecture)
//!
//! This module handles the preparation of SolverTask for Solver execution.
//! According to solver-tee3 design:
//!
//! - **Validator prepares everything**: coin selection, read_set, Merkle proofs
//! - **Solver is pass-through**: receives SolverTask, passes to TEE
//! - **TEE validates and executes**: verifies proofs, executes STF
//!
//! ## Components
//!
//! - [`TaskPreparer`]: Single-transfer task preparation
//! - [`BatchTaskPreparer`]: Optimized batch preparation (recommended for high throughput)
//!
//! ## BatchTaskPreparer Optimization
//!
//! | Metric | Before (per-tx) | After (batch) | Improvement |
//! |--------|-----------------|---------------|-------------|
//! | Lock acquisitions | 5-6N | 2 | ~99.6% |
//! | state_root calc | N | 1 | ~99.9% |
//!
//! ## Flow
//!
//! ```text
//! User Request (Transfer)
//!       │
//!       ▼
//! Validator.prepare_solver_task()
//!       │
//!       ├── 1. Convert Transfer to Event (account model)
//!       ├── 2. Select coins for sender (object model)
//!       ├── 3. Build ResolvedInputs with object references
//!       ├── 4. Build read_set with Merkle proofs
//!       ├── 5. Generate task_id for Attestation binding
//!       └── 6. Create SolverTask
//!       │
//!       ▼
//! SolverTask → Solver → TEE
//! ```
//!
//! ## StateProvider
//!
//! The `StateProvider` trait and `MerkleStateProvider` implementation are
//! defined in `setu_storage::state_provider`. Use `TaskPreparer::new_for_testing()`
//! for tests, which creates a real MerkleStateProvider with pre-initialized accounts.

mod single;
mod batch;

// Re-export main types
pub use single::TaskPreparer;
pub use batch::{BatchTaskPreparer, BatchPrepareResult, BatchPrepareStats};

// Re-export shared types from storage
pub use setu_storage::{StateProvider, CoinInfo, SimpleMerkleProof, BatchStateSnapshot, BatchSnapshotStats};

use setu_types::task::MerkleProof;

/// Maximum merge sources mirroring runtime's MAX_MERGE_SOURCES.
pub const MAX_MERGE_SOURCES: usize = 50;

/// Result of coin selection for a transfer.
///
/// When no single coin suffices, `NeedMerge` instructs the caller to
/// issue a `MergeThenTransfer` compound operation.
#[derive(Debug, Clone)]
pub enum CoinSelectionResult {
    /// A single coin covers the required amount.
    SingleCoin(CoinInfo),
    /// No single coin is large enough; merge these coins first.
    NeedMerge {
        /// Coin that will absorb merged balances (largest balance, deterministic tie-break).
        target: CoinInfo,
        /// Source coins to merge into target.
        sources: Vec<CoinInfo>,
    },
}

/// Errors during task preparation
#[derive(Debug, thiserror::Error, Clone)]
pub enum TaskPrepareError {
    #[error("Insufficient balance: required {required}, available {available}")]
    InsufficientBalance { required: u64, available: u64 },
    
    #[error("No coins found for address {0}")]
    NoCoinsFound(String),
    
    #[error("Object not found: {0}")]
    ObjectNotFound(String),
    
    #[error("Failed to create event: {0}")]
    EventCreationFailed(String),
    
    #[error("Merkle proof not available for object {0}")]
    MerkleProofNotAvailable(String),
    
    #[error("All {coin_count} coins for sender {sender} are currently reserved")]
    AllCoinsReserved { sender: String, coin_count: usize },
    
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Invalid module bytecode: {0}")]
    InvalidModule(String),

    #[error("Too many module dependencies (max {max}): found {found}")]
    TooManyDependencies { max: usize, found: usize },

    #[error("Shared objects not supported (ADR-1)")]
    SharedObjectNotSupported,

    #[error("Object {object_id} not owned by sender {sender}")]
    NotOwnedBySender { object_id: String, sender: String },

    /// Caller declared an `Immutable` object at `index` of `input_object_ids`
    /// but the same index appears in `mutable_indices`. Frozen objects
    /// must not be bound to `&mut T` Move parameters.
    /// See `docs/feat/fix-immutable-mutable-ref-not-blocked/design.md` §2.
    #[error("Object {object_id} is Immutable and cannot be passed as &mut at index {index}")]
    ImmutableObjectCannotBeMutated { object_id: String, index: usize },

    /// Caller declared an `Immutable` object at `index` of `input_object_ids`
    /// but the same index appears in `consumed_indices`. Frozen objects
    /// must not be consumed by-value.
    #[error("Object {object_id} is Immutable and cannot be consumed by-value at index {index}")]
    ImmutableObjectCannotBeConsumed { object_id: String, index: usize },

    /// Caller passed an `ObjectOwner`-owned object (i.e. a dynamic-field
    /// entry) directly through `input_object_ids`. DF entries must be
    /// accessed via `dynamic_field_accesses[]`, never as a raw input —
    /// otherwise the parent's authorisation is bypassed.
    /// See `docs/feat/fix-objectowner-mutable-ref-not-blocked/design.md` §2.
    #[error(
        "Object {object_id} is ObjectOwner(parent={parent_object_id}) at index {index}; \
         dynamic-field entries must be accessed via dynamic_field_accesses, \
         not input_object_ids"
    )]
    ObjectOwnerNotAllowedInInputs {
        object_id: String,
        parent_object_id: String,
        index: usize,
    },

    // ---- PWOO (Phase-1 Writable Owned Objects) errors ----
    /// Caller passed a shared object through `input_object_ids`. Shared objects
    /// must be declared via `shared_object_ids` so the preparer can mark them
    /// appropriately for concurrent access detection.
    #[error("Object {object_id} is Shared; pass it via shared_object_ids")]
    UseSharedObjectIdsInstead { object_id: String },

    /// Caller passed a non-Shared object through `shared_object_ids`.
    #[error("Object {object_id} is not Shared; pass it via input_object_ids")]
    NotShared { object_id: String },

    /// Same object id appears in both `input_object_ids` and `shared_object_ids`.
    #[error("Object {object_id} appears in both input_object_ids and shared_object_ids")]
    DuplicateObjectInLists { object_id: String },

    // ---- Dynamic Field (DF FDP M4) errors ----
    /// A declared DF's `parent_object_id` was not also declared in
    /// `input_object_ids` or `shared_object_ids`.
    #[error("Dynamic field parent {parent} not declared in input_object_ids or shared_object_ids")]
    DynamicFieldParentNotDeclared { parent: String },

    /// Read/Mutate/Delete declared against a DF that does not exist.
    #[error("Dynamic field not found: parent={parent}, key_type={key_type}")]
    DynamicFieldNotFound { parent: String, key_type: String },

    /// Create declared against a DF that already exists (early rejection
    /// so the TEE doesn't waste execution time only to abort).
    #[error("Dynamic field already exists: parent={parent}, key_type={key_type}")]
    DynamicFieldAlreadyExists { parent: String, key_type: String },

    /// The on-disk DF envelope's `ObjectOwner` does not match the declared
    /// parent (defence-in-depth against crafted `df_oid` → foreign DF).
    #[error("Dynamic field parent mismatch")]
    DynamicFieldParentMismatch,

    /// Mutate/Create/Delete declared on a DF whose parent is Immutable.
    #[error("Dynamic field on Immutable parent can only be accessed read-only")]
    DynamicFieldOnImmutableParent,

    /// Declared parent is itself a DF entry (ownership = ObjectOwner).
    /// Nested DF is out of scope for this phase.
    #[error("Dynamic field parent must be a root object (nested DF not supported)")]
    DynamicFieldParentNotRoot,

    /// Storage returned bytes that do not decode as any known envelope
    /// format (corrupted blob or unknown future format).
    #[error("Failed to decode envelope: {0}")]
    EnvelopeDecode(String),

    // ---- PTB event-wire errors (FDP move-vm-phase9-ptb-event-wire) ----
    /// PTB `ObjectArg::SharedObject` rejected — Phase-1 only supports
    /// owned/immutable inputs in PTB. See design.md §7 D5.
    #[error("Shared object {object_id} not yet supported in PTB (Phase-1 limitation)")]
    SharedObjectsNotYetSupported { object_id: String },

    /// PTB stale-read defense: client-supplied version does not match the
    /// on-chain envelope version. See design.md §7 D4.
    #[error("Object {object_id} version mismatch: expected {expected}, actual {actual}")]
    StaleObjectVersion {
        object_id: String,
        expected: u64,
        actual: u64,
    },

    /// PTB stale-read defense: blake3(envelope_bytes) does not match the
    /// client-supplied digest. See design.md §7 D4.
    #[error("Object {object_id} digest mismatch")]
    ObjectDigestMismatch { object_id: String },
}

/// Convert SimpleMerkleProof to MerkleProof (for TEE)
#[allow(dead_code)]
pub(crate) fn to_enclave_proof(proof: &SimpleMerkleProof) -> MerkleProof {
    MerkleProof {
        siblings: proof.siblings.clone(),
        path_bits: proof.path_bits.clone(),
        leaf_index: Some(0),
    }
}

// ============================================================================
// Shared Test Utilities
// ============================================================================

/// Create a MerkleStateProvider with pre-initialized seed accounts.
///
/// This is a shared utility function to avoid code duplication between
/// `TaskPreparer::new_for_testing()` and `BatchTaskPreparer::new_for_testing()`.
///
/// ## Initialized accounts (3 seed accounts):
/// - `alice`, `bob`, `charlie`: 1,000,000,000 balance each (1B tokens)
///
/// These seed accounts have high balances to support:
/// 1. Direct benchmark testing with 3 accounts
/// 2. Funding test accounts via transfers (benchmark --init-accounts)
///
/// ## Usage
///
/// ```rust,ignore
/// let state_provider = create_test_state_provider();
/// let preparer = TaskPreparer::new("validator-1".to_string(), state_provider);
/// ```
///
/// ## Note
/// This returns a shared Arc to a singleton MerkleStateProvider.
/// All callers will get the same state provider instance.
///
/// ## For High-Concurrency Testing
/// Use `setu-benchmark --init-accounts N` to create N test accounts by
/// transferring from these seed accounts. This decouples Validator from
/// benchmark-specific account requirements.
pub fn create_test_state_provider() -> std::sync::Arc<setu_storage::MerkleStateProvider> {
    use once_cell::sync::Lazy;
    use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider, init_coins_split};
    use std::sync::Arc;

    /// Number of coins per seed account in test state.
    ///
    /// Higher values allow more parallel init transfers from each seed,
    /// significantly accelerating `--init-accounts` benchmark setup.
    /// Each coin can be reserved independently, so N coins per seed means
    /// N concurrent outbound transfers from that seed.
    const SEED_COINS_PER_ACCOUNT: u32 = 5;

    // Singleton state provider - shared across all TaskPreparer and BatchTaskPreparer instances
    static TEST_STATE_PROVIDER: Lazy<Arc<MerkleStateProvider>> = Lazy::new(|| {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));

        // Initialize seed accounts with multiple coins for parallel init transfers.
        //
        // Design rationale:
        // - Validator only knows about seed accounts
        // - Benchmark creates test accounts dynamically via --init-accounts
        // - This decouples Validator from benchmark-specific requirements
        // - Pre-sharding seed accounts into N coins enables N concurrent
        //   outbound transfers per seed, dramatically speeding up init
        {
            let mut manager = shared.lock_write();
            
            // Seed accounts with 1B each, split into SEED_COINS_PER_ACCOUNT coins.
            // With 3 seeds × 5 coins = 15 concurrent init transfers at a time.
            init_coins_split(&mut manager, "alice", 1_000_000_000, SEED_COINS_PER_ACCOUNT, "ROOT");
            init_coins_split(&mut manager, "bob", 1_000_000_000, SEED_COINS_PER_ACCOUNT, "ROOT");
            init_coins_split(&mut manager, "charlie", 1_000_000_000, SEED_COINS_PER_ACCOUNT, "ROOT");
            shared.publish_snapshot(&manager);
        }

        tracing::info!(
            "Initialized shared test state provider with 3 seed accounts × {} coins each",
            SEED_COINS_PER_ACCOUNT,
        );
        Arc::new(MerkleStateProvider::new(shared))
    });

    Arc::clone(&TEST_STATE_PROVIDER)
}