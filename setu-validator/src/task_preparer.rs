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

use setu_enclave::{
    SolverTask, ResolvedInputs, ResolvedObject,
    GasBudget, ReadSetEntry, MerkleProof,
};
use setu_types::{Event, EventType, SubnetId, Transfer as SetuTransfer, ObjectId};
use setu_types::event::VLCSnapshot;
use sha2::{Sha256, Digest};
use std::sync::Arc;
use tracing::{debug, info};

/// Coin data (placeholder - should match runtime's Coin struct)
#[derive(Debug, Clone)]
pub struct CoinInfo {
    pub object_id: ObjectId,
    pub owner: String,
    pub balance: u64,
    pub version: u64,
}

/// State provider trait for reading current state
/// 
/// In production, this would be backed by the Merkle store
pub trait StateProvider: Send + Sync {
    /// Get coins owned by an address
    fn get_coins_for_address(&self, address: &str) -> Vec<CoinInfo>;
    
    /// Get object by ID
    fn get_object(&self, object_id: &ObjectId) -> Option<Vec<u8>>;
    
    /// Get current state root
    fn get_state_root(&self) -> [u8; 32];
    
    /// Get Merkle proof for object
    fn get_merkle_proof(&self, object_id: &ObjectId) -> Option<MerkleProof>;
    
    /// Get the event ID that last modified this object
    ///
    /// This is used to derive event dependencies from input objects.
    /// Returns None for genesis objects or if tracking is not available.
    fn get_last_modifying_event(&self, object_id: &ObjectId) -> Option<String>;
}

/// Mock state provider for development
pub struct MockStateProvider {
    state_root: [u8; 32],
    default_balance: u64,
}

impl MockStateProvider {
    pub fn new() -> Self {
        Self {
            state_root: [0u8; 32],
            default_balance: 1_000_000,
        }
    }
    
    pub fn with_state_root(mut self, root: [u8; 32]) -> Self {
        self.state_root = root;
        self
    }
}

impl Default for MockStateProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StateProvider for MockStateProvider {
    fn get_coins_for_address(&self, address: &str) -> Vec<CoinInfo> {
        // Mock: return a single coin with default balance
        let mut hasher = Sha256::new();
        hasher.update(b"coin:");
        hasher.update(address.as_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        let object_id = ObjectId::new(hash);
        
        vec![CoinInfo {
            object_id,
            owner: address.to_string(),
            balance: self.default_balance,
            version: 1,
        }]
    }
    
    fn get_object(&self, _object_id: &ObjectId) -> Option<Vec<u8>> {
        // Mock: return empty object
        Some(vec![])
    }
    
    fn get_state_root(&self) -> [u8; 32] {
        self.state_root
    }
    
    fn get_merkle_proof(&self, _object_id: &ObjectId) -> Option<MerkleProof> {
        // Mock: return empty proof
        Some(MerkleProof {
            siblings: vec![],
            path_bits: vec![],
            leaf_index: Some(0),
        })
    }
    
    fn get_last_modifying_event(&self, _object_id: &ObjectId) -> Option<String> {
        // Mock: no event tracking, objects are genesis
        None
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
    
    /// Create with mock state provider
    pub fn new_mock(validator_id: String) -> Self {
        Self {
            validator_id,
            state_provider: Arc::new(MockStateProvider::new()),
        }
    }
    
    /// Prepare a SolverTask from a Transfer request
    ///
    /// This is the main entry point for task preparation.
    /// Returns a fully prepared SolverTask ready for Solver execution.
    pub fn prepare_transfer_task(
        &self,
        transfer: &core_types::Transfer,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        let amount = transfer.amount.unsigned_abs() as u64;
        
        debug!(
            transfer_id = %transfer.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = amount,
            "Preparing SolverTask for transfer"
        );
        
        // Step 1: Select coins for sender
        let sender_coins = self.state_provider.get_coins_for_address(&transfer.from);
        let selected_coin = self.select_coin_for_transfer(&sender_coins, amount)?;
        
        debug!(
            object_id = ?selected_coin.object_id,
            coin_balance = selected_coin.balance,
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
        let coin_data = self.state_provider.get_object(&selected_coin.object_id)
            .ok_or(TaskPrepareError::ObjectNotFound(hex::encode(&selected_coin.object_id)))?;
        
        let merkle_proof = self.state_provider.get_merkle_proof(&selected_coin.object_id);
        
        let read_set = vec![
            ReadSetEntry::new(
                format!("coin:{}", hex::encode(&selected_coin.object_id)),
                coin_data,
            ).with_proof(
                merkle_proof
                    .map(|p| bincode::serialize(&p).unwrap_or_default())
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
        transfer: &core_types::Transfer,
        parent_ids: Vec<String>,
    ) -> Result<Event, TaskPrepareError> {
        let mut event = Event::new(
            EventType::Transfer,
            parent_ids,  // Dependencies derived from input objects
            VLCSnapshot::default(),
            self.validator_id.clone(),
        );
        
        // Attach transfer data
        event = event.with_transfer(SetuTransfer {
            from: transfer.from.clone(),
            to: transfer.to.clone(),
            amount: transfer.amount.unsigned_abs() as u64,
        });
        
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
    use core_types::{Transfer, TransferType, Vlc};
    use setu_enclave::OperationType;
    
    fn create_test_transfer() -> Transfer {
        Transfer {
            id: "test-tx-1".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100,
            transfer_type: TransferType::FluxTransfer,
            resources: vec![],
            vlc: Vlc::new(),
            power: 10,
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
            assigned_vlc: None,
        }
    }
    
    #[test]
    fn test_prepare_transfer_task() {
        let preparer = TaskPreparer::new_mock("validator-1".to_string());
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
        let preparer = TaskPreparer::new_mock("validator-1".to_string());
        
        let coins = vec![
            CoinInfo {
                object_id: ObjectId::new([1u8; 32]),
                owner: "alice".to_string(),
                balance: 500,
                version: 1,
            },
            CoinInfo {
                object_id: ObjectId::new([2u8; 32]),
                owner: "alice".to_string(),
                balance: 200,
                version: 1,
            },
            CoinInfo {
                object_id: ObjectId::new([3u8; 32]),
                owner: "alice".to_string(),
                balance: 1000,
                version: 1,
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
        let preparer = TaskPreparer::new_mock("validator-1".to_string());
        
        let coins = vec![
            CoinInfo {
                object_id: ObjectId::new([1u8; 32]),
                owner: "alice".to_string(),
                balance: 50,
                version: 1,
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
