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
use setu_storage::{GlobalStateManager, MerkleStateProvider};
use setu_types::{
    Address, SubnetId,
    registration::{SubnetRegistration, UserRegistration},
    event::{Event, EventPayload, ExecutionResult, StateChange as EventStateChange},
};
use setu_vlc::VLCSnapshot;
use std::sync::{Arc, RwLock};
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

        let ctx = ExecutionContext {
            executor_id: self.validator_id.clone(),
            timestamp,
            in_tee: false,
        };

        // Create a temporary InMemoryStateStore for execution
        // In production, this would integrate with the actual state
        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let owner = Address::from(registration.owner.as_str());

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

        // Set execution result
        let state_changes: Vec<EventStateChange> = output.state_changes.iter().map(|sc| {
            EventStateChange {
                key: format!("object:{}", sc.object_id),
                old_value: sc.old_state.clone(),
                new_value: sc.new_state.clone(),
            }
        }).collect();

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

        let ctx = ExecutionContext {
            executor_id: self.validator_id.clone(),
            timestamp,
            in_tee: false,
        };

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let user_address = Address::from(registration.address.as_str());
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

        let state_changes: Vec<EventStateChange> = output.state_changes.iter().map(|sc| {
            EventStateChange {
                key: format!("object:{}", sc.object_id),
                old_value: sc.old_state.clone(),
                new_value: sc.new_state.clone(),
            }
        }).collect();

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
    fn apply_state_changes(&self, output: &ExecutionOutput) -> Result<(), String> {
        // Get write access to the state manager
        let state_manager = self.state_provider.state_manager();
        let mut manager = state_manager.write()
            .map_err(|e| format!("Failed to acquire state manager lock: {}", e))?;

        for state_change in &output.state_changes {
            let object_id_bytes: [u8; 32] = *state_change.object_id.as_bytes();
            
            if let Some(ref new_state) = state_change.new_state {
                // Insert or update
                manager.upsert_object(
                    SubnetId::ROOT,
                    object_id_bytes,
                    new_state.clone(),
                );
            } else {
                // Delete (not commonly used for registration)
                warn!(
                    object_id = %state_change.object_id,
                    "Delete operation in registration (unexpected)"
                );
            }
        }

        // Register coin types in the index for new coins
        for object_id in &output.created_objects {
            // If we created a coin, we need to register it in the index
            // This requires parsing the state to get owner and coin_type
            // For now, we rely on rebuild_coin_type_index at startup
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_storage::GlobalStateManager;

    #[test]
    fn test_subnet_register() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(state_manager));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = SubnetRegistration::new("subnet-test", "Test Subnet", "owner-addr", "TEST")
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
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(state_manager));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = UserRegistration::from_metamask(
            "0x1234567890abcdef1234567890abcdef12345678",
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
}
