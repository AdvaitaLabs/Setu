//! Event types for the DAG-based ledger
//!
//! Events are the fundamental unit of state change in Setu.
//! They form a DAG (Directed Acyclic Graph) with causal ordering.

use serde::{Deserialize, Serialize};

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
    /// Agent chat event
    AgentChat,
    /// SBT/Relationship event
    Relationship,
    /// Smart contract / custom module function call
    /// Not bound to Move — also usable for WASM or other execution engines
    ContractCall,
    /// Smart contract / module publish
    ContractPublish,
    /// Coin merge event (merge multiple coins into one)
    CoinMerge,
    /// Coin split event (split one coin into multiple)
    CoinSplit,
    /// Atomic compound: merge coins then transfer (partial or full)
    CoinMergeThenTransfer,
    /// Governance proposal and execution (payload distinguishes action)
    Governance,
}

impl EventType {
    /// Check if this event type requires execution (by Solver or Validator)
    ///
    /// NOTE: Despite the name, this function means "requires execution" (not
    /// specifically Solver execution). All events that need state changes return true.
    /// Use `is_root_event()` / `is_validator_executed()` to distinguish who executes.
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
                | EventType::AgentChat
                | EventType::Relationship
                | EventType::ContractCall
                | EventType::ContractPublish
                | EventType::CoinMerge
                | EventType::CoinSplit
                | EventType::CoinMergeThenTransfer
                | EventType::Governance
        )
    }
    
    /// Check if this event should be routed to ROOT subnet
    /// These events are executed by validators directly, not solvers
    ///
    /// NOTE: `ContractPublish` is classified as a root event in Phase 1
    /// (all module publishes go through ROOT / Validator). When per-subnet
    /// contract deployment is needed, move it out and add subnet-aware routing.
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
                | EventType::ContractPublish
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
            EventType::ContractCall => "ContractCall",
            EventType::ContractPublish => "ContractPublish",
            EventType::CoinMerge => "CoinMerge",
            EventType::CoinSplit => "CoinSplit",
            EventType::CoinMergeThenTransfer => "CoinMergeThenTransfer",
            EventType::Governance => "Governance",
        }
    }
}

use crate::genesis::GenesisConfig;
use crate::governance::GovernancePayload;
use crate::object::ObjectId;

// ========== Move-specific Payload Types ==========

fn default_true() -> bool { true }

/// Move function call payload (paired with EventType::ContractCall)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCallPayload {
    /// Transaction sender address (hex).
    /// Validator MUST verify: sender == Event signer public key derived address.
    pub sender: String,
    /// Target module address (hex), e.g. "0x1"
    pub package: String,
    /// Module name, e.g. "coin"
    pub module: String,
    /// Function name, e.g. "transfer"
    pub function: String,
    /// Type arguments (Move TypeTag string representation)
    pub type_args: Vec<String>,
    /// Pure arguments — BCS serialized, no object references.
    /// Mapped to `pure_args` in OperationType::MoveCall.
    pub args: Vec<Vec<u8>>,
    /// Input object IDs (referenced or consumed)
    pub input_object_ids: Vec<ObjectId>,
    /// Shared object IDs (Phase 0-4: must be empty — ADR-1)
    pub shared_object_ids: Vec<ObjectId>,
    /// Mutable reference indices into input_object_ids (&mut T params)
    #[serde(default)]
    pub mutable_indices: Option<Vec<usize>>,
    /// Consumed object indices into input_object_ids (by-value T params)
    #[serde(default)]
    pub consumed_indices: Option<Vec<usize>>,
    /// Whether the target function takes a `&mut TxContext` last parameter.
    /// When true, engine.execute() auto-appends BCS(TxContext) to args.
    #[serde(default = "default_true")]
    pub needs_tx_context: bool,
    /// Dynamic-field accesses declared by the client (DF FDP M4).
    ///
    /// Each entry is resolved by `TaskPreparer` against the pre-execution SMT
    /// and becomes a `ResolvedDynamicField` in `ResolvedInputs.dynamic_fields`,
    /// from which the VM builds the `SetuObjectRuntime.df_cache`. `#[serde(default)]`
    /// keeps payloads written before M4 wire-compatible.
    #[serde(default)]
    pub dynamic_field_accesses: Vec<DynamicFieldAccess>,
}

/// Client-declared dynamic-field access (DF FDP M4, see
/// `docs/feat/dynamic-fields/design.md` §3.3).
///
/// `parent_object_id` and `key_bcs_hex` are plain hex strings (optional
/// `"oid:"` prefix on the parent is tolerated). `key_type` / `value_type`
/// are canonical Move `TypeTag` strings (e.g. `"u64"`, `"address"`,
/// `"0xcafe::pool::Pair"`).
///
/// `value_type` is `Some` and required when `mode == Create` (the DF entry
/// does not yet exist so the type cannot be inferred from on-disk bytes);
/// it may be `None` for Read/Mutate/Delete since the value type is recovered
/// from the parent DF envelope at prepare time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldAccess {
    pub parent_object_id: String,
    pub key_type: String,
    pub key_bcs_hex: String,
    pub mode: crate::dynamic_field::DfAccessMode,
    #[serde(default)]
    pub value_type: Option<String>,
}

/// Move module publish payload (paired with EventType::ContractPublish)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePublishPayload {
    /// Compiled module bytecode (one package may contain multiple modules)
    pub modules: Vec<Vec<u8>>,
}

/// Move package upgrade payload (B5 — paired with EventType::ContractPublish
/// umbrella, distinguished from publish via `EventPayload::MoveUpgrade`).
///
/// **BCS field order is LOCKED** (design.md §3 R1g) — appending fields is fine,
/// re-ordering or inserting silently invalidates every upgrade event in storage
/// and all historical hashes (G1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveUpgradePayload {
    /// EOA submitting the upgrade (cap holder).
    pub sender: crate::object::Address,
    /// Family root = v0 package address (immutable).
    pub family_id: crate::object::ObjectId,
    /// Current latest package being superseded (= prev package address).
    pub prev_package: crate::object::ObjectId,
    /// Post-relink, deterministically derived (design.md §4.5).
    pub new_package_addr: crate::object::Address,
    /// = prev_version + 1; replay-asserted.
    pub new_version: u64,
    /// Post-relink BCS bytes for each module in the bundle.
    pub modules: Vec<Vec<u8>>,
    /// Dependency package IDs (matches `Command::Publish.deps` shape).
    pub deps: Vec<crate::object::ObjectId>,
    /// Ticket-supplied digest = bcs(modules || deps); validator re-derives
    /// and asserts equality at apply time.
    pub digest: Vec<u8>,
    /// Owned object ID of the UpgradeCap consumed by this upgrade. Used at
    /// replay for mutation-order assertion.
    pub upgrade_cap_id: crate::object::ObjectId,
    /// Compatibility policy copied from the ticket (0=Compatible, 1=Additive,
    /// 2=DepOnly).
    pub policy: u8,
}

/// Programmable Transaction Block payload (paired with EventType::ContractCall).
///
/// PTB is the "many commands, one atomic effect" Move execution mode.
/// Compared to [`MoveCallPayload`], all input-object metadata (ID, version,
/// digest, mutable flag) lives inside `ptb.inputs[].ObjectArg` — there is no
/// parallel `input_object_ids` / `mutable_indices` / `consumed_indices` list.
///
/// EventType reuse: PTB rides on `EventType::ContractCall` (not a new
/// EventType variant). See `docs/feat/move-vm-phase9-ptb-event-wire/design.md`
/// §4 for the rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePtbPayload {
    /// Transaction sender (hex address). Validator MUST verify equality
    /// against the Event signer's derived address (mirror of MoveCallPayload).
    pub sender: String,
    /// Programmable transaction body. Its BCS encoding is what consensus hashes.
    pub ptb: crate::ptb::ProgrammableTransaction,
}

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
    /// Agent chat interaction
    AgentChat {
        agent_id: String,
        message: String,
    },
    /// Social graph / relationship operation
    Relationship {
        from: String,
        to: String,
        relation_type: String,
    },
    /// Smart contract / module function call (Phase 1: generic structure)
    ContractCall {
        /// Target contract/module identifier (e.g., "0x1::coin::transfer")
        target: String,
        /// Call arguments (BCS or JSON serialized)
        args: Vec<Vec<u8>>,
    },
    /// Smart contract / module publish
    ContractPublish {
        /// Compiled module bytecode
        modules: Vec<Vec<u8>>,
    },
    /// Coin merge operation (merge sources into target)
    CoinMerge {
        /// Target coin (receives merged balance)
        target_coin_id: String,
        /// Source coins to merge (will be deleted)
        source_coin_ids: Vec<String>,
    },
    /// Coin split operation (split source into multiple)
    CoinSplit {
        /// Source coin to split
        source_coin_id: String,
        /// Amounts for each new coin
        amounts: Vec<u64>,
    },
    /// Atomic compound: merge coins then transfer to recipient
    CoinMergeThenTransfer {
        /// Target coin (receives merged balance, then sends)
        target_coin_id: String,
        /// Source coins to merge (will be deleted)
        source_coin_ids: Vec<String>,
        /// Recipient of the transfer
        recipient: String,
        /// Amount to transfer after merge
        amount: u64,
    },
    /// Governance action (propose or execute)
    Governance(GovernancePayload),
    /// Move function call (paired with EventType::ContractCall)
    MoveCall(MoveCallPayload),
    /// Move module publish (paired with EventType::ContractPublish)
    MovePublish(MovePublishPayload),
    /// Programmable transaction block (paired with EventType::ContractCall).
    ///
    /// **BCS discriminant order is load-bearing.** This MUST stay the tail
    /// variant of `EventPayload`; appending future variants is fine, mid-enum
    /// insertion silently invalidates every PTB event already in storage.
    MovePtb(MovePtbPayload),
    /// Move package upgrade (B5 — paired with `EventType::ContractPublish`
    /// umbrella, see design.md §4 path 乙').
    ///
    /// **MUST stay the tail variant.** Inserting variants between `MovePtb`
    /// and `MoveUpgrade` is forbidden — it would shift the BCS discriminant
    /// for every previously-stored event payload (G1).
    MoveUpgrade(MoveUpgradePayload),
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
    /// Target subnet for this state change. None = use the Event's own subnet_id.
    /// Enables cross-subnet effects (e.g., Governance event writing to ROOT subnet).
    /// Note: must NOT use skip_serializing_if — bincode requires all fields present.
    #[serde(default)]
    pub target_subnet: Option<crate::subnet::SubnetId>,
}

impl StateChange {
    pub fn new(key: impl Into<String>, old_value: Option<Vec<u8>>, new_value: Option<Vec<u8>>) -> Self {
        Self {
            key: key.into(),
            old_value,
            new_value,
            target_subnet: None,
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

    /// Set the target subnet for cross-subnet state changes.
    pub fn with_target_subnet(mut self, subnet_id: crate::subnet::SubnetId) -> Self {
        self.target_subnet = Some(subnet_id);
        self
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
        } else if matches!(event_type, EventType::Governance) {
            Some(crate::subnet::SubnetId::GOVERNANCE)
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

    /// Create a Move function call event (ContractCall)
    pub fn move_call(
        payload: MoveCallPayload,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::ContractCall, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::MoveCall(payload);
        event
    }

    /// Create a Move PTB event (reuses `EventType::ContractCall`).
    ///
    /// EventType reuse: PTB shares the ContractCall umbrella with MoveCall.
    /// See `docs/feat/move-vm-phase9-ptb-event-wire/design.md` §4.
    pub fn move_ptb(
        payload: MovePtbPayload,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::ContractCall, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::MovePtb(payload);
        event
    }

    /// Create a contract publish event (Move module deployment)
    pub fn contract_publish(
        sender: String,
        modules: Vec<Vec<u8>>,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let mut event = Self::new(EventType::ContractPublish, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::MovePublish(MovePublishPayload { modules });
        event.creator = sender;
        // Self::new() sealed event.id against the `creator` argument. We just
        // overwrote `event.creator` with `sender`, which invalidates the ID.
        // Must recompute so `verify_id()` (anti-tampering check in
        // consensus_integration::submit_event) accepts the event.
        event.recompute_id();
        event
    }

    /// Create a Move package upgrade event (B5).
    ///
    /// Reuses the `EventType::ContractPublish` umbrella per design.md §4
    /// path 乙' so the EventType variant count stays at 19 (G13). The
    /// payload's `MoveUpgrade` discriminant distinguishes it from
    /// `MovePublish` at apply / replay time.
    pub fn move_upgrade(
        payload: MoveUpgradePayload,
        parent_ids: Vec<EventId>,
        vlc_snapshot: VLCSnapshot,
        creator: String,
    ) -> Self {
        let sender_hex = format!("0x{}", hex::encode(payload.sender.as_bytes()));
        let mut event = Self::new(EventType::ContractPublish, parent_ids, vlc_snapshot, creator);
        event.payload = EventPayload::MoveUpgrade(payload);
        event.creator = sender_hex;
        event.recompute_id();
        event
    }

    fn compute_id(
        parent_ids: &[EventId],
        vlc_snapshot: &VLCSnapshot,
        creator: &str,
        timestamp: u64,
    ) -> EventId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_EVENT_ID:");
        for parent_id in parent_ids {
            hasher.update(parent_id.as_bytes());
        }
        hasher.update(&vlc_snapshot.logical_time.to_le_bytes());
        hasher.update(creator.as_bytes());
        hasher.update(&timestamp.to_le_bytes());
        hex::encode(hasher.finalize().as_bytes())
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

    /// Recompute and update the event ID based on current fields.
    ///
    /// Must be called after modifying `creator`, `timestamp`, or `vlc_snapshot`
    /// to keep the ID consistent. Used for deterministic genesis events.
    pub fn recompute_id(&mut self) {
        self.id = Self::compute_id(
            &self.parent_ids,
            &self.vlc_snapshot,
            &self.creator,
            self.timestamp,
        );
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
            EventPayload::AgentChat { agent_id, .. } => {
                vec![format!("agent:{}", agent_id)]
            }
            EventPayload::Relationship { from, to, .. } => {
                vec![format!("user:{}", from), format!("user:{}", to)]
            }
            EventPayload::ContractCall { target, .. } => {
                vec![format!("contract:{}", target)]
            }
            EventPayload::ContractPublish { .. } => {
                // Publisher is sender, captured by event.sender
                vec![]
            }
            EventPayload::CoinMerge { target_coin_id, source_coin_ids } => {
                let mut resources = vec![format!("coin:{}", target_coin_id)];
                resources.extend(source_coin_ids.iter().map(|id| format!("coin:{}", id)));
                resources
            }
            EventPayload::CoinSplit { source_coin_id, .. } => {
                vec![format!("coin:{}", source_coin_id)]
            }
            EventPayload::CoinMergeThenTransfer { target_coin_id, source_coin_ids, recipient, .. } => {
                let mut resources = vec![format!("coin:{}", target_coin_id)];
                resources.extend(source_coin_ids.iter().map(|id| format!("coin:{}", id)));
                resources.push(format!("user:{}", recipient));
                resources
            }
            EventPayload::Governance(g) => {
                vec![format!("governance:{}", hex::encode(g.proposal_id))]
            }
            EventPayload::MoveCall(payload) => {
                let mut resources: Vec<String> = payload.input_object_ids.iter()
                    .map(|id| format!("oid:{}", hex::encode(id.as_bytes())))
                    .collect();
                resources.push(format!("contract:{}::{}::{}", payload.package, payload.module, payload.function));
                resources
            }
            EventPayload::MovePublish(_) => {
                vec![]
            }
            EventPayload::MovePtb(payload) => {
                // Resources: every Object input + every (package, module, function)
                // touched by Command::MoveCall in the PTB.
                let mut resources: Vec<String> = Vec::new();
                for arg in &payload.ptb.inputs {
                    if let crate::ptb::CallArg::Object(obj) = arg {
                        let id = match obj {
                            crate::ptb::ObjectArg::ImmOrOwnedObject(id, _, _) => id,
                            crate::ptb::ObjectArg::SharedObject { id, .. } => id,
                        };
                        resources.push(format!("oid:{}", hex::encode(id.as_bytes())));
                    }
                }
                for cmd in &payload.ptb.commands {
                    if let crate::ptb::Command::MoveCall(mc) = cmd {
                        resources.push(format!(
                            "contract:{}::{}::{}",
                            hex::encode(mc.package.as_bytes()),
                            mc.module,
                            mc.function,
                        ));
                    }
                }
                resources
            }
            EventPayload::MoveUpgrade(payload) => {
                // Resources: the cap object, the prev package, plus a
                // contract:* marker so concurrent upgrades on the same
                // family serialize through the dependency scheduler.
                vec![
                    format!("oid:{}", hex::encode(payload.upgrade_cap_id.as_bytes())),
                    format!("oid:{}", hex::encode(payload.prev_package.as_bytes())),
                    format!("contract:{}", hex::encode(payload.family_id.as_bytes())),
                ]
            }
            EventPayload::None => vec![],
            EventPayload::Genesis(g) => {
                g.accounts.iter()
                    .map(|a| format!("account:{}", a.address))
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

    /// Regression test for docs/bugs/20260424-contract-publish-event-id-tampering.md:
    /// `Event::contract_publish` overwrites `event.creator` after `Self::new`
    /// seals the id. Must call `recompute_id()` or consensus rejects the event
    /// with "Event ID verification failed - possible tampering".
    #[test]
    fn test_contract_publish_event_id_is_self_consistent() {
        let event = Event::contract_publish(
            "alice".to_string(),
            vec![vec![0xCA, 0xFE, 0xBA, 0xBE]],
            vec![],
            create_vlc_snapshot(),
            "validator-1".to_string(),
        );
        assert_eq!(event.creator, "alice", "creator should be overwritten to sender");
        assert!(
            event.verify_id(),
            "publish event id must verify after creator overwrite (recompute_id must be called)"
        );
    }
    
    #[test]
    fn test_transfer_event() {
        let transfer = Transfer::new("tx-1", "alice", "bob", 100)
            .with_type(TransferType::SetuTransfer);
        
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

    // ── Governance tests ──

    #[test]
    fn test_governance_event_type_exists() {
        let t = EventType::Governance;
        assert_eq!(t, EventType::Governance);
    }

    #[test]
    fn test_governance_requires_execution() {
        assert!(EventType::Governance.requires_solver_execution());
    }

    #[test]
    fn test_governance_not_root_event() {
        assert!(!EventType::Governance.is_root_event());
    }

    #[test]
    fn test_governance_not_validator_executed() {
        assert!(!EventType::Governance.is_validator_executed());
    }

    #[test]
    fn test_governance_event_name() {
        assert_eq!(EventType::Governance.name(), "Governance");
    }

    #[test]
    fn test_state_change_target_subnet() {
        let change = StateChange::insert("oid:abc", vec![1])
            .with_target_subnet(crate::subnet::SubnetId::ROOT);
        assert_eq!(change.target_subnet, Some(crate::subnet::SubnetId::ROOT));

        // Serde round-trip
        let json = serde_json::to_string(&change).unwrap();
        let decoded: StateChange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.target_subnet, Some(crate::subnet::SubnetId::ROOT));
    }

    #[test]
    fn test_state_change_target_subnet_default_none() {
        let change = StateChange::insert("oid:abc", vec![1]);
        assert_eq!(change.target_subnet, None);

        // Deserializing old-format JSON (no target_subnet field) should default to None
        let json = r#"{"key":"oid:abc","old_value":null,"new_value":[1]}"#;
        let decoded: StateChange = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.target_subnet, None);
    }

    #[test]
    fn test_subnet_id_governance_const() {
        let gov = crate::subnet::SubnetId::GOVERNANCE;
        assert!(gov.is_system());
        assert!(!gov.is_root());
        assert!(!gov.is_app());
    }

    #[test]
    fn test_governance_event_infers_governance_subnet() {
        let event = Event::new(
            EventType::Governance,
            vec![],
            create_vlc_snapshot(),
            "validator1".to_string(),
        );
        assert_eq!(event.get_subnet_id(), crate::subnet::SubnetId::GOVERNANCE);
    }

    // ── DF FDP M4 — MoveCallPayload.dynamic_field_accesses ──

    #[test]
    fn test_dynamic_field_access_serde_default() {
        // T1: payloads written before M4 must still deserialize; the new
        // `dynamic_field_accesses` field defaults to an empty Vec.
        let json = r#"{
            "sender": "alice",
            "package": "0x1",
            "module": "counter",
            "function": "inc",
            "type_args": [],
            "args": [],
            "input_object_ids": [],
            "shared_object_ids": []
        }"#;
        let p: MoveCallPayload = serde_json::from_str(json).expect("legacy payload");
        assert!(p.dynamic_field_accesses.is_empty());
        assert!(p.needs_tx_context, "default_true applies");
    }

    #[test]
    fn test_dynamic_field_access_roundtrip() {
        let payload = MoveCallPayload {
            sender: "alice".to_string(),
            package: "0x1".to_string(),
            module: "m".to_string(),
            function: "f".to_string(),
            type_args: vec![],
            args: vec![],
            input_object_ids: vec![],
            shared_object_ids: vec![],
            mutable_indices: None,
            consumed_indices: None,
            needs_tx_context: false,
            dynamic_field_accesses: vec![DynamicFieldAccess {
                parent_object_id: "oid:00".repeat(32),
                key_type: "u64".to_string(),
                key_bcs_hex: "2a00000000000000".to_string(),
                mode: crate::dynamic_field::DfAccessMode::Read,
                value_type: None,
            }],
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: MoveCallPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.dynamic_field_accesses.len(), 1);
        assert_eq!(
            back.dynamic_field_accesses[0].mode,
            crate::dynamic_field::DfAccessMode::Read
        );
    }

    // ===== PTB event-wire tests (B6b 收尾 / FDP move-vm-phase9-ptb-event-wire) =====

    /// U1 — `MovePtbPayload` survives BCS round-trip without field loss.
    #[test]
    fn move_ptb_payload_round_trips_bcs() {
        use crate::ptb::{CallArg, Command, ProgrammableTransaction};

        let payload = MovePtbPayload {
            sender: "0xabcd".to_string(),
            ptb: ProgrammableTransaction {
                inputs: vec![CallArg::Pure(vec![1, 2, 3])],
                commands: vec![Command::TransferObjects(
                    vec![],
                    crate::ptb::Argument::Input(0),
                )],
                dynamic_field_accesses: vec![],
            },
        };
        let bytes = bcs::to_bytes(&payload).expect("bcs encode");
        let back: MovePtbPayload = bcs::from_bytes(&bytes).expect("bcs decode");
        assert_eq!(back.sender, payload.sender);
        assert_eq!(back.ptb.inputs.len(), 1);
        assert_eq!(back.ptb.commands.len(), 1);
    }

    /// U2 — `Event::move_ptb` constructor sets `EventType::ContractCall` and
    /// stores the payload under `EventPayload::MovePtb`.
    #[test]
    fn event_move_ptb_constructor_sets_contract_call_type() {
        use crate::ptb::ProgrammableTransaction;

        let payload = MovePtbPayload {
            sender: "0xabcd".to_string(),
            ptb: ProgrammableTransaction {
                inputs: vec![],
                commands: vec![],
                dynamic_field_accesses: vec![],
            },
        };
        let event = Event::move_ptb(
            payload,
            vec![],
            VLCSnapshot::default(),
            "validator-1".to_string(),
        );
        assert_eq!(event.event_type, EventType::ContractCall);
        assert!(matches!(event.payload, EventPayload::MovePtb(_)));
    }

    /// U3 — `EventPayload::MoveUpgrade` is the **tail** variant (BCS
    /// discriminant stability). Adding mid-enum variants in the future
    /// would silently break every Move event already in storage.
    #[test]
    fn event_payload_move_ptb_is_tail_variant() {
        let move_publish = EventPayload::MovePublish(MovePublishPayload { modules: vec![] });
        let move_ptb = EventPayload::MovePtb(MovePtbPayload {
            sender: String::new(),
            ptb: crate::ptb::ProgrammableTransaction {
                inputs: vec![],
                commands: vec![],
                dynamic_field_accesses: vec![],
            },
        });
        let move_upgrade = EventPayload::MoveUpgrade(MoveUpgradePayload {
            sender: crate::object::Address::ZERO,
            family_id: crate::object::ObjectId::new([0u8; 32]),
            prev_package: crate::object::ObjectId::new([0u8; 32]),
            new_package_addr: crate::object::Address::ZERO,
            new_version: 0,
            modules: vec![],
            deps: vec![],
            digest: vec![],
            upgrade_cap_id: crate::object::ObjectId::new([0u8; 32]),
            policy: 0,
        });
        let publish_bytes = bcs::to_bytes(&move_publish).unwrap();
        let ptb_bytes = bcs::to_bytes(&move_ptb).unwrap();
        let upgrade_bytes = bcs::to_bytes(&move_upgrade).unwrap();
        // BCS encodes enum discriminants as ULEB128. Variant counts stay
        // < 128, so the discriminant byte is the first byte.
        assert!(ptb_bytes[0] > publish_bytes[0], "MovePtb must be after MovePublish");
        assert!(
            upgrade_bytes[0] > ptb_bytes[0],
            "MoveUpgrade must be the tail variant (upgrade={} ptb={})",
            upgrade_bytes[0],
            ptb_bytes[0]
        );
    }
}
