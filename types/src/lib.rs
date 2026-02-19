// ========== Core Modules ==========
pub mod consensus;
pub mod event;
pub mod merkle; // Merkle tree types for state commitment
pub mod node;
pub mod object;
pub mod registration; // Registration types
pub mod subnet; // Subnet (sub-application) types
pub mod task;
pub mod transfer; // Transfer and routing types // Task types for Validator → Solver communication

// ========== Object Model ==========
pub mod account_view;
pub mod coin; // Coin object (transferable asset)
pub mod profile; // Profile & Credential (identity)
pub mod relation; // RelationGraph object (social) // Account aggregated view

// Re-export VLC types from setu-vlc (single source of truth)
pub use setu_vlc::{VLCSnapshot, VectorClock};

// Export from transfer module
pub use transfer::{AssignedVlc, ClockKey, ResourceKey, Transfer, TransferId, TransferType};

// Export from registration module
pub use registration::{
    NodeType, PowerConsumption, SolverRegistration, SubnetRegistration, SubnetResourceLimits,
    TaskSubmission, Unregistration, UserRegistration, ValidatorRegistration,
};

// Export from event module
pub use event::{
    Event, EventId, EventPayload, EventStatus, EventType, ExecutionResult, StateChange,
};

// Export from consensus module
pub use consensus::{Anchor, AnchorId, CFId, CFStatus, ConsensusConfig, ConsensusFrame, Vote};
pub use node::*;

// ========== Object Model Exports ==========
pub use object::{
    generate_object_id, Address, Object, ObjectDigest, ObjectId, ObjectMetadata, ObjectType,
    Ownership,
};

// Coin related
pub use coin::{create_coin, create_typed_coin, Balance, Coin, CoinData, CoinType};

// Profile & Credential related
pub use profile::{
    create_achievement_credential, create_kyc_credential, create_membership_credential,
    create_profile, Credential, CredentialData, CredentialStatus, Profile, ProfileData,
};

// RelationGraph related
pub use relation::{
    create_professional_graph,
    create_social_graph,
    create_user_relation_network,
    // User relation network
    relation_type,
    Relation,
    RelationGraph,
    RelationGraphData,
    SubnetInteractionSummary,
    UserRelationNetwork,
    UserRelationNetworkObject,
};

// Subnet related
pub use subnet::{
    CrossSubnetContext,
    // Subnet interaction tracking
    InteractionType,
    LocalRelation,
    SubnetConfig,
    SubnetId,
    SubnetInteraction,
    SubnetType,
    UserSubnetActivity,
    UserSubnetMembership,
};

// Merkle tree types
pub use merkle::{
    object_type, AnchorMerkleRoots, CrossSubnetLock, CrossSubnetLockStatus, HashValue,
    MerkleExecutionResult, ObjectStateValue, SubnetStateRoot, ZERO_HASH,
};

// Aggregated views
pub use account_view::AccountView;

// Task types for Validator → Solver communication
pub use task::{
    Attestation, AttestationData, AttestationError, AttestationResult, AttestationType, GasBudget,
    GasUsage, MerkleProof, OperationType, ReadSetEntry, ResolvedInputs, ResolvedObject, SolverTask,
    VerifiedAttestation,
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
