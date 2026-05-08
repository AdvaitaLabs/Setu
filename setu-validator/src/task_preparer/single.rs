//! Single-transfer task preparation
//!
//! Use [`TaskPreparer`] for preparing individual SolverTasks.
//! For batch operations, see [`super::BatchTaskPreparer`].

use setu_types::task::{
    SolverTask, ResolvedInputs, ResolvedObject,
    GasBudget, ReadSetEntry,
};
use setu_types::{Event, EventType, SubnetId, ObjectId};
use setu_types::{flux_state_object_id, power_state_object_id};
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
    /// ## Initialized accounts:
    /// - `alice`, `bob`, `charlie`: 1,000,000,000 each, split across 5 coins
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
        
        let mut read_set = vec![
            ReadSetEntry::new(
                format!("oid:{}", hex::encode(&selected_coin.object_id)),
                coin_data,
            ).with_proof(
                merkle_proof
                    .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                    .unwrap_or_default()
            ),
        ];
        
        // Add FluxState and PowerState for the sender (for Power/Flux in TEE)
        let flux_oid = flux_state_object_id(&transfer.from);
        let power_oid = power_state_object_id(&transfer.from);
        if let Some(flux_data) = self.state_provider.get_object(&flux_oid) {
            read_set.push(ReadSetEntry::new(
                format!("oid:{}", hex::encode(&flux_oid)),
                flux_data,
            ));
        }
        if let Some(power_data) = self.state_provider.get_object(&power_oid) {
            read_set.push(ReadSetEntry::new(
                format!("oid:{}", hex::encode(&power_oid)),
                power_data,
            ));
        }

        // Add ResourceParams from GOVERNANCE subnet (for parameterized Power/Flux in TEE)
        let rp_oid = setu_types::resource_params_object_id();
        if let Some(rp_data) = self.state_provider.get_object_from_subnet(
            &rp_oid,
            &setu_types::SubnetId::GOVERNANCE,
        ) {
            read_set.push(ReadSetEntry::new(
                format!("oid:{}", hex::encode(rp_oid.as_bytes())),
                rp_data,
            ));
        }
        
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
                let mut read_set = vec![
                    setu_types::task::ReadSetEntry::new(
                        format!("oid:{}", hex::encode(&selected_coin.object_id)),
                        coin_data,
                    ).with_proof(
                        merkle_proof
                            .map(|p| bcs::to_bytes(&p).unwrap_or_default())
                            .unwrap_or_default()
                    ),
                ];
                
                // Add FluxState and PowerState for the sender
                let flux_oid = flux_state_object_id(&transfer.from);
                let power_oid = power_state_object_id(&transfer.from);
                if let Some(flux_data) = self.state_provider.get_object(&flux_oid) {
                    read_set.push(setu_types::task::ReadSetEntry::new(
                        format!("oid:{}", hex::encode(&flux_oid)),
                        flux_data,
                    ));
                }
                if let Some(power_data) = self.state_provider.get_object(&power_oid) {
                    read_set.push(setu_types::task::ReadSetEntry::new(
                        format!("oid:{}", hex::encode(&power_oid)),
                        power_data,
                    ));
                }

                // Add ResourceParams from GOVERNANCE subnet
                let rp_oid = setu_types::resource_params_object_id();
                if let Some(rp_data) = self.state_provider.get_object_from_subnet(
                    &rp_oid,
                    &setu_types::SubnetId::GOVERNANCE,
                ) {
                    read_set.push(setu_types::task::ReadSetEntry::new(
                        format!("oid:{}", hex::encode(rp_oid.as_bytes())),
                        rp_data,
                    ));
                }

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

    // ========== Phase 4: MoveCall task preparation ==========

    /// Prepare a SolverTask for a MoveCall request
    ///
    /// Collects input objects, resolves module dependencies, and builds
    /// a SolverTask with ResolvedInputs::MoveCall + module_read_set.
    pub fn prepare_move_call_task(
        &self,
        event: &setu_types::Event,
        call: &setu_types::event::MoveCallPayload,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        // PWOO: shared_object_ids is now supported. Enforce per-list semantics
        // (owned-only for input_object_ids, shared-only for shared_object_ids)
        // and detect duplicates across the two lists.
        {
            use std::collections::HashSet;
            let input_set: HashSet<&setu_types::ObjectId> =
                call.input_object_ids.iter().collect();
            for oid in &call.shared_object_ids {
                if input_set.contains(oid) {
                    return Err(TaskPrepareError::DuplicateObjectInLists {
                        object_id: hex::encode(oid.as_bytes()),
                    });
                }
            }
        }

        // 0. Parse sender address (once, outside loop)
        let sender_addr = setu_types::Address::normalize(&call.sender);

        // 1. Collect owned input objects (Path A)
        let mut read_set = Vec::new();
        let mut resolved_objects = Vec::new();

        // Slices are tiny (typically n ≤ 4), `.contains` is fine without HashSet.
        let mutable_idx_slice = call.mutable_indices.as_deref().unwrap_or(&[]);
        let consumed_idx_slice = call.consumed_indices.as_deref().unwrap_or(&[]);

        for (idx, object_id) in call.input_object_ids.iter().enumerate() {
            let data = self.state_provider.get_object(object_id)
                .ok_or_else(|| TaskPrepareError::ObjectNotFound(
                    hex::encode(object_id.as_bytes()),
                ))?;

            // Parse once: ownership check + version extraction
            let parsed = setu_types::envelope::detect_and_parse(&data);

            // Ownership check: sender must own AddressOwner objects
            match &parsed {
                setu_types::envelope::StorageFormat::Envelope(env) => {
                    match env.metadata.ownership {
                        setu_types::Ownership::AddressOwner(owner) => {
                            if owner != sender_addr {
                                return Err(TaskPrepareError::NotOwnedBySender {
                                    object_id: hex::encode(object_id.as_bytes()),
                                    sender: call.sender.clone(),
                                });
                            }
                        }
                        setu_types::Ownership::Immutable => {
                            // Frozen objects: read-only by anyone, but must
                            // not be bound to `&mut T` or consumed by-value.
                            // See docs/feat/fix-immutable-mutable-ref-not-blocked.
                            if mutable_idx_slice.contains(&idx) {
                                return Err(TaskPrepareError::ImmutableObjectCannotBeMutated {
                                    object_id: hex::encode(object_id.as_bytes()),
                                    index: idx,
                                });
                            }
                            if consumed_idx_slice.contains(&idx) {
                                return Err(TaskPrepareError::ImmutableObjectCannotBeConsumed {
                                    object_id: hex::encode(object_id.as_bytes()),
                                    index: idx,
                                });
                            }
                        }
                        setu_types::Ownership::ObjectOwner(parent) => {
                            // DF entries (the only producer of ObjectOwner today)
                            // must be accessed via `dynamic_field_accesses[]`,
                            // never through raw `input_object_ids[]`. Allowing
                            // the raw path would bypass parent authorisation
                            // and the DF subsystem's invariants (FDP §3.4 / §3.8).
                            // See docs/feat/fix-objectowner-mutable-ref-not-blocked.
                            return Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs {
                                object_id: hex::encode(object_id.as_bytes()),
                                parent_object_id: hex::encode(parent.as_bytes()),
                                index: idx,
                            });
                        }
                        setu_types::Ownership::Shared { .. } => {
                            // PWOO: shared objects must be declared in the
                            // dedicated list so concurrent-swap detection
                            // can target them precisely.
                            return Err(TaskPrepareError::UseSharedObjectIdsInstead {
                                object_id: hex::encode(object_id.as_bytes()),
                            });
                        }
                    }
                }
                setu_types::envelope::StorageFormat::LegacyCoinState(cs) => {
                    let cs_owner = setu_types::Address::normalize(&cs.owner);
                    if cs_owner != sender_addr {
                        return Err(TaskPrepareError::NotOwnedBySender {
                            object_id: hex::encode(object_id.as_bytes()),
                            sender: call.sender.clone(),
                        });
                    }
                }
                setu_types::envelope::StorageFormat::Unknown => {
                    // Unknown format — allow for now (module bytecode etc.)
                }
            }

            let key = format!("oid:{}", hex::encode(object_id.as_bytes()));
            read_set.push(ReadSetEntry::new(key, data.clone()));

            // Extract version from parsed format
            let version = match &parsed {
                setu_types::envelope::StorageFormat::Envelope(env) => env.metadata.version,
                _ => 0,
            };

            resolved_objects.push(ResolvedObject {
                object_id: *object_id,
                object_type: "MoveObject".to_string(),
                expected_version: version,
            });
        }

        // 1b. Collect shared input objects (Path B, PWOO).
        //
        // Append after owned inputs so existing `mutable_indices` /
        // `consumed_indices` (computed against `input_object_ids`) still line
        // up with the front of `resolved_objects`. Shared objects occupy the
        // tail and carry their current version as `expected_version`, which
        // is echoed back to the solver and ultimately surfaces in the
        // byte-level `old_value` conflict detection in the storage layer.
        for object_id in &call.shared_object_ids {
            let data = self.state_provider.get_object(object_id)
                .ok_or_else(|| TaskPrepareError::ObjectNotFound(
                    hex::encode(object_id.as_bytes()),
                ))?;

            let parsed = setu_types::envelope::detect_and_parse(&data);

            // Must be Shared ownership.
            let version = match &parsed {
                setu_types::envelope::StorageFormat::Envelope(env) => {
                    match env.metadata.ownership {
                        setu_types::Ownership::Shared { .. } => env.metadata.version,
                        _ => {
                            return Err(TaskPrepareError::NotShared {
                                object_id: hex::encode(object_id.as_bytes()),
                            });
                        }
                    }
                }
                _ => {
                    return Err(TaskPrepareError::NotShared {
                        object_id: hex::encode(object_id.as_bytes()),
                    });
                }
            };

            let key = format!("oid:{}", hex::encode(object_id.as_bytes()));
            read_set.push(ReadSetEntry::new(key, data.clone()));

            resolved_objects.push(ResolvedObject {
                object_id: *object_id,
                object_type: "MoveObject".to_string(),
                expected_version: version,
            });
        }

        // 1c. Resolve declared dynamic-field accesses (DF FDP M4).
        //
        // Design `docs/feat/dynamic-fields/design.md` §3.4: each
        // `DynamicFieldAccess` is resolved against the pre-TX SMT so the
        // TEE receives the DF entry's value bytes (or `None` for Create)
        // plus a byte-copy in `read_set` for PWOO conflict detection.
        //
        // Ordering constraint: runs AFTER owned + shared input collection
        // (so `declared_parents` is complete) and BEFORE index validation
        // (so failures short-circuit cheaply).
        let declared_parents: std::collections::HashSet<ObjectId> = call
            .input_object_ids
            .iter()
            .chain(call.shared_object_ids.iter())
            .copied()
            .collect();

        let mut resolved_dfs: Vec<setu_types::task::ResolvedDynamicField> = Vec::new();
        // Convert hex-string DF accesses (MoveCall wire form) → internal binary
        // form, then delegate to the shared resolver. The hex-decode of
        // `parent_object_id` (which tolerates the optional `"oid:"` prefix) and
        // of `key_bcs_hex` happens here; downstream all forms agree on bytes.
        let mut internal_dfs: Vec<DfAccessInternal> =
            Vec::with_capacity(call.dynamic_field_accesses.len());
        for df in &call.dynamic_field_accesses {
            let parent = parse_oid_with_optional_prefix(&df.parent_object_id)?;
            let key_bcs = hex::decode(&df.key_bcs_hex).map_err(|e| {
                TaskPrepareError::InvalidInput(format!(
                    "invalid key_bcs_hex for DF parent={}: {}",
                    df.parent_object_id, e,
                ))
            })?;
            internal_dfs.push(DfAccessInternal {
                parent,
                key_type: df.key_type.clone(),
                key_bcs,
                mode: df.mode,
                value_type: df.value_type.clone(),
            });
        }
        resolve_dynamic_fields_into(
            &*self.state_provider,
            sender_addr,
            &call.sender,
            &internal_dfs,
            &declared_parents,
            &mut read_set,
            &mut resolved_dfs,
        )?;

        // 2. Validate indices
        //
        // Design §3.2 / §6.2: `mutable_indices` and `consumed_indices` index
        // into the CONCATENATED list `[input_object_ids..., shared_object_ids...]`.
        // Owned entries occupy `0..owned_count`; shared entries occupy
        // `owned_count..owned_count+shared_count`.
        //
        // Rules:
        //   * `mutable_indices[i]` may point at either an owned or a shared
        //     object (up to `owned_count + shared_count`).
        //   * `consumed_indices[i]` may ONLY point at an owned object —
        //     shared objects cannot be consumed by-value in Phase 1 (there
        //     is no `transfer::delete_shared_object`). A shared index in
        //     `consumed_indices` is an explicit client error.
        let mutable_indices = call.mutable_indices.clone().unwrap_or_default();
        let consumed_indices = call.consumed_indices.clone().unwrap_or_default();
        let owned_count = call.input_object_ids.len();
        let total_count = owned_count + call.shared_object_ids.len();
        for &idx in mutable_indices.iter() {
            if idx >= total_count {
                return Err(TaskPrepareError::InvalidInput(format!(
                    "mutable_indices[{}] out of range (total object count: {})",
                    idx, total_count
                )));
            }
        }
        for &idx in consumed_indices.iter() {
            if idx >= owned_count {
                return Err(TaskPrepareError::InvalidInput(format!(
                    "consumed_indices[{}] must reference an owned input \
                     (owned count: {}); shared objects cannot be consumed \
                     by-value",
                    idx, owned_count
                )));
            }
        }

        // 3. Resolve module dependencies
        let module_read_set = self.resolve_module_dependencies(&call.package, &call.module)?;

        // 4. Build ResolvedInputs::MoveCall
        let mut resolved_inputs = ResolvedInputs::move_call(
            call.package.clone(),
            call.module.clone(),
            call.function.clone(),
            call.type_args.clone(),
            call.args.clone(),
            resolved_objects,
            mutable_indices,
            consumed_indices,
        );
        resolved_inputs.dynamic_fields = resolved_dfs;

        // 5. Derive dependencies (for future DAG ordering)
        let _parent_ids = self.derive_dependencies(
            &call.input_object_ids.iter()
                .chain(call.shared_object_ids.iter())
                .collect::<Vec<_>>(),
        );

        // 6. Build SolverTask
        let task_id = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TASK:");
            hasher.update(event.id.as_bytes());
            *hasher.finalize().as_bytes()
        };

        let pre_state_root = self.state_provider.get_state_root();

        Ok(SolverTask {
            task_id,
            subnet_id,
            pre_state_root,
            event: event.clone(),
            read_set,
            resolved_inputs,
            gas_budget: setu_types::task::GasBudget::default(),
            module_read_set,
        })
    }

    // ========== Phase 9: PTB task preparation (FDP move-vm-phase9-ptb-event-wire) ==========

    /// Prepare a SolverTask for a Programmable Transaction Block.
    ///
    /// PTB is the "many commands, one atomic effect" Move execution mode.
    /// Compared to `prepare_move_call_task`, all input-object metadata
    /// (version, digest, mutability) lives inside `ptb.inputs[].ObjectArg` —
    /// there is no parallel `input_object_ids` / `mutable_indices` /
    /// `consumed_indices` list. This function:
    ///
    /// 1. Walks `ptb.inputs` once; resolves every `ObjectArg` from the SMT.
    ///    Owned-object size invariant: `input_objects.len() ≤ ptb.inputs.len()`.
    ///    Pure args contribute no SMT lookup; the engine builds its
    ///    `Slot[i]` vec linearly over `ptb.inputs` later, looking objects up
    ///    by ObjectId (`engine.rs:792-803`), so the asymmetry is safe.
    /// 2. Enforces stale-read defense (D4): `actual_version == expected_version`
    ///    AND `blake3(envelope_bytes) == digest`.
    /// 3. Rejects `ObjectArg::SharedObject` (D5 Phase-1 limitation —
    ///    SharedObject's wire form has no digest, so a future unblock must
    ///    introduce a separate stale-read mechanism).
    /// 4. Resolves `ptb.dynamic_field_accesses` via the shared
    ///    `resolve_dynamic_fields_into` helper.
    /// 5. Collects every distinct `(package, module)` referenced by
    ///    `Command::MoveCall` into `module_read_set`.
    pub fn prepare_move_ptb_task(
        &self,
        event: &setu_types::Event,
        payload: &setu_types::event::MovePtbPayload,
        subnet_id: SubnetId,
    ) -> Result<SolverTask, TaskPrepareError> {
        use setu_types::ptb::{CallArg, ObjectArg};

        let sender_addr = setu_types::Address::normalize(&payload.sender);
        let ptb = &payload.ptb;

        // 1. Resolve Object inputs.
        let mut input_objects: Vec<ResolvedObject> = Vec::new();
        let mut read_set: Vec<ReadSetEntry> = Vec::new();
        let mut declared_parents: std::collections::HashSet<ObjectId> =
            std::collections::HashSet::new();

        for (i, arg) in ptb.inputs.iter().enumerate() {
            match arg {
                CallArg::Pure(_) => continue,
                CallArg::Object(ObjectArg::ImmOrOwnedObject(id, version, digest)) => {
                    let env = load_envelope(&*self.state_provider, id)?
                        .ok_or_else(|| TaskPrepareError::ObjectNotFound(
                            hex::encode(id.as_bytes()),
                        ))?;

                    // Ownership check (mirrors prepare_move_call_task §1).
                    match env.metadata.ownership {
                        setu_types::Ownership::AddressOwner(owner) => {
                            if owner != sender_addr {
                                return Err(TaskPrepareError::NotOwnedBySender {
                                    object_id: hex::encode(id.as_bytes()),
                                    sender: payload.sender.clone(),
                                });
                            }
                        }
                        setu_types::Ownership::Immutable => {
                            // Allowed read-only; engine's signature-based
                            // mutability check still gates &mut binding at
                            // command-resolve time. PTB has no separate
                            // mutable_indices list to cross-check here.
                        }
                        setu_types::Ownership::ObjectOwner(parent) => {
                            return Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs {
                                object_id: hex::encode(id.as_bytes()),
                                parent_object_id: hex::encode(parent.as_bytes()),
                                index: i,
                            });
                        }
                        setu_types::Ownership::Shared { .. } => {
                            // A `Shared` on-chain object reaching us via
                            // `ImmOrOwnedObject` is a client error — they
                            // must use `ObjectArg::SharedObject` (which is
                            // currently rejected anyway by D5).
                            return Err(TaskPrepareError::UseSharedObjectIdsInstead {
                                object_id: hex::encode(id.as_bytes()),
                            });
                        }
                    }

                    // D4: stale-read defense — version + digest.
                    if env.metadata.version != *version {
                        return Err(TaskPrepareError::StaleObjectVersion {
                            object_id: hex::encode(id.as_bytes()),
                            expected: *version,
                            actual: env.metadata.version,
                        });
                    }
                    let envelope_bytes = env.to_bytes();
                    let actual_digest = blake3::hash(&envelope_bytes);
                    if actual_digest.as_bytes() != digest {
                        return Err(TaskPrepareError::ObjectDigestMismatch {
                            object_id: hex::encode(id.as_bytes()),
                        });
                    }

                    let key = format!("oid:{}", hex::encode(id.as_bytes()));
                    read_set.push(ReadSetEntry::new(key, envelope_bytes));
                    input_objects.push(
                        ResolvedObject::new(*id, "MoveObject")
                            .with_version(*version),
                    );
                    declared_parents.insert(*id);
                }
                CallArg::Object(ObjectArg::SharedObject { id, .. }) => {
                    // D5 (R1-ISSUE-3): Phase-1 hard reject. When unblocked
                    // later, a separate stale-read mechanism is required —
                    // SharedObject wire form carries no digest.
                    return Err(TaskPrepareError::SharedObjectsNotYetSupported {
                        object_id: hex::encode(id.as_bytes()),
                    });
                }
            }
        }

        // 2. Resolve PTB dynamic_field_accesses via the shared helper.
        let internal_dfs: Vec<DfAccessInternal> = ptb
            .dynamic_field_accesses
            .iter()
            .map(|d| DfAccessInternal {
                parent: d.parent,
                key_type: d.key_type.clone(),
                key_bcs: d.key_bcs.clone(),
                mode: d.mode,
                value_type: d.value_type.clone(),
            })
            .collect();
        let mut resolved_dfs: Vec<setu_types::task::ResolvedDynamicField> = Vec::new();
        resolve_dynamic_fields_into(
            &*self.state_provider,
            sender_addr,
            &payload.sender,
            &internal_dfs,
            &declared_parents,
            &mut read_set,
            &mut resolved_dfs,
        )?;

        // 3. Module read-set: union of every (package, module) referenced by
        //    Command::MoveCall in ptb.commands, plus the `linkage:latest:` +
        //    prev-package `mod:{addr}::{name}` entries that
        //    `lower_upgrade_inline` (engine.rs:1968 / engine.rs:2056) needs
        //    when a `Command::Upgrade` is present. See
        //    docs/feat/fix-ptb-upgrade-zero-events/ for the analysis.
        let mut module_read_set: Vec<ReadSetEntry> = Vec::new();
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut seen_raw: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for cmd in &ptb.commands {
            match cmd {
                setu_types::ptb::Command::MoveCall(mc) => {
                    // Use canonical (zero-stripped) form to match how `mod:`
                    // SMT keys are written — `infra_executor.rs::execute_contract_publish`
                    // uses `to_hex_literal()`. Padded `hex::encode(.as_bytes())` here
                    // would mis-key for any package whose address has leading zeros
                    // (e.g. v0 family `0xcafe`). See iter-7
                    // docs/feat/fix-package-addr-hex-encoding/.
                    let pkg_hex = crate::network::move_handler::canonical_addr_hex(
                        &format!("0x{}", hex::encode(mc.package.as_bytes())),
                    );
                    if !seen.insert((pkg_hex.clone(), mc.module.clone())) {
                        continue;
                    }
                    let module_set =
                        self.resolve_module_dependencies(&pkg_hex, &mc.module)?;
                    module_read_set.extend(module_set);
                }
                setu_types::ptb::Command::Upgrade {
                    current_package,
                    modules,
                    ..
                } => {
                    // (a) linkage:latest:{family_hex} — engine.rs:1968 reads this
                    //     to discover prev_package_addr + prev_version. Without it
                    //     `lower_upgrade_inline` aborts with "no linkage entry for
                    //     family=...". Format mirrors the publish writer at
                    //     mock/mod.rs:140 (hex::encode of 32-byte family_id).
                    let family_hex = hex::encode(current_package.as_bytes());
                    let linkage_key = format!("linkage:latest:{}", family_hex);
                    if seen_raw.insert(linkage_key.clone()) {
                        let linkage_bytes =
                            self.state_provider.get_raw(&linkage_key).ok_or_else(|| {
                                TaskPrepareError::ModuleNotFound(linkage_key.clone())
                            })?;
                        // (b) Decode (Address, u64) shape written by
                        //     apply_module_changes_to_diff in mock/mod.rs:138.
                        let (prev_addr, _prev_ver): (
                            setu_types::object::Address,
                            u64,
                        ) = bcs::from_bytes(&linkage_bytes).map_err(|e| {
                            TaskPrepareError::InvalidModule(format!(
                                "linkage:latest decode for {family_hex}: {e}"
                            ))
                        })?;
                        module_read_set.push(ReadSetEntry::new(
                            linkage_key,
                            linkage_bytes,
                        ));

                        // (c) For each module in the upgrade bundle, look up the
                        //     prev_addr's `mod:{prev_addr}::{name}` entry that
                        //     engine.rs:2056 feeds to `Compatibility::check`.
                        //     Use to_hex_literal() to match the engine reader byte-for-byte.
                        let prev_account_hex_literal =
                            move_core_types::account_address::AccountAddress::new(
                                *prev_addr.as_bytes(),
                            )
                            .to_hex_literal();
                        for bytecode in modules {
                            let cm = move_binary_format::CompiledModule::deserialize_with_defaults(bytecode)
                                .map_err(|e| {
                                    TaskPrepareError::InvalidModule(format!(
                                        "Upgrade input module deserialize failed: {e}"
                                    ))
                                })?;
                            let name = cm.self_id().name().to_string();
                            let key = format!(
                                "mod:{}::{}",
                                prev_account_hex_literal, name
                            );
                            if !seen_raw.insert(key.clone()) {
                                continue;
                            }
                            let bytes =
                                self.state_provider.get_raw(&key).ok_or_else(|| {
                                    TaskPrepareError::ModuleNotFound(key.clone())
                                })?;
                            module_read_set
                                .push(ReadSetEntry::new(key, bytes));
                        }
                    }
                }
                _ => {}
            }
        }

        // 4. Assemble SolverTask.
        let task_id = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TASK:");
            hasher.update(event.id.as_bytes());
            *hasher.finalize().as_bytes()
        };
        let pre_state_root = self.state_provider.get_state_root();

        let resolved_inputs = ResolvedInputs {
            operation: setu_types::task::OperationType::ProgrammableTransaction(
                ptb.clone(),
            ),
            input_objects,
            dynamic_fields: resolved_dfs,
        };

        Ok(SolverTask {
            task_id,
            subnet_id,
            pre_state_root,
            event: event.clone(),
            read_set,
            resolved_inputs,
            gas_budget: setu_types::task::GasBudget::default(),
            module_read_set,
        })
    }

    /// Resolve transitive module dependencies for a MoveCall
    ///
    /// Walks the dependency graph via move-binary-format deserialization,
    /// loading each module from storage. Skips stdlib (0x1) since it's
    /// embedded in the binary.
    fn resolve_module_dependencies(
        &self,
        package_addr: &str,
        module_name: &str,
    ) -> Result<Vec<ReadSetEntry>, TaskPrepareError> {
        use move_binary_format::CompiledModule;

        const MAX_MODULE_DEPENDENCIES: usize = 256;
        const STDLIB_ADDRS: &[&str] = &[
            "0x1",
            "0000000000000000000000000000000000000000000000000000000000000001",
        ];

        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![(package_addr.to_string(), module_name.to_string())];

        while let Some((addr, name)) = stack.pop() {
            let key = format!("mod:{}::{}", addr, name);
            if visited.contains(&key) {
                continue;
            }
            if visited.len() >= MAX_MODULE_DEPENDENCIES {
                return Err(TaskPrepareError::TooManyDependencies {
                    max: MAX_MODULE_DEPENDENCIES,
                    found: visited.len() + 1,
                });
            }
            visited.insert(key.clone());

            // Skip stdlib — embedded in binary
            if STDLIB_ADDRS.iter().any(|&a| addr == a) {
                continue;
            }

            // Load from storage
            let bytecode = self.state_provider.get_raw(&key)
                .ok_or_else(|| TaskPrepareError::ModuleNotFound(key.clone()))?;

            result.push(ReadSetEntry::new(key, bytecode.clone()));

            // Parse dependencies
            let compiled = CompiledModule::deserialize_with_defaults(&bytecode)
                .map_err(|e| TaskPrepareError::InvalidModule(format!("{}: {}", name, e)))?;
            for dep in compiled.immediate_dependencies() {
                stack.push((
                    dep.address().to_hex_literal(),
                    dep.name().to_string(),
                ));
            }
        }

        Ok(result)
    }
}

// ═══════════════════════════════════════════════════════════
// DF FDP M4 — module-level helpers
// ═══════════════════════════════════════════════════════════

/// Parse an ObjectId from an optional `"oid:"` prefix + hex string.
fn parse_oid_with_optional_prefix(raw: &str) -> Result<ObjectId, TaskPrepareError> {
    let trimmed = raw.strip_prefix("oid:").unwrap_or(raw);
    let bytes = hex::decode(trimmed).map_err(|e| {
        TaskPrepareError::InvalidInput(format!(
            "invalid object id hex '{}': {}",
            raw, e,
        ))
    })?;
    if bytes.len() != 32 {
        return Err(TaskPrepareError::InvalidInput(format!(
            "object id must be 32 bytes (got {} from '{}')",
            bytes.len(),
            raw,
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ObjectId::new(arr))
}

/// Load + decode an ObjectEnvelope from the state provider.
///
/// Uses `detect_and_parse` (the same guard PWOO uses in `single.rs:~940`
/// and `manager.rs:779/915/951`) so corrupted / legacy bytes are rejected
/// explicitly instead of being silently accepted by `bcs::from_bytes`.
fn load_envelope(
    state_provider: &dyn StateProvider,
    oid: &ObjectId,
) -> Result<Option<setu_types::ObjectEnvelope>, TaskPrepareError> {
    let Some(bytes) = state_provider.get_object(oid) else {
        return Ok(None);
    };
    match setu_types::envelope::detect_and_parse(&bytes) {
        setu_types::envelope::StorageFormat::Envelope(env) => Ok(Some(env)),
        // DF parent entries must be Move envelopes — legacy coin state is
        // never a DF parent. Reuse the mismatch variant to keep the error
        // surface focused.
        setu_types::envelope::StorageFormat::LegacyCoinState(_) => {
            Err(TaskPrepareError::DynamicFieldParentMismatch)
        }
        setu_types::envelope::StorageFormat::Unknown => {
            Err(TaskPrepareError::EnvelopeDecode(format!(
                "unknown storage format for oid {}",
                hex::encode(oid.as_bytes()),
            )))
        }
    }
}

/// Assert the DF envelope's ownership points back at `expected_parent`.
///
/// Defence-in-depth: even if the client computed a valid-looking `df_oid`,
/// the on-disk DF entry must declare the same parent via
/// `Ownership::ObjectOwner(parent)`.
fn ensure_df_ownership(
    env: &setu_types::ObjectEnvelope,
    expected_parent: ObjectId,
) -> Result<(), TaskPrepareError> {
    match env.metadata.ownership {
        setu_types::Ownership::ObjectOwner(parent) if parent == expected_parent => Ok(()),
        _ => Err(TaskPrepareError::DynamicFieldParentMismatch),
    }
}

// ═══════════════════════════════════════════════════════════
// DF resolver — shared between MoveCall and PTB prepare paths
// (FDP move-vm-phase9-ptb-event-wire R1-ISSUE-1)
// ═══════════════════════════════════════════════════════════

/// Internal binary form of a declared dynamic-field access.
///
/// Both `MoveCallPayload.dynamic_field_accesses` (hex-string
/// `DynamicFieldAccess`) and `ProgrammableTransaction.dynamic_field_accesses`
/// (binary `PtbDfAccess`) convert to this before resolution. Keeping a single
/// resolver eliminates the cross-layer-mismatch class of bugs (G12).
pub(crate) struct DfAccessInternal {
    pub parent: ObjectId,
    pub key_type: String,
    pub key_bcs: Vec<u8>,
    pub mode: setu_types::dynamic_field::DfAccessMode,
    pub value_type: Option<String>,
}

/// Resolve a list of `DfAccessInternal` against the SMT, appending to
/// `read_set` and `resolved_dfs`. Performs:
///
/// 1. Parent-declared check against `declared_parents` (caller-supplied set).
/// 2. Parent ownership / authorisation (AddressOwner == sender, Shared OK,
///    Immutable read-only, ObjectOwner forbidden).
/// 3. Per-mode resolution:
///    - Mutate / Delete: presence required, decode `DfFieldValue` envelope.
///    - Read           : presence optional (absent-Read sentinel).
///    - Create         : entry must NOT exist.
///
/// Mirrors the body that previously lived inline inside
/// `prepare_move_call_task`.
pub(crate) fn resolve_dynamic_fields_into(
    state_provider: &dyn StateProvider,
    sender_addr: setu_types::Address,
    sender_hex_for_errors: &str,
    accesses: &[DfAccessInternal],
    declared_parents: &std::collections::HashSet<ObjectId>,
    read_set: &mut Vec<ReadSetEntry>,
    resolved_dfs: &mut Vec<setu_types::task::ResolvedDynamicField>,
) -> Result<(), TaskPrepareError> {
    for df in accesses {
        let parent_oid = df.parent;

        if !declared_parents.contains(&parent_oid) {
            return Err(TaskPrepareError::DynamicFieldParentNotDeclared {
                parent: hex::encode(parent_oid.as_bytes()),
            });
        }

        // Parent ownership / authorisation (§3.4).
        let parent_env = load_envelope(state_provider, &parent_oid)?
            .ok_or_else(|| TaskPrepareError::ObjectNotFound(
                hex::encode(parent_oid.as_bytes()),
            ))?;
        match parent_env.metadata.ownership {
            setu_types::Ownership::AddressOwner(owner) => {
                if owner != sender_addr {
                    return Err(TaskPrepareError::NotOwnedBySender {
                        object_id: hex::encode(parent_oid.as_bytes()),
                        sender: sender_hex_for_errors.to_string(),
                    });
                }
            }
            setu_types::Ownership::Shared { .. } => { /* any sender OK */ }
            setu_types::Ownership::Immutable => {
                if df.mode != setu_types::dynamic_field::DfAccessMode::Read {
                    return Err(TaskPrepareError::DynamicFieldOnImmutableParent);
                }
            }
            setu_types::Ownership::ObjectOwner(_) => {
                return Err(TaskPrepareError::DynamicFieldParentNotRoot);
            }
        }

        let df_oid = setu_types::dynamic_field::derive_df_oid(
            &parent_oid,
            &df.key_type,
            &df.key_bcs,
        );

        // Per-mode resolution (fix-df-exists-absent-rejected, 2026-04-27):
        //   - Mutate / Delete: presence required (need on-disk envelope
        //     bytes for the StateChange `old_value`).
        //   - Read           : presence OPTIONAL — `exists_` must work
        //     for absent keys, and `borrow` cleanly aborts at the VM
        //     native via `E_DF_DOES_NOT_EXIST` when `value_bytes=None`.
        //   - Create         : entry must NOT exist.
        let (value_bytes, value_type_tag_resolved, envelope_bytes) = match df.mode {
            setu_types::dynamic_field::DfAccessMode::Mutate
            | setu_types::dynamic_field::DfAccessMode::Delete => {
                let df_env = load_envelope(state_provider, &df_oid)?
                    .ok_or_else(|| TaskPrepareError::DynamicFieldNotFound {
                        parent: hex::encode(parent_oid.as_bytes()),
                        key_type: df.key_type.clone(),
                    })?;
                ensure_df_ownership(&df_env, parent_oid)?;
                let key = format!("oid:{}", hex::encode(df_oid.as_bytes()));
                let envelope_bytes = df_env.to_bytes();
                read_set.push(ReadSetEntry::new(key, envelope_bytes.clone()));

                let on_disk = bcs::from_bytes::<
                    setu_types::dynamic_field::DfFieldValue,
                >(&df_env.data)
                .map_err(|e| TaskPrepareError::InvalidInput(format!(
                    "malformed DfFieldValue at df_oid={}: {}",
                    hex::encode(df_oid.as_bytes()), e,
                )))?;
                (Some(on_disk.value_bcs), on_disk.value_type_tag, Some(envelope_bytes))
            }
            setu_types::dynamic_field::DfAccessMode::Read => {
                let key = format!("oid:{}", hex::encode(df_oid.as_bytes()));
                match load_envelope(state_provider, &df_oid)? {
                    Some(df_env) => {
                        ensure_df_ownership(&df_env, parent_oid)?;
                        let envelope_bytes = df_env.to_bytes();
                        read_set.push(ReadSetEntry::new(key, envelope_bytes.clone()));
                        let on_disk = bcs::from_bytes::<
                            setu_types::dynamic_field::DfFieldValue,
                        >(&df_env.data)
                        .map_err(|e| TaskPrepareError::InvalidInput(format!(
                            "malformed DfFieldValue at df_oid={}: {}",
                            hex::encode(df_oid.as_bytes()), e,
                        )))?;
                        (
                            Some(on_disk.value_bcs),
                            on_disk.value_type_tag,
                            Some(envelope_bytes),
                        )
                    }
                    None => {
                        // Absent-Read sentinel (see fix-df-exists-absent-rejected).
                        read_set.push(ReadSetEntry::new(key, Vec::new()));
                        (None, df.value_type.clone().unwrap_or_default(), None)
                    }
                }
            }
            setu_types::dynamic_field::DfAccessMode::Create => {
                if load_envelope(state_provider, &df_oid)?.is_some() {
                    return Err(TaskPrepareError::DynamicFieldAlreadyExists {
                        parent: hex::encode(parent_oid.as_bytes()),
                        key_type: df.key_type.clone(),
                    });
                }
                let key = format!("oid:{}", hex::encode(df_oid.as_bytes()));
                read_set.push(ReadSetEntry::new(key, Vec::new()));
                (None, df.value_type.clone().unwrap_or_default(), None)
            }
        };

        resolved_dfs.push(setu_types::task::ResolvedDynamicField {
            parent_object_id: parent_oid,
            df_object_id: df_oid,
            name_type_tag: df.key_type.clone(),
            name_bcs: df.key_bcs.clone(),
            value_bytes,
            value_type_tag: value_type_tag_resolved,
            mode: df.mode,
            envelope_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Transfer, TransferType};
    use setu_types::task::OperationType;
    
    fn create_test_transfer() -> Transfer {
        Transfer::new("test-tx-1", "alice", "bob", 100)
            .with_type(TransferType::SetuTransfer)
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

    // ========== MoveCall preparation tests ==========

    /// Helper: create a valid Move module bytecode with the given address and name
    fn make_module_bytes(
        addr: move_binary_format::file_format::AddressIdentifierIndex,
        addr_val: move_core_types::account_address::AccountAddress,
        name: &str,
    ) -> Vec<u8> {
        use move_binary_format::file_format::*;
        let mut module = empty_module();
        module.address_identifiers[0] = addr_val;
        module.identifiers[0] = move_core_types::identifier::Identifier::new(name).unwrap();
        let mut buf = Vec::new();
        module.serialize_with_version(move_binary_format::file_format_common::VERSION_MAX, &mut buf)
            .expect("serialize module");
        buf
    }

    /// Helper: create a TaskPreparer backed by a fresh state with a module stored
    fn make_preparer_with_module(module_key: &str, module_bytes: &[u8]) -> TaskPreparer {
        use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider};
        use setu_types::event::StateChange;
        use std::sync::Arc;

        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        {
            let mut gsm = shared.lock_write();
            let sc = StateChange::insert(module_key.to_string(), module_bytes.to_vec());
            gsm.apply_state_change(SubnetId::ROOT, &sc);
            shared.publish_snapshot(&gsm);
        }
        let provider: Arc<dyn StateProvider> = Arc::new(MerkleStateProvider::new(shared));
        TaskPreparer::new("validator-test".to_string(), provider)
    }

    fn test_move_call_payload(package: &str, module: &str, function: &str) -> setu_types::event::MoveCallPayload {
        setu_types::event::MoveCallPayload {
            sender: "alice".to_string(),
            package: package.to_string(),
            module: module.to_string(),
            function: function.to_string(),
            type_args: vec![],
            args: vec![],
            input_object_ids: vec![],
            shared_object_ids: vec![],
            mutable_indices: None,
            consumed_indices: None,
            needs_tx_context: false,
            dynamic_field_accesses: vec![],
        }
    }

    fn test_event() -> setu_types::Event {
        setu_types::Event::new(
            setu_types::event::EventType::ContractCall,
            vec![],
            setu_types::event::VLCSnapshot {
                vector_clock: setu_vlc::VectorClock::new(),
                logical_time: 1,
                physical_time: 1000,
            },
            "alice".to_string(),
        )
    }

    #[test]
    fn test_prepare_move_call_task_basic() {
        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());
        let preparer = make_preparer_with_module(&module_key, &bytecode);
        let event = test_event();
        let call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(result.is_ok(), "Error: {:?}", result.unwrap_err());
        let task = result.unwrap();

        // Verify module_read_set populated
        assert!(!task.module_read_set.is_empty());
        assert!(task.module_read_set[0].key.contains("counter"));

        // Verify ResolvedInputs is MoveCall
        match &task.resolved_inputs.operation {
            OperationType::MoveCall { package, module_name, function_name, .. } => {
                assert!(package.contains("dead"));
                assert_eq!(module_name, "counter");
                assert_eq!(function_name, "increment");
            }
            _ => panic!("Expected MoveCall operation"),
        }
    }

    #[test]
    fn test_prepare_move_call_shared_object_missing_errors() {
        // PWOO: passing a shared_object_id for a non-existent object yields
        // ObjectNotFound (no longer a blanket SharedObjectNotSupported).
        let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
        let event = test_event();
        let mut call = test_move_call_payload("0x1", "coin", "transfer");
        call.shared_object_ids = vec![ObjectId::new([0xAA; 32])];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::ObjectNotFound(_))),
            "Expected ObjectNotFound, got: {:?}", result);
    }

    #[test]
    fn test_resolve_module_dependencies_missing_module() {
        // Preparer with no modules stored
        use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider};
        use std::sync::Arc;
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        {
            let gsm = shared.lock_write();
            shared.publish_snapshot(&gsm);
        }
        let provider: Arc<dyn StateProvider> = Arc::new(MerkleStateProvider::new(shared));
        let preparer = TaskPreparer::new("validator-test".to_string(), provider);

        let result = preparer.resolve_module_dependencies("0xdead", "counter");
        assert!(matches!(result, Err(TaskPrepareError::ModuleNotFound(_))));
    }

    #[test]
    fn test_resolve_module_dependencies_stdlib_skipped() {
        // Ask to resolve a stdlib module — should return empty (stdlib is embedded)
        use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider};
        use std::sync::Arc;
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        {
            let gsm = shared.lock_write();
            shared.publish_snapshot(&gsm);
        }
        let provider: Arc<dyn StateProvider> = Arc::new(MerkleStateProvider::new(shared));
        let preparer = TaskPreparer::new("validator-test".to_string(), provider);

        // "0x1" is stdlib — should be skipped entirely
        let result = preparer.resolve_module_dependencies("0x1", "vector");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ========== Sender-Owner consistency check tests (Phase 5b) ==========

    /// Helper: create a preparer with an ObjectEnvelope stored for a given owner
    fn make_preparer_with_owned_object(
        object_id: ObjectId,
        owner: setu_types::Address,
        ownership: setu_types::Ownership,
        module_key: Option<(&str, &[u8])>,
    ) -> TaskPreparer {
        use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider};
        use setu_types::event::StateChange;
        use setu_types::envelope::ObjectEnvelope;
        use setu_types::object::ObjectDigest;
        use std::sync::Arc;

        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        {
            let mut gsm = shared.lock_write();

            // Store the ObjectEnvelope
            let env = ObjectEnvelope::from_move_result(
                object_id,
                owner,
                1,
                ownership,
                "0xcafe::test::TestObj".to_string(),
                vec![0u8; 32], // dummy BCS data
            );
            let key = format!("oid:{}", hex::encode(object_id.as_bytes()));
            let sc = StateChange::insert(key, env.to_bytes());
            gsm.apply_state_change(SubnetId::ROOT, &sc);

            // Optionally store a module
            if let Some((mk, mb)) = module_key {
                let sc = StateChange::insert(mk.to_string(), mb.to_vec());
                gsm.apply_state_change(SubnetId::ROOT, &sc);
            }

            shared.publish_snapshot(&gsm);
        }
        let provider: Arc<dyn StateProvider> = Arc::new(MerkleStateProvider::new(shared));
        TaskPreparer::new("validator-test".to_string(), provider)
    }

    #[test]
    fn test_move_call_owned_object_by_owner_passes() {
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x11; 32]);

        // Create module bytecode (required for prepare_move_call_task)
        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::AddressOwner(alice),
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(result.is_ok(), "Owner should be able to use own object: {:?}", result.err());
    }

    #[test]
    fn test_move_call_non_owner_rejected() {
        let alice = setu_types::Address::normalize("alice");
        let bob = setu_types::Address::normalize("bob");
        let obj_id = ObjectId::new([0x22; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        // Object owned by alice
        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::AddressOwner(alice),
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        // Bob tries to use alice's object
        call.sender = bob.to_string().strip_prefix("0x").unwrap_or(&bob.to_string()).to_string();
        call.input_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::NotOwnedBySender { .. })),
            "Non-owner should be rejected, got: {:?}", result);
    }

    #[test]
    fn test_move_call_immutable_object_any_sender_passes() {
        let alice = setu_types::Address::normalize("alice");
        let bob = setu_types::Address::normalize("bob");
        let obj_id = ObjectId::new([0x33; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        // Immutable object
        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::Immutable,
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        // Bob can use immutable object
        call.sender = bob.to_string().strip_prefix("0x").unwrap_or(&bob.to_string()).to_string();
        call.input_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(result.is_ok(), "Any sender should be able to read immutable object: {:?}", result.err());
    }

    // ========== Immutable mutate/consume rejection tests
    // (docs/feat/fix-immutable-mutable-ref-not-blocked) ==========

    /// Helper for the Immutable-rejection suite: build a preparer whose only
    /// owned object at `obj_id` is `Immutable`, with a stored module under
    /// the standard test address `0xdead::counter`.
    fn make_immutable_preparer(obj_id: ObjectId) -> (TaskPreparer, String) {
        let alice = setu_types::Address::normalize("alice");
        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());
        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::Immutable,
            Some((&module_key, &bytecode)),
        );
        (preparer, addr.to_hex_literal())
    }

    #[test]
    fn test_immutable_in_mutable_indices_rejected() {
        let obj_id = ObjectId::new([0x61; 32]);
        let (preparer, pkg) = make_immutable_preparer(obj_id);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![obj_id];
        call.mutable_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        match result {
            Err(TaskPrepareError::ImmutableObjectCannotBeMutated { index, .. }) => {
                assert_eq!(index, 0, "should report the offending index");
            }
            other => panic!("expected ImmutableObjectCannotBeMutated, got {:?}", other),
        }
    }

    #[test]
    fn test_immutable_in_consumed_indices_rejected() {
        let obj_id = ObjectId::new([0x62; 32]);
        let (preparer, pkg) = make_immutable_preparer(obj_id);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![obj_id];
        call.consumed_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        match result {
            Err(TaskPrepareError::ImmutableObjectCannotBeConsumed { index, .. }) => {
                assert_eq!(index, 0);
            }
            other => panic!("expected ImmutableObjectCannotBeConsumed, got {:?}", other),
        }
    }

    #[test]
    fn test_immutable_read_only_accepted() {
        // Frozen object passed in input_object_ids but NOT marked mutable
        // or consumed: still the long-standing "anyone can read" path.
        let obj_id = ObjectId::new([0x63; 32]);
        let (preparer, pkg) = make_immutable_preparer(obj_id);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![obj_id];
        // mutable_indices and consumed_indices remain None.

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(result.is_ok(),
            "Read-only Immutable should pass, got {:?}", result.err());
    }

    #[test]
    fn test_address_owner_mutable_still_accepted_regression() {
        // Regression: AddressOwner in mutable_indices is the normal PWOO
        // happy path and must continue to work.
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x64; 32]);
        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());
        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::AddressOwner(alice),
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or("alice").to_string();
        call.input_object_ids = vec![obj_id];
        call.mutable_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(result.is_ok(),
            "AddressOwner+mutable_indices is the PWOO happy path, got {:?}", result.err());
    }

    #[test]
    fn test_immutable_index_error_carries_oid() {
        let obj_id = ObjectId::new([0x65; 32]);
        let (preparer, pkg) = make_immutable_preparer(obj_id);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![obj_id];
        call.mutable_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        match result {
            Err(TaskPrepareError::ImmutableObjectCannotBeMutated { object_id, index }) => {
                assert_eq!(index, 0);
                assert_eq!(object_id, hex::encode(obj_id.as_bytes()),
                    "error must echo the offending object id in hex");
            }
            other => panic!("expected ImmutableObjectCannotBeMutated, got {:?}", other),
        }
    }

    #[test]
    fn test_immutable_index_consumed_takes_priority_over_read() {
        // If an Immutable object appears in BOTH mutable_indices AND
        // consumed_indices, the loop checks mutable first and we should
        // surface ImmutableObjectCannotBeMutated. (Consistency anchor: the
        // first violation found is the one reported.)
        let obj_id = ObjectId::new([0x66; 32]);
        let (preparer, pkg) = make_immutable_preparer(obj_id);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![obj_id];
        call.mutable_indices = Some(vec![0]);
        call.consumed_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::ImmutableObjectCannotBeMutated { .. })),
            "mutable_indices is checked first, got {:?}", result);
    }

    // ========== ObjectOwner raw-input rejection tests
    // (docs/feat/fix-objectowner-mutable-ref-not-blocked) ==========

    /// Helper: build a preparer whose only owned object at `df_oid` carries
    /// `Ownership::ObjectOwner(parent)`, simulating a DF entry. The standard
    /// test module is stored under `0xdead::counter`.
    fn make_objectowner_preparer(
        df_oid: ObjectId,
        parent: ObjectId,
    ) -> (TaskPreparer, String) {
        let alice = setu_types::Address::normalize("alice");
        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());
        let preparer = make_preparer_with_owned_object(
            df_oid, alice,
            setu_types::Ownership::ObjectOwner(parent),
            Some((&module_key, &bytecode)),
        );
        (preparer, addr.to_hex_literal())
    }

    #[test]
    fn test_objectowner_in_input_rejected_read_only() {
        // Even read-only access (no mutable_indices, no consumed_indices,
        // no dynamic_field_accesses) must be rejected: DF entries are
        // addressable only through `dynamic_field_accesses[]`.
        let df_oid = ObjectId::new([0x71; 32]);
        let parent = ObjectId::new([0xa0; 32]);
        let (preparer, pkg) = make_objectowner_preparer(df_oid, parent);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![df_oid];
        // no mutable_indices, no consumed_indices, no dynamic_field_accesses

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        match result {
            Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs { index, .. }) => {
                assert_eq!(index, 0, "should report the offending index");
            }
            other => panic!("expected ObjectOwnerNotAllowedInInputs, got {:?}", other),
        }
    }

    #[test]
    fn test_objectowner_in_input_with_mutable_rejected() {
        // The new gate runs in the ownership match arm, so it fires before
        // any mutable/consumed cross-check. The error is the same.
        let df_oid = ObjectId::new([0x72; 32]);
        let parent = ObjectId::new([0xa1; 32]);
        let (preparer, pkg) = make_objectowner_preparer(df_oid, parent);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![df_oid];
        call.mutable_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result,
            Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs { index: 0, .. })),
            "expected ObjectOwnerNotAllowedInInputs, got {:?}", result);
    }

    #[test]
    fn test_objectowner_in_input_with_consumed_rejected() {
        let df_oid = ObjectId::new([0x73; 32]);
        let parent = ObjectId::new([0xa2; 32]);
        let (preparer, pkg) = make_objectowner_preparer(df_oid, parent);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![df_oid];
        call.consumed_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result,
            Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs { index: 0, .. })),
            "expected ObjectOwnerNotAllowedInInputs, got {:?}", result);
    }

    #[test]
    fn test_objectowner_error_carries_parent_oid() {
        // Verify the error variant carries object_id + parent_object_id +
        // index, all hex-encoded.
        let df_oid = ObjectId::new([0x74; 32]);
        let parent = ObjectId::new([0xa3; 32]);
        let (preparer, pkg) = make_objectowner_preparer(df_oid, parent);
        let event = test_event();
        let mut call = test_move_call_payload(&pkg, "counter", "increment");
        call.input_object_ids = vec![df_oid];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        match result {
            Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs {
                object_id, parent_object_id, index,
            }) => {
                assert_eq!(object_id, hex::encode(df_oid.as_bytes()),
                    "object_id must echo df_oid hex");
                assert_eq!(parent_object_id, hex::encode(parent.as_bytes()),
                    "parent_object_id must echo parent hex");
                assert_eq!(index, 0);
            }
            other => panic!("expected ObjectOwnerNotAllowedInInputs, got {:?}", other),
        }
    }

    #[test]
    fn test_move_call_shared_object_via_input_list_rejected() {
        // PWOO: a Shared object passed through `input_object_ids` must be
        // redirected to `shared_object_ids`.
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x44; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::Shared { initial_shared_version: 0 },
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::UseSharedObjectIdsInstead { .. })),
            "Shared-in-input_object_ids should hint UseSharedObjectIdsInstead, got: {:?}", result);
    }

    // ========== PWOO positive tests ==========

    #[test]
    fn test_move_call_shared_object_in_shared_list_passes() {
        // PWOO: Shared object passed through `shared_object_ids` is accepted,
        // and `resolved_objects` carries the current envelope version.
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x55; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::Shared { initial_shared_version: 7 },
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![];
        call.shared_object_ids = vec![obj_id];

        let task = preparer
            .prepare_move_call_task(&event, &call, SubnetId::ROOT)
            .expect("shared object via shared_object_ids should be accepted");

        // resolved_objects tail = shared list; expected_version carried.
        assert_eq!(task.resolved_inputs.input_objects.len(), 1);
        assert_eq!(task.resolved_inputs.input_objects[0].object_id, obj_id);
        assert_eq!(task.resolved_inputs.input_objects[0].expected_version, 1,
            "expected_version should match stored envelope version");
        assert!(matches!(
            task.resolved_inputs.operation,
            setu_types::task::OperationType::MoveCall { .. }
        ));
    }

    #[test]
    fn test_move_call_owned_in_shared_list_rejected() {
        // PWOO: placing a non-Shared object in shared_object_ids → NotShared.
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x66; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::AddressOwner(alice),
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.shared_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::NotShared { .. })),
            "Owned-in-shared_object_ids should produce NotShared, got: {:?}", result);
    }

    #[test]
    fn test_move_call_duplicate_across_lists_rejected() {
        // PWOO: same object in both lists → DuplicateObjectInLists.
        let alice = setu_types::Address::normalize("alice");
        let obj_id = ObjectId::new([0x77; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            obj_id, alice,
            setu_types::Ownership::Shared { initial_shared_version: 0 },
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![obj_id];
        call.shared_object_ids = vec![obj_id];

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::DuplicateObjectInLists { .. })),
            "Duplicate across lists should be rejected, got: {:?}", result);
    }

    // R4-ISSUE-1 regression: mutable_indices must allow indices into the
    // concatenated `[input_object_ids..., shared_object_ids...]` list.
    #[test]
    fn test_move_call_mutable_index_into_shared_tail_allowed() {
        use setu_storage::{GlobalStateManager, SharedStateManager, MerkleStateProvider};
        use setu_types::event::StateChange;
        use setu_types::envelope::ObjectEnvelope;
        use std::sync::Arc;

        let alice = setu_types::Address::normalize("alice");
        let owned_id = ObjectId::new([0xA1; 32]);
        let shared_id = ObjectId::new([0xA2; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let shared_state = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        {
            let mut gsm = shared_state.lock_write();
            let owned_env = ObjectEnvelope::from_move_result(
                owned_id, alice, 1,
                setu_types::Ownership::AddressOwner(alice),
                "0xcafe::test::TestObj".to_string(), vec![0u8; 32],
            );
            gsm.apply_state_change(
                SubnetId::ROOT,
                &StateChange::insert(
                    format!("oid:{}", hex::encode(owned_id.as_bytes())),
                    owned_env.to_bytes(),
                ),
            );
            let shared_env = ObjectEnvelope::from_move_result(
                shared_id, alice, 1,
                setu_types::Ownership::Shared { initial_shared_version: 0 },
                "0xcafe::test::TestObj".to_string(), vec![0u8; 32],
            );
            gsm.apply_state_change(
                SubnetId::ROOT,
                &StateChange::insert(
                    format!("oid:{}", hex::encode(shared_id.as_bytes())),
                    shared_env.to_bytes(),
                ),
            );
            gsm.apply_state_change(
                SubnetId::ROOT,
                &StateChange::insert(module_key.clone(), bytecode.clone()),
            );
            shared_state.publish_snapshot(&gsm);
        }
        let provider: Arc<dyn StateProvider> = Arc::new(MerkleStateProvider::new(shared_state));
        let preparer = TaskPreparer::new("validator-test".to_string(), provider);

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![owned_id];
        call.shared_object_ids = vec![shared_id];
        // idx 1 points at the shared tail — must be accepted
        call.mutable_indices = Some(vec![1]);

        let task = preparer
            .prepare_move_call_task(&event, &call, SubnetId::ROOT)
            .expect("R4-ISSUE-1: mutable idx into shared tail should be accepted");
        assert_eq!(task.resolved_inputs.input_objects.len(), 2);
    }

    // R4-ISSUE-1/3 regression: consumed_indices MUST NOT target the shared tail.
    #[test]
    fn test_move_call_consumed_index_into_shared_tail_rejected() {
        let alice = setu_types::Address::normalize("alice");
        let shared_id = ObjectId::new([0xB1; 32]);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead").unwrap();
        let bytecode = make_module_bytes(
            move_binary_format::file_format::AddressIdentifierIndex(0),
            addr,
            "counter",
        );
        let module_key = format!("mod:{}::counter", addr.to_hex_literal());

        let preparer = make_preparer_with_owned_object(
            shared_id, alice,
            setu_types::Ownership::Shared { initial_shared_version: 0 },
            Some((&module_key, &bytecode)),
        );

        let event = test_event();
        let mut call = test_move_call_payload(&addr.to_hex_literal(), "counter", "increment");
        call.sender = alice.to_string().strip_prefix("0x").unwrap_or(&alice.to_string()).to_string();
        call.input_object_ids = vec![];
        call.shared_object_ids = vec![shared_id];
        // idx 0 is into the shared tail; consumed-by-value is forbidden there
        call.consumed_indices = Some(vec![0]);

        let result = preparer.prepare_move_call_task(&event, &call, SubnetId::ROOT);
        assert!(matches!(result, Err(TaskPrepareError::InvalidInput(_))),
            "R4-ISSUE-3: consumed idx into shared tail must be rejected, got: {:?}", result);
    }

    // ═══════════════════════════════════════════════════════════
    // DF FDP M4 — Dynamic Field access resolution tests
    // ═══════════════════════════════════════════════════════════

    mod m4_df {
        use super::*;
        use setu_types::dynamic_field::{derive_df_oid, DfAccessMode, DfFieldValue};
        use setu_types::event::DynamicFieldAccess;
        use setu_types::envelope::ObjectEnvelope;

        /// Build a TaskPreparer seeded with a parent object (owned by sender
        /// unless overridden) and optionally a DF entry hanging off it.
        fn make_preparer_with_df(
            parent_id: ObjectId,
            parent_owner: setu_types::Ownership,
            df_entry: Option<(ObjectId, ObjectId, DfFieldValue)>,
            module_key: (&str, &[u8]),
        ) -> TaskPreparer {
            use setu_storage::{
                GlobalStateManager, MerkleStateProvider, SharedStateManager,
            };
            use setu_types::event::StateChange;
            use std::sync::Arc;

            let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
            {
                let mut gsm = shared.lock_write();

                let parent_env = ObjectEnvelope::from_move_result(
                    parent_id,
                    setu_types::Address::ZERO,
                    1,
                    parent_owner,
                    "0xcafe::pool::Pool".to_string(),
                    vec![0u8; 16],
                );
                let pkey = format!("oid:{}", hex::encode(parent_id.as_bytes()));
                gsm.apply_state_change(
                    SubnetId::ROOT,
                    &StateChange::insert(pkey, parent_env.to_bytes()),
                );

                if let Some((df_oid, expected_parent, payload)) = df_entry {
                    let data = bcs::to_bytes(&payload).unwrap();
                    let df_env = ObjectEnvelope::from_move_result(
                        df_oid,
                        setu_types::Address::ZERO,
                        1,
                        setu_types::Ownership::ObjectOwner(expected_parent),
                        format!(
                            "0x1::dynamic_field::Field<{}, {}>",
                            payload.name_type_tag, payload.value_type_tag
                        ),
                        data,
                    );
                    let dkey = format!("oid:{}", hex::encode(df_oid.as_bytes()));
                    gsm.apply_state_change(
                        SubnetId::ROOT,
                        &StateChange::insert(dkey, df_env.to_bytes()),
                    );
                }

                let (mk, mb) = module_key;
                gsm.apply_state_change(
                    SubnetId::ROOT,
                    &StateChange::insert(mk.to_string(), mb.to_vec()),
                );

                shared.publish_snapshot(&gsm);
            }
            let provider: Arc<dyn StateProvider> =
                Arc::new(MerkleStateProvider::new(shared));
            TaskPreparer::new("validator-test".to_string(), provider)
        }

        fn module_setup() -> (
            String,
            Vec<u8>,
            move_core_types::account_address::AccountAddress,
        ) {
            let addr = move_core_types::account_address::AccountAddress::from_hex_literal(
                "0xdead",
            )
            .unwrap();
            let bytecode = make_module_bytes(
                move_binary_format::file_format::AddressIdentifierIndex(0),
                addr,
                "counter",
            );
            let key = format!("mod:{}::counter", addr.to_hex_literal());
            (key, bytecode, addr)
        }

        /// V1: Create mode — DF not yet present; resolved value_bytes=None.
        #[test]
        fn test_df_create_path() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x70; 32]);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None,
                (&mkey, &mbytes),
            );

            let key_bcs = vec![1, 0, 0, 0, 0, 0, 0, 0];
            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode: DfAccessMode::Create,
                value_type: Some("u64".to_string()),
            }];

            let task = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .expect("prepare ok");
            assert_eq!(task.resolved_inputs.dynamic_fields.len(), 1);
            let rdf = &task.resolved_inputs.dynamic_fields[0];
            assert_eq!(rdf.parent_object_id, parent_id);
            assert_eq!(rdf.mode, DfAccessMode::Create);
            assert!(rdf.value_bytes.is_none());
            assert_eq!(rdf.value_type_tag, "u64");
            // read_set has the df entry with empty value
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let want_key = format!("oid:{}", hex::encode(df_oid.as_bytes()));
            assert!(task.read_set.iter().any(|e| e.key == want_key && e.value.is_empty()));
        }

        /// V2/V3/V4: Read/Mutate/Delete mode on existing DF entry.
        fn check_existing_df_mode(mode: DfAccessMode) {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x71; 32]);
            let key_bcs = vec![2, 0, 0, 0, 0, 0, 0, 0];
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let payload = DfFieldValue {
                name_type_tag: "u64".into(),
                name_bcs: key_bcs.clone(),
                value_type_tag: "u64".into(),
                value_bcs: vec![9u8; 8],
            };
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                Some((df_oid, parent_id, payload.clone())),
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode,
                value_type: None,
            }];

            let task = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .expect("prepare ok");
            assert_eq!(task.resolved_inputs.dynamic_fields.len(), 1);
            let rdf = &task.resolved_inputs.dynamic_fields[0];
            assert_eq!(rdf.mode, mode);
            assert_eq!(rdf.df_object_id, df_oid);
            let got_value = rdf.value_bytes.as_ref().expect("value_bytes set");
            // Production decodes the on-disk `DfFieldValue` wrapper and stores
            // its INNER `value_bcs` here (see L1158–1170). So `got_value`
            // already IS the inner BCS payload (e.g. raw u64 LE bytes for V=u64),
            // not a serialized `DfFieldValue`. Compare directly.
            assert_eq!(got_value, &payload.value_bcs);
            assert_eq!(rdf.value_type_tag, "u64", "value_type_tag read from disk");
        }

        #[test]
        fn test_df_read_path() {
            check_existing_df_mode(DfAccessMode::Read);
        }

        #[test]
        fn test_df_mutate_path() {
            check_existing_df_mode(DfAccessMode::Mutate);
        }

        #[test]
        fn test_df_delete_path() {
            check_existing_df_mode(DfAccessMode::Delete);
        }

        /// V5: parent not declared in input_object_ids/shared_object_ids.
        #[test]
        fn test_df_parent_not_declared() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x72; 32]);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None,
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![]; // parent NOT declared
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode([1u8; 8]),
                mode: DfAccessMode::Create,
                value_type: Some("u64".into()),
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldParentNotDeclared { .. }));
        }

        /// V6: Create mode but df_oid already populated.
        #[test]
        fn test_df_already_exists_on_create() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x73; 32]);
            let key_bcs = vec![3, 0, 0, 0, 0, 0, 0, 0];
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let payload = DfFieldValue {
                name_type_tag: "u64".into(),
                name_bcs: key_bcs.clone(),
                value_type_tag: "u64".into(),
                value_bcs: vec![1u8; 8],
            };
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                Some((df_oid, parent_id, payload)),
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode: DfAccessMode::Create,
                value_type: Some("u64".into()),
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldAlreadyExists { .. }));
        }

        /// V7: Read mode tolerates absent df_oid (fix-df-exists-absent-rejected).
        ///
        /// Pre-fix: TaskPreparer aborted with `DynamicFieldNotFound`, blocking
        /// the VM `exists_internal` native from returning `false`. Post-fix:
        /// Read mode emits a sentinel `ResolvedDynamicField` with
        /// `value_bytes=None`, `envelope_bytes=None`, falling back to the
        /// client-declared value_type (or empty string).
        #[test]
        fn test_df_read_absent_returns_sentinel() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x74; 32]);
            let key_bcs = vec![4u8; 8];
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None, // <-- absent
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode: DfAccessMode::Read,
                value_type: Some("u64".into()),
            }];

            let task = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .expect("Read on absent must succeed");
            assert_eq!(task.resolved_inputs.dynamic_fields.len(), 1);
            let rdf = &task.resolved_inputs.dynamic_fields[0];
            assert_eq!(rdf.df_object_id, df_oid);
            assert_eq!(rdf.mode, DfAccessMode::Read);
            assert!(
                rdf.value_bytes.is_none(),
                "absent-Read MUST yield value_bytes=None"
            );
            assert!(
                rdf.envelope_bytes.is_none(),
                "absent-Read MUST yield envelope_bytes=None"
            );
            assert_eq!(
                rdf.value_type_tag, "u64",
                "client-declared value_type echoed verbatim"
            );
        }

        /// V7b: Mutate on absent df_oid is STILL rejected
        /// (fix-df-exists-absent-rejected design constraint: only Read is
        /// relaxed; Mutate / Delete keep their presence guards).
        #[test]
        fn test_df_mutate_absent_still_rejected() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x76; 32]);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None,
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode([7u8; 8]),
                mode: DfAccessMode::Mutate,
                value_type: Some("u64".into()),
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldNotFound { .. }));
        }

        /// V7c: Delete on absent df_oid is STILL rejected (symmetric to V7b).
        #[test]
        fn test_df_delete_absent_still_rejected() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x77; 32]);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None,
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode([8u8; 8]),
                mode: DfAccessMode::Delete,
                value_type: Some("u64".into()),
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldNotFound { .. }));
        }

        /// V8: DF envelope's ObjectOwner points at a different parent.
        #[test]
        fn test_df_parent_mismatch() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x75; 32]);
            let other_parent = ObjectId::new([0xAA; 32]);
            let key_bcs = vec![5, 0, 0, 0, 0, 0, 0, 0];
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let payload = DfFieldValue {
                name_type_tag: "u64".into(),
                name_bcs: key_bcs.clone(),
                value_type_tag: "u64".into(),
                value_bcs: vec![0u8; 8],
            };
            // seed df entry but ownership -> other_parent (mismatch)
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                Some((df_oid, other_parent, payload)),
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode: DfAccessMode::Read,
                value_type: None,
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldParentMismatch));
        }

        /// V9: Immutable parent + Mutate mode must be rejected.
        #[test]
        fn test_df_immutable_parent_mutate_rejected() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x76; 32]);
            let key_bcs = vec![6, 0, 0, 0, 0, 0, 0, 0];
            let df_oid = derive_df_oid(&parent_id, "u64", &key_bcs);
            let payload = DfFieldValue {
                name_type_tag: "u64".into(),
                name_bcs: key_bcs.clone(),
                value_type_tag: "u64".into(),
                value_bcs: vec![0u8; 8],
            };
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::Immutable,
                Some((df_oid, parent_id, payload)),
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            call.dynamic_field_accesses = vec![DynamicFieldAccess {
                parent_object_id: hex::encode(parent_id.as_bytes()),
                key_type: "u64".to_string(),
                key_bcs_hex: hex::encode(&key_bcs),
                mode: DfAccessMode::Mutate,
                value_type: None,
            }];

            let err = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .unwrap_err();
            assert!(matches!(err, TaskPrepareError::DynamicFieldOnImmutableParent));
        }

        /// V10: empty `dynamic_field_accesses` preserves pre-M4 behaviour.
        #[test]
        fn test_move_call_no_df_back_compat() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0x77; 32]);
            let (mkey, mbytes, maddr) = module_setup();
            let preparer = make_preparer_with_df(
                parent_id,
                setu_types::Ownership::AddressOwner(alice),
                None,
                (&mkey, &mbytes),
            );

            let mut call =
                test_move_call_payload(&maddr.to_hex_literal(), "counter", "inc");
            call.sender = hex::encode(alice.as_bytes());
            call.input_object_ids = vec![parent_id];
            // no DF accesses declared
            let task = preparer
                .prepare_move_call_task(&test_event(), &call, SubnetId::ROOT)
                .expect("prepare ok");
            assert!(task.resolved_inputs.dynamic_fields.is_empty());
        }
    }

    // ════════════════════════════════════════════════════════════
    // PTB event-wire tests (FDP move-vm-phase9-ptb-event-wire)
    // U4-U13 from the test matrix.
    // ════════════════════════════════════════════════════════════
    mod ptb_event_wire_tests {
        use super::*;
        use setu_types::ptb::{CallArg, ObjectArg, ProgrammableTransaction};
        use setu_types::event::MovePtbPayload;

        fn empty_ptb() -> ProgrammableTransaction {
            ProgrammableTransaction {
                inputs: vec![],
                commands: vec![],
                dynamic_field_accesses: vec![],
            }
        }

        /// Compute (envelope_bytes, blake3_digest) for a deterministic
        /// alice-owned envelope at version 1.
        fn make_owned_envelope_for_alice(
            object_id: ObjectId,
        ) -> (setu_types::ObjectEnvelope, [u8; 32]) {
            let alice = setu_types::Address::normalize("alice");
            let env = setu_types::ObjectEnvelope::from_move_result(
                object_id,
                alice,
                1,
                setu_types::Ownership::AddressOwner(alice),
                "0xcafe::test::TestObj".to_string(),
                vec![0u8; 32],
            );
            let bytes = env.to_bytes();
            let digest = *blake3::hash(&bytes).as_bytes();
            (env, digest)
        }

        fn ptb_payload(ptb: ProgrammableTransaction) -> MovePtbPayload {
            let alice = setu_types::Address::normalize("alice");
            MovePtbPayload {
                sender: hex::encode(alice.as_bytes()),
                ptb,
            }
        }

        /// U4 — PTB with only `Pure` inputs needs no SMT lookup.
        #[test]
        fn u4_prepare_ptb_pure_only_succeeds() {
            let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Pure(vec![1, 2, 3]));
            let payload = ptb_payload(ptb);

            let task = preparer
                .prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT)
                .expect("pure-only PTB must succeed");
            assert!(
                task.resolved_inputs.input_objects.is_empty(),
                "Pure inputs must NOT contribute to input_objects"
            );
            // SolverTask::operation must be PTB.
            assert!(matches!(
                task.resolved_inputs.operation,
                setu_types::task::OperationType::ProgrammableTransaction(_)
            ));
        }

        /// U5 — owned object passes ownership + version + digest checks.
        #[test]
        fn u5_prepare_ptb_owned_object_passes() {
            let alice = setu_types::Address::normalize("alice");
            let obj_id = ObjectId::new([0x11; 32]);
            let preparer = make_preparer_with_owned_object(
                obj_id,
                alice,
                setu_types::Ownership::AddressOwner(alice),
                None,
            );
            let (_, digest) = make_owned_envelope_for_alice(obj_id);

            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                obj_id, 1, digest,
            )));
            let payload = ptb_payload(ptb);

            let task = preparer
                .prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT)
                .expect("owned object must pass");
            assert_eq!(task.resolved_inputs.input_objects.len(), 1);
            assert_eq!(task.resolved_inputs.input_objects[0].object_id, obj_id);
            assert_eq!(task.resolved_inputs.input_objects[0].expected_version, 1);
            // read_set must contain the oid envelope entry.
            let oid_key = format!("oid:{}", hex::encode(obj_id.as_bytes()));
            assert!(task.read_set.iter().any(|e| e.key == oid_key));
        }

        /// U6 — wrong owner is rejected with `NotOwnedBySender`.
        #[test]
        fn u6_prepare_ptb_wrong_owner_rejects() {
            let bob = setu_types::Address::normalize("bob");
            let obj_id = ObjectId::new([0x22; 32]);
            // Stored under bob, but sender is alice (via ptb_payload).
            let preparer = make_preparer_with_owned_object(
                obj_id,
                bob,
                setu_types::Ownership::AddressOwner(bob),
                None,
            );
            // Compute digest for the bob-owned envelope.
            let bob_env = setu_types::ObjectEnvelope::from_move_result(
                obj_id,
                bob,
                1,
                setu_types::Ownership::AddressOwner(bob),
                "0xcafe::test::TestObj".to_string(),
                vec![0u8; 32],
            );
            let digest = *blake3::hash(&bob_env.to_bytes()).as_bytes();

            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                obj_id, 1, digest,
            )));
            let payload = ptb_payload(ptb); // sender = alice

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            assert!(
                matches!(result, Err(TaskPrepareError::NotOwnedBySender { .. })),
                "got: {:?}", result
            );
        }

        /// U7 — version mismatch yields `StaleObjectVersion`.
        #[test]
        fn u7_prepare_ptb_stale_version_rejects() {
            let alice = setu_types::Address::normalize("alice");
            let obj_id = ObjectId::new([0x33; 32]);
            let preparer = make_preparer_with_owned_object(
                obj_id,
                alice,
                setu_types::Ownership::AddressOwner(alice),
                None,
            );
            let (_, digest) = make_owned_envelope_for_alice(obj_id);

            let mut ptb = empty_ptb();
            // Client claims version 99, on-chain is 1.
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                obj_id, 99, digest,
            )));
            let payload = ptb_payload(ptb);

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            match result {
                Err(TaskPrepareError::StaleObjectVersion { expected, actual, .. }) => {
                    assert_eq!(expected, 99);
                    assert_eq!(actual, 1);
                }
                other => panic!("expected StaleObjectVersion, got: {:?}", other),
            }
        }

        /// U8 — digest mismatch yields `ObjectDigestMismatch`.
        #[test]
        fn u8_prepare_ptb_digest_mismatch_rejects() {
            let alice = setu_types::Address::normalize("alice");
            let obj_id = ObjectId::new([0x44; 32]);
            let preparer = make_preparer_with_owned_object(
                obj_id,
                alice,
                setu_types::Ownership::AddressOwner(alice),
                None,
            );

            let mut ptb = empty_ptb();
            // Client supplies a wrong digest.
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                obj_id, 1, [0xFFu8; 32],
            )));
            let payload = ptb_payload(ptb);

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            assert!(
                matches!(result, Err(TaskPrepareError::ObjectDigestMismatch { .. })),
                "got: {:?}", result
            );
        }

        /// U9 — `ObjectOwner`-owned objects must NOT be passed through raw
        /// PTB inputs (DF entries belong in `dynamic_field_accesses`).
        #[test]
        fn u9_prepare_ptb_objectowner_rejected() {
            let alice = setu_types::Address::normalize("alice");
            let parent_id = ObjectId::new([0xAB; 32]);
            let df_id = ObjectId::new([0xCD; 32]);

            // Store a DF entry (ObjectOwner = parent).
            let preparer = make_preparer_with_owned_object(
                df_id,
                alice,
                setu_types::Ownership::ObjectOwner(parent_id),
                None,
            );
            // Recompute the actual envelope digest stored under df_id.
            let df_env = setu_types::ObjectEnvelope::from_move_result(
                df_id,
                alice,
                1,
                setu_types::Ownership::ObjectOwner(parent_id),
                "0xcafe::test::TestObj".to_string(),
                vec![0u8; 32],
            );
            let digest = *blake3::hash(&df_env.to_bytes()).as_bytes();

            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                df_id, 1, digest,
            )));
            let payload = ptb_payload(ptb);

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            assert!(
                matches!(result, Err(TaskPrepareError::ObjectOwnerNotAllowedInInputs { .. })),
                "got: {:?}", result
            );
        }

        /// U10 — `ObjectArg::SharedObject` is hard-rejected in Phase 1.
        #[test]
        fn u10_prepare_ptb_shared_object_phase1_rejected() {
            let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
            let obj_id = ObjectId::new([0x55; 32]);

            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Object(ObjectArg::SharedObject {
                id: obj_id,
                initial_shared_version: 1,
                mutable: true,
            }));
            let payload = ptb_payload(ptb);

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            assert!(
                matches!(result, Err(TaskPrepareError::SharedObjectsNotYetSupported { .. })),
                "got: {:?}", result
            );
        }

        /// U11 — referencing an unknown ObjectId yields `ObjectNotFound`.
        #[test]
        fn u11_prepare_ptb_unknown_object_rejects() {
            let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
            let obj_id = ObjectId::new([0x66; 32]);

            let mut ptb = empty_ptb();
            ptb.inputs.push(CallArg::Object(ObjectArg::ImmOrOwnedObject(
                obj_id, 1, [0u8; 32],
            )));
            let payload = ptb_payload(ptb);

            let result =
                preparer.prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT);
            assert!(
                matches!(result, Err(TaskPrepareError::ObjectNotFound(_))),
                "got: {:?}", result
            );
        }

        /// U12 — iter-7 regression gate: PTB lowering of a `Command::MoveCall`
        /// against a v0-family package address with leading-zero bytes
        /// (e.g. `0xcafe`) must reach the `mod:0xcafe::counter` SMT key,
        /// not the padded `mod:0x000…cafe::counter` form.
        ///
        /// Pre-iter-7 hex-encoding-fix the PTB read-set computation used
        /// `format!("0x{}", hex::encode(mc.package.as_bytes()))` which
        /// produced the padded form, so this test would have hit
        /// `ModuleNotFound("mod:0x000…cafe::counter")`.
        ///
        /// See docs/feat/fix-package-addr-hex-encoding/.
        #[test]
        fn u12_ptb_movecall_v0_family_addr_finds_module() {
            use setu_types::ptb::{Command, MoveCall};
            use move_core_types::account_address::AccountAddress;

            // Module is keyed under canonical (zero-stripped) form, matching
            // how `infra_executor::execute_contract_publish` writes it.
            let addr = AccountAddress::from_hex_literal("0xcafe").unwrap();
            let bytecode = make_module_bytes(
                move_binary_format::file_format::AddressIdentifierIndex(0),
                addr,
                "counter",
            );
            let module_key = format!("mod:{}::counter", addr.to_hex_literal());
            assert_eq!(module_key, "mod:0xcafe::counter");
            let preparer = make_preparer_with_module(&module_key, &bytecode);

            // Build a single-Command PTB targeting `0xcafe::counter::increment`.
            // `mc.package` is an ObjectId whose raw bytes left-pad `0xcafe`.
            let mut pkg_bytes = [0u8; 32];
            pkg_bytes[30..].copy_from_slice(&[0xca, 0xfe]);
            let mut ptb = empty_ptb();
            ptb.commands.push(Command::MoveCall(MoveCall {
                package: ObjectId::new(pkg_bytes),
                module: "counter".to_string(),
                function: "increment".to_string(),
                type_arguments: vec![],
                arguments: vec![],
            }));
            let payload = ptb_payload(ptb);

            let task = preparer
                .prepare_move_ptb_task(&test_event(), &payload, SubnetId::ROOT)
                .expect("PTB MoveCall against 0xcafe must resolve module");
            // Module read-set must contain the canonical key.
            assert!(
                task.module_read_set.iter().any(|e| e.key == "mod:0xcafe::counter"),
                "module_read_set missing canonical key; got: {:?}",
                task.module_read_set.iter().map(|e| &e.key).collect::<Vec<_>>()
            );
        }
    }
}
