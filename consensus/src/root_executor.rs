//! ROOT Subnet Executor
//!
//! This module implements the ROOT Subnet (SubnetId=0) executor as defined in mkt-3.md.
//! The ROOT Subnet handles system-level operations executed by validators, including:
//! - Validator registration and management
//! - Staking and reward distribution
//! - Global configuration updates
//! - System object management
//!
//! Unlike App subnets which use Solver + TEE execution, ROOT subnet events are
//! executed directly by validators with deterministic execution.

use setu_merkle::sha256;
use setu_types::{
    Event, EventType, SubnetId, 
    ObjectStateValue, object_type, HashValue as TypesHash, ZERO_HASH,
};
use std::collections::HashMap;
use thiserror::Error;

/// Errors that can occur during ROOT subnet execution
#[derive(Error, Debug)]
pub enum RootExecutorError {
    #[error("Event is not a ROOT subnet event")]
    NotRootEvent,
    
    #[error("Invalid event type for ROOT subnet: {0:?}")]
    InvalidEventType(EventType),
    
    #[error("Missing required payload data")]
    MissingPayload,
    
    #[error("Object not found: {0}")]
    ObjectNotFound(String),
    
    #[error("Invalid state transition: {0}")]
    InvalidTransition(String),
    
    #[error("Merkle tree error: {0}")]
    MerkleError(String),
}

/// Result of executing a ROOT subnet event
#[derive(Debug, Clone)]
pub struct RootExecutionResult {
    /// Updated objects (key -> new state value)
    pub updated_objects: HashMap<[u8; 32], ObjectStateValue>,
    
    /// Deleted objects (their keys)
    pub deleted_objects: Vec<[u8; 32]>,
    
    /// New state root after execution
    pub new_state_root: TypesHash,
    
    /// Event that was executed
    pub event_id: String,
}

impl RootExecutionResult {
    /// Create an empty result (no state changes)
    pub fn empty(event_id: String, current_root: TypesHash) -> Self {
        Self {
            updated_objects: HashMap::new(),
            deleted_objects: Vec::new(),
            new_state_root: current_root,
            event_id,
        }
    }
    
    /// Check if there were any state changes
    pub fn has_changes(&self) -> bool {
        !self.updated_objects.is_empty() || !self.deleted_objects.is_empty()
    }
}

/// ROOT Subnet Executor handles system-level operations
pub struct RootSubnetExecutor {
    /// Current state root
    current_state_root: TypesHash,
    
    /// Pending updates (not yet committed)
    pending_updates: HashMap<[u8; 32], ObjectStateValue>,
    
    /// Pending deletions
    pending_deletions: Vec<[u8; 32]>,
}

impl RootSubnetExecutor {
    /// Create a new ROOT subnet executor
    pub fn new(initial_state_root: TypesHash) -> Self {
        Self {
            current_state_root: initial_state_root,
            pending_updates: HashMap::new(),
            pending_deletions: Vec::new(),
        }
    }
    
    /// Create with empty state
    pub fn empty() -> Self {
        Self::new(ZERO_HASH)
    }
    
    /// Get the current state root
    pub fn state_root(&self) -> TypesHash {
        self.current_state_root
    }
    
    /// Execute a ROOT subnet event
    ///
    /// # Arguments
    /// * `event` - The event to execute
    ///
    /// # Returns
    /// * `Ok(RootExecutionResult)` - The result of execution
    /// * `Err(RootExecutorError)` - If execution fails
    pub fn execute(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        // Validate this is a ROOT subnet event
        if !event.is_validator_executed() {
            return Err(RootExecutorError::NotRootEvent);
        }
        
        match event.event_type {
            EventType::ValidatorRegister => self.execute_validator_register(event),
            EventType::ValidatorUnregister => self.execute_validator_unregister(event),
            EventType::SolverRegister => self.execute_solver_register(event),
            EventType::SolverUnregister => self.execute_solver_unregister(event),
            EventType::System => self.execute_system_event(event),
            _ => {
                // For other event types, just acknowledge without state change
                Ok(RootExecutionResult::empty(
                    event.id.clone(),
                    self.current_state_root,
                ))
            }
        }
    }
    
    /// Execute multiple events in sequence
    pub fn execute_batch(&mut self, events: &[Event]) -> Vec<Result<RootExecutionResult, RootExecutorError>> {
        events.iter().map(|e| self.execute(e)).collect()
    }
    
    /// Execute validator registration
    fn execute_validator_register(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        // Generate object key from event
        let key = self.generate_validator_key(&event.creator);
        
        // Create validator object state
        let state = ObjectStateValue::new(
            self.address_to_bytes(&event.creator),
            1, // Initial version
            object_type::VALIDATOR_INFO,
            *sha256(event.id.as_bytes()).as_bytes(),
            SubnetId::ROOT,
        );
        
        self.pending_updates.insert(key, state.clone());
        
        // Compute new state root
        let new_root = self.compute_pending_root();
        self.current_state_root = new_root;
        
        let mut updated = HashMap::new();
        updated.insert(key, state);
        
        Ok(RootExecutionResult {
            updated_objects: updated,
            deleted_objects: Vec::new(),
            new_state_root: new_root,
            event_id: event.id.clone(),
        })
    }
    
    /// Execute validator unregistration
    fn execute_validator_unregister(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        let key = self.generate_validator_key(&event.creator);
        
        self.pending_deletions.push(key);
        
        let new_root = self.compute_pending_root();
        self.current_state_root = new_root;
        
        Ok(RootExecutionResult {
            updated_objects: HashMap::new(),
            deleted_objects: vec![key],
            new_state_root: new_root,
            event_id: event.id.clone(),
        })
    }
    
    /// Execute solver registration
    fn execute_solver_register(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        let key = self.generate_solver_key(&event.creator);
        
        let state = ObjectStateValue::new(
            self.address_to_bytes(&event.creator),
            1,
            object_type::SOLVER_INFO,
            *sha256(event.id.as_bytes()).as_bytes(),
            SubnetId::ROOT,
        );
        
        self.pending_updates.insert(key, state.clone());
        
        let new_root = self.compute_pending_root();
        self.current_state_root = new_root;
        
        let mut updated = HashMap::new();
        updated.insert(key, state);
        
        Ok(RootExecutionResult {
            updated_objects: updated,
            deleted_objects: Vec::new(),
            new_state_root: new_root,
            event_id: event.id.clone(),
        })
    }
    
    /// Execute solver unregistration
    fn execute_solver_unregister(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        let key = self.generate_solver_key(&event.creator);
        
        self.pending_deletions.push(key);
        
        let new_root = self.compute_pending_root();
        self.current_state_root = new_root;
        
        Ok(RootExecutionResult {
            updated_objects: HashMap::new(),
            deleted_objects: vec![key],
            new_state_root: new_root,
            event_id: event.id.clone(),
        })
    }
    
    /// Execute system event (config updates, etc.)
    fn execute_system_event(&mut self, event: &Event) -> Result<RootExecutionResult, RootExecutorError> {
        let key = self.generate_config_key("global");
        
        let state = ObjectStateValue::new(
            [0u8; 32], // System owned
            1,
            object_type::GLOBAL_CONFIG,
            *sha256(event.id.as_bytes()).as_bytes(),
            SubnetId::ROOT,
        );
        
        self.pending_updates.insert(key, state.clone());
        
        let new_root = self.compute_pending_root();
        self.current_state_root = new_root;
        
        let mut updated = HashMap::new();
        updated.insert(key, state);
        
        Ok(RootExecutionResult {
            updated_objects: updated,
            deleted_objects: Vec::new(),
            new_state_root: new_root,
            event_id: event.id.clone(),
        })
    }
    
    /// Generate object key for a validator
    fn generate_validator_key(&self, validator_id: &str) -> [u8; 32] {
        let mut key = [0u8; 32];
        key[0] = object_type::VALIDATOR_INFO;
        let hash = sha256(validator_id.as_bytes());
        key[1..].copy_from_slice(&hash.as_bytes()[..31]);
        key
    }
    
    /// Generate object key for a solver
    fn generate_solver_key(&self, solver_id: &str) -> [u8; 32] {
        let mut key = [0u8; 32];
        key[0] = object_type::SOLVER_INFO;
        let hash = sha256(solver_id.as_bytes());
        key[1..].copy_from_slice(&hash.as_bytes()[..31]);
        key
    }
    
    /// Generate object key for config
    fn generate_config_key(&self, config_name: &str) -> [u8; 32] {
        let mut key = [0u8; 32];
        key[0] = object_type::GLOBAL_CONFIG;
        let hash = sha256(config_name.as_bytes());
        key[1..].copy_from_slice(&hash.as_bytes()[..31]);
        key
    }
    
    /// Convert address string to bytes
    fn address_to_bytes(&self, address: &str) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        let hash = sha256(address.as_bytes());
        bytes.copy_from_slice(hash.as_bytes());
        bytes
    }
    
    /// Compute the pending state root (simplified version)
    /// In production, this would use the actual SMT
    fn compute_pending_root(&self) -> TypesHash {
        // Simplified: hash all pending updates together
        let mut hasher_input = Vec::new();
        hasher_input.extend_from_slice(&self.current_state_root);
        
        for (key, value) in &self.pending_updates {
            hasher_input.extend_from_slice(key);
            hasher_input.extend_from_slice(&value.hash());
        }
        
        for key in &self.pending_deletions {
            hasher_input.extend_from_slice(key);
            hasher_input.extend_from_slice(&[0xFF; 32]); // Deletion marker
        }
        
        *sha256(&hasher_input).as_bytes()
    }
    
    /// Commit pending changes
    pub fn commit(&mut self) {
        self.pending_updates.clear();
        self.pending_deletions.clear();
    }
    
    /// Rollback pending changes
    pub fn rollback(&mut self) {
        self.pending_updates.clear();
        self.pending_deletions.clear();
    }
}

impl Default for RootSubnetExecutor {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::VLCSnapshot;

    fn create_validator_register_event(creator: &str) -> Event {
        let mut event = Event::new(
            EventType::ValidatorRegister,
            vec![],
            VLCSnapshot::default(),
            creator.to_string(),
        );
        event.subnet_id = Some(SubnetId::ROOT);
        event
    }

    fn create_solver_register_event(creator: &str) -> Event {
        let mut event = Event::new(
            EventType::SolverRegister,
            vec![],
            VLCSnapshot::default(),
            creator.to_string(),
        );
        event.subnet_id = Some(SubnetId::ROOT);
        event
    }

    #[test]
    fn test_execute_validator_register() {
        let mut executor = RootSubnetExecutor::empty();
        let event = create_validator_register_event("validator1");
        
        let result = executor.execute(&event).unwrap();
        
        assert!(result.has_changes());
        assert_eq!(result.updated_objects.len(), 1);
        assert!(result.deleted_objects.is_empty());
        assert_ne!(result.new_state_root, ZERO_HASH);
    }

    #[test]
    fn test_execute_solver_register() {
        let mut executor = RootSubnetExecutor::empty();
        let event = create_solver_register_event("solver1");
        
        let result = executor.execute(&event).unwrap();
        
        assert!(result.has_changes());
        assert_eq!(result.updated_objects.len(), 1);
    }

    #[test]
    fn test_execute_batch() {
        let mut executor = RootSubnetExecutor::empty();
        
        let events = vec![
            create_validator_register_event("v1"),
            create_validator_register_event("v2"),
            create_solver_register_event("s1"),
        ];
        
        let results = executor.execute_batch(&events);
        
        assert_eq!(results.len(), 3);
        for result in results {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_state_root_changes() {
        let mut executor = RootSubnetExecutor::empty();
        let initial_root = executor.state_root();
        
        let event = create_validator_register_event("validator1");
        executor.execute(&event).unwrap();
        
        let new_root = executor.state_root();
        assert_ne!(initial_root, new_root);
        
        // Another event should produce another different root
        let event2 = create_validator_register_event("validator2");
        executor.execute(&event2).unwrap();
        
        let final_root = executor.state_root();
        assert_ne!(new_root, final_root);
    }
}
