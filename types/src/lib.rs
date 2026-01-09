// ========== Core Modules ==========
pub mod event;
pub mod consensus;
pub mod node;
pub mod object;
pub mod subnet;          // Subnet (sub-application) types
pub mod merkle;          // Merkle tree types for state commitment

// ========== Object Model ==========
pub mod coin;           // Coin object (transferable asset)
pub mod profile;        // Profile & Credential (identity)
pub mod relation;       // RelationGraph object (social)
pub mod account_view;   // Account aggregated view

// Export commonly used types
pub use event::{Event, EventId, EventStatus, EventType, Transfer, EventPayload};
pub use consensus::{Anchor, AnchorId, ConsensusFrame, CFId, CFStatus, Vote, ConsensusConfig};
pub use node::*;

// Re-export VLC types from setu-vlc
pub use setu_vlc::{VectorClock, VLCSnapshot};

// ========== Object Model Exports ==========
pub use object::{Object, ObjectId, Address, ObjectDigest, ObjectType, ObjectMetadata, Ownership, generate_object_id};

// Coin related
pub use coin::{Coin, CoinType, CoinData, Balance, create_coin, create_typed_coin};

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
};

// Subnet related
pub use subnet::{SubnetId, SubnetType, SubnetConfig, UserSubnetMembership, CrossSubnetContext};

// Merkle tree types
pub use merkle::{
    HashValue, ObjectStateValue, SubnetStateRoot, AnchorMerkleRoots,
    MerkleExecutionResult, CrossSubnetLock, CrossSubnetLockStatus,
    object_type, ZERO_HASH,
};

// Aggregated views
pub use account_view::AccountView;

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
