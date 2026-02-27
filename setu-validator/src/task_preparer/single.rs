//! Single-transfer task preparation
//!
//! Use [`TaskPreparer`] for preparing individual SolverTasks.
//! For batch operations, see [`super::BatchTaskPreparer`].

use setu_types::task::{
    SolverTask, ResolvedInputs, ResolvedObject,
    GasBudget, ReadSetEntry,
};
use setu_types::{Event, EventType, SubnetId, ObjectId};
use setu_types::event::VLCSnapshot;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{TaskPrepareError, CoinInfo, StateProvider};

/// SolverTask preparer for single transfers
///
/// Prepares SolverTask from Transfer requests by:
/// 1. Selecting coins for sender
/// 2. Building object references
/// 3. Creating read_set with proofs
/// 4. Generating task_id
///
/// ## Example
///
/// ```rust,ignore
/// let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
/// let task = preparer.prepare_transfer_task(&transfer, SubnetId::ROOT)?;
/// ```
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
    
    /// Get the underlying state provider
    /// 
    /// This is used to share the state provider with BatchTaskPreparer.
    #[allow(dead_code)]
    pub fn state_provider(&self) -> &Arc<dyn StateProvider> {
        &self.state_provider
    }
    
    /// Get the validator ID
    #[allow(dead_code)]
    pub fn validator_id(&self) -> &str {
        &self.validator_id
    }
    
    /// Create a TaskPreparer with pre-initialized test accounts
    /// 
    /// This creates a real `MerkleStateProvider` backed by `GlobalStateManager`
    /// with some test coins initialized.
    /// 
    /// ## Initialized accounts (20 total):
    /// - `alice`, `bob`, `charlie`: 10,000,000 balance each
    /// - `user_01` to `user_17`: 5,000,000 balance each
    /// 
    /// ## Example
    /// 
    /// ```rust,ignore
    /// let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
    /// let task = preparer.prepare_transfer_task(&transfer, SubnetId::ROOT)?;
    /// ```
    pub fn new_for_testing(validator_id: String) -> Self {
        // Use shared test utility to avoid code duplication
        let state_provider = super::create_test_state_provider();
        
        // For testing: we don't need to rebuild index since we just created fresh state
        // For production with persisted state, use new_with_state_manager() which calls rebuild
        
        Self::new(validator_id, state_provider)
    }
    
    /// Create a TaskPreparer from an existing GlobalStateManager (production use)
    /// 
    /// This is used when loading state from persistent storage (RocksDB).
    /// It will rebuild the coin_type_index from the Merkle Tree state.
    /// 
    /// ## Performance
    /// - Rebuild time: ~1 second per 1M objects
    /// - Only needs to run once at startup
    pub fn new_with_state_manager(
        validator_id: String,
        state_manager: Arc<std::sync::RwLock<setu_storage::GlobalStateManager>>,
    ) -> Self {
        use setu_storage::MerkleStateProvider;
        
        let merkle_provider = MerkleStateProvider::new(state_manager);
        
        // Rebuild coin_type_index from persisted Merkle Tree state
        // This is critical for multi-token support after restart
        let indexed_count = merkle_provider.rebuild_coin_type_index();
        tracing::info!(
            indexed_count = indexed_count,
            "Rebuilt coin_type_index from Merkle Tree at startup"
        );
        
        let state_provider = Arc::new(merkle_provider);
        Self::new(validator_id, state_provider)
    }
    
    /// Prepare a SolverTask from a Transfer request
    ///
    /// This is the main entry point for task preparation.
    /// Returns a fully prepared SolverTask ready for Solver execution.
    /// 
    /// The coin is selected based on the subnet_id (1 subnet : 1 token binding).
    /// - If `subnet_id` is ROOT, uses ROOT subnet's native token
    /// - Otherwise, uses the subnet's native token
    pub fn prepare_transfer_task(
        &self,
        transfer: &setu_types::Transfer,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        let amount = transfer.amount;
        
        // Use subnet_id as the coin namespace (1:1 binding)
        // For ROOT subnet, use "ROOT" as the identifier
        let subnet_id_str = if subnet_id == SubnetId::ROOT {
            "ROOT".to_string()
        } else {
            subnet_id.to_string()
        };
        
        debug!(
            transfer_id = %transfer.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = amount,
            subnet_id = %subnet_id_str,
            "Preparing SolverTask for transfer"
        );
        
        // Step 1: Select coins for sender filtered by subnet_id
        let sender_coins = self.state_provider.get_coins_for_address_by_type(
            &transfer.from,
            &subnet_id_str,
        );
        let selected_coin = self.select_coin_for_transfer(&sender_coins, amount)?;
        
        debug!(
            object_id = ?selected_coin.object_id,
            coin_balance = selected_coin.balance,
            subnet_id = %selected_coin.coin_type,
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

    /// Prepare a SolverTask with coin reservation to prevent double-spend
    ///
    /// This method is similar to `prepare_transfer_task`, but also reserves the selected
    /// coin using CoinReservationManager. This prevents concurrent single/batch requests
    /// from using the same coin.
    ///
    /// # Returns
    /// - `Ok((task, handle))`: Task and reservation handle (must be released after TEE completion)
    /// - `Err(error)`: Preparation failed (no reservation made)
    ///
    /// # Example
    /// ```rust,ignore
    /// let (task, handle) = preparer.prepare_transfer_task_with_reservation(
    ///     &transfer, SubnetId::ROOT, &reservation_mgr
    /// )?;
    /// // ... execute TEE task ...
    /// reservation_mgr.release(&handle);
    /// ```
    pub fn prepare_transfer_task_with_reservation(
        &self,
        transfer: &setu_types::Transfer,
        subnet_id: SubnetId,
        reservation_mgr: &crate::coin_reservation::CoinReservationManager,
    ) -> Result<(SolverTask, crate::coin_reservation::ReservationHandle), TaskPrepareError> {
        let amount = transfer.amount;
        
        // Use subnet_id as the coin namespace (1:1 binding)
        let subnet_id_str = if subnet_id == SubnetId::ROOT {
            "ROOT".to_string()
        } else {
            subnet_id.to_string()
        };
        
        debug!(
            transfer_id = %transfer.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = amount,
            subnet_id = %subnet_id_str,
            "Preparing SolverTask with reservation"
        );
        
        // Step 1: Get all coins for sender filtered by subnet_id
        let sender_coins = self.state_provider.get_coins_for_address_by_type(
            &transfer.from,
            &subnet_id_str,
        );
        
        if sender_coins.is_empty() {
            return Err(TaskPrepareError::NoCoinsFound(
                format!("sender {} has no coins in subnet {}", transfer.from, subnet_id_str)
            ));
        }
        
        // Step 2: Filter eligible coins and sort by balance (ascending)
        let mut eligible_coins: Vec<_> = sender_coins
            .iter()
            .filter(|c| c.balance >= amount)
            .cloned()
            .collect();
        
        if eligible_coins.is_empty() {
            let total_balance: u64 = sender_coins.iter().map(|c| c.balance).sum();
            return Err(TaskPrepareError::InsufficientBalance {
                required: amount,
                available: total_balance,
            });
        }
        
        eligible_coins.sort_by_key(|c| c.balance);
        
        // Step 3: Try to reserve each eligible coin until one succeeds
        let mut selected_coin: Option<CoinInfo> = None;
        let mut reservation_handle: Option<crate::coin_reservation::ReservationHandle> = None;
        
        for coin in &eligible_coins {
            if let Some(handle) = reservation_mgr.try_reserve(
                &coin.object_id,
                amount,
                &transfer.id,
            ) {
                selected_coin = Some(coin.clone());
                reservation_handle = Some(handle);
                break;
            }
            // Coin already reserved by another transfer, try next one
            debug!(
                object_id = %hex::encode(&coin.object_id),
                "Coin already reserved, trying next"
            );
        }
        
        let selected_coin = selected_coin.ok_or_else(|| {
            TaskPrepareError::AllCoinsReserved {
                sender: transfer.from.clone(),
                coin_count: eligible_coins.len(),
            }
        })?;
        
        let reservation_handle = reservation_handle.expect("handle must exist if coin is selected");
        
        debug!(
            object_id = ?selected_coin.object_id,
            coin_balance = selected_coin.balance,
            "Selected and reserved coin for transfer"
        );
        
        // Step 4: Build ResolvedObject and ResolvedInputs (same as non-reservation path)
        let resolved_coin = setu_types::task::ResolvedObject {
            object_id: selected_coin.object_id,
            object_type: "Coin".to_string(),
            expected_version: selected_coin.version,
        };
        
        let resolved_inputs = setu_types::task::ResolvedInputs::transfer(resolved_coin.clone(), amount);
        
        // Step 5: Derive event dependencies
        let input_objects: Vec<&setu_types::ObjectId> = vec![&selected_coin.object_id];
        let parent_ids = self.derive_dependencies(&input_objects);
        
        // Step 6: Build read_set with Merkle proof
        let coin_data = self.state_provider.get_object(&selected_coin.object_id)
            .ok_or(TaskPrepareError::ObjectNotFound(hex::encode(&selected_coin.object_id)))?;
        
        let merkle_proof = self.state_provider.get_merkle_proof(&selected_coin.object_id);
        
        let read_set = vec![
            setu_types::task::ReadSetEntry::new(
                format!("coin:{}", hex::encode(&selected_coin.object_id)),
                coin_data,
            ).with_proof(
                merkle_proof
                    .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                    .unwrap_or_default()
            ),
        ];
        
        // Step 7: Create Event
        let event = self.create_event_from_transfer(transfer, parent_ids)?;
        
        // Step 8: Get pre-state root
        let pre_state_root = self.state_provider.get_state_root();
        
        // Step 9: Generate task_id and create SolverTask
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);
        
        let task = SolverTask::new(
            task_id,
            event,
            resolved_inputs,
            pre_state_root,
            subnet_id,
        )
        .with_read_set(read_set)
        .with_gas_budget(setu_types::task::GasBudget::default());
        
        info!(
            transfer_id = %transfer.id,
            task_id = %hex::encode(&task_id[..8]),
            reservation_id = %reservation_handle.reservation_id,
            "SolverTask prepared with reservation"
        );
        
        Ok((task, reservation_handle))
    }
    
    /// Select the best coin for a transfer
    ///
    /// Strategy: Select the smallest coin that can cover the transfer amount
    pub(crate) fn select_coin_for_transfer(
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
        // Use the VLC assigned by Validator (from transfer) to ensure unique event_id
        // If no assigned_vlc, fall back to timestamp-based VLC (but this shouldn't happen in production)
        let vlc_snapshot = match &transfer.assigned_vlc {
            Some(vlc) => {
                // Create VLCSnapshot with proper vector clock for the assigning validator
                let mut snapshot = VLCSnapshot::for_node(vlc.validator_id.clone());
                snapshot.logical_time = vlc.logical_time;
                snapshot.physical_time = vlc.physical_time;
                snapshot
            },
            None => {
                // Fallback: use current timestamp to ensure uniqueness
                // This is safer than default() which would use logical_time=0
                let now_nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                warn!(
                    transfer_id = %transfer.id,
                    fallback_logical_time = now_nanos,
                    "Transfer missing assigned_vlc, using timestamp-based fallback"
                );
                let mut snapshot = VLCSnapshot::for_node(self.validator_id.clone());
                snapshot.logical_time = now_nanos;
                snapshot.physical_time = now_nanos / 1_000_000; // Convert to milliseconds
                snapshot
            }
        };
        
        let mut event = Event::new(
            EventType::Transfer,
            parent_ids,  // Dependencies derived from input objects
            vlc_snapshot,
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
        
        // coin_type now represents subnet_id (ROOT = root subnet)
        let coins = vec![
            CoinInfo {
                object_id: ObjectId::new([1u8; 32]),
                owner: "alice".to_string(),
                balance: 500,
                version: 1,
                coin_type: "ROOT".to_string(),
            },
            CoinInfo {
                object_id: ObjectId::new([2u8; 32]),
                owner: "alice".to_string(),
                balance: 200,
                version: 1,
                coin_type: "ROOT".to_string(),
            },
            CoinInfo {
                object_id: ObjectId::new([3u8; 32]),
                owner: "alice".to_string(),
                balance: 1000,
                version: 1,
                coin_type: "ROOT".to_string(),
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
                coin_type: "ROOT".to_string(),  // subnet_id
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
