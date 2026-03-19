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