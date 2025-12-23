// ========== Core Modules ==========
pub mod event;
pub mod consensus;
pub mod node;
pub mod object;

// ========== New Object Model ==========
pub mod coin;        // New: Coin object
pub mod sbt;         // Refactored: SBT object
pub mod relation;    // Refactored: RelationGraph object
pub mod sbt_view;    // New: SBT aggregated view

// ========== Deprecated (Backward Compatibility) ==========
// TODO: If Account backward compatibility is needed, implement a simplified account module
// #[deprecated(note = "Account concept removed. Use SBT as identity instead.")]
// pub mod account;

// Export commonly used types
pub use event::{Event, EventId, EventStatus, EventType, Transfer};
pub use consensus::{Anchor, AnchorId, ConsensusFrame, CFId, CFStatus, Vote, ConsensusConfig};
pub use node::*;

// Re-export VLC types from setu-vlc
pub use setu_vlc::{VectorClock, VLCSnapshot};

// ========== New Object Model Exports ==========
pub use object::{Object, ObjectId, Address, ObjectType, ObjectMetadata, Ownership};

// Coin related
pub use coin::{Coin, Balance, create_coin};

// SBT related
pub use sbt::{SBT, SBTData, Credential, create_sbt, create_personal_sbt, create_organization_sbt};

// RelationGraph related
pub use relation::{
    RelationGraph, RelationGraphData, Relation,
    create_social_graph, create_professional_graph,
};

// Aggregated views
pub use sbt_view::SBTView;

// ========== Deprecated Types (Backward Compatibility) - Temporarily Commented ==========
// TODO: If backward compatibility is needed, implement a simplified account module
// #[deprecated(note = "Use SBT instead of Account")]
// pub use account::{AccountData, create_account};
//
// #[deprecated(note = "Use SBT directly")]
// pub type AccountObject = account::Account;
//
// #[deprecated(note = "Use SBT directly")]
// pub type SBTObject = SBT;
//
// #[deprecated(note = "Use RelationGraph directly")]
// pub type RelationGraphObject = RelationGraph;

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
// pub use account::*;
// pub use sbt::*;
// pub use relation::*;
