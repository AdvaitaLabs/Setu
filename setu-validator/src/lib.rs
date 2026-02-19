//! Setu Validator - Verification and coordination node
//!
//! The validator is responsible for:
//! - Receiving transfers from relay and routing to solvers
//! - Receiving events from solvers
//! - Verifying event validity
//! - Maintaining the global Foldgraph
//! - Coordinating consensus
//! - Providing registration service for solvers and validators
//!
//! ## Primary Entry Point
//!
//! Use [`ConsensusValidator`] which integrates with the consensus engine
//! for proper DAG management, VLC tracking, and CF voting.
//!
//! ## solver-tee3 Architecture
//!
//! In the new architecture, Validator is responsible for:
//! - Preparing SolverTask with coin selection and Merkle proofs
//! - Sending SolverTask to Solver (pass-through to TEE)
//! - Verifying Attestation from TEE execution results
//!
//! ## Module Structure
//!
//! - `network/` - Network service (modular)
//!   - `types` - Request/Response types and helpers
//!   - `handlers` - HTTP route handlers  
//!   - `service` - Core service logic
//!   - `registration` - Registration handler (RegistrationHandler trait impl)
//! - `network_adapter/` - Bridge between network and consensus layers
//!   - `router` - Message routing to consensus engine
//!   - `sync_protocol` - Event/CF synchronization protocol
//! - `consensus_integration` - ConsensusValidator wrapping ConsensusEngine

pub mod broadcaster;
pub mod consensus_integration;
mod network;
pub mod network_adapter;
pub mod persistence;
pub mod protocol;
mod router_manager;
pub mod task_preparer;
mod user_handler;

pub use network::{
    current_timestamp_millis, current_timestamp_secs, GetBalanceResponse, GetObjectResponse,
    NetworkServiceConfig, SubmitEventRequest, SubmitEventResponse, TransferTracker, ValidatorInfo,
    ValidatorNetworkService, ValidatorRegistrationHandler,
};
pub use router_manager::{RouterError, RouterManager, SolverConnection};
pub use task_preparer::{TaskPrepareError, TaskPreparer};
pub use user_handler::ValidatorUserHandler;

// Re-export consensus integration types
pub use consensus_integration::{
    ConsensusMessageHandler, ConsensusValidator, ConsensusValidatorConfig, ConsensusValidatorStats,
};

// Re-export broadcaster types
pub use broadcaster::{
    AnemoConsensusBroadcaster, BroadcastError, BroadcastResult, ConsensusBroadcaster,
    MockBroadcaster, NoOpBroadcaster,
};

// Re-export network adapter types
pub use network_adapter::{
    InMemorySyncStore, MessageRouter, NetworkEventHandler, SyncProtocol, SyncStore,
};

// Re-export protocol types (consensus-specific message definitions)
pub use protocol::{
    MessageCodec, MessageCodecError, MessageType, NetworkEvent, SerializedConsensusFrame,
    SerializedEvent, SerializedVote, SetuMessage, SyncConsensusFramesRequest,
    SyncConsensusFramesResponse, SyncEventsRequest, SyncEventsResponse,
};

// Re-export consensus types from the consensus crate
pub use consensus::{
    AnchorBuilder, ConsensusEngine, ConsensusManager, ConsensusMessage, Dag as ConsensusDag,
    DagError as ConsensusDagError, DagStats as ConsensusDagStats, TeeAttestation, TeeVerifier,
    ValidatorSet, VerificationResult, VLC,
};

// Re-export StateProvider types from storage (canonical location)
pub use setu_storage::{CoinInfo, MerkleStateProvider, SimpleMerkleProof, StateProvider};

/// Event verification error
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Event has no execution result")]
    NoExecutionResult,

    #[error("Event execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Invalid event creator: {0}")]
    InvalidCreator(String),

    #[error("Event timestamp is in the future")]
    FutureTimestamp,

    #[error("Missing parent event: {0}")]
    MissingParent(String),

    #[error("Invalid VLC snapshot")]
    InvalidVLC,
}
