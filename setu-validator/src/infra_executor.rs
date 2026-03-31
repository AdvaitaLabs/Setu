//! Infrastructure Event Executor
//!
//! Executes infrastructure events (SubnetRegister, UserRegister) directly in Validator.
//! These events are NOT routed to Solvers/TEE because they are core infrastructure
//! operations managed by validators.
//!
//! ## Architecture
//!
//! ```text
//! Infrastructure Events (Validator-executed):
//! - SubnetRegister: Create subnet, mint initial tokens
//! - UserRegister: Register user membership
//! - ValidatorRegister/Unregister
//! - SolverRegister/Unregister
//!
//! Application Events (Solver-executed via TEE):
//! - Transfer: Token transfers
//! - (Future) Smart contract calls
//! ```
//!
//! ## Design Philosophy
//!
//! Infrastructure primitives (subnet/user registration) are handled by Validator
//! to ensure consistency and avoid TEE complexity for non-economic operations.
//! Token operations (initial minting, airdrops) use the same RuntimeExecutor
//! logic as TEE to maintain consistency.

use setu_runtime::{RuntimeExecutor, ExecutionContext, ExecutionOutput, InMemoryStateStore};
use setu_storage::{SharedStateManager, MerkleStateProvider};
use setu_types::{
    Address, SubnetId,
    registration::{SubnetRegistration, UserRegistration},
    event::{Event, EventPayload, ExecutionResult, StateChange as EventStateChange},
};
use setu_vlc::VLCSnapshot;
use std::sync::Arc;
use tracing::{info, warn, error};

/// Infrastructure event executor for Validator
///
/// Executes SubnetRegister, UserRegister events directly without TEE.
pub struct InfraExecutor {
    /// Validator ID
    validator_id: String,
    /// State provider for reading/writing state
    state_provider: Arc<MerkleStateProvider>,
}

impl InfraExecutor {
    /// Create a new infrastructure executor
    pub fn new(validator_id: String, state_provider: Arc<MerkleStateProvider>) -> Self {
        Self {
            validator_id,
            state_provider,
        }
    }

    /// Execute a SubnetRegister event
    ///
    /// This creates a subnet and optionally mints initial tokens to the owner.
    pub fn execute_subnet_register(
        &self,
        registration: &SubnetRegistration,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        // Derive tx_hash from registration for deterministic ID generation
        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:SUBNET:");
            hasher.update(registration.subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(),
            timestamp,
            false,
            tx_hash,
        );

        // Create a temporary InMemoryStateStore for execution
        // In production, this would integrate with the actual state
        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let owner = Address::from_hex(&registration.owner)
            .map_err(|e| format!("Invalid owner address '{}': {}", registration.owner, e))?;

        let output = runtime.execute_subnet_register(
            &registration.subnet_id,
            &registration.name,
            &owner,
            registration.token_symbol.as_deref(),
            registration.initial_token_supply,
            &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet registration failed".to_string()));
        }

        // Convert RuntimeExecutor output to Event
        let mut event = Event::subnet_register(
            registration.clone(),
            vec![], // No parent events
            vlc_snapshot,
            self.validator_id.clone(),
        );

        // Apply state changes to the actual state provider first
        self.apply_state_changes(&output)?;

        // Set execution result — use canonical "oid:{hex}" key format
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();

        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(
            subnet_id = %registration.subnet_id,
            owner = %registration.owner,
            token_symbol = ?registration.token_symbol,
            initial_supply = ?registration.initial_token_supply,
            event_id = %event.id,
            "Subnet registered by Validator"
        );

        Ok(event)
    }

    /// Execute a UserRegister event
    ///
    /// This registers a user in a subnet (membership only, no automatic airdrop).
    pub fn execute_user_register(
        &self,
        registration: &UserRegistration,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        // Derive tx_hash from registration for deterministic ID generation
        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:USER:");
            hasher.update(registration.address.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(),
            timestamp,
            false,
            tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let user_address = Address::from_hex(&registration.address)
            .map_err(|e| format!("Invalid user address '{}': {}", registration.address, e))?;
        let subnet_id = registration.subnet_id.as_deref().unwrap_or("subnet-0");

        let output = runtime.execute_user_register(
            &user_address,
            subnet_id,
            &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "User registration failed".to_string()));
        }

        // Convert to Event
        let mut event = Event::user_register(
            registration.clone(),
            vec![],
            vlc_snapshot,
            self.validator_id.clone(),
        );

        // Apply state changes first
        self.apply_state_changes(&output)?;

        // Use canonical "oid:{hex}" key format
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();

        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(
            user = %registration.address,
            subnet_id = %subnet_id,
            event_id = %event.id,
            "User registered by Validator"
        );

        Ok(event)
    }

    /// Apply state changes to the MerkleStateProvider
    ///
    /// Routes through `apply_state_change()` which handles both SMT updates
    /// AND coin index updates (coin_type_index, owner_coin_index).
    fn apply_state_changes(&self, output: &ExecutionOutput) -> Result<(), String> {
        // Get write access to the state manager
        let shared = self.state_provider.shared_state_manager();
        let mut manager = shared.lock_write();

        for sc in &output.state_changes {
            // Convert to event-layer StateChange with canonical "oid:{hex}" key format
            // and route through apply_state_change for proper SMT + index updates
            let event_sc = sc.to_event_state_change();
            manager.apply_state_change(SubnetId::ROOT, &event_sc);
        }
        
        // Publish snapshot so read path sees the infra changes immediately
        shared.publish_snapshot(&manager);

        Ok(())
    }

    // ========== Phase 3: Profile & Subnet Membership ==========

    /// Execute a profile update event
    pub fn execute_profile_update(
        &self,
        user_address: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        bio: Option<&str>,
        attributes: &std::collections::HashMap<String, String>,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:PROFILE:");
            hasher.update(user_address.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_profile_update(
            &address, display_name, avatar_url, bio, attributes, &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Profile update failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        self.apply_state_changes(&output)?;

        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, event_id = %event.id, "Profile updated by Validator");
        Ok(event)
    }

    /// Execute a subnet join event
    pub fn execute_subnet_join(
        &self,
        user_address: &str,
        subnet_id: &str,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:JOIN:");
            hasher.update(user_address.as_bytes());
            hasher.update(subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_subnet_join(&address, subnet_id, &ctx)
            .map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet join failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        self.apply_state_changes(&output)?;

        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, subnet_id = %subnet_id, event_id = %event.id, "Subnet join by Validator");
        Ok(event)
    }

    /// Execute a subnet leave event
    pub fn execute_subnet_leave(
        &self,
        user_address: &str,
        subnet_id: &str,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:LEAVE:");
            hasher.update(user_address.as_bytes());
            hasher.update(subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_subnet_leave(&address, subnet_id, &ctx)
            .map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet leave failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        self.apply_state_changes(&output)?;

        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, subnet_id = %subnet_id, event_id = %event.id, "Subnet leave by Validator");
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_storage::GlobalStateManager;

    #[test]
    fn test_subnet_register() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = SubnetRegistration::new("subnet-test", "Test Subnet", "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf", "TEST")
            .with_initial_supply(1_000_000);

        let vlc = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        };

        let result = executor.execute_subnet_register(&registration, vlc);
        assert!(result.is_ok());
        
        let event = result.unwrap();
        assert!(event.execution_result.is_some());
        assert!(event.execution_result.as_ref().unwrap().success);
    }

    #[test]
    fn test_user_register() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = UserRegistration::from_metamask(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            1000,
        ).with_subnet("subnet-0");

        let vlc = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        };

        let result = executor.execute_user_register(&registration, vlc);
        assert!(result.is_ok());
        
        let event = result.unwrap();
        assert!(event.execution_result.is_some());
        assert!(event.execution_result.as_ref().unwrap().success);
    }

    fn test_vlc() -> VLCSnapshot {
        VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        }
    }

    #[test]
    fn test_infra_profile_update_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);
        let attrs = std::collections::HashMap::new();

        let result = executor.execute_profile_update(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            Some("Alice"), None, Some("Hello world"), &attrs, test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 1);
        // Key must be "oid:{hex}" format
        assert!(er.state_changes[0].key.starts_with("oid:"));
    }

    #[test]
    fn test_infra_subnet_join_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_subnet_join(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "defi-subnet", test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 2);
        assert!(er.state_changes[0].key.starts_with("oid:"));
        assert!(er.state_changes[1].key.starts_with("oid:"));
        // Both have new_value (Create)
        assert!(er.state_changes[0].new_value.is_some());
        assert!(er.state_changes[1].new_value.is_some());
    }

    #[test]
    fn test_infra_subnet_leave_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_subnet_leave(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "defi-subnet", test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 2);
        // Delete: new_value = None
        assert!(er.state_changes[0].new_value.is_none());
        assert!(er.state_changes[1].new_value.is_none());
    }
}
