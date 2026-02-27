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

mod router_manager;
mod network;
pub mod task_preparer;
mod user_handler;
pub mod infra_executor;
pub mod consensus_integration;
pub mod broadcaster;
pub mod network_adapter;
pub mod persistence;
pub mod protocol;
pub mod coin_reservation;

pub use router_manager::{RouterManager, RouterError, SolverConnection};
pub use network::{
    ValidatorNetworkService, ValidatorRegistrationHandler, NetworkServiceConfig,
    ValidatorInfo, TransferTracker, SubmitEventRequest, SubmitEventResponse,
    GetBalanceResponse, GetObjectResponse, current_timestamp_secs, current_timestamp_millis,
};
pub use task_preparer::{
    TaskPreparer, TaskPrepareError,
    BatchTaskPreparer, BatchPrepareResult, BatchPrepareStats,
};
pub use user_handler::ValidatorUserHandler;
pub use infra_executor::InfraExecutor;
pub use coin_reservation::{CoinReservationManager, ReservationHandle};

// Re-export consensus integration types
pub use consensus_integration::{
    ConsensusValidator, ConsensusValidatorConfig, ConsensusValidatorStats,
    ConsensusMessageHandler,
};

// Re-export broadcaster types
pub use broadcaster::{
    AnemoConsensusBroadcaster, ConsensusBroadcaster, BroadcastError, BroadcastResult,
    NoOpBroadcaster, MockBroadcaster,
};

// Re-export network adapter types
pub use network_adapter::{
    MessageRouter, NetworkEventHandler, SyncProtocol, SyncStore, InMemorySyncStore,
};

// Re-export protocol types (consensus-specific message definitions)
pub use protocol::{
    SetuMessage, MessageType, NetworkEvent, MessageCodec, MessageCodecError,
    SerializedEvent, SerializedConsensusFrame, SerializedVote,
    SyncEventsRequest, SyncEventsResponse,
    SyncConsensusFramesRequest, SyncConsensusFramesResponse,
};

// Re-export consensus types from the consensus crate
pub use consensus::{
    ConsensusEngine, ConsensusMessage, ConsensusManager,
    Dag as ConsensusDag, DagError as ConsensusDagError, DagStats as ConsensusDagStats,
    ValidatorSet, VLC, AnchorBuilder,
    TeeVerifier, TeeAttestation, VerificationResult,
};

// Re-export StateProvider types from storage (canonical location)
pub use setu_storage::{StateProvider, CoinInfo, SimpleMerkleProof, MerkleStateProvider};

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
