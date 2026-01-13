//! Mock enclave implementation for development and testing.
//!
//! The MockEnclave simulates TEE behavior without requiring actual hardware.
//! It provides:
//! - Simulated STF execution
//! - Mock attestations
//! - Deterministic behavior for testing

use crate::{
    attestation::Attestation,
    stf::{ExecutionStats, FailedEvent, StateDiff, StfError, StfInput, StfOutput, StfResult, WriteSetEntry},
    traits::{EnclaveConfig, EnclaveInfo, EnclavePlatform, EnclaveRuntime},
};
use async_trait::async_trait;
use setu_types::EventId;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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
pub struct MockEnclave {
    config: EnclaveConfig,
    /// Simulated state (for testing state transitions)
    state: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// Execution counter for statistics
    execution_count: Arc<RwLock<u64>>,
}

impl MockEnclave {
    /// Create a new mock enclave with the given configuration
    pub fn new(config: EnclaveConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(HashMap::new())),
            execution_count: Arc::new(RwLock::new(0)),
        }
    }
    
    /// Create a mock enclave with default configuration
    pub fn default_with_solver_id(solver_id: String) -> Self {
        let config = EnclaveConfig::default().with_solver_id(solver_id);
        Self::new(config)
    }
    
    /// Get the current execution count
    pub async fn execution_count(&self) -> u64 {
        *self.execution_count.read().await
    }
    
    /// Simulate applying events to state
    async fn simulate_execution(&self, input: &StfInput) -> StfResult<(StateDiff, Vec<EventId>, Vec<FailedEvent>)> {
        let start = std::time::Instant::now();
        let mut state = self.state.write().await;
        let mut diff = StateDiff::new();
        let mut processed = Vec::new();
        let mut failed = Vec::new();
        
        // Apply read set to state (simulating initial state)
        for entry in &input.read_set {
            state.insert(entry.key.clone(), entry.value.clone());
        }
        
        // Process each event
        for event in &input.events {
            // Check timeout
            if start.elapsed().as_millis() as u64 > self.config.max_execution_time_ms {
                return Err(StfError::ExecutionTimeout);
            }
            
            // Simulate event execution
            match self.execute_single_event(&mut state, event, &mut diff) {
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
    
    /// Execute a single event (mock implementation)
    fn execute_single_event(
        &self,
        state: &mut HashMap<String, Vec<u8>>,
        event: &setu_types::Event,
        diff: &mut StateDiff,
    ) -> Result<(), String> {
        // Mock execution: just record that we processed the event
        // In real implementation, this would invoke setu-runtime
        
        // Generate a state change based on event
        let key = format!("event:{}", event.id);
        let old_value = state.get(&key).cloned();
        let new_value = format!("processed:{}", event.id).into_bytes();
        
        state.insert(key.clone(), new_value.clone());
        
        let mut write_entry = WriteSetEntry::new(key, new_value);
        if let Some(old) = old_value {
            write_entry = write_entry.with_old_value(old);
        }
        diff.add_write(write_entry);
        
        Ok(())
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
        
        // Simulate execution
        let (diff, events_processed, events_failed) = self.simulate_execution(&input).await?;
        
        // Compute post-state root
        let post_state_root = {
            let state = self.state.read().await;
            Self::compute_state_root(&state)
        };
        
        // Compute output hash for attestation
        let diff_commitment = diff.commitment();
        let output_hash = Self::compute_output_hash(
            &input.subnet_id,
            &input.pre_state_root,
            &post_state_root,
            &diff_commitment,
        );
        
        // Generate mock attestation
        let attestation = Attestation::mock(output_hash)
            .with_solver_id(self.config.solver_id.clone());
        
        let execution_time = start.elapsed();
        
        Ok(StfOutput {
            subnet_id: input.subnet_id,
            post_state_root,
            state_diff: diff,
            events_processed,
            events_failed,
            attestation,
            stats: ExecutionStats {
                execution_time_us: execution_time.as_micros() as u64,
                reads: input.read_set.len() as u64,
                writes: 0, // Will be updated in the actual write set
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
        
        MockEnclave {
            config,
            state: Arc::new(RwLock::new(self.initial_state)),
            execution_count: Arc::new(RwLock::new(0)),
        }
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
        let enclave = MockEnclave::default_with_solver_id("solver1".to_string());
        
        let input = StfInput::new(SubnetId::ROOT, [0u8; 32])
            .with_events(vec![create_test_event("evt1")]);
        
        let output = enclave.execute_stf(input).await.unwrap();
        
        assert_eq!(output.subnet_id, SubnetId::ROOT);
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
