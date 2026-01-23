//! SolverTask Preparation Module (solver-tee3 Architecture)
//!
//! This module handles the preparation of SolverTask for Solver execution.
//! According to solver-tee3 design:
//!
//! - **Validator prepares everything**: coin selection, read_set, Merkle proofs
//! - **Solver is pass-through**: receives SolverTask, passes to TEE
//! - **TEE validates and executes**: verifies proofs, executes STF
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

use setu_types::task::{
    SolverTask, ResolvedInputs, ResolvedObject,
    GasBudget, ReadSetEntry, MerkleProof,
};
use setu_types::{Event, EventType, SubnetId, ObjectId};
use setu_types::event::VLCSnapshot;
use std::sync::Arc;
use tracing::{debug, info};

// Re-export StateProvider and CoinInfo from storage
pub use setu_storage::{StateProvider, CoinInfo, SimpleMerkleProof};

/// Convert SimpleMerkleProof to MerkleProof
#[allow(dead_code)]
fn to_enclave_proof(proof: &SimpleMerkleProof) -> MerkleProof {
    MerkleProof {
        siblings: proof.siblings.clone(),
        path_bits: proof.path_bits.clone(),
        leaf_index: Some(0),
    }
}

/// SolverTask preparer
///
/// Prepares SolverTask from Transfer requests by:
/// 1. Selecting coins for sender
/// 2. Building object references
/// 3. Creating read_set with proofs
/// 4. Generating task_id
pub struct TaskPreparer {
    validator_id: String,
    state_provider: Arc<dyn StateProvider>,
}

impl TaskPreparer {
    pub fn new(validator_id: String, state_provider: Arc<dyn StateProvider>) -> Self {
        Self {
            validator_id,
            state_provider,
        }
    }
    
    /// Create a TaskPreparer with pre-initialized test accounts
    /// 
    /// This creates a real `MerkleStateProvider` backed by `GlobalStateManager`
    /// with some test coins initialized.
    /// 
    /// ## Initialized accounts:
    /// - `alice`: 10,000,000 balance
    /// - `bob`: 5,000,000 balance  
    /// - `charlie`: 1,000,000 balance
    /// 
    /// ## Example
    /// 
    /// ```rust,ignore
    /// let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
    /// let task = preparer.prepare_transfer_task(&transfer)?;
    /// ```
    pub fn new_for_testing(validator_id: String) -> Self {
        use setu_storage::{GlobalStateManager, MerkleStateProvider, init_coin};
        use std::sync::RwLock;
        
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        
        // Initialize test accounts with real Merkle state
        {
            let mut manager = state_manager.write().unwrap();
            init_coin(&mut manager, "alice", 10_000_000);
            init_coin(&mut manager, "bob", 5_000_000);
            init_coin(&mut manager, "charlie", 1_000_000);
        }
        
        let state_provider = Arc::new(MerkleStateProvider::new(state_manager));
        Self::new(validator_id, state_provider)
    }
    
    /// Prepare a SolverTask from a Transfer request
    ///
    /// This is the main entry point for task preparation.
    /// Returns a fully prepared SolverTask ready for Solver execution.
    pub fn prepare_transfer_task(
        &self,
        transfer: &setu_types::Transfer,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        // Use default coin type (native SETU)
        self.prepare_transfer_task_with_coin_type(transfer, subnet_id, None)
    }
    
    /// Prepare a SolverTask from a Transfer request with specified coin type
    ///
    /// This variant allows specifying a coin type for multi-subnet scenarios
    /// where each subnet application may have its own token type.
    pub fn prepare_transfer_task_with_coin_type(
        &self,
        transfer: &setu_types::Transfer,
        subnet_id: SubnetId,
        coin_type: Option<&str>,
    ) -> Result<SolverTask, TaskPrepareError> {
        let amount = transfer.amount;
        
        debug!(
            transfer_id = %transfer.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = amount,
            coin_type = ?coin_type,
            "Preparing SolverTask for transfer"
        );
        
        // Step 1: Select coins for sender (filter by coin_type if specified)
        let sender_coins = match coin_type {
            Some(ct) => self.state_provider.get_coins_for_address_by_type(&transfer.from, ct),
            None => self.state_provider.get_coins_for_address(&transfer.from),
        };
        let selected_coin = self.select_coin_for_transfer(&sender_coins, amount)?;
        
        debug!(
            object_id = ?selected_coin.object_id,
            coin_balance = selected_coin.balance,
            coin_type = %selected_coin.coin_type,
            "Selected coin for transfer"
        );
        
        // Step 2: Build ResolvedObject and ResolvedInputs
        let resolved_coin = ResolvedObject {
            object_id: selected_coin.object_id,
            object_type: "Coin".to_string(),
            expected_version: selected_coin.version,
        };
        
        let resolved_inputs = ResolvedInputs::transfer(resolved_coin.clone(), amount);
        
        // Step 3: Derive event dependencies from input objects
        let input_objects: Vec<&ObjectId> = vec![&selected_coin.object_id];
        let parent_ids = self.derive_dependencies(&input_objects);
        
        // Step 4: Build read_set with Merkle proof
        // Pass raw storage data (CoinState) so TEE can verify Merkle proof
        // TEE is responsible for converting CoinState → Object<CoinData>
        let coin_data = self.state_provider.get_object(&selected_coin.object_id)
            .ok_or(TaskPrepareError::ObjectNotFound(hex::encode(&selected_coin.object_id)))?;
        
        let merkle_proof = self.state_provider.get_merkle_proof(&selected_coin.object_id);
        
        let read_set = vec![
            ReadSetEntry::new(
                format!("coin:{}", hex::encode(&selected_coin.object_id)),
                coin_data,
            ).with_proof(
                merkle_proof
                    .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                    .unwrap_or_default()
            ),
        ];
        
        // Step 5: Create Event from Transfer with derived dependencies
        let event = self.create_event_from_transfer(transfer, parent_ids)?;
        
        // Step 6: Get pre-state root
        let pre_state_root = self.state_provider.get_state_root();
        
        // Step 7: Generate task_id
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);
        
        // Step 8: Create SolverTask
        let task = SolverTask::new(
            task_id,
            event,
            resolved_inputs,
            pre_state_root,
            subnet_id,
        )
        .with_read_set(read_set)
        .with_gas_budget(GasBudget::default());
        
        info!(
            transfer_id = %transfer.id,
            task_id = %hex::encode(&task_id[..8]),
            "SolverTask prepared successfully"
        );
        
        Ok(task)
    }
    
    /// Select the best coin for a transfer
    ///
    /// Strategy: Select the smallest coin that can cover the transfer amount
    fn select_coin_for_transfer(
        &self,
        coins: &[CoinInfo],
        amount: u64,
    ) -> Result<CoinInfo, TaskPrepareError> {
        // Check if coins is empty first
        if coins.is_empty() {
            return Err(TaskPrepareError::NoCoinsFound("sender has no coins".to_string()));
        }
        
        // Filter coins with sufficient balance
        let mut eligible_coins: Vec<_> = coins
            .iter()
            .filter(|c| c.balance >= amount)
            .cloned()
            .collect();
        
        if eligible_coins.is_empty() {
            let total_balance: u64 = coins.iter().map(|c| c.balance).sum();
            return Err(TaskPrepareError::InsufficientBalance {
                required: amount,
                available: total_balance,
            });
        }
        
        // Sort by balance (ascending) to select smallest sufficient coin
        eligible_coins.sort_by_key(|c| c.balance);
        
        Ok(eligible_coins.remove(0))
    }
    
    /// Create Event from Transfer with derived parent IDs
    fn create_event_from_transfer(
        &self,
        transfer: &setu_types::Transfer,
        parent_ids: Vec<String>,
    ) -> Result<Event, TaskPrepareError> {
        let mut event = Event::new(
            EventType::Transfer,
            parent_ids,  // Dependencies derived from input objects
            VLCSnapshot::default(),
            self.validator_id.clone(),
        );
        
        // Attach transfer data (clone it)
        event = event.with_transfer(transfer.clone());
        
        Ok(event)
    }
    
    /// Derive event dependencies from input objects
    ///
    /// For each input object, find the last event that modified it.
    /// These events become the parent_ids (dependencies) of the new event.
    fn derive_dependencies(&self, input_objects: &[&ObjectId]) -> Vec<String> {
        let mut parent_ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        
        for object_id in input_objects {
            if let Some(event_id) = self.state_provider.get_last_modifying_event(object_id) {
                // Deduplicate: same event might have modified multiple objects
                if seen.insert(event_id.clone()) {
                    debug!(
                        object_id = %object_id,
                        parent_event = %event_id,
                        "Found dependency from input object"
                    );
                    parent_ids.push(event_id);
                }
            }
        }
        
        debug!(
            input_count = input_objects.len(),
            dependency_count = parent_ids.len(),
            "Derived event dependencies from input objects"
        );
        
        parent_ids
    }
}

/// Errors during task preparation
#[derive(Debug, thiserror::Error)]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Transfer, TransferType};
    use setu_types::task::OperationType;
    
    fn create_test_transfer() -> Transfer {
        Transfer::new("test-tx-1", "alice", "bob", 100)
            .with_type(TransferType::FluxTransfer)
            .with_power(10)
    }
    
    #[test]
    fn test_prepare_transfer_task() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let transfer = create_test_transfer();
        
        let task = preparer.prepare_transfer_task(&transfer, SubnetId::ROOT);
        
        assert!(task.is_ok());
        let task = task.unwrap();
        
        // Check task_id is not empty
        assert_ne!(task.task_id, [0u8; 32]);
        
        // Check resolved_inputs has correct operation
        match &task.resolved_inputs.operation {
            OperationType::Transfer { amount, .. } => {
                assert_eq!(*amount, 100);
            }
            _ => panic!("Expected Transfer operation"),
        }
        
        // Check read_set is not empty
        assert!(!task.read_set.is_empty());
    }
    
    #[test]
    fn test_select_smallest_sufficient_coin() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        
        let coins = vec![
            CoinInfo {
                object_id: ObjectId::new([1u8; 32]),
                owner: "alice".to_string(),
                balance: 500,
                version: 1,
                coin_type: "SETU".to_string(),
            },
            CoinInfo {
                object_id: ObjectId::new([2u8; 32]),
                owner: "alice".to_string(),
                balance: 200,
                version: 1,
                coin_type: "SETU".to_string(),
            },
            CoinInfo {
                object_id: ObjectId::new([3u8; 32]),
                owner: "alice".to_string(),
                balance: 1000,
                version: 1,
                coin_type: "SETU".to_string(),
            },
        ];
        
        // For amount 150, should select coin with balance 200 (smallest sufficient)
        let selected = preparer.select_coin_for_transfer(&coins, 150);
        assert!(selected.is_ok());
        assert_eq!(selected.unwrap().balance, 200);
        
        // For amount 300, should select coin with balance 500
        let selected = preparer.select_coin_for_transfer(&coins, 300);
        assert!(selected.is_ok());
        assert_eq!(selected.unwrap().balance, 500);
    }
    
    #[test]
    fn test_insufficient_balance() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        
        let coins = vec![
            CoinInfo {
                object_id: ObjectId::new([1u8; 32]),
                owner: "alice".to_string(),
                balance: 50,
                version: 1,
                coin_type: "SETU".to_string(),
            },
        ];
        
        let result = preparer.select_coin_for_transfer(&coins, 100);
        assert!(result.is_err());
        
        match result {
            Err(TaskPrepareError::InsufficientBalance { required, available }) => {
                assert_eq!(required, 100);
                assert_eq!(available, 50);
            }
            _ => panic!("Expected InsufficientBalance error"),
        }
    }
}
