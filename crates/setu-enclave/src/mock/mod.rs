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
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

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
    "SETU".to_string()
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
    /// Creates a Coin object owned by the address
    pub async fn init_account(&self, address: &str, balance: u64) {
        let addr = Address::from(address);
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
    /// read_set entries use key format: "coin:{hex_object_id}"
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
            // Parse object key: "coin:{hex_object_id}"
            let hex_id = entry.key.strip_prefix("coin:");
            
            if let Some(hex_id) = hex_id {
                // Parse ObjectId from hex string
                let object_id = ObjectId::from_hex(hex_id)
                    .map_err(|e| StfError::InvalidResolvedInputs(format!("Invalid object ID: {}", e)))?;
                
                // TODO: Verify Merkle proof here (for production)
                // let leaf_hash = sha256(entry.value);
                // verify_proof(leaf_hash, entry.proof, pre_state_root)?;
                
                // Deserialize CoinState from BCS (raw storage format)
                let coin_state: CoinState = bcs::from_bytes(&entry.value)
                    .map_err(|e| StfError::InvalidResolvedInputs(format!(
                        "Failed to deserialize CoinState {}: {}", hex_id, e
                    )))?;
                
                // Convert CoinState → Object<CoinData> for runtime
                let owner = Address::from(coin_state.owner.as_str());
                let coin_data = CoinData {
                    coin_type: CoinType::new(&coin_state.coin_type),
                    balance: Balance::new(coin_state.balance),
                };
                
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                
                let coin_object = Object {
                    metadata: setu_types::ObjectMetadata {
                        id: object_id,
                        version: coin_state.version,
                        digest: setu_types::ObjectDigest::ZERO,
                        object_type: setu_types::ObjectType::OwnedObject,
                        owner: Some(owner),
                        ownership: setu_types::Ownership::AddressOwner(owner),
                        created_at: now,
                        updated_at: now,
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
    #[allow(dead_code)]
    async fn simulate_execution(
        &self, 
        input: &StfInput,
        _local_runtime: Option<RuntimeExecutor<InMemoryStateStore>>,
    ) -> StfResult<(StateDiff, Vec<EventId>, Vec<FailedEvent>)> {
        let start = std::time::Instant::now();
        let mut diff = StateDiff::new();
        let mut processed = Vec::new();
        let mut failed = Vec::new();
        
        // Process each event using shared self.runtime (legacy mode)
        for event in &input.events {
            // Check timeout
            if start.elapsed().as_millis() as u64 > self.config.max_execution_time_ms {
                return Err(StfError::ExecutionTimeout);
            }
            
            // Execute event using self.runtime (shared)
            let result = self.execute_single_event(event, &input.resolved_inputs, &mut diff).await;
            
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
    
    /// Execute a single event using setu-runtime
    /// 
    /// This is the core STF (State Transition Function) execution.
    /// For transfer events, uses resolved_inputs.primary_coin() to get the coin_id
    /// that Validator has already selected.
    /// For other events, records them in legacy state.
    /// 
    /// NOTE: This method uses self.runtime (shared). For concurrent execution,
    /// use execute_single_event_with_runtime() instead.
    async fn execute_single_event(
        &self,
        event: &setu_types::Event,
        resolved_inputs: &ResolvedInputs,
        diff: &mut StateDiff,
    ) -> Result<(), String> {
        debug!(event_id = %event.id, event_type = ?event.event_type, "Executing event via setu-runtime");
        
        // Check if this is a transfer event
        if let Some(transfer) = &event.transfer {
            return self.execute_transfer_via_runtime(event, transfer, resolved_inputs, diff).await;
        }
        
        // For non-transfer events, record in legacy state
        let mut legacy_state = self.legacy_state.write().await;
        self.record_event_processed(&mut legacy_state, event, diff);
        Ok(())
    }
    
    /// Execute a single event with an isolated local runtime (solver-tee3 mode)
    /// 
    /// This variant takes a mutable reference to a LOCAL RuntimeExecutor,
    /// ensuring complete isolation from concurrent tasks.
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
        
        // For non-transfer events, record in legacy state
        let mut legacy_state = self.legacy_state.write().await;
        self.record_event_processed(&mut legacy_state, event, diff);
        Ok(())
    }
    
    /// Execute a transfer event via setu-runtime (solver-tee3 architecture)
    /// 
    /// Uses resolved_inputs.primary_coin() to get the coin_id that Validator
    /// has already selected. This follows the solver-tee3 design where 
    /// Validator prepares everything - if coin_id is missing, it's an error.
    /// 
    /// NOTE: This uses self.runtime (shared). For concurrent execution,
    /// use execute_transfer_with_local_runtime() instead.
    async fn execute_transfer_via_runtime(
        &self,
        event: &setu_types::Event,
        transfer: &setu_types::Transfer,
        resolved_inputs: &ResolvedInputs,
        diff: &mut StateDiff,
    ) -> Result<(), String> {
        let ctx = ExecutionContext {
            executor_id: self.config.solver_id.clone(),
            timestamp: event.timestamp,
            in_tee: false, // Mock enclave
        };
        
        // solver-tee3: resolved_inputs MUST have primary_coin
        let resolved_coin = resolved_inputs.primary_coin()
            .ok_or_else(|| "Missing resolved_inputs.primary_coin - TaskPreparer error".to_string())?;
        
        debug!(
            event_id = %event.id,
            coin_id = %resolved_coin.object_id,
            from = %transfer.from,
            to = %transfer.to,
            amount = transfer.amount,
            "Executing transfer with resolved coin_id (solver-tee3)"
        );
        
        // Use the coin_id from resolved_inputs
        let output = {
            let mut runtime = self.runtime.write().await;
            runtime.execute_transfer_with_coin(
                resolved_coin.object_id.clone(),
                &transfer.from,
                &transfer.to,
                Some(transfer.amount as u64),
                &ctx,
            ).map_err(|e| format!("Runtime error: {}", e))?
        };
        
        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Transfer failed".to_string()));
        }
        
        // Convert setu-runtime StateChanges to enclave StateDiff using helper method
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
            "Transfer executed successfully via setu-runtime"
        );
        
        Ok(())
    }
    
    /// Execute a transfer event with an isolated local runtime (solver-tee3 mode)
    /// 
    /// This variant takes a mutable reference to a LOCAL RuntimeExecutor,
    /// ensuring complete isolation from concurrent tasks.
    async fn execute_transfer_with_local_runtime(
        &self,
        event: &setu_types::Event,
        transfer: &setu_types::Transfer,
        resolved_inputs: &ResolvedInputs,
        diff: &mut StateDiff,
        local_runtime: &mut RuntimeExecutor<InMemoryStateStore>,
    ) -> Result<(), String> {
        let ctx = ExecutionContext {
            executor_id: self.config.solver_id.clone(),
            timestamp: event.timestamp,
            in_tee: false, // Mock enclave
        };
        
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
            Some(transfer.amount as u64),
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
    fn compute_state_root(state: &HashMap<String, Vec<u8>>) -> [u8; 32] {
        let mut hasher = Sha256::new();
        
        // Sort keys for determinism
        let mut keys: Vec<_> = state.keys().collect();
        keys.sort();
        
        for key in keys {
            if let Some(value) = state.get(key) {
                hasher.update(key.as_bytes());
                hasher.update(value);
            }
        }
        
        hasher.finalize().into()
    }
    
    /// Compute hash of output for attestation user_data
    fn compute_output_hash(
        subnet_id: &setu_types::SubnetId,
        pre_state_root: &[u8; 32],
        post_state_root: &[u8; 32],
        diff_commitment: &[u8; 32],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(subnet_id.as_bytes());
        hasher.update(pre_state_root);
        hasher.update(post_state_root);
        hasher.update(diff_commitment);
        hasher.finalize().into()
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
