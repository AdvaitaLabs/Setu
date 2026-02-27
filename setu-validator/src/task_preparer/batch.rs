//! Batch task preparation for high-throughput scenarios
//!
//! Use [`BatchTaskPreparer`] for preparing multiple SolverTasks efficiently.
//! This reduces lock acquisitions from 5-6N to just 2 for N transfers.

use setu_types::task::{
    SolverTask, ResolvedInputs, ResolvedObject,
    GasBudget, ReadSetEntry,
};
use setu_types::{Event, EventType, SubnetId, ObjectId};
use setu_types::event::VLCSnapshot;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{TaskPrepareError, CoinInfo, SimpleMerkleProof, BatchStateSnapshot};
use crate::coin_reservation::{CoinReservationManager, ReservationHandle};

/// Result of batch task preparation
#[derive(Debug)]
pub struct BatchPrepareResult {
    /// Successfully prepared tasks
    pub tasks: Vec<SolverTask>,
    /// Failed transfers with error messages
    pub failures: Vec<(setu_types::Transfer, TaskPrepareError)>,
    /// Preparation statistics
    pub stats: BatchPrepareStats,
    /// Snapshot version used (for staleness detection)
    pub snapshot_version: u64,
    /// Reservation handles for each successful task (indexed by task position)
    /// Used with `prepare_transfers_batch_with_reservation`
    pub reservations: Vec<Option<ReservationHandle>>,
}

/// Statistics from batch preparation
#[derive(Debug, Default)]
pub struct BatchPrepareStats {
    /// Total transfers in the batch
    pub total_transfers: usize,
    /// Successfully prepared tasks
    pub successful: usize,
    /// Failed transfers
    pub failed: usize,
    /// Unique (sender, subnet) pairs
    pub unique_sender_subnet_pairs: usize,
    /// Coins selected
    pub coins_selected: usize,
    /// Same-sender balance conflicts (overdraft prevention)
    pub same_sender_conflicts: usize,
}

/// Batch task preparer for high-throughput scenarios
///
/// Optimizes task preparation by:
/// 1. Creating a single `BatchStateSnapshot` (TWO lock acquisitions total)
/// 2. Grouping transfers by sender with conflict detection
/// 3. Lock-free task assembly using cached state_root
///
/// ## Performance
///
/// | Metric | Before (per-tx) | After (batch) | Improvement |
/// |--------|-----------------|---------------|-------------|
/// | Lock acquisitions | 5-6N | 2 | ~99.6% |
/// | state_root calculations | N | 1 | ~99.9% |
///
/// ## Usage
///
/// ```rust,ignore
/// let batch_preparer = BatchTaskPreparer::new(validator_id, state_provider);
/// let result = batch_preparer.prepare_transfers_batch(&transfers);
/// for task in result.tasks {
///     // spawn TEE execution
/// }
/// ```
pub struct BatchTaskPreparer {
    validator_id: String,
    state_provider: Arc<setu_storage::MerkleStateProvider>,
}

impl BatchTaskPreparer {
    /// Create a new BatchTaskPreparer
    pub fn new(
        validator_id: String,
        state_provider: Arc<setu_storage::MerkleStateProvider>,
    ) -> Self {
        Self {
            validator_id,
            state_provider,
        }
    }

    /// Create a BatchTaskPreparer with pre-initialized test accounts
    ///
    /// Same initialization as `TaskPreparer::new_for_testing()`:
    /// - `alice`, `bob`, `charlie`: 10,000,000 balance each
    /// - `user_01` to `user_17`: 5,000,000 balance each
    pub fn new_for_testing(validator_id: String) -> Self {
        // Use shared test utility to avoid code duplication
        let state_provider = super::create_test_state_provider();
        Self::new(validator_id, state_provider)
    }

    /// Prepare multiple transfers in a single batch
    ///
    /// This is the main entry point for batch task preparation.
    ///
    /// ## Optimization Summary
    ///
    /// - Lock acquisitions: 2 (regardless of batch size)
    /// - state_root calculations: 1 (cached in snapshot)
    /// - Balance accumulation detection: prevents overdraft on same coin
    pub fn prepare_transfers_batch(
        &self,
        transfers: &[setu_types::Transfer],
    ) -> BatchPrepareResult {
        let mut stats = BatchPrepareStats {
            total_transfers: transfers.len(),
            ..Default::default()
        };
        let mut failures: Vec<(setu_types::Transfer, TaskPrepareError)> = Vec::new();

        if transfers.is_empty() {
            return BatchPrepareResult {
                tasks: Vec::new(),
                failures,
                stats,
                snapshot_version: 0,
                reservations: Vec::new(),
            };
        }

        // ════════════════════════════════════════════════════════════════
        // PHASE 1: Collect unique (sender, subnet_id) pairs (NO LOCK)
        // ════════════════════════════════════════════════════════════════
        let mut sender_subnet_set: HashSet<(String, SubnetId)> = HashSet::new();
        for transfer in transfers {
            let subnet_id = Self::resolve_subnet_id(transfer);
            sender_subnet_set.insert((transfer.from.clone(), subnet_id));
        }
        stats.unique_sender_subnet_pairs = sender_subnet_set.len();

        // Convert to reference pairs for create_batch_snapshot
        let sender_subnet_pairs: Vec<(String, SubnetId)> = sender_subnet_set.into_iter().collect();
        let pairs_ref: Vec<(&str, &SubnetId)> = sender_subnet_pairs
            .iter()
            .map(|(s, id)| (s.as_str(), id))
            .collect();

        // ════════════════════════════════════════════════════════════════
        // PHASE 2: Create BatchStateSnapshot (2 LOCK ACQUISITIONS)
        // ════════════════════════════════════════════════════════════════
        let snapshot = self.state_provider.create_batch_snapshot(&pairs_ref);
        let cached_state_root = snapshot.state_root();
        let snapshot_version = snapshot.snapshot_version();

        debug!(
            pairs = pairs_ref.len(),
            state_root = %hex::encode(&cached_state_root[..8]),
            "Created BatchStateSnapshot"
        );

        // ════════════════════════════════════════════════════════════════
        // PHASE 3: Select coins with BALANCE ACCUMULATION detection (NO LOCK)
        //
        // Track remaining balance per coin to prevent overdraft when
        // same sender sends multiple transfers that individually fit
        // but together exceed available balance.
        // ════════════════════════════════════════════════════════════════
        let mut selected_coins: HashMap<usize, (CoinInfo, SubnetId)> = HashMap::new();
        let mut remaining_balance: HashMap<ObjectId, u64> = HashMap::new();

        // Group transfers by (sender, subnet_id) for conflict detection
        let mut by_sender_subnet: HashMap<(String, SubnetId), Vec<(usize, &setu_types::Transfer)>> =
            HashMap::new();
        for (idx, transfer) in transfers.iter().enumerate() {
            let subnet_id = Self::resolve_subnet_id(transfer);
            by_sender_subnet
                .entry((transfer.from.clone(), subnet_id))
                .or_default()
                .push((idx, transfer));
        }

        // Process each (sender, subnet)'s transfers sequentially
        for ((sender, subnet_id), sender_transfers) in by_sender_subnet {
            let available_coins = snapshot
                .get_coins_for_sender_subnet(&sender, &subnet_id)
                .cloned()
                .unwrap_or_default();

            for (idx, transfer) in sender_transfers {
                // Filter by remaining balance (overdraft prevention)
                let eligible: Vec<_> = available_coins
                    .iter()
                    .filter(|c| {
                        let remaining = remaining_balance
                            .get(&c.object_id)
                            .copied()
                            .unwrap_or(c.balance);
                        remaining >= transfer.amount
                    })
                    .cloned()
                    .collect();

                match Self::select_coin_for_transfer(&eligible, transfer.amount) {
                    Ok(coin) => {
                        // Deduct from remaining balance
                        let entry = remaining_balance
                            .entry(coin.object_id.clone())
                            .or_insert(coin.balance);
                        *entry = entry.saturating_sub(transfer.amount);
                        selected_coins.insert(idx, (coin, subnet_id.clone()));
                    }
                    Err(e) => {
                        if !available_coins.is_empty() && eligible.is_empty() {
                            stats.same_sender_conflicts += 1;
                        }
                        failures.push((transfer.clone(), e));
                    }
                }
            }
        }

        stats.coins_selected = selected_coins.len();

        // ════════════════════════════════════════════════════════════════
        // PHASE 4: Assemble tasks (NO LOCK - pure computation)
        // ════════════════════════════════════════════════════════════════
        let mut tasks = Vec::with_capacity(selected_coins.len());

        for (idx, (coin, subnet_id)) in selected_coins {
            let transfer = &transfers[idx];

            // Get data from snapshot (NO LOCK)
            let object_data = match snapshot.get_object(&coin.object_id) {
                Some(data) => data.clone(),
                None => {
                    failures.push((
                        transfer.clone(),
                        TaskPrepareError::ObjectNotFound(hex::encode(&coin.object_id)),
                    ));
                    continue;
                }
            };

            let proof = snapshot.get_proof(&coin.object_id).cloned();

            // Assemble task with CACHED state_root
            match self.assemble_task(
                transfer,
                &coin,
                object_data,
                proof.as_ref(),
                &subnet_id,
                cached_state_root,
                &snapshot,
            ) {
                Ok(task) => tasks.push(task),
                Err(e) => failures.push((transfer.clone(), e)),
            }
        }

        stats.successful = tasks.len();
        stats.failed = failures.len();

        info!(
            total = stats.total_transfers,
            successful = stats.successful,
            failed = stats.failed,
            conflicts = stats.same_sender_conflicts,
            "Batch task preparation completed"
        );

        BatchPrepareResult {
            tasks,
            failures,
            stats,
            snapshot_version,
            reservations: vec![], // No reservations in basic mode
        }
    }

    /// Prepare multiple transfers with coin reservation for cross-batch double-spend prevention
    ///
    /// This method is similar to `prepare_transfers_batch`, but also reserves coins
    /// via `CoinReservationManager` to prevent the same coin from being selected
    /// by concurrent batch preparations.
    ///
    /// ## Usage
    ///
    /// ```rust,ignore
    /// let result = batch_preparer.prepare_transfers_batch_with_reservation(
    ///     &transfers,
    ///     &coin_reservation_manager,
    /// );
    ///
    /// for (idx, task) in result.tasks.iter().enumerate() {
    ///     let reservation = result.reservations[idx].clone();
    ///     tee_executor.spawn_tee_task_with_reservation(
    ///         transfer_id, solver_id, task.clone(), reservation
    ///     );
    /// }
    /// ```
    pub fn prepare_transfers_batch_with_reservation(
        &self,
        transfers: &[setu_types::Transfer],
        reservation_mgr: &CoinReservationManager,
    ) -> BatchPrepareResult {
        let mut stats = BatchPrepareStats {
            total_transfers: transfers.len(),
            ..Default::default()
        };
        let mut failures: Vec<(setu_types::Transfer, TaskPrepareError)> = Vec::new();

        if transfers.is_empty() {
            return BatchPrepareResult {
                tasks: Vec::new(),
                failures,
                stats,
                snapshot_version: 0,
                reservations: vec![],
            };
        }

        // ════════════════════════════════════════════════════════════════
        // PHASE 1: Collect unique (sender, subnet_id) pairs (NO LOCK)
        // ════════════════════════════════════════════════════════════════
        let mut sender_subnet_set: HashSet<(String, SubnetId)> = HashSet::new();
        for transfer in transfers {
            let subnet_id = Self::resolve_subnet_id(transfer);
            sender_subnet_set.insert((transfer.from.clone(), subnet_id));
        }
        stats.unique_sender_subnet_pairs = sender_subnet_set.len();

        let sender_subnet_pairs: Vec<(String, SubnetId)> = sender_subnet_set.into_iter().collect();
        let pairs_ref: Vec<(&str, &SubnetId)> = sender_subnet_pairs
            .iter()
            .map(|(s, id)| (s.as_str(), id))
            .collect();

        // ════════════════════════════════════════════════════════════════
        // PHASE 2: Create BatchStateSnapshot (2 LOCK ACQUISITIONS)
        // ════════════════════════════════════════════════════════════════
        let snapshot = self.state_provider.create_batch_snapshot(&pairs_ref);
        let cached_state_root = snapshot.state_root();
        let snapshot_version = snapshot.snapshot_version();

        debug!(
            pairs = pairs_ref.len(),
            state_root = %hex::encode(&cached_state_root[..8]),
            "Created BatchStateSnapshot (with reservation)"
        );

        // ════════════════════════════════════════════════════════════════
        // PHASE 3: Select coins WITH RESERVATION
        //
        // For each selected coin, try to reserve it. If reservation fails
        // (coin already reserved by another batch), try the next eligible coin.
        // ════════════════════════════════════════════════════════════════
        let mut selected_coins: HashMap<usize, (CoinInfo, SubnetId, ReservationHandle)> = HashMap::new();
        let mut remaining_balance: HashMap<ObjectId, u64> = HashMap::new();

        // Group transfers by (sender, subnet_id)
        let mut by_sender_subnet: HashMap<(String, SubnetId), Vec<(usize, &setu_types::Transfer)>> =
            HashMap::new();
        for (idx, transfer) in transfers.iter().enumerate() {
            let subnet_id = Self::resolve_subnet_id(transfer);
            by_sender_subnet
                .entry((transfer.from.clone(), subnet_id))
                .or_default()
                .push((idx, transfer));
        }

        // Process each (sender, subnet)'s transfers
        for ((sender, subnet_id), sender_transfers) in by_sender_subnet {
            let available_coins = snapshot
                .get_coins_for_sender_subnet(&sender, &subnet_id)
                .cloned()
                .unwrap_or_default();

            for (idx, transfer) in sender_transfers {
                // Filter by remaining balance (overdraft prevention)
                let eligible: Vec<_> = available_coins
                    .iter()
                    .filter(|c| {
                        let remaining = remaining_balance
                            .get(&c.object_id)
                            .copied()
                            .unwrap_or(c.balance);
                        remaining >= transfer.amount
                    })
                    .cloned()
                    .collect();

                if eligible.is_empty() {
                    if !available_coins.is_empty() {
                        stats.same_sender_conflicts += 1;
                    }
                    failures.push((
                        transfer.clone(),
                        TaskPrepareError::NoCoinsFound(
                            "no eligible coins with sufficient balance".to_string(),
                        ),
                    ));
                    continue;
                }

                // Sort by balance (smallest first)
                let mut sorted_coins = eligible.clone();
                sorted_coins.sort_by_key(|c| c.balance);

                // Try to reserve each coin until one succeeds
                let mut reserved_coin: Option<(CoinInfo, ReservationHandle)> = None;
                for coin in sorted_coins {
                    if let Some(handle) = reservation_mgr.try_reserve(
                        &coin.object_id,
                        transfer.amount,
                        &transfer.id,
                    ) {
                        reserved_coin = Some((coin, handle));
                        break;
                    }
                    // Coin already reserved by another batch, try next
                }

                match reserved_coin {
                    Some((coin, handle)) => {
                        // Deduct from remaining balance
                        let entry = remaining_balance
                            .entry(coin.object_id.clone())
                            .or_insert(coin.balance);
                        *entry = entry.saturating_sub(transfer.amount);
                        selected_coins.insert(idx, (coin, subnet_id.clone(), handle));
                    }
                    None => {
                        // All eligible coins are reserved by other batches
                        failures.push((
                            transfer.clone(),
                            TaskPrepareError::AllCoinsReserved {
                                sender: sender.clone(),
                                coin_count: eligible.len(),
                            },
                        ));
                    }
                }
            }
        }

        stats.coins_selected = selected_coins.len();

        // ════════════════════════════════════════════════════════════════
        // PHASE 4: Assemble tasks (NO LOCK)
        // ════════════════════════════════════════════════════════════════
        let mut tasks = Vec::with_capacity(selected_coins.len());
        let mut reservations: Vec<Option<ReservationHandle>> = Vec::with_capacity(selected_coins.len());

        // Sort by index to maintain order
        let mut sorted_selected: Vec<_> = selected_coins.into_iter().collect();
        sorted_selected.sort_by_key(|(idx, _)| *idx);

        for (idx, (coin, subnet_id, handle)) in sorted_selected {
            let transfer = &transfers[idx];

            let object_data = match snapshot.get_object(&coin.object_id) {
                Some(data) => data.clone(),
                None => {
                    // Release reservation since we can't proceed
                    reservation_mgr.release(&handle);
                    failures.push((
                        transfer.clone(),
                        TaskPrepareError::ObjectNotFound(hex::encode(&coin.object_id)),
                    ));
                    continue;
                }
            };

            let proof = snapshot.get_proof(&coin.object_id).cloned();

            match self.assemble_task(
                transfer,
                &coin,
                object_data,
                proof.as_ref(),
                &subnet_id,
                cached_state_root,
                &snapshot,
            ) {
                Ok(task) => {
                    tasks.push(task);
                    reservations.push(Some(handle));
                }
                Err(e) => {
                    // Release reservation on assembly failure
                    reservation_mgr.release(&handle);
                    failures.push((transfer.clone(), e));
                }
            }
        }

        stats.successful = tasks.len();
        stats.failed = failures.len();

        info!(
            total = stats.total_transfers,
            successful = stats.successful,
            failed = stats.failed,
            conflicts = stats.same_sender_conflicts,
            reserved = reservations.iter().filter(|r| r.is_some()).count(),
            "Batch task preparation with reservation completed"
        );

        BatchPrepareResult {
            tasks,
            failures,
            stats,
            snapshot_version,
            reservations,
        }
    }

    /// Resolve subnet_id from transfer (defaults to ROOT)
    fn resolve_subnet_id(transfer: &setu_types::Transfer) -> SubnetId {
        transfer
            .subnet_id
            .as_ref()
            .and_then(|s| SubnetId::from_hex(s).ok())
            .unwrap_or(SubnetId::ROOT)
    }

    /// Select best coin for a transfer (smallest sufficient)
    fn select_coin_for_transfer(
        coins: &[CoinInfo],
        _amount: u64,  // Used by caller for pre-filtering; kept for API consistency
    ) -> Result<CoinInfo, TaskPrepareError> {
        if coins.is_empty() {
            return Err(TaskPrepareError::NoCoinsFound(
                "no eligible coins with sufficient balance".to_string(),
            ));
        }

        let mut sorted_coins = coins.to_vec();
        sorted_coins.sort_by_key(|c| c.balance);

        // Return smallest coin (already filtered for sufficient balance by caller)
        Ok(sorted_coins.remove(0))
    }

    /// Assemble a single SolverTask from pre-fetched data
    fn assemble_task(
        &self,
        transfer: &setu_types::Transfer,
        coin: &CoinInfo,
        object_data: Vec<u8>,
        proof: Option<&SimpleMerkleProof>,
        subnet_id: &SubnetId,
        pre_state_root: [u8; 32],
        snapshot: &BatchStateSnapshot,
    ) -> Result<SolverTask, TaskPrepareError> {
        // Build ResolvedInputs
        let resolved_coin = ResolvedObject {
            object_id: coin.object_id.clone(),
            object_type: "Coin".to_string(),
            expected_version: coin.version,
        };
        let resolved_inputs = ResolvedInputs::transfer(resolved_coin, transfer.amount);

        // Build read_set
        let read_set = vec![ReadSetEntry::new(
            format!("coin:{}", hex::encode(&coin.object_id)),
            object_data,
        )
        .with_proof(
            proof
                .map(|p| bcs::to_bytes(p).unwrap_or_default())
                .unwrap_or_default(),
        )];

        // Derive parent_ids from snapshot (NO LOCK)
        let parent_ids = self.derive_dependencies_from_snapshot(&coin.object_id, snapshot);

        // Create Event
        let event = self.create_event_from_transfer(transfer, parent_ids)?;

        // Generate task_id using CACHED state_root
        let task_id = SolverTask::generate_task_id(&event, &pre_state_root);

        // Create SolverTask
        let task = SolverTask::new(task_id, event, resolved_inputs, pre_state_root, subnet_id.clone())
            .with_read_set(read_set)
            .with_gas_budget(GasBudget::default());

        Ok(task)
    }

    /// Derive parent_ids from BatchStateSnapshot (NO LOCK)
    fn derive_dependencies_from_snapshot(
        &self,
        coin_object_id: &ObjectId,
        snapshot: &BatchStateSnapshot,
    ) -> Vec<String> {
        snapshot
            .get_last_modifying_event(coin_object_id)
            .map(|event_id| vec![event_id.clone()])
            .unwrap_or_default()
    }

    /// Create Event from Transfer with pre-derived parent_ids
    fn create_event_from_transfer(
        &self,
        transfer: &setu_types::Transfer,
        parent_ids: Vec<String>,
    ) -> Result<Event, TaskPrepareError> {
        let vlc_snapshot = match &transfer.assigned_vlc {
            Some(vlc) => {
                let mut snapshot = VLCSnapshot::for_node(vlc.validator_id.clone());
                snapshot.logical_time = vlc.logical_time;
                snapshot.physical_time = vlc.physical_time;
                snapshot
            }
            None => {
                let now_nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                warn!(
                    transfer_id = %transfer.id,
                    "Transfer missing assigned_vlc, using timestamp fallback"
                );
                let mut snapshot = VLCSnapshot::for_node(self.validator_id.clone());
                snapshot.logical_time = now_nanos;
                snapshot.physical_time = now_nanos / 1_000_000;
                snapshot
            }
        };

        let mut event = Event::new(
            EventType::Transfer,
            parent_ids,
            vlc_snapshot,
            self.validator_id.clone(),
        );

        event = event.with_transfer(transfer.clone());

        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Transfer, TransferType};
    use setu_types::task::OperationType;

    #[test]
    fn test_batch_prepare_single_transfer() {
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());
        let transfer = Transfer::new("batch-tx-1", "alice", "bob", 100)
            .with_type(TransferType::FluxTransfer)
            .with_power(10);

        let result = preparer.prepare_transfers_batch(&[transfer]);

        assert_eq!(result.stats.total_transfers, 1);
        assert_eq!(result.stats.successful, 1);
        assert_eq!(result.stats.failed, 0);
        assert_eq!(result.tasks.len(), 1);
        assert!(result.failures.is_empty());

        // Verify the task is properly formed
        let task = &result.tasks[0];
        assert_ne!(task.task_id, [0u8; 32]);
        match &task.resolved_inputs.operation {
            OperationType::Transfer { amount, .. } => {
                assert_eq!(*amount, 100);
            }
            _ => panic!("Expected Transfer operation"),
        }
    }

    #[test]
    fn test_batch_prepare_multiple_senders() {
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());

        let transfers = vec![
            Transfer::new("batch-tx-1", "alice", "bob", 100)
                .with_type(TransferType::FluxTransfer),
            Transfer::new("batch-tx-2", "bob", "charlie", 200)
                .with_type(TransferType::FluxTransfer),
            Transfer::new("batch-tx-3", "charlie", "alice", 300)
                .with_type(TransferType::FluxTransfer),
        ];

        let result = preparer.prepare_transfers_batch(&transfers);

        assert_eq!(result.stats.total_transfers, 3);
        assert_eq!(result.stats.successful, 3);
        assert_eq!(result.stats.failed, 0);
        assert_eq!(result.stats.unique_sender_subnet_pairs, 3);
        assert_eq!(result.tasks.len(), 3);
    }

    #[test]
    fn test_batch_prepare_same_sender_overdraft() {
        // Test balance accumulation detection (overdraft prevention)
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());

        // alice has 10,000,000 balance (single coin)
        // Send 3 transfers totaling 15M - should detect overdraft
        let transfers = vec![
            Transfer::new("tx-1", "alice", "bob", 6_000_000)
                .with_type(TransferType::FluxTransfer),
            Transfer::new("tx-2", "alice", "charlie", 6_000_000)
                .with_type(TransferType::FluxTransfer),
            Transfer::new("tx-3", "alice", "bob", 3_000_000)  // This should fail
                .with_type(TransferType::FluxTransfer),
        ];

        let result = preparer.prepare_transfers_batch(&transfers);

        // Two should succeed, one should fail due to overdraft prevention
        assert_eq!(result.stats.total_transfers, 3);
        // Note: The order of processing may vary, but at most 2 can succeed
        // with total amount <= 10M
        assert!(result.stats.successful <= 2, "At most 2 should succeed (overdraft)");
        assert!(result.stats.failed >= 1, "At least 1 should fail (overdraft)");
        assert!(
            result.stats.same_sender_conflicts >= 1,
            "Should detect same-sender conflict"
        );
    }

    #[test]
    fn test_batch_prepare_empty_batch() {
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());

        let result = preparer.prepare_transfers_batch(&[]);

        assert_eq!(result.stats.total_transfers, 0);
        assert_eq!(result.stats.successful, 0);
        assert_eq!(result.stats.failed, 0);
        assert!(result.tasks.is_empty());
        assert!(result.failures.is_empty());
        assert_eq!(result.snapshot_version, 0);
    }

    #[test]
    fn test_batch_prepare_insufficient_balance() {
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());

        // alice only has 10,000,000 - requesting 100M should fail
        let transfer = Transfer::new("tx-over", "alice", "bob", 100_000_000)
            .with_type(TransferType::FluxTransfer);

        let result = preparer.prepare_transfers_batch(&[transfer]);

        assert_eq!(result.stats.total_transfers, 1);
        assert_eq!(result.stats.successful, 0);
        assert_eq!(result.stats.failed, 1);
        assert!(result.tasks.is_empty());
        assert_eq!(result.failures.len(), 1);

        // Verify error type
        match &result.failures[0].1 {
            TaskPrepareError::InsufficientBalance { .. } => {}
            TaskPrepareError::NoCoinsFound(_) => {}  // Also acceptable
            e => panic!("Expected InsufficientBalance or NoCoinsFound, got: {:?}", e),
        }
    }

    #[test]
    fn test_batch_prepare_stats_accuracy() {
        let preparer = BatchTaskPreparer::new_for_testing("validator-1".to_string());

        let transfers = vec![
            // alice -> bob: should succeed
            Transfer::new("tx-1", "alice", "bob", 1_000)
                .with_type(TransferType::FluxTransfer),
            // alice -> charlie: should succeed (different transfer, same coin)
            Transfer::new("tx-2", "alice", "charlie", 2_000)
                .with_type(TransferType::FluxTransfer),
            // bob -> charlie: should succeed (different sender)
            Transfer::new("tx-3", "bob", "charlie", 3_000)
                .with_type(TransferType::FluxTransfer),
        ];

        let result = preparer.prepare_transfers_batch(&transfers);

        assert_eq!(result.stats.total_transfers, 3);
        assert_eq!(result.stats.successful, 3);
        assert_eq!(result.stats.failed, 0);
        // alice and bob are 2 unique senders (all in ROOT subnet)
        assert_eq!(result.stats.unique_sender_subnet_pairs, 2);
        assert_eq!(result.stats.coins_selected, 3);
    }
}
