//! TEE (Trusted Execution Environment) integration for Solver
//!
//! This module provides a wrapper around `setu-enclave` for Solver-specific use cases.
//! It bridges the gap between the generic enclave abstraction and the Solver's needs.
//!
//! ## Architecture (Scheme B - Stateless Solver)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                          Solver                                     │
//! │  ┌───────────────────────────────────────────────────────────────┐  │
//! │  │                   TeeExecutor (this module)                    │  │
//! │  │   • Wraps EnclaveRuntime                                       │  │
//! │  │   • Reads current state from Validator via StateReader         │  │
//! │  │   • Computes final state values (not deltas)                   │  │
//! │  │   • Handles attestation generation                             │  │
//! │  └───────────────────────────────────────────────────────────────┘  │
//! │                              │                                      │
//! │                              ▼                                      │
//! │  ┌───────────────────────────────────────────────────────────────┐  │
//! │  │                setu-enclave (EnclaveRuntime)                   │  │
//! │  │   • MockEnclave (dev/test)                                     │  │
//! │  │   • NitroEnclave (production)                                  │  │
//! │  └───────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## State Management (Scheme B)
//!
//! - Solver is **stateless** - does not maintain local state
//! - Reads current committed state from Validator via `StateReader`
//! - Computes **final state values** (not deltas) for `StateChange.new_value`
//! - Validator is the single source of truth for state

use async_trait::async_trait;
use core_types::Transfer;
use setu_enclave::{
    Attestation, EnclaveInfo, EnclaveRuntime,
    MockEnclave, StfInput, StfOutput, ReadSetEntry,
};
use setu_types::event::{Event, ExecutionResult, StateChange};
use setu_types::SubnetId;
use std::sync::Arc;
use tracing::{info, debug, warn};

/// State reader trait for reading current state from Validator
///
/// In Scheme B, Solver is stateless. It reads current committed state
/// from Validator before executing transactions.
#[async_trait]
pub trait StateReader: Send + Sync {
    /// Get the current balance for an account
    /// Returns 0 if account doesn't exist
    async fn get_balance(&self, account: &str) -> anyhow::Result<u128>;
    
    /// Get raw object data by key
    async fn get_object(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
}

/// Mock state reader for testing
/// Returns predefined balances or defaults
pub struct MockStateReader {
    default_balance: u128,
}

impl MockStateReader {
    pub fn new(default_balance: u128) -> Self {
        Self { default_balance }
    }
}

impl Default for MockStateReader {
    fn default() -> Self {
        Self { default_balance: 1_000_000 } // 1M default balance for testing
    }
}

#[async_trait]
impl StateReader for MockStateReader {
    async fn get_balance(&self, _account: &str) -> anyhow::Result<u128> {
        Ok(self.default_balance)
    }
    
    async fn get_object(&self, _key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

/// TEE Executor for Solver nodes
///
/// Wraps an `EnclaveRuntime` implementation and provides high-level APIs
/// for event execution and attestation generation.
///
/// In Scheme B, TeeExecutor uses `StateReader` to fetch current state from
/// Validator before computing state changes.
pub struct TeeExecutor {
    solver_id: String,
    enclave: Arc<dyn EnclaveRuntime>,
    /// State reader for fetching current state from Validator
    state_reader: Arc<dyn StateReader>,
}

impl TeeExecutor {
    /// Create a new TEE Executor with the default enclave and mock state reader
    pub fn new(solver_id: String) -> Self {
        let enclave = MockEnclave::default_with_solver_id(solver_id.clone());
        
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            "Initializing TEE Executor with MockStateReader"
        );
        
        Self {
            solver_id,
            enclave: Arc::new(enclave),
            state_reader: Arc::new(MockStateReader::default()),
        }
    }
    
    /// Create with a custom enclave implementation
    pub fn with_enclave(solver_id: String, enclave: Arc<dyn EnclaveRuntime>) -> Self {
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            simulated = enclave.is_simulated(),
            "Initializing TEE Executor with custom enclave and MockStateReader"
        );
        
        Self {
            solver_id,
            enclave,
            state_reader: Arc::new(MockStateReader::default()),
        }
    }
    
    /// Create with custom enclave and state reader
    pub fn with_enclave_and_reader(
        solver_id: String,
        enclave: Arc<dyn EnclaveRuntime>,
        state_reader: Arc<dyn StateReader>,
    ) -> Self {
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            simulated = enclave.is_simulated(),
            "Initializing TEE Executor with custom enclave and state reader"
        );
        
        Self {
            solver_id,
            enclave,
            state_reader,
        }
    }
    
    /// Create with custom state reader (using default MockEnclave)
    pub fn with_state_reader(solver_id: String, state_reader: Arc<dyn StateReader>) -> Self {
        let enclave = MockEnclave::default_with_solver_id(solver_id.clone());
        
        info!(
            solver_id = %solver_id,
            platform = %enclave.info().platform,
            "Initializing TEE Executor with custom state reader"
        );
        
        Self {
            solver_id,
            enclave: Arc::new(enclave),
            state_reader,
        }
    }

    /// Execute events and generate attestation
    ///
    /// This is the main entry point for Solver event execution.
    pub async fn execute_events(
        &self,
        subnet_id: SubnetId,
        pre_state_root: [u8; 32],
        events: Vec<Event>,
        read_set: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<TeeExecutionResult> {
        debug!(
            solver_id = %self.solver_id,
            subnet_id = ?subnet_id,
            event_count = events.len(),
            "Executing events in TEE"
        );
        
        // Build StfInput
        let input = StfInput::new(subnet_id.clone(), pre_state_root)
            .with_events(events)
            .with_read_set(
                read_set.into_iter()
                    .map(|(k, v)| ReadSetEntry::new(k, v))
                    .collect()
            );
        
        // Execute STF
        let output = self.enclave.execute_stf(input).await
            .map_err(|e| anyhow::anyhow!("STF execution failed: {}", e))?;
        
        info!(
            solver_id = %self.solver_id,
            events_processed = output.events_processed.len(),
            events_failed = output.events_failed.len(),
            "TEE execution completed"
        );
        
        Ok(TeeExecutionResult::from_stf_output(output))
    }
    
    /// Get enclave information
    pub fn enclave_info(&self) -> EnclaveInfo {
        self.enclave.info()
    }
    
    /// Check if running in simulated mode
    pub fn is_simulated(&self) -> bool {
        self.enclave.is_simulated()
    }
    
    /// Get enclave measurement
    #[allow(dead_code)]
    pub fn measurement(&self) -> [u8; 32] {
        self.enclave.measurement()
    }
    
    /// Execute a transfer in TEE and return execution result
    ///
    /// This method combines the enclave execution with result conversion,
    /// providing the primary API for Solver's transfer execution pipeline.
    ///
    /// In Scheme B:
    /// 1. Reads current state from Validator via StateReader
    /// 2. Validates transaction (e.g., sufficient balance)
    /// 3. Executes in TEE enclave
    /// 4. Returns ExecutionResult with final state values (not deltas)
    pub async fn execute_in_tee(&self, transfer: &Transfer) -> anyhow::Result<ExecutionResult> {
        debug!(
            transfer_id = %transfer.id,
            from = %transfer.from,
            to = %transfer.to,
            amount = %transfer.amount,
            "Executing transfer in TEE"
        );
        
        // Step 1: Compute state changes (reads current state from Validator)
        // This validates the transaction and computes final state values
        let state_changes = match self.compute_transfer_state_changes(transfer).await {
            Ok(changes) => changes,
            Err(e) => {
                warn!(
                    transfer_id = %transfer.id,
                    error = %e,
                    "Transfer validation failed"
                );
                return Ok(ExecutionResult {
                    success: false,
                    message: Some(format!("Validation failed: {}", e)),
                    state_changes: vec![],
                });
            }
        };
        
        // Convert transfer to event
        let event = self.transfer_to_event(transfer)?;
        
        // Get subnet_id from transfer or default to ROOT
        let subnet_id = transfer.subnet_id
            .as_ref()
            .map(|s| SubnetId::from_str_id(s))
            .unwrap_or(SubnetId::ROOT);
        
        // Build read set with actual current values for TEE verification
        let read_set: Vec<(String, Vec<u8>)> = state_changes.iter()
            .filter_map(|sc| {
                sc.old_value.as_ref().map(|v| (sc.key.clone(), v.clone()))
            })
            .collect();
        
        // Use zero as pre-state root for now
        // TODO: fetch actual state root from Validator
        let pre_state_root = [0u8; 32];
        
        // Step 2: Execute in TEE enclave
        let tee_result = self.execute_events(subnet_id, pre_state_root, vec![event], read_set).await?;
        
        info!(
            transfer_id = %transfer.id,
            success = tee_result.events_failed == 0,
            attestation_type = ?tee_result.attestation.attestation_type,
            state_changes_count = state_changes.len(),
            "Transfer TEE execution completed"
        );
        
        Ok(ExecutionResult {
            success: tee_result.events_failed == 0,
            message: Some(format!(
                "TEE execution: {} events processed, {} failed",
                tee_result.events_processed,
                tee_result.events_failed
            )),
            state_changes,
        })
    }
    
    /// Apply state changes to local storage
    ///
    /// DEPRECATED: In Scheme B, Solver is stateless. State changes are applied
    /// only by Validator after ConsensusFrame finalization.
    /// This method is kept for backward compatibility but should not be called.
    /// 
    /// See: storage/src/subnet_state.rs - apply_committed_events()
    #[deprecated(since = "0.2.0", note = "Solver is stateless in Scheme B. State is managed by Validator only.")]
    pub async fn apply_state_changes(&self, changes: &[StateChange]) -> anyhow::Result<()> {
        warn!(
            changes_count = changes.len(),
            "apply_state_changes called but Solver is stateless - changes will be applied by Validator"
        );
        
        // Log for debugging only, do not actually apply
        for change in changes {
            debug!(
                key = %change.key,
                has_old = change.old_value.is_some(),
                has_new = change.new_value.is_some(),
                "State change (not applied locally)"
            );
        }
        
        Ok(())
    }
    
    /// Compute state changes for a transfer
    ///
    /// In Scheme B, this method:
    /// 1. Reads current balance from Validator via StateReader
    /// 2. Validates sufficient balance for sender
    /// 3. Computes final balance values (not deltas)
    /// 4. Returns StateChange with old_value and new_value as final states
    async fn compute_transfer_state_changes(&self, transfer: &Transfer) -> anyhow::Result<Vec<StateChange>> {
        let amount = transfer.amount.unsigned_abs();
        
        // Read current balances from Validator
        let sender_balance = self.state_reader.get_balance(&transfer.from).await?;
        let receiver_balance = self.state_reader.get_balance(&transfer.to).await?;
        
        debug!(
            transfer_id = %transfer.id,
            sender = %transfer.from,
            sender_balance = sender_balance,
            receiver = %transfer.to,
            receiver_balance = receiver_balance,
            amount = amount,
            "Read current balances from Validator"
        );
        
        // Validate sufficient balance
        if sender_balance < amount {
            return Err(anyhow::anyhow!(
                "Insufficient balance: {} has {} but trying to send {}",
                transfer.from,
                sender_balance,
                amount
            ));
        }
        
        // Compute final balances
        let new_sender_balance = sender_balance - amount;
        let new_receiver_balance = receiver_balance + amount;
        
        debug!(
            transfer_id = %transfer.id,
            new_sender_balance = new_sender_balance,
            new_receiver_balance = new_receiver_balance,
            "Computed final balances"
        );
        
        Ok(vec![
            // Debit from sender - store FINAL balance value
            StateChange {
                key: format!("balance:{}", transfer.from),
                old_value: Some(Self::encode_balance(sender_balance)),
                new_value: Some(Self::encode_balance(new_sender_balance)),
            },
            // Credit to receiver - store FINAL balance value
            StateChange {
                key: format!("balance:{}", transfer.to),
                old_value: Some(Self::encode_balance(receiver_balance)),
                new_value: Some(Self::encode_balance(new_receiver_balance)),
            },
        ])
    }
    
    /// Encode a balance as bytes (u128 little-endian)
    fn encode_balance(balance: u128) -> Vec<u8> {
        balance.to_le_bytes().to_vec()
    }
    
    /// Decode bytes to balance (u128 little-endian)
    #[allow(dead_code)]
    fn decode_balance(bytes: &[u8]) -> u128 {
        if bytes.len() >= 16 {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&bytes[..16]);
            u128::from_le_bytes(arr)
        } else {
            0
        }
    }
    
    /// Encode a balance change (DEPRECATED - kept for backward compatibility)
    #[deprecated(since = "0.2.0", note = "Use encode_balance() instead. new_value should be final state, not delta.")]
    #[allow(dead_code)]
    fn encode_balance_change(&self, amount: u128, is_credit: bool) -> Vec<u8> {
        let mut result = vec![if is_credit { 0x01 } else { 0x00 }];
        result.extend_from_slice(&amount.to_le_bytes());
        result
    }
    
    // Helper: Convert Transfer to Event
    fn transfer_to_event(&self, transfer: &Transfer) -> anyhow::Result<Event> {
        use setu_types::event::{EventType, VLCSnapshot};
        
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            self.solver_id.clone(),
        );
        
        // Attach transfer data
        event = event.with_transfer(setu_types::event::Transfer {
            from: transfer.from.clone(),
            to: transfer.to.clone(),
            amount: transfer.amount as u64,
        });
        
        Ok(event)
    }
}

/// Result of TEE execution
#[derive(Debug, Clone)]
pub struct TeeExecutionResult {
    /// Subnet that was executed
    pub subnet_id: SubnetId,
    /// Post-execution state root
    pub post_state_root: [u8; 32],
    /// State changes to apply
    pub state_changes: Vec<StateChange>,
    /// Number of events processed
    pub events_processed: usize,
    /// Number of events failed
    pub events_failed: usize,
    /// TEE attestation
    pub attestation: Attestation,
    /// Execution time in microseconds
    pub execution_time_us: u64,
}

impl TeeExecutionResult {
    /// Convert from StfOutput
    pub fn from_stf_output(output: StfOutput) -> Self {
        // Convert StateDiff to Vec<StateChange>
        let state_changes: Vec<StateChange> = output.state_diff.writes
            .into_iter()
            .map(|w| StateChange {
                key: w.key,
                old_value: w.old_value,
                new_value: Some(w.new_value),
            })
            .chain(
                output.state_diff.deletes.into_iter().map(|k| StateChange {
                    key: k,
                    old_value: None,
                    new_value: None,
                })
            )
            .collect();
        
        Self {
            subnet_id: output.subnet_id,
            post_state_root: output.post_state_root,
            state_changes,
            events_processed: output.events_processed.len(),
            events_failed: output.events_failed.len(),
            attestation: output.attestation,
            execution_time_us: output.stats.execution_time_us,
        }
    }
    
    /// Convert to ExecutionResult (for backward compatibility)
    pub fn to_execution_result(&self) -> ExecutionResult {
        ExecutionResult {
            success: self.events_failed == 0,
            message: if self.events_failed == 0 {
                Some(format!("Processed {} events", self.events_processed))
            } else {
                Some(format!("{} events failed", self.events_failed))
            },
            state_changes: self.state_changes.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_enclave::EnclavePlatform;
    use setu_types::event::{EventType, VLCSnapshot};
    
    #[tokio::test]
    async fn test_tee_executor_creation() {
        let executor = TeeExecutor::new("test-solver".to_string());
        let info = executor.enclave_info();
        
        assert_eq!(info.platform, EnclavePlatform::Mock);
        assert!(executor.is_simulated());
    }
    
    #[tokio::test]
    async fn test_execute_events() {
        let executor = TeeExecutor::new("test-solver".to_string());
        
        let event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        
        let result = executor.execute_events(
            SubnetId::ROOT,
            [0u8; 32],
            vec![event],
            vec![],
        ).await;
        
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.events_processed, 1);
        assert!(result.attestation.is_mock());
    }
    
    #[tokio::test]
    async fn test_execution_result_conversion() {
        let executor = TeeExecutor::new("test-solver".to_string());
        
        let event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        
        let result = executor.execute_events(
            SubnetId::ROOT,
            [0u8; 32],
            vec![event],
            vec![],
        ).await.unwrap();
        
        let exec_result = result.to_execution_result();
        assert!(exec_result.success);
    }
}

