// ========== Core Modules ==========
pub mod event;
pub mod transfer;       // Transfer and routing types
pub mod registration;   // Registration types
pub mod consensus;
pub mod node;
pub mod object;
pub mod subnet;          // Subnet (sub-application) types
pub mod merkle;          // Merkle tree types for state commitment
pub mod task;            // Task types for Validator → Solver communication

// ========== Object Model ==========
pub mod coin;           // Coin object (transferable asset)
pub mod profile;        // Profile & Credential (identity)
pub mod relation;       // RelationGraph object (social)
pub mod account_view;   // Account aggregated view
pub mod genesis;        // Genesis configuration

// Re-export VLC types from setu-vlc (single source of truth)
pub use setu_vlc::{VectorClock, VLCSnapshot};

// Export from transfer module
pub use transfer::{
    Transfer, TransferId, ClockKey, ResourceKey, TransferType, AssignedVlc,
};

// Export from registration module
pub use registration::{
    ValidatorRegistration, SolverRegistration, Unregistration, NodeType,
    SubnetRegistration, SubnetResourceLimits,
    UserRegistration, PowerConsumption, TaskSubmission,
};

// Export from event module
pub use event::{
    Event, EventId, EventStatus, EventType, EventPayload,
    ExecutionResult, StateChange,
};

// Export from consensus module
pub use consensus::{Anchor, AnchorId, ConsensusFrame, CFId, CFStatus, Vote, ConsensusConfig};
pub use node::*;

// ========== Object Model Exports ==========
pub use object::{Object, ObjectId, Address, ObjectDigest, ObjectType, ObjectMetadata, Ownership, generate_object_id};

// Coin related
pub use coin::{Coin, CoinType, CoinData, CoinState, Balance, create_coin, create_typed_coin, deterministic_coin_id, deterministic_coin_id_from_str};

// Profile & Credential related
pub use profile::{
    Profile, ProfileData,
    Credential, CredentialData, CredentialStatus,
    create_profile, create_kyc_credential, create_membership_credential, create_achievement_credential,
};

// RelationGraph related
pub use relation::{
    RelationGraph, RelationGraphData, Relation,
    create_social_graph, create_professional_graph,
    // User relation network
    relation_type, UserRelationNetwork, UserRelationNetworkObject,
    SubnetInteractionSummary, create_user_relation_network,
};

// Subnet related
pub use subnet::{
    SubnetId, SubnetType, SubnetConfig, UserSubnetMembership, CrossSubnetContext,
    // Subnet interaction tracking
    InteractionType, SubnetInteraction, LocalRelation, UserSubnetActivity,
};

// Merkle tree types
pub use merkle::{
    HashValue, ObjectStateValue, SubnetStateRoot, AnchorMerkleRoots,
    MerkleExecutionResult, CrossSubnetLock, CrossSubnetLockStatus,
    object_type, ZERO_HASH,
};

// Aggregated views
pub use account_view::AccountView;

// Genesis config
pub use genesis::{GenesisConfig, GenesisAccount, GenesisError};

// Task types for Validator → Solver communication
pub use task::{
    SolverTask, ResolvedInputs, OperationType, ResolvedObject,
    ReadSetEntry, MerkleProof,
    Attestation, AttestationType, AttestationData,
    AttestationError, AttestationResult, VerifiedAttestation,
    GasBudget, GasUsage,
};

// Error types
pub type SetuResult<T> = Result<T, SetuError>;

#[derive(Debug, thiserror::Error)]
pub enum SetuError {
    #[error("Storage error: {0}")]
    StorageError(String),
    
    #[error("Not found: {0}")]
    NotFound(String),
    
    #[error("Invalid data: {0}")]
    InvalidData(String),
    
    #[error("Invalid transfer: {0}")]
    InvalidTransfer(String),
    
    #[error("Other error: {0}")]
    Other(String),
}
