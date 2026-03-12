//! Mock enclave implementation for development and testing.
//!
//! The MockEnclave simulates TEE behavior without requiring actual hardware.
//! It provides:
//! - Simulated STF execution via setu-runtime
//! - Mock attestations
//! - Deterministic behavior for testing
//!
//! ## Architecture
//!
//! ```text
//! User Request (Transfer: from/to/amount)
//!       │
//!       ▼
//! MockEnclave.execute_stf()
//!       │
//!       ├── execute_single_event()
//!       │         │
//!       │         ▼
//!       │   setu-runtime::RuntimeExecutor  ← Object-based STF
//!       │         • execute_simple_transfer()
//!       │         • Manages Coin objects internally
//!       │         • Future: MoveVM integration
//!       │
//!       └── generate_attestation() → Mock attestation
//!
//! State Model:
//! - External: Account-based (Transfer: from/to/amount)
//! - Internal: Object-based (Coin objects with ownership)
//! - setu-runtime bridges the two models
//! ```

use crate::{
    stf::{ExecutionStats, FailedEvent, StateDiff, StfError, StfInput, StfOutput, StfResult, WriteSetEntry},
    traits::{EnclaveConfig, EnclaveInfo, EnclavePlatform, EnclaveRuntime},
};
// Use types from setu-types (canonical source)
use setu_types::task::{
    Attestation, AttestationData,
    ResolvedInputs, GasUsage,
    ReadSetEntry,
};
use async_trait::async_trait;
use setu_runtime::{RuntimeExecutor, ExecutionContext, InMemoryStateStore, StateStore};
use setu_types::{EventId, create_coin, Address, Object, CoinData, ObjectId, Balance, CoinType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// CoinState from storage layer - matches storage/src/state_provider.rs
/// 
/// TEE receives raw CoinState (BCS) from read_set so it can:
/// 1. Verify Merkle proof (hash must match what's stored in tree)
/// 2. Convert to Object<CoinData> for runtime execution
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CoinState {
    pub owner: String,
    pub balance: u64,
    pub version: u64,
    #[serde(default = "default_coin_type")]
    pub coin_type: String,
}

fn default_coin_type() -> String {
    "ROOT".to_string()  // ROOT subnet's native token (consistent with storage)
}

/// Mock enclave measurement (constant for testing)
const MOCK_MEASUREMENT: [u8; 32] = [
    0x4d, 0x4f, 0x43, 0x4b, // "MOCK"
    0x5f, 0x45, 0x4e, 0x43, // "_ENC"
    0x4c, 0x41, 0x56, 0x45, // "LAVE"
    0x5f, 0x56, 0x31, 0x00, // "_V1\0"
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

/// Mock enclave for development and testing
/// 
/// This enclave simulates TEE execution by calling setu-runtime
/// for actual state transition logic. In production, this would be
/// replaced with a real TEE implementation (e.g., NitroEnclave).
pub struct MockEnclave {
    config: EnclaveConfig,
    /// Runtime executor for object-based state transitions
    /// Uses InMemoryStateStore for testing
    runtime: Arc<RwLock<RuntimeExecutor<InMemoryStateStore>>>,
    /// Legacy state map (for non-transfer events and backward compatibility)
    legacy_state: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// Execution counter for statistics
    execution_count: Arc<RwLock<u64>>,
}

impl MockEnclave {
    /// Create a new mock enclave with the given configuration
    pub fn new(config: EnclaveConfig) -> Self {
        let store = InMemoryStateStore::new();
        let runtime = RuntimeExecutor::new(store);
        
        Self {
            config,
            runtime: Arc::new(RwLock::new(runtime)),
            legacy_state: Arc::new(RwLock::new(HashMap::new())),
            execution_count: Arc::new(RwLock::new(0)),
        }
    }
    
    /// Create a mock enclave with default configuration
    pub fn default_with_solver_id(solver_id: String) -> Self {
        let config = EnclaveConfig::default().with_solver_id(solver_id);
        Self::new(config)
    }
    
    /// Initialize account with balance (for testing)
    /// Creates a Coin object owned by the address (must be hex format)
    pub async fn init_account(&self, address: &str, balance: u64) {
        let addr = Address::from_hex(address)
            .expect("init_account requires valid hex address");
        let coin = create_coin(addr, balance);
        let coin_id = *coin.id();
        
        let mut runtime = self.runtime.write().await;
        runtime.state_mut().set_object(coin_id, coin).expect("Failed to init account");
        
        info!(address = %address, balance = balance, coin_id = %coin_id, "Initialized account");
    }
    
    /// Get the current execution count
    pub async fn execution_count(&self) -> u64 {
        *self.execution_count.read().await
    }
    
    /// Build temporary state from read_set (solver-tee3 architecture)
    /// 
    /// This is the key function that loads objects from read_set into
    /// a fresh InMemoryStateStore, making the TEE execution truly stateless.
    /// 
    /// ## Format
    /// 
    /// read_set entries use key format: "oid:{hex_object_id}"
    /// value is BCS-serialized CoinState (raw storage format)
    /// 
    /// ## Security
    /// 
    /// TEE receives raw CoinState so it can verify Merkle proof:
    /// - hash(CoinState) must match the leaf in Merkle tree
    /// - If TaskPreparer converted to Object<CoinData>, verification would fail
    /// 
    /// ## Conversion
    /// 
    /// After verification, TEE converts CoinState → Object<CoinData> for runtime
    fn build_state_from_read_set(&self, read_set: &[ReadSetEntry]) -> StfResult<InMemoryStateStore> {
        let mut store = InMemoryStateStore::new();
        let mut loaded_count = 0;
        
        for entry in read_set {
            // Parse object key: "oid:{hex_object_id}"
            let hex_id = entry.key.strip_prefix("oid:");
            
            if let Some(hex_id) = hex_id {
                // Parse ObjectId from hex string
                let object_id = ObjectId::from_hex(hex_id)
                    .map_err(|e| StfError::InvalidResolvedInputs(format!("Invalid object ID: {}", e)))?;
                
                // TODO: Verify Merkle proof here (for production)
                // verify_proof(leaf_hash, entry.proof, pre_state_root)?;
                
                // Deserialize CoinState from BCS (raw storage format)
                let coin_state: CoinState = bcs::from_bytes(&entry.value)
                    .map_err(|e| StfError::InvalidResolvedInputs(format!(
                        "Failed to deserialize CoinState {}: {}", hex_id, e
                    )))?;
                
                // Convert CoinState → Object<CoinData> for runtime
                // CoinState.owner is stored in hex format ("0x..."), use from_hex.
                let owner = Address::from_hex(&coin_state.owner)
                    .map_err(|e| StfError::InvalidResolvedInputs(format!(
                        "Invalid owner address in CoinState {}: {}", hex_id, e
                    )))?;
                let coin_data = CoinData {
                    coin_type: CoinType::new(&coin_state.coin_type),
                    balance: Balance::new(coin_state.balance),
                };
                
                // Use deterministic timestamp (0) instead of SystemTime::now().
                // created_at / updated_at are NOT included in CoinState (the consensus-
                // critical BCS format), so they don't affect state hashes or conflict
                // detection. Using 0 eliminates non-determinism in the TEE path.
                let coin_object = Object {
                    metadata: setu_types::ObjectMetadata {
                        id: object_id,
                        version: coin_state.version,
                        digest: setu_types::ObjectDigest::ZERO,
                        object_type: setu_types::ObjectType::OwnedObject,
                        owner: Some(owner),
                        ownership: setu_types::Ownership::AddressOwner(owner),
                        created_at: 0,
                        updated_at: 0,
                    },
                    data: coin_data,
                };
                
                // Store in the temporary state
                store.set_object(object_id, coin_object)
                    .map_err(|e| StfError::InternalError(format!("Failed to store object: {}", e)))?;
                
                loaded_count += 1;
                debug!(
                    object_id = %hex_id,
                    owner = %coin_state.owner,
                    balance = coin_state.balance,
                    "Loaded coin from read_set"
                );
            }
        }
        
        info!(
            loaded_count = loaded_count,
            total_entries = read_set.len(),
            "Built temporary state from read_set"
        );
        
        Ok(store)
    }
    
    /// Simulate applying events to state using self.runtime (LEGACY mode)
    /// 
    /// WARNING: This uses the shared self.runtime which can cause race conditions
    /// in concurrent execution. For solver-tee3 mode, use simulate_execution_isolated().
    /// 
    /// This method is kept for backward compatibility with non-read_set executions.
    async fn simulate_execution(
        &self, 
        input: &StfInput,
        _local_runtime: Option<RuntimeExecutor<InMemoryStateStore>>,
    ) -> StfResult<(StateDiff, Vec<EventId>, Vec<FailedEvent>)> {
        let start = std::time::Instant::now();
        let mut diff = StateDiff::new();
        let mut processed = Vec::new();
        let mut failed = Vec::new();
        
        // Legacy mode: acquire write lock once for the entire execution.
        // This eliminates the TOCTOU gap from the previous read-then-write pattern
        // and unifies the code path with simulate_execution_isolated.
        let mut runtime_guard = self.runtime.write().await;
        
        for event in &input.events {
            // Check timeout
            if start.elapsed().as_millis() as u64 > self.config.max_execution_time_ms {
                return Err(StfError::ExecutionTimeout);
            }
            
            // Execute event using shared runtime (acquired write lock above)
            let result = self.execute_single_event_with_runtime(
                event, &input.resolved_inputs, &mut diff, &mut *runtime_guard,
            ).await;
            
            match result {
                Ok(()) => {
                    processed.push(event.id.clone());
                }
                Err(reason) => {
                    failed.push(FailedEvent {
                        event_id: event.id.clone(),
                        reason,
                    });
                }
            }
        }
        
        // Increment execution counter
        *self.execution_count.write().await += 1;
        
        Ok((diff, processed, failed))
    }
    
    /// Execute events with an isolated local runtime (solver-tee3 mode)
    /// 
    /// This method takes ownership of a local RuntimeExecutor, ensuring complete
    /// isolation from concurrent tasks. Each task gets its own state snapshot.
    async fn simulate_execution_isolated(
        &self,
        input: &StfInput,
        mut local_runtime: RuntimeExecutor<InMemoryStateStore>,
    ) -> StfResult<(StateDiff, Vec<EventId>, Vec<FailedEvent>)> {
        let start = std::time::Instant::now();
        let mut diff = StateDiff::new();
        let mut processed = Vec::new();
        let mut failed = Vec::new();
        
        // Process each event with the isolated local runtime
        for event in &input.events {
            // Check timeout
            if start.elapsed().as_millis() as u64 > self.config.max_execution_time_ms {
                return Err(StfError::ExecutionTimeout);
            }
            
            // Execute event using the LOCAL runtime (not self.runtime!)
            let result = self.execute_single_event_with_runtime(
                event, 
                &input.resolved_inputs, 
                &mut diff,
                &mut local_runtime,
            ).await;
            
            match result {
                Ok(()) => {
                    processed.push(event.id.clone());
                }
                Err(reason) => {
                    failed.push(FailedEvent {
                        event_id: event.id.clone(),
                        reason,
                    });
                }
            }
        }
        
        // Increment execution counter
        *self.execution_count.write().await += 1;
        
        Ok((diff, processed, failed))
    }
    
    /// Execute a single event using the provided runtime
    /// 
    /// Core STF (State Transition Function) execution logic used by both
    /// legacy (shared runtime via write lock) and solver-tee3 (isolated runtime) modes.
    /// The caller provides the appropriate RuntimeExecutor reference.
    ///
    /// For transfer events, delegates to execute_transfer_with_local_runtime.
    /// For merge/split/merge-then-transfer, executes directly via runtime.
    /// Infrastructure events (SubnetRegister, etc.) are rejected — they must
    /// be handled by Validator's InfraExecutor.
    async fn execute_single_event_with_runtime(
        &self,
        event: &setu_types::Event,
        resolved_inputs: &ResolvedInputs,
        diff: &mut StateDiff,
        local_runtime: &mut RuntimeExecutor<InMemoryStateStore>,
    ) -> Result<(), String> {
        debug!(event_id = %event.id, event_type = ?event.event_type, "Executing event via isolated runtime");
        
        // Check if this is a transfer event
        if let Some(transfer) = &event.transfer {
            return self.execute_transfer_with_local_runtime(
                event, transfer, resolved_inputs, diff, local_runtime
            ).await;
        }
        
        // Infrastructure / root events should NEVER reach TEE.
        // They are executed directly by Validator via InfraExecutor.
        // If we receive such events here, it indicates a routing bug.
        match &event.payload {
            setu_types::event::EventPayload::SubnetRegister(_) => {
                warn!(event_id = %event.id, "SubnetRegister event incorrectly routed to TEE - should use InfraExecutor");
                return Err("SubnetRegister is a Validator-executed event, should not reach TEE".to_string());
            }
            setu_types::event::EventPayload::UserRegister(_) => {
                warn!(event_id = %event.id, "UserRegister event incorrectly routed to TEE - should use InfraExecutor");
                return Err("UserRegister is a Validator-executed event, should not reach TEE".to_string());
            }
            setu_types::event::EventPayload::ContractPublish { .. } => {
                warn!(event_id = %event.id, "ContractPublish event incorrectly routed to TEE - should use InfraExecutor");
                return Err("ContractPublish is a Validator-executed event, should not reach TEE".to_string());
            }
            setu_types::event::EventPayload::CoinMerge { .. } => {
                let ctx = ExecutionContext::new(
                    self.config.solver_id.clone(),
                    event.timestamp,
                    false,
                    Self::derive_tx_hash(&event.id),
                );
                if let setu_types::OperationType::MergeCoins { target_index, source_indices } = &resolved_inputs.operation {
                    let target_coin_id = resolved_inputs.input_objects[*target_index].object_id.clone();
                    let source_coin_ids: Vec<setu_types::object::ObjectId> = source_indices.iter()
                        .map(|&i| resolved_inputs.input_objects[i].object_id.clone())
                        .collect();
                    let owner = local_runtime.state()
                        .get_object(&target_coin_id)
                        .map_err(|e| format!("Failed to read target coin: {}", e))?
                        .ok_or_else(|| format!("Target coin {} not found", target_coin_id))?
                        .metadata.owner.clone()
                        .ok_or_else(|| "Target coin has no owner".to_string())?;
                    let output = local_runtime.execute_merge_coins(
                        &owner,
                        target_coin_id,
                        &source_coin_ids,
                        &ctx,
                    ).map_err(|e| format!("Runtime merge error: {}", e))?;
                    if !output.success {
                        return Err(output.message.unwrap_or_else(|| "Merge failed".to_string()));
                    }
                    diff.add_state_changes(&output.state_changes);
                } else {
                    return Err("CoinMerge payload but operation is not MergeCoins".to_string());
                }
                let mut legacy_state = self.legacy_state.write().await;
                self.record_event_processed(&mut legacy_state, event, diff);
                return Ok(());
            }
            setu_types::event::EventPayload::CoinSplit { .. } => {
                let ctx = ExecutionContext::new(
                    self.config.solver_id.clone(),
                    event.timestamp,
                    false,
                    Self::derive_tx_hash(&event.id),
                );
                if let setu_types::OperationType::SplitCoin { source_index, amounts } = &resolved_inputs.operation {
                    let source_coin_id = resolved_inputs.input_objects[*source_index].object_id.clone();
                    let owner = local_runtime.state()
                        .get_object(&source_coin_id)
                        .map_err(|e| format!("Failed to read source coin: {}", e))?
                        .ok_or_else(|| format!("Source coin {} not found", source_coin_id))?
                        .metadata.owner.clone()
                        .ok_or_else(|| "Source coin has no owner".to_string())?;
                    let output = local_runtime.execute_split_coin(
                        &owner,
                        source_coin_id,
                        amounts,
                        &ctx,
                    ).map_err(|e| format!("Runtime split error: {}", e))?;
                    if !output.success {
                        return Err(output.message.unwrap_or_else(|| "Split failed".to_string()));
                    }
                    diff.add_state_changes(&output.state_changes);
                } else {
                    return Err("CoinSplit payload but operation is not SplitCoin".to_string());
                }
                let mut legacy_state = self.legacy_state.write().await;
                self.record_event_processed(&mut legacy_state, event, diff);
                return Ok(());
            }
            setu_types::event::EventPayload::CoinMergeThenTransfer { .. } => {
                let ctx = ExecutionContext::new(
                    self.config.solver_id.clone(),
                    event.timestamp,
                    false,
                    Self::derive_tx_hash(&event.id),
                );
                if let setu_types::OperationType::MergeThenTransfer {
                    target_index, source_indices, recipient, amount
                } = &resolved_inputs.operation {
                    let target_coin_id = resolved_inputs.input_objects[*target_index].object_id.clone();
                    let source_coin_ids: Vec<setu_types::object::ObjectId> = source_indices.iter()
                        .map(|&i| resolved_inputs.input_objects[i].object_id.clone())
                        .collect();
                    let owner = local_runtime.state()
                        .get_object(&target_coin_id)
                        .map_err(|e| format!("Failed to read target coin: {}", e))?
                        .ok_or_else(|| format!("Target coin {} not found", target_coin_id))?
                        .metadata.owner.clone()
                        .ok_or_else(|| "Target coin has no owner".to_string())?;

                    // Step 1: Merge — accumulate sources into target (same local_runtime)
                    let mut merge_output = local_runtime.execute_merge_coins(
                        &owner,
                        target_coin_id.clone(),
                        &source_coin_ids,
                        &ctx,
                    ).map_err(|e| format!("Runtime merge error: {}", e))?;
                    if !merge_output.success {
                        return Err(merge_output.message.unwrap_or_else(|| "Merge failed".to_string()));
                    }

                    // Step 2: Transfer from merged coin to recipient
                    // ⚠️ Uses the SAME local_runtime — Step 2 reads Step 1's writes.
                    // If Step 2 fails, the entire function returns Err and local_runtime
                    // is discarded — no partial writes are committed.
                    let transfer_output = local_runtime.execute_transfer_with_coin(
                        target_coin_id,
                        &owner.to_string(),
                        &recipient.to_string(),
                        Some(*amount),
                        &ctx,
                    ).map_err(|e| format!("Runtime transfer error: {}", e))?;
                    if !transfer_output.success {
                        return Err(transfer_output.message.unwrap_or_else(|| "Transfer failed".to_string()));
                    }

                    // Step 3: Combine state_changes (order matters! merge before transfer)
                    merge_output.state_changes.extend(transfer_output.state_changes);
                    merge_output.created_objects.extend(transfer_output.created_objects);
                    merge_output.deleted_objects.extend(transfer_output.deleted_objects);
                    diff.add_state_changes(&merge_output.state_changes);
                } else {
                    return Err("CoinMergeThenTransfer payload but operation is not MergeThenTransfer".to_string());
                }
                let mut legacy_state = self.legacy_state.write().await;
                self.record_event_processed(&mut legacy_state, event, diff);
                return Ok(());
            }
            _ => {}
        }
        
        // For non-transfer events, record in legacy state
        let mut legacy_state = self.legacy_state.write().await;
        self.record_event_processed(&mut legacy_state, event, diff);
        Ok(())
    }
    
    /// Execute a transfer event using the provided runtime
    /// 
    /// Uses resolved_inputs.primary_coin() to get the coin_id that Validator
    /// has already selected. This follows the solver-tee3 design where
    /// Validator prepares everything - if coin_id is missing, it's an error.
    ///
    /// Used by both legacy (shared runtime via write lock) and solver-tee3
    /// (isolated runtime) execution paths.
    async fn execute_transfer_with_local_runtime(
        &self,
        event: &setu_types::Event,
        transfer: &setu_types::Transfer,
        resolved_inputs: &ResolvedInputs,
        diff: &mut StateDiff,
        local_runtime: &mut RuntimeExecutor<InMemoryStateStore>,
    ) -> Result<(), String> {
        let ctx = ExecutionContext::new(
            self.config.solver_id.clone(),
            event.timestamp,
            false, // Mock enclave
            Self::derive_tx_hash(&event.id),
        );
        
        // solver-tee3: resolved_inputs MUST have primary_coin
        let resolved_coin = resolved_inputs.primary_coin()
            .ok_or_else(|| "Missing resolved_inputs.primary_coin - TaskPreparer error".to_string())?;
        
        debug!(
            event_id = %event.id,
            coin_id = %resolved_coin.object_id,
            from = %transfer.from,
            to = %transfer.to,
            amount = transfer.amount,
            "Executing transfer with isolated runtime (solver-tee3)"
        );
        
        // Use the LOCAL runtime (not self.runtime!)
        let output = local_runtime.execute_transfer_with_coin(
            resolved_coin.object_id.clone(),
            &transfer.from,
            &transfer.to,
            Some(transfer.amount),
            &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;
        
        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Transfer failed".to_string()));
        }
        
        // Convert setu-runtime StateChanges to enclave StateDiff
        diff.add_state_changes(&output.state_changes);
        
        // Record event as processed
        let mut legacy_state = self.legacy_state.write().await;
        self.record_event_processed(&mut legacy_state, event, diff);
        
        info!(
            event_id = %event.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = transfer.amount,
            state_changes = output.state_changes.len(),
            "Transfer executed successfully via isolated runtime"
        );
        
        Ok(())
    }
    
    // NOTE: Subnet & User Registration handlers have been removed.
    // These are infrastructure events that should NEVER reach TEE.
    // They are executed directly by Validator via InfraExecutor.
    // See: setu_validator::InfraExecutor
    
    /// Derive a deterministic tx_hash from an event ID.
    fn derive_tx_hash(event_id: &str) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_TX_HASH:");
        hasher.update(event_id.as_bytes());
        *hasher.finalize().as_bytes()
    }
    
    /// Record that an event was processed (for non-transfer events)
    fn record_event_processed(
        &self,
        state: &mut HashMap<String, Vec<u8>>,
        event: &setu_types::Event,
        diff: &mut StateDiff,
    ) {
        let key = format!("event:{}", event.id);
        let old_value = state.get(&key).cloned();
        let new_value = format!("processed:{}", event.id).into_bytes();
        
        state.insert(key.clone(), new_value.clone());
        
        let mut write_entry = WriteSetEntry::new(key, new_value);
        if let Some(old) = old_value {
            write_entry = write_entry.with_old_value(old);
        }
        diff.add_write(write_entry);
    }
    
    /// Compute post-state root from state
    ///
    /// WARNING: This uses a simple hash over sorted key-value pairs, which does NOT match
    /// the real SMT state root computation. This mock should be replaced
    /// with an actual SMT instance for accurate testing.
    #[deprecated(since = "0.3.0", note = "Mock state root uses simple hash, not SMT. Replace with real SMT for accurate testing.")]
    fn compute_state_root(state: &HashMap<String, Vec<u8>>) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_MOCK_STATE:");
        
        // Sort keys for determinism
        let mut keys: Vec<_> = state.keys().collect();
        keys.sort();
        
        for key in keys {
            if let Some(value) = state.get(key) {
                hasher.update(key.as_bytes());
                hasher.update(value);
            }
        }
        
        *hasher.finalize().as_bytes()
    }
    
    /// Compute hash of output for attestation user_data
    /// 
    /// Reserved for future use: binding attestation to output commitment
    #[allow(dead_code)]
    fn compute_output_hash(
        subnet_id: &setu_types::SubnetId,
        pre_state_root: &[u8; 32],
        post_state_root: &[u8; 32],
        diff_commitment: &[u8; 32],
    ) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_OUTPUT_HASH:");
        hasher.update(subnet_id.as_bytes());
        hasher.update(pre_state_root);
        hasher.update(post_state_root);
        hasher.update(diff_commitment);
        *hasher.finalize().as_bytes()
    }
}

#[async_trait]
impl EnclaveRuntime for MockEnclave {
    async fn execute_stf(&self, input: StfInput) -> StfResult<StfOutput> {
        let start = std::time::Instant::now();
        
        // TODO (solver-tee3): Verify read_set Merkle proofs against pre_state_root
        // For now, skip verification in mock mode
        // self.verify_read_set(&input.read_set, &input.pre_state_root)?;
        
        // ========== solver-tee3: Build ISOLATED state from read_set ==========
        // CRITICAL FIX: Each task gets its own local RuntimeExecutor.
        // This prevents concurrent tasks from overwriting each other's state.
        // 
        // Previous bug: We replaced self.runtime with a new temp_store, causing
        // race conditions where concurrent tasks would see each other's (or empty) state.
        let use_read_set_state = !input.read_set.is_empty();
        
        let (diff, events_processed, events_failed) = if use_read_set_state {
            // Build temporary state from read_set into a LOCAL store
            let local_store = self.build_state_from_read_set(&input.read_set)?;
            let local_runtime = RuntimeExecutor::new(local_store);
            
            info!(
                read_set_entries = input.read_set.len(),
                "Using isolated read_set state for execution (solver-tee3 mode)"
            );
            
            // Execute using the ISOLATED local runtime (not self.runtime!)
            self.simulate_execution_isolated(&input, local_runtime).await?
        } else {
            // Legacy mode: use shared self.runtime (only for backward compatibility)
            self.simulate_execution(&input, None).await?
        };
        
        // Compute post-state root from legacy state
        // Note: For full object model, should compute from RuntimeExecutor state
        let post_state_root = {
            let state = self.legacy_state.read().await;
            Self::compute_state_root(&state)
        };
        
        // Compute input hash for attestation binding
        let input_hash = input.input_hash();
        
        // Create AttestationData binding task_id, input_hash, and state roots
        let attestation_data = AttestationData::new(
            input.task_id,
            input_hash,
            input.pre_state_root,
            post_state_root,
        );
        
        // Generate mock attestation with proper data binding
        let attestation = Attestation::mock_with_data(attestation_data)
            .with_solver_id(self.config.solver_id.clone());
        
        let execution_time = start.elapsed();
        let writes_count = diff.writes.len() as u64;
        
        // Calculate gas usage (mock: 100 gas per write, 10 gas per read)
        let gas_used = execution_time.as_micros() as u64 / 10 + writes_count * 100 + input.read_set.len() as u64 * 10;
        let gas_usage = GasUsage::new(gas_used, Some(1)); // mock gas_price = 1
        
        Ok(StfOutput {
            task_id: input.task_id,
            subnet_id: input.subnet_id,
            post_state_root,
            state_diff: diff,
            events_processed,
            events_failed,
            gas_usage,
            attestation,
            stats: ExecutionStats {
                execution_time_us: execution_time.as_micros() as u64,
                reads: input.read_set.len() as u64,
                writes: writes_count,
                peak_memory_bytes: 0, // Not tracked in mock
            },
        })
    }
    
    async fn generate_attestation(&self, user_data: [u8; 32]) -> StfResult<Attestation> {
        Ok(Attestation::mock(user_data).with_solver_id(self.config.solver_id.clone()))
    }
    
    async fn verify_attestation(&self, attestation: &Attestation) -> StfResult<bool> {
        // Mock verification: accept all mock attestations
        Ok(attestation.is_mock())
    }
    
    fn info(&self) -> EnclaveInfo {
        EnclaveInfo {
            enclave_id: self.config.enclave_id.clone(),
            platform: EnclavePlatform::Mock,
            measurement: MOCK_MEASUREMENT,
            version: env!("CARGO_PKG_VERSION").to_string(),
            is_simulated: true,
        }
    }
    
    fn measurement(&self) -> [u8; 32] {
        MOCK_MEASUREMENT
    }
    
    fn is_simulated(&self) -> bool {
        true
    }
}

/// Builder for MockEnclave
pub struct MockEnclaveBuilder {
    solver_id: String,
    max_execution_time_ms: u64,
    max_memory_bytes: u64,
    debug_logging: bool,
    initial_state: HashMap<String, Vec<u8>>,
}

impl MockEnclaveBuilder {
    pub fn new(solver_id: impl Into<String>) -> Self {
        Self {
            solver_id: solver_id.into(),
            max_execution_time_ms: 30000,
            max_memory_bytes: 512 * 1024 * 1024,
            debug_logging: false,
            initial_state: HashMap::new(),
        }
    }
    
    pub fn max_execution_time(mut self, ms: u64) -> Self {
        self.max_execution_time_ms = ms;
        self
    }
    
    pub fn max_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }
    
    pub fn debug_logging(mut self, enabled: bool) -> Self {
        self.debug_logging = enabled;
        self
    }
    
    pub fn with_initial_state(mut self, key: String, value: Vec<u8>) -> Self {
        self.initial_state.insert(key, value);
        self
    }
    
    pub fn build(self) -> MockEnclave {
        let config = EnclaveConfig {
            enclave_id: format!("mock-{}", uuid::Uuid::new_v4()),
            solver_id: self.solver_id,
            max_execution_time_ms: self.max_execution_time_ms,
            max_memory_bytes: self.max_memory_bytes,
            enable_debug_logging: self.debug_logging,
        };
        
        let store = InMemoryStateStore::new();
        let runtime = RuntimeExecutor::new(store);
        
        let enclave = MockEnclave {
            config,
            runtime: Arc::new(RwLock::new(runtime)),
            legacy_state: Arc::new(RwLock::new(self.initial_state)),
            execution_count: Arc::new(RwLock::new(0)),
        };
        
        enclave
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Event, SubnetId, EventType, VLCSnapshot};
    
    fn create_test_event(id: &str) -> Event {
        Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            format!("creator_{}", id),
        )
    }
    
    #[tokio::test]
    async fn test_mock_enclave_creation() {
        let enclave = MockEnclave::default_with_solver_id("solver1".to_string());
        let info = enclave.info();
        
        assert_eq!(info.platform, EnclavePlatform::Mock);
        assert!(info.is_simulated);
    }
    
    #[tokio::test]
    async fn test_mock_enclave_stf_execution() {
        use crate::solver_task::{ResolvedInputs, GasBudget};
        
        let enclave = MockEnclave::default_with_solver_id("solver1".to_string());
        
        let task_id = [1u8; 32];
        let resolved_inputs = ResolvedInputs::new();
        let gas_budget = GasBudget::default();
        
        let input = StfInput::new(
            task_id,
            SubnetId::ROOT,
            [0u8; 32],
            resolved_inputs,
            gas_budget,
        ).with_events(vec![create_test_event("evt1")]);
        
        let output = enclave.execute_stf(input).await.unwrap();
        
        assert_eq!(output.subnet_id, SubnetId::ROOT);
        assert_eq!(output.task_id, task_id);
        assert_eq!(output.events_processed.len(), 1);
        assert!(output.events_failed.is_empty());
        assert!(output.attestation.is_mock());
    }
    
    #[tokio::test]
    async fn test_mock_enclave_generates_attestation() {
        let enclave = MockEnclave::default_with_solver_id("solver1".to_string());
        
        let user_data = [42u8; 32];
        let attestation = enclave.generate_attestation(user_data).await.unwrap();
        
        assert!(attestation.is_mock());
        assert_eq!(attestation.user_data, user_data);
    }
    
    #[tokio::test]
    async fn test_mock_enclave_builder() {
        let enclave = MockEnclaveBuilder::new("test_solver")
            .max_execution_time(5000)
            .debug_logging(true)
            .build();
        
        assert!(enclave.is_simulated());
        assert_eq!(enclave.measurement(), MOCK_MEASUREMENT);
    }
}
