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
        state_manager: Arc<setu_storage::SharedStateManager>,
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
        let selection = self.select_coins_for_transfer(&sender_coins, amount)?;

        // Auto-escalate: NeedMerge → MergeThenTransfer
        match selection {
            super::CoinSelectionResult::NeedMerge { target, sources } => {
                let recipient = setu_types::object::Address::normalize(&transfer.to);
                debug!(
                    transfer_id = %transfer.id,
                    target = %hex::encode(&target.object_id),
                    source_count = sources.len(),
                    "Auto-escalating to MergeThenTransfer"
                );
                return self.prepare_merge_then_transfer_task(
                    &target, &sources, recipient, amount, subnet_id,
                );
            }
            super::CoinSelectionResult::SingleCoin(ref selected_coin) => {
                debug!(
                    object_id = ?selected_coin.object_id,
                    coin_balance = selected_coin.balance,
                    subnet_id = %selected_coin.coin_type,
                    "Selected single coin for transfer"
                );
            }
        }

        // SingleCoin path
        let selected_coin = match selection {
            super::CoinSelectionResult::SingleCoin(c) => c,
            _ => unreachable!(), // NeedMerge returned above
        };
        
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
                format!("oid:{}", hex::encode(&selected_coin.object_id)),
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
    ) -> Result<(SolverTask, Vec<crate::coin_reservation::ReservationHandle>), TaskPrepareError> {
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

        // Step 2: Select coin(s) — may be single or NeedMerge
        let selection = self.select_coins_for_transfer(&sender_coins, amount)?;

        match selection {
            super::CoinSelectionResult::SingleCoin(_) => {
                // --- Single coin path: try to reserve ANY eligible coin ---
                // Collect all eligible coins (balance >= amount), sorted by balance ascending
                let mut eligible: Vec<_> = sender_coins.iter()
                    .filter(|c| c.balance >= amount)
                    .cloned()
                    .collect();
                eligible.sort_by(|a, b| a.balance.cmp(&b.balance)
                    .then_with(|| a.object_id.cmp(&b.object_id)));

                // Try each eligible coin until one reservation succeeds
                let (selected_coin, handle) = {
                    let mut reserved = None;
                    for coin in &eligible {
                        if let Some(h) = reservation_mgr
                            .try_reserve(&coin.object_id, amount, &transfer.id)
                        {
                            reserved = Some((coin.clone(), h));
                            break;
                        }
                        // This coin is already reserved, try the next one
                    }
                    reserved.ok_or_else(|| TaskPrepareError::AllCoinsReserved {
                        sender: transfer.from.clone(),
                        coin_count: eligible.len(),
                    })?
                };

                debug!(
                    object_id = ?selected_coin.object_id,
                    coin_balance = selected_coin.balance,
                    eligible_coins = eligible.len(),
                    "Selected and reserved single coin for transfer"
                );

                let resolved_coin = setu_types::task::ResolvedObject {
                    object_id: selected_coin.object_id,
                    object_type: "Coin".to_string(),
                    expected_version: selected_coin.version,
                };
                let resolved_inputs = setu_types::task::ResolvedInputs::transfer(resolved_coin.clone(), amount);

                let input_objects: Vec<&setu_types::ObjectId> = vec![&selected_coin.object_id];
                let parent_ids = self.derive_dependencies(&input_objects);

                let coin_data = self.state_provider.get_object(&selected_coin.object_id)
                    .ok_or(TaskPrepareError::ObjectNotFound(hex::encode(&selected_coin.object_id)))?;
                let merkle_proof = self.state_provider.get_merkle_proof(&selected_coin.object_id);
                let read_set = vec![
                    setu_types::task::ReadSetEntry::new(
                        format!("oid:{}", hex::encode(&selected_coin.object_id)),
                        coin_data,
                    ).with_proof(
                        merkle_proof
                            .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                            .unwrap_or_default()
                    ),
                ];

                let event = self.create_event_from_transfer(transfer, parent_ids)?;
                let pre_state_root = self.state_provider.get_state_root();
                let task_id = SolverTask::generate_task_id(&event, &pre_state_root);

                let task = SolverTask::new(task_id, event, resolved_inputs, pre_state_root, subnet_id)
                    .with_read_set(read_set)
                    .with_gas_budget(setu_types::task::GasBudget::default());

                info!(
                    transfer_id = %transfer.id,
                    task_id = %hex::encode(&task_id[..8]),
                    reservation_id = %handle.reservation_id,
                    "SolverTask prepared with single-coin reservation"
                );

                Ok((task, vec![handle]))
            }

            super::CoinSelectionResult::NeedMerge { target, sources } => {
                // --- Multi-coin path: batch reserve all coins, then MergeThenTransfer ---
                let mut batch_items: Vec<(&setu_types::ObjectId, u64)> = Vec::with_capacity(1 + sources.len());
                batch_items.push((&target.object_id, target.balance));
                for s in &sources {
                    batch_items.push((&s.object_id, s.balance));
                }

                let handles = reservation_mgr
                    .try_reserve_batch(&batch_items, &transfer.id)
                    .ok_or_else(|| TaskPrepareError::AllCoinsReserved {
                        sender: transfer.from.clone(),
                        coin_count: 1 + sources.len(),
                    })?;

                debug!(
                    transfer_id = %transfer.id,
                    target = %hex::encode(&target.object_id),
                    source_count = sources.len(),
                    "Batch-reserved coins for MergeThenTransfer"
                );

                let recipient = setu_types::object::Address::normalize(&transfer.to);
                match self.prepare_merge_then_transfer_task(&target, &sources, recipient, amount, subnet_id) {
                    Ok(task) => Ok((task, handles)),
                    Err(e) => {
                        // Rollback reservations on task preparation failure
                        reservation_mgr.release_batch(&handles);
                        Err(e)
                    }
                }
            }
        }
    }
    
    /// Prepare a SolverTask for merging multiple coins into one.
    ///
    /// The target coin accumulates balances from all source coins.
    /// Source coins are deleted after merge.
    pub fn prepare_merge_task(
        &self,
        target_coin: &CoinInfo,
        source_coins: &[CoinInfo],
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        if source_coins.is_empty() {
            return Err(TaskPrepareError::InvalidInput(
                "Must provide at least one source coin to merge".into(),
            ));
        }

        let target_resolved = ResolvedObject {
            object_id: target_coin.object_id,
            object_type: "Coin".to_string(),
            expected_version: target_coin.version,
        };
        let source_resolved: Vec<ResolvedObject> = source_coins
            .iter()
            .map(|c| ResolvedObject {
                object_id: c.object_id,
                object_type: "Coin".to_string(),
                expected_version: c.version,
            })
            .collect();
        let resolved_inputs = ResolvedInputs::merge_coins(target_resolved, source_resolved);

        // Collect all coin ObjectIds for dependencies & read_set
        let mut all_ids: Vec<ObjectId> = vec![target_coin.object_id];
        all_ids.extend(source_coins.iter().map(|c| c.object_id));

        let input_refs: Vec<&ObjectId> = all_ids.iter().collect();
        let parent_ids = self.derive_dependencies(&input_refs);

        let read_set = self.build_read_set(&all_ids)?;

        let vlc_snapshot = self.generate_vlc_snapshot();
        let mut event = Event::new(
            EventType::CoinMerge,
            parent_ids,
            vlc_snapshot,
            self.validator_id.clone(),
        );
        event.payload = setu_types::event::EventPayload::CoinMerge {
            target_coin_id: hex::encode(&target_coin.object_id),
            source_coin_ids: source_coins.iter().map(|c| hex::encode(&c.object_id)).collect(),
        };

        let pre_state_root = self.state_provider.get_state_root();
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);

        let task = SolverTask::new(task_id, event, resolved_inputs, pre_state_root, subnet_id)
            .with_read_set(read_set)
            .with_gas_budget(GasBudget::default());

        info!(
            task_id = %hex::encode(&task_id[..8]),
            target = %hex::encode(&target_coin.object_id),
            source_count = source_coins.len(),
            "Merge SolverTask prepared"
        );
        Ok(task)
    }

    /// Prepare a SolverTask for splitting one coin into multiple.
    pub fn prepare_split_task(
        &self,
        source_coin: &CoinInfo,
        amounts: Vec<u64>,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        if amounts.is_empty() {
            return Err(TaskPrepareError::InvalidInput(
                "Split amounts cannot be empty".into(),
            ));
        }
        if amounts.iter().any(|&a| a == 0) {
            return Err(TaskPrepareError::InvalidInput(
                "Split amount must be > 0 (zero-balance coins are not allowed)".into(),
            ));
        }
        let total: u64 = amounts.iter()
            .try_fold(0u64, |acc, &a| acc.checked_add(a))
            .ok_or_else(|| TaskPrepareError::InvalidInput(
                "Split amounts overflow u64".into(),
            ))?;
        if total > source_coin.balance {
            return Err(TaskPrepareError::InsufficientBalance {
                required: total,
                available: source_coin.balance,
            });
        }

        let source_resolved = ResolvedObject {
            object_id: source_coin.object_id,
            object_type: "Coin".to_string(),
            expected_version: source_coin.version,
        };
        let resolved_inputs = ResolvedInputs::split_coin(source_resolved, amounts.clone());

        let input_refs: Vec<&ObjectId> = vec![&source_coin.object_id];
        let parent_ids = self.derive_dependencies(&input_refs);

        let read_set = self.build_read_set(&[source_coin.object_id])?;

        let vlc_snapshot = self.generate_vlc_snapshot();
        let mut event = Event::new(
            EventType::CoinSplit,
            parent_ids,
            vlc_snapshot,
            self.validator_id.clone(),
        );
        event.payload = setu_types::event::EventPayload::CoinSplit {
            source_coin_id: hex::encode(&source_coin.object_id),
            amounts,
        };

        let pre_state_root = self.state_provider.get_state_root();
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);

        let task = SolverTask::new(task_id, event, resolved_inputs, pre_state_root, subnet_id)
            .with_read_set(read_set)
            .with_gas_budget(GasBudget::default());

        info!(
            task_id = %hex::encode(&task_id[..8]),
            source = %hex::encode(&source_coin.object_id),
            "Split SolverTask prepared"
        );
        Ok(task)
    }

    /// Prepare a SolverTask for atomic merge-then-transfer.
    ///
    /// Merges source coins into target, then transfers `amount` to `recipient`.
    /// This is a compound operation executed atomically in TEE.
    pub fn prepare_merge_then_transfer_task(
        &self,
        target_coin: &CoinInfo,
        source_coins: &[CoinInfo],
        recipient: setu_types::object::Address,
        amount: u64,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        if source_coins.is_empty() {
            return Err(TaskPrepareError::InvalidInput(
                "Must provide at least one source coin to merge".into(),
            ));
        }

        // Verify merged balance will be sufficient
        let merged_balance: u64 = target_coin.balance
            + source_coins.iter().map(|c| c.balance).sum::<u64>();
        if merged_balance < amount {
            return Err(TaskPrepareError::InsufficientBalance {
                required: amount,
                available: merged_balance,
            });
        }

        let target_resolved = ResolvedObject {
            object_id: target_coin.object_id,
            object_type: "Coin".to_string(),
            expected_version: target_coin.version,
        };
        let source_resolved: Vec<ResolvedObject> = source_coins
            .iter()
            .map(|c| ResolvedObject {
                object_id: c.object_id,
                object_type: "Coin".to_string(),
                expected_version: c.version,
            })
            .collect();
        let resolved_inputs = ResolvedInputs::merge_then_transfer(
            target_resolved,
            source_resolved,
            recipient.clone(),
            amount,
        );

        let mut all_ids: Vec<ObjectId> = vec![target_coin.object_id];
        all_ids.extend(source_coins.iter().map(|c| c.object_id));

        let input_refs: Vec<&ObjectId> = all_ids.iter().collect();
        let parent_ids = self.derive_dependencies(&input_refs);

        let read_set = self.build_read_set(&all_ids)?;

        let vlc_snapshot = self.generate_vlc_snapshot();
        let mut event = Event::new(
            EventType::CoinMergeThenTransfer,
            parent_ids,
            vlc_snapshot,
            self.validator_id.clone(),
        );
        event.payload = setu_types::event::EventPayload::CoinMergeThenTransfer {
            target_coin_id: hex::encode(&target_coin.object_id),
            source_coin_ids: source_coins.iter().map(|c| hex::encode(&c.object_id)).collect(),
            recipient: recipient.to_string(),
            amount,
        };

        let pre_state_root = self.state_provider.get_state_root();
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);

        let task = SolverTask::new(task_id, event, resolved_inputs, pre_state_root, subnet_id)
            .with_read_set(read_set)
            .with_gas_budget(GasBudget::default());

        info!(
            task_id = %hex::encode(&task_id[..8]),
            target = %hex::encode(&target_coin.object_id),
            source_count = source_coins.len(),
            amount = amount,
            "MergeThenTransfer SolverTask prepared"
        );
        Ok(task)
    }

    /// Build read_set entries for a list of object IDs.
    fn build_read_set(
        &self,
        object_ids: &[ObjectId],
    ) -> Result<Vec<ReadSetEntry>, TaskPrepareError> {
        let mut read_set = Vec::with_capacity(object_ids.len());
        for oid in object_ids {
            let coin_data = self.state_provider.get_object(oid)
                .ok_or(TaskPrepareError::ObjectNotFound(hex::encode(oid)))?;
            let merkle_proof = self.state_provider.get_merkle_proof(oid);
            read_set.push(
                ReadSetEntry::new(
                    format!("oid:{}", hex::encode(oid)),
                    coin_data,
                ).with_proof(
                    merkle_proof
                        .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                        .unwrap_or_default()
                ),
            );
        }
        Ok(read_set)
    }

    /// Generate a VLC snapshot for non-transfer events.
    fn generate_vlc_snapshot(&self) -> VLCSnapshot {
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut snapshot = VLCSnapshot::for_node(self.validator_id.clone());
        snapshot.logical_time = now_nanos;
        snapshot.physical_time = now_nanos / 1_000_000;
        snapshot
    }
    
    /// Select the best coin for a transfer
    ///
    /// Strategy: Select the smallest coin that can cover the transfer amount
    pub(crate) fn select_coin_for_transfer(
        &self,
        coins: &[CoinInfo],
        amount: u64,
    ) -> Result<CoinInfo, TaskPrepareError> {
        match self.select_coins_for_transfer(coins, amount)? {
            super::CoinSelectionResult::SingleCoin(coin) => Ok(coin),
            super::CoinSelectionResult::NeedMerge { target, .. } => {
                // Legacy callers get the largest coin; they don't support auto-merge
                Ok(target)
            }
        }
    }

    /// Select coins for a transfer, potentially requiring merge.
    ///
    /// Returns `SingleCoin` when one coin covers the amount, or `NeedMerge`
    /// when multiple coins must be merged first.
    pub(crate) fn select_coins_for_transfer(
        &self,
        coins: &[CoinInfo],
        amount: u64,
    ) -> Result<super::CoinSelectionResult, TaskPrepareError> {
        if amount == 0 {
            return Err(TaskPrepareError::InvalidInput(
                "Transfer amount must be > 0".into(),
            ));
        }
        if coins.is_empty() {
            return Err(TaskPrepareError::NoCoinsFound("sender has no coins".to_string()));
        }

        // Strategy 1: find a single coin that covers the amount (smallest sufficient)
        let mut eligible: Vec<_> = coins
            .iter()
            .filter(|c| c.balance >= amount)
            .cloned()
            .collect();

        // Sort ascending by balance, then ObjectId tie-break (R12)
        eligible.sort_by(|a, b| {
            a.balance.cmp(&b.balance)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });

        if let Some(coin) = eligible.into_iter().next() {
            return Ok(super::CoinSelectionResult::SingleCoin(coin));
        }

        // Strategy 2: greedy merge (largest coins first until accumulated >= amount)
        let mut sorted: Vec<_> = coins.to_vec();
        // Sort descending by balance, then ascending ObjectId tie-break
        sorted.sort_by(|a, b| {
            b.balance.cmp(&a.balance)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });

        let mut selected = Vec::new();
        let mut accumulated = 0u64;

        for coin in sorted {
            selected.push(coin.clone());
            accumulated = accumulated.checked_add(coin.balance)
                .unwrap_or(u64::MAX);
            if accumulated >= amount {
                break;
            }
            if selected.len() >= super::MAX_MERGE_SOURCES + 1 {
                // +1 because first element becomes target, rest are sources
                break;
            }
        }

        if accumulated < amount {
            return Err(TaskPrepareError::InsufficientBalance {
                required: amount,
                available: accumulated,
            });
        }

        // First coin (largest) is the merge target, rest are sources
        let target = selected.remove(0);
        Ok(super::CoinSelectionResult::NeedMerge {
            target,
            sources: selected,
        })
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

    fn make_coin(id_byte: u8, balance: u64) -> CoinInfo {
        CoinInfo {
            object_id: ObjectId::new([id_byte; 32]),
            owner: "alice".to_string(),
            balance,
            version: 1,
            coin_type: "ROOT".to_string(),
        }
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
            make_coin(1, 500),
            make_coin(2, 200),
            make_coin(3, 1000),
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
        
        let coins = vec![make_coin(1, 50)];
        
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

    // ── NeedMerge coin selection tests ──

    #[test]
    fn test_select_coins_single_coin_sufficient() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let coins = vec![make_coin(1, 500), make_coin(2, 200)];

        match preparer.select_coins_for_transfer(&coins, 150).unwrap() {
            super::super::CoinSelectionResult::SingleCoin(c) => {
                assert_eq!(c.balance, 200, "should pick smallest sufficient coin");
            }
            _ => panic!("Expected SingleCoin"),
        }
    }

    #[test]
    fn test_select_coins_need_merge_two_coins() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        // No single coin covers 250; need to merge 200+100=300 ≥ 250
        let coins = vec![make_coin(1, 200), make_coin(2, 100), make_coin(3, 50)];

        match preparer.select_coins_for_transfer(&coins, 250).unwrap() {
            super::super::CoinSelectionResult::NeedMerge { target, sources } => {
                assert_eq!(target.balance, 200, "target should be largest coin");
                assert_eq!(sources.len(), 1, "only need one source");
                assert_eq!(sources[0].balance, 100);
            }
            _ => panic!("Expected NeedMerge"),
        }
    }

    #[test]
    fn test_select_coins_need_merge_all_coins() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let coins = vec![make_coin(1, 30), make_coin(2, 40), make_coin(3, 50)];

        match preparer.select_coins_for_transfer(&coins, 100).unwrap() {
            super::super::CoinSelectionResult::NeedMerge { target, sources } => {
                assert_eq!(target.balance, 50, "target should be largest coin");
                assert_eq!(sources.len(), 2, "need all remaining sources");
                let total: u64 = target.balance + sources.iter().map(|s| s.balance).sum::<u64>();
                assert!(total >= 100, "total should cover amount");
            }
            _ => panic!("Expected NeedMerge"),
        }
    }

    #[test]
    fn test_select_coins_insufficient_even_merged() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let coins = vec![make_coin(1, 30), make_coin(2, 40)];

        match preparer.select_coins_for_transfer(&coins, 200) {
            Err(TaskPrepareError::InsufficientBalance { required, available }) => {
                assert_eq!(required, 200);
                assert_eq!(available, 70);
            }
            other => panic!("Expected InsufficientBalance, got: {:?}", other.is_ok()),
        }
    }

    #[test]
    fn test_select_coins_empty() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let coins: Vec<CoinInfo> = vec![];

        assert!(preparer.select_coins_for_transfer(&coins, 100).is_err());
    }

    #[test]
    fn test_select_coins_deterministic_tie_break() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        // Two coins with same balance → ObjectId tie-break
        let coins = vec![make_coin(2, 100), make_coin(1, 100)];

        match preparer.select_coins_for_transfer(&coins, 100).unwrap() {
            super::super::CoinSelectionResult::SingleCoin(c) => {
                // Ascending ObjectId tie-break → coin with id_byte=1 should win
                assert_eq!(c.object_id, ObjectId::new([1u8; 32]));
            }
            _ => panic!("Expected SingleCoin"),
        }
    }

    #[test]
    fn test_need_merge_greedy_stops_early() {
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        // Need 150: greedy picks 80+60=140 < 150, so adds 50 → 190 ≥ 150
        let coins = vec![
            make_coin(1, 80),
            make_coin(2, 60),
            make_coin(3, 50),
            make_coin(4, 10),
        ];

        match preparer.select_coins_for_transfer(&coins, 150).unwrap() {
            super::super::CoinSelectionResult::NeedMerge { target, sources } => {
                assert_eq!(target.balance, 80);
                assert_eq!(sources.len(), 2); // 60+50
                assert_eq!(sources[0].balance, 60);
                assert_eq!(sources[1].balance, 50);
                // coin 4 (10) not included because accumulated already >= 150
            }
            _ => panic!("Expected NeedMerge"),
        }
    }
}
