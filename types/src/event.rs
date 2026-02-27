//! Event types for the DAG-based ledger
//!
//! Events are the fundamental unit of state change in Setu.
//! They form a DAG (Directed Acyclic Graph) with causal ordering.

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

// Re-export VLC types from setu-vlc (single source of truth)
pub use setu_vlc::{VectorClock, VLCSnapshot};

// Import from sibling modules
use crate::transfer::Transfer;
use crate::registration::{
    ValidatorRegistration, SolverRegistration, Unregistration,
    SubnetRegistration, UserRegistration,
    PowerConsumption, TaskSubmission,
};

// ========== Event ID ==========

pub type EventId = String;

// ========== Event Status ==========

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventStatus {
    /// Event created, not yet processed
    Pending,
    /// Event in work queue for execution
    InWorkQueue,
    /// Event executed successfully
    Executed,
    /// Event confirmed by consensus
    Confirmed,
    /// Event finalized (irreversible)
    Finalized,
    /// Event execution failed
    Failed,
}

impl Default for EventStatus {
    fn default() -> Self {
        EventStatus::Pending
    }
}

// ========== Event Type ==========

/// Event types supported by the system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    /// Genesis event (first event in the DAG)
    Genesis,
    /// System event (internal operations)
    System,
    /// Transfer event (Flux transfer between accounts)
    Transfer,
    /// Validator registration event
    ValidatorRegister,
    /// Validator unregistration event
    ValidatorUnregister,
    /// Solver registration event
    SolverRegister,
    /// Solver unregistration event
    SolverUnregister,
    /// Subnet registration event (create a new subnet)
    SubnetRegister,
    /// User registration event (register a user in a subnet)
    UserRegister,
    /// Power consumption event
    PowerConsume,
    /// Task submission event
    TaskSubmit,
    /// Agent chat event (future)
    AgentChat,
    /// SBT/Relationship event (future)
    Relationship,
}

impl EventType {
    /// Check if this event type requires execution by Solver
    pub fn requires_solver_execution(&self) -> bool {
        matches!(
            self,
            EventType::Transfer
                | EventType::ValidatorRegister
                | EventType::ValidatorUnregister
                | EventType::SolverRegister
                | EventType::SolverUnregister
                | EventType::SubnetRegister
                | EventType::UserRegister
                | EventType::PowerConsume
                | EventType::TaskSubmit
        )
    }
    
    /// Check if this event should be routed to ROOT subnet
    /// These events are executed by validators directly, not solvers
    pub fn is_root_event(&self) -> bool {
        matches!(
            self,
            EventType::Genesis
                | EventType::System
                | EventType::ValidatorRegister
                | EventType::ValidatorUnregister
                | EventType::SolverRegister
                | EventType::SolverUnregister
                | EventType::SubnetRegister
                | EventType::UserRegister
        )
    }
    
    /// Check if this event should be executed by validators (ROOT) or solvers (App)
    pub fn is_validator_executed(&self) -> bool {
        self.is_root_event()
    }
    
    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            EventType::Genesis => "Genesis",
            EventType::System => "System",
            EventType::Transfer => "Transfer",
            EventType::ValidatorRegister => "ValidatorRegister",
            EventType::ValidatorUnregister => "ValidatorUnregister",
            EventType::SolverRegister => "SolverRegister",
            EventType::SolverUnregister => "SolverUnregister",
            EventType::SubnetRegister => "SubnetRegister",
            EventType::UserRegister => "UserRegister",
            EventType::PowerConsume => "PowerConsume",
            EventType::TaskSubmit => "TaskSubmit",
            EventType::AgentChat => "AgentChat",
            EventType::Relationship => "Relationship",
        }
    }
}

use crate::genesis::GenesisConfig;

// ========== Event Payload ==========

/// Event payload - contains the actual data for different event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventPayload {
    None,
    Genesis(GenesisConfig),
    Transfer(Transfer),
    ValidatorRegister(ValidatorRegistration),
    ValidatorUnregister(Unregistration),
    SolverRegister(SolverRegistration),
    SolverUnregister(Unregistration),
    SubnetRegister(SubnetRegistration),
    UserRegister(UserRegistration),
    PowerConsume(PowerConsumption),
    TaskSubmit(TaskSubmission),
}

impl Default for EventPayload {
    fn default() -> Self {
        EventPayload::None
    }
}

// ========== Execution Result ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub message: Option<String>,
    pub state_changes: Vec<StateChange>,
}

impl ExecutionResult {
    pub fn success() -> Self {
        Self {
            success: true,
            message: None,
            state_changes: vec![],
        }
    }
    
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            state_changes: vec![],
        }
    }
    
    pub fn with_changes(mut self, changes: Vec<StateChange>) -> Self {
        self.state_changes = changes;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub key: String,
    pub old_value: Option<Vec<u8>>,
    pub new_value: Option<Vec<u8>>,
}

impl StateChange {
    pub fn new(key: impl Into<String>, old_value: Option<Vec<u8>>, new_value: Option<Vec<u8>>) -> Self {
        Self {
            key: key.into(),
            old_value,
            new_value,
        }
    }
    
    pub fn insert(key: impl Into<String>, value: Vec<u8>) -> Self {
        Self::new(key, None, Some(value))
    }
    
    pub fn delete(key: impl Into<String>, old_value: Vec<u8>) -> Self {
        Self::new(key, Some(old_value), None)
    }
    
    pub fn update(key: impl Into<String>, old_value: Vec<u8>, new_value: Vec<u8>) -> Self {
        Self::new(key, Some(old_value), Some(new_value))
    }
}

// ========== Event ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique event identifier (hash-based)
    pub id: EventId,
    
    /// Type of this event
    pub event_type: EventType,
    
    /// Parent event IDs (DAG edges)
    pub parent_ids: Vec<EventId>,
    
    /// The subnet this event belongs to (determines routing)
    #[serde(default)]
    pub subnet_id: Option<crate::subnet::SubnetId>,
    
    /// Legacy transfer field (for backward compatibility)
    #[serde(default)]
    pub transfer: Option<Transfer>,
    
    /// Unified payload field
    #[serde(default)]
    pub payload: EventPayload,
    
    /// VLC snapshot at event creation
    pub vlc_snapshot: VLCSnapshot,
    
    /// Creator node ID
    pub creator: String,
    
    /// Current status
    pub status: EventStatus,
    
    /// Execution result (if executed)
    #[serde(default)]
    pub execution_result: Option<ExecutionResult>,
    
    /// Creation timestamp (milliseconds since epoch)
    pub timestamp: u64,
}

impl Event {
    /// Create a new event
    pub fn new(
        event_type: EventType,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        let id = Self::compute_id(&parent_ids, &vlc_snapshot, &creator, timestamp);
        
        // Infer subnet_id from event_type
        let subnet_id = if event_type.is_root_event() {
            Some(crate::subnet::SubnetId::ROOT)
        } else {
            None
        };
        
        Self {
            id,
            event_type,
            parent_ids,
            subnet_id,
            transfer: None,
            payload: EventPayload::None,
            vlc_snapshot,
            creator,
            status: EventStatus::Pending,
            execution_result: None,
            timestamp,
        }
    }

    /// Create a genesis event
    pub fn genesis(creator: String, vlc_snapshot: VLCSnapshot) -> Self {
        Self::new(EventType::Genesis, vec![], vlc_snapshot, creator)
    }
    
    /// Create a transfer event
    pub fn transfer(
        transfer: Transfer,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::Transfer, parent_ids, vlc_snapshot, creator);
        event.transfer = Some(transfer.clone());
        event.payload = EventPayload::Transfer(transfer);
        event
    }
    
    /// Create a validator registration event
    pub fn validator_register(
        registration: ValidatorRegistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::ValidatorRegister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::ValidatorRegister(registration);
        event
    }
    
    /// Create a solver registration event
    pub fn solver_register(
        registration: SolverRegistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::SolverRegister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::SolverRegister(registration);
        event
    }
    
    /// Create a validator unregistration event
    pub fn validator_unregister(
        unregistration: Unregistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::ValidatorUnregister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::ValidatorUnregister(unregistration);
        event
    }
    
    /// Create a solver unregistration event
    pub fn solver_unregister(
        unregistration: Unregistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::SolverUnregister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::SolverUnregister(unregistration);
        event
    }
    
    /// Create a power consumption event
    pub fn power_consume(
        consumption: PowerConsumption,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::PowerConsume, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::PowerConsume(consumption);
        event
    }
    
    /// Create a task submission event
    pub fn task_submit(
        task: TaskSubmission,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::TaskSubmit, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::TaskSubmit(task);
        event
    }
    
    /// Create a subnet registration event
    pub fn subnet_register(
        registration: SubnetRegistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::SubnetRegister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::SubnetRegister(registration);
        event
    }
    
    /// Create a user registration event
    pub fn user_register(
        registration: UserRegistration,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::UserRegister, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::UserRegister(registration);
        event
    }

    fn compute_id(
        parent_ids: &[EventId],
        vlc_snapshot: &VLCSnapshot,
        creator: &str,
        timestamp: u64,
    ) -> EventId {
        let mut hasher = Sha256::new();
        for parent_id in parent_ids {
            hasher.update(parent_id.as_bytes());
        }
        hasher.update(vlc_snapshot.logical_time.to_le_bytes());
        hasher.update(creator.as_bytes());
        hasher.update(timestamp.to_le_bytes());
        hex::encode(hasher.finalize())
    }
    
    /// Verify that the event ID matches the content (anti-tampering check)
    /// 
    /// This should be called when receiving events from untrusted sources
    /// (e.g., network peers) to ensure the event wasn't tampered with.
    /// 
    /// Returns `true` if the ID is valid, `false` if tampered.
    pub fn verify_id(&self) -> bool {
        let computed = Self::compute_id(
            &self.parent_ids,
            &self.vlc_snapshot,
            &self.creator,
            self.timestamp,
        );
        computed == self.id
    }

    /// Legacy method for backward compatibility
    pub fn with_transfer(mut self, transfer: Transfer) -> Self {
        self.transfer = Some(transfer.clone());
        self.payload = EventPayload::Transfer(transfer);
        self
    }
    
    /// Set payload
    pub fn with_payload(mut self, payload: EventPayload) -> Self {
        self.payload = payload;
        self
    }

    pub fn set_status(&mut self, status: EventStatus) {
        self.status = status;
    }

    pub fn set_execution_result(&mut self, result: ExecutionResult) {
        let success = result.success;
        self.execution_result = Some(result);
        self.status = if success { EventStatus::Executed } else { EventStatus::Failed };
    }

    pub fn is_genesis(&self) -> bool {
        self.event_type == EventType::Genesis
    }

    pub fn has_parents(&self) -> bool {
        !self.parent_ids.is_empty()
    }

    pub fn depends_on(&self, event_id: &EventId) -> bool {
        self.parent_ids.contains(event_id)
    }
    
    /// Check if this event is a registration event
    pub fn is_registration(&self) -> bool {
        matches!(
            self.event_type,
            EventType::ValidatorRegister 
                | EventType::SolverRegister
                | EventType::SubnetRegister
                | EventType::UserRegister
        )
    }
    
    /// Check if this event is an unregistration event
    pub fn is_unregistration(&self) -> bool {
        matches!(
            self.event_type,
            EventType::ValidatorUnregister | EventType::SolverUnregister
        )
    }
    
    /// Get the subnet this event belongs to
    pub fn get_subnet_id(&self) -> crate::subnet::SubnetId {
        self.subnet_id.unwrap_or_else(|| {
            if self.event_type.is_root_event() {
                crate::subnet::SubnetId::ROOT
            } else {
                crate::subnet::SubnetId::ROOT // Default for backward compatibility
            }
        })
    }
    
    /// Set the subnet for this event
    pub fn with_subnet(mut self, subnet_id: crate::subnet::SubnetId) -> Self {
        self.subnet_id = Some(subnet_id);
        self
    }
    
    /// Check if this event has an explicit subnet assignment
    pub fn has_subnet(&self) -> bool {
        self.subnet_id.is_some()
    }
    
    /// Check if this event should be executed by validators
    pub fn is_validator_executed(&self) -> bool {
        self.event_type.is_validator_executed()
    }
    
    /// Get resources affected by this event (for dependency tracking)
    pub fn affected_resources(&self) -> Vec<String> {
        match &self.payload {
            EventPayload::Transfer(t) => t.affected_accounts(),
            EventPayload::ValidatorRegister(r) => {
                vec![format!("validator:{}", r.validator_id)]
            }
            EventPayload::SolverRegister(r) => {
                vec![format!("solver:{}", r.solver_id)]
            }
            EventPayload::ValidatorUnregister(u) | EventPayload::SolverUnregister(u) => {
                vec![format!("node:{}", u.node_id)]
            }
            EventPayload::SubnetRegister(s) => {
                vec![format!("subnet:{}", s.subnet_id)]
            }
            EventPayload::UserRegister(u) => {
                vec![
                    format!("user:{}", u.address),
                    format!("subnet:{}", u.get_subnet()),
                ]
            }
            EventPayload::PowerConsume(p) => {
                vec![format!("power:{}", p.user_id)]
            }
            EventPayload::TaskSubmit(t) => {
                vec![format!("task:{}", t.task_id)]
            }
            EventPayload::None => vec![],
            EventPayload::Genesis(g) => {
                g.accounts.iter()
                    .map(|a| format!("account:{}", a.name))
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::TransferType;

    fn create_vlc_snapshot() -> VLCSnapshot {
        VLCSnapshot {
            vector_clock: VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        }
    }

    #[test]
    fn test_event_creation() {
        let event = Event::new(
            EventType::Transfer,
            vec![],
            create_vlc_snapshot(),
            "node1".to_string(),
        );
        assert!(!event.id.is_empty());
        assert_eq!(event.status, EventStatus::Pending);
    }

    #[test]
    fn test_genesis_event() {
        let event = Event::genesis("node1".to_string(), create_vlc_snapshot());
        assert!(event.is_genesis());
        assert!(!event.has_parents());
    }
    
    #[test]
    fn test_transfer_event() {
        let transfer = Transfer::new("tx-1", "alice", "bob", 100)
            .with_type(TransferType::FluxTransfer);
        
        let event = Event::transfer(
            transfer,
            vec![],
            create_vlc_snapshot(),
            "solver1".to_string(),
        );
        assert_eq!(event.event_type, EventType::Transfer);
        assert!(event.transfer.is_some());
        
        let resources = event.affected_resources();
        assert!(resources.contains(&"account:alice".to_string()));
        assert!(resources.contains(&"account:bob".to_string()));
    }
    
    #[test]
    fn test_solver_register_event() {
        let registration = SolverRegistration::new(
            "solver-1",
            "127.0.0.1",
            9001,
            "0xabcd1234",
            vec![1, 2, 3],
            vec![4, 5, 6],
        )
            .with_shard("shard-0")
            .with_resources(vec!["ETH".to_string()]);
        
        let event = Event::solver_register(
            registration,
            vec![],
            create_vlc_snapshot(),
            "solver-1".to_string(),
        );
        assert_eq!(event.event_type, EventType::SolverRegister);
        assert!(event.is_registration());
        
        let resources = event.affected_resources();
        assert!(resources.contains(&"solver:solver-1".to_string()));
    }
    
    #[test]
    fn test_event_type_requires_solver() {
        assert!(EventType::Transfer.requires_solver_execution());
        assert!(EventType::SolverRegister.requires_solver_execution());
        assert!(EventType::ValidatorRegister.requires_solver_execution());
        assert!(EventType::SubnetRegister.requires_solver_execution());
        assert!(EventType::UserRegister.requires_solver_execution());
        assert!(!EventType::Genesis.requires_solver_execution());
        assert!(!EventType::System.requires_solver_execution());
    }
    
    #[test]
    fn test_subnet_register_event() {
        use crate::registration::SubnetResourceLimits;
        
        let limits = SubnetResourceLimits::new()
            .with_tps(1000)
            .with_storage(1024 * 1024 * 1024);
        
        let registration = SubnetRegistration::new("subnet-1", "DeFi App", "alice", "DEFI")
            .with_limits(limits)
            .with_solvers(vec!["solver-1".to_string(), "solver-2".to_string()]);
        
        let event = Event::subnet_register(
            registration,
            vec![],
            create_vlc_snapshot(),
            "alice".to_string(),
        );
        assert_eq!(event.event_type, EventType::SubnetRegister);
        assert!(event.is_registration());
        
        let resources = event.affected_resources();
        assert!(resources.contains(&"subnet:subnet-1".to_string()));
    }
    
    #[test]
    fn test_user_register_event() {
        let registration = UserRegistration::from_nostr(
            "0x1234567890abcdef",  // address
            vec![1; 32],            // nostr_pubkey (32 bytes)
            vec![4, 5, 6],          // signature
            1234567890,             // timestamp
        )
            .with_subnet("subnet-1")
            .with_display_name("Alice");
        
        let event = Event::user_register(
            registration,
            vec![],
            create_vlc_snapshot(),
            "0x1234567890abcdef".to_string(),
        );
        assert_eq!(event.event_type, EventType::UserRegister);
        assert!(event.is_registration());
        
        let resources = event.affected_resources();
        assert!(resources.contains(&"user:0x1234567890abcdef".to_string()));
        assert!(resources.contains(&"subnet:subnet-1".to_string()));
    }
    
    #[test]
    fn test_execution_result() {
        let result = ExecutionResult::success()
            .with_changes(vec![StateChange::insert("key1", vec![1, 2, 3])]);
        assert!(result.success);
        assert_eq!(result.state_changes.len(), 1);
    }
}
