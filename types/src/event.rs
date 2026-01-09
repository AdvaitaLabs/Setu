use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

// Use independent VLC library
pub use setu_vlc::{VectorClock, VLCSnapshot};

// Placeholder types (to be replaced with actual implementations)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub from: String,
    pub to: String,
    pub amount: u64,
}

/// Validator registration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorRegistration {
    pub validator_id: String,
    pub address: String,
    pub port: u16,
    pub public_key: Option<Vec<u8>>,
    pub stake: Option<u64>,
}

/// Solver registration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverRegistration {
    pub solver_id: String,
    pub address: String,
    pub port: u16,
    pub capacity: u32,
    pub shard_id: Option<String>,
    pub resources: Vec<String>,
    pub public_key: Option<Vec<u8>>,
}

/// Power consumption data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerConsumption {
    pub user_id: String,
    pub amount: u64,
    pub reason: String,
}

/// Task submission data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmission {
    pub task_id: String,
    pub task_type: String,
    pub submitter: String,
    pub payload: Vec<u8>,
}

/// Unregistration data (for both Validator and Solver)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unregistration {
    pub node_id: String,
    pub node_type: String, // "validator" or "solver"
}

pub type EventId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventStatus {
    Pending,
    InWorkQueue,
    Executed,
    Confirmed,
    Finalized,
    Failed,
}

/// Event types supported by the system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
            EventType::PowerConsume => "PowerConsume",
            EventType::TaskSubmit => "TaskSubmit",
            EventType::AgentChat => "AgentChat",
            EventType::Relationship => "Relationship",
        }
    }
}

/// Event payload - contains the actual data for different event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventPayload {
    None,
    Transfer(Transfer),
    ValidatorRegister(ValidatorRegistration),
    ValidatorUnregister(Unregistration),
    SolverRegister(SolverRegistration),
    SolverUnregister(Unregistration),
    PowerConsume(PowerConsumption),
    TaskSubmit(TaskSubmission),
}

impl Default for EventPayload {
    fn default() -> Self {
        EventPayload::None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub event_type: EventType,
    pub parent_ids: Vec<EventId>,
    /// The subnet this event belongs to (determines routing)
    /// If None, will be inferred from event_type
    #[serde(default)]
    pub subnet_id: Option<crate::subnet::SubnetId>,
    /// Legacy transfer field (for backward compatibility)
    pub transfer: Option<Transfer>,
    /// New unified payload field
    pub payload: EventPayload,
    pub vlc_snapshot: VLCSnapshot,
    pub creator: String,
    pub status: EventStatus,
    pub execution_result: Option<ExecutionResult>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub message: Option<String>,
    pub state_changes: Vec<StateChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub key: String,
    pub old_value: Option<Vec<u8>>,
    pub new_value: Option<Vec<u8>>,
}

impl Event {
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
            None // App subnet, to be specified by caller
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
        if success {
            self.status = EventStatus::Executed;
        } else {
            self.status = EventStatus::Failed;
        }
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
            EventType::ValidatorRegister | EventType::SolverRegister
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
    /// Returns ROOT subnet if not explicitly set and event is a root event
    pub fn get_subnet_id(&self) -> crate::subnet::SubnetId {
        self.subnet_id.unwrap_or_else(|| {
            if self.event_type.is_root_event() {
                crate::subnet::SubnetId::ROOT
            } else {
                // Default to ROOT if not specified (for backward compatibility)
                crate::subnet::SubnetId::ROOT
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
    
    /// Check if this event should be executed by validators (ROOT) or solvers (App)
    pub fn is_validator_executed(&self) -> bool {
        self.event_type.is_validator_executed()
    }
    
    /// Get resources affected by this event (for dependency tracking)
    pub fn affected_resources(&self) -> Vec<String> {
        match &self.payload {
            EventPayload::Transfer(t) => {
                vec![format!("account:{}", t.from), format!("account:{}", t.to)]
            }
            EventPayload::ValidatorRegister(r) => {
                vec![format!("validator:{}", r.validator_id)]
            }
            EventPayload::SolverRegister(r) => {
                vec![format!("solver:{}", r.solver_id)]
            }
            EventPayload::ValidatorUnregister(u) | EventPayload::SolverUnregister(u) => {
                vec![format!("node:{}", u.node_id)]
            }
            EventPayload::PowerConsume(p) => {
                vec![format!("power:{}", p.user_id)]
            }
            EventPayload::TaskSubmit(t) => {
                vec![format!("task:{}", t.task_id)]
            }
            EventPayload::None => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let transfer = Transfer {
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100,
        };
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
        let registration = SolverRegistration {
            solver_id: "solver-1".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9001,
            capacity: 100,
            shard_id: Some("shard-0".to_string()),
            resources: vec!["ETH".to_string()],
            public_key: None,
        };
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
        assert!(!EventType::Genesis.requires_solver_execution());
        assert!(!EventType::System.requires_solver_execution());
    }
}
