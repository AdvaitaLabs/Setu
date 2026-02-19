// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Setu Consensus Module
//!
//! This module implements the consensus mechanism for the Setu network.
//! It includes:
//! - DAG-based consensus with ConsensusFrames (CF)
//! - VLC-based leader rotation
//! - Leader election strategies (rotating, reputation-based)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     ConsensusEngine                          │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
//! │  │     DAG      │  │     VLC      │  │ ValidatorSet │      │
//! │  └──────────────┘  └──────────────┘  └──────┬───────┘      │
//! │                                              │               │
//! │                                    ┌─────────▼─────────┐    │
//! │                                    │  ProposerElection │    │
//! │                                    │  (RotatingProposer│    │
//! │                                    │   or Reputation)  │    │
//! │                                    └───────────────────┘    │
//! │  ┌──────────────────────────────────────────────────┐      │
//! │  │              ConsensusManager (Folder)            │      │
//! │  │  - Creates CFs when VLC delta reaches threshold   │      │
//! │  │  - Manages voting and finalization               │      │
//! │  └──────────────────────────────────────────────────┘      │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod anchor_builder;
pub mod broadcaster;
pub mod dag;
pub mod dag_manager;
pub mod engine;
pub mod folder;
pub mod liveness;
pub mod merkle_integration;
pub mod recent_cache;
pub mod root_executor;
pub mod router;
pub mod tee_verifier;
pub mod validator_set;
pub mod vlc;

// Re-export main types
pub use anchor_builder::{
    AnchorBuildError, AnchorBuildResult, AnchorBuilder, BuilderStateSnapshot, PendingAnchorBuild,
    StateChangeEntry,
};
pub use broadcaster::{
    BroadcastError, BroadcastResult, ConsensusBroadcaster, MockBroadcaster, NoOpBroadcaster,
    OptionalBroadcaster,
};
pub use dag::{Dag, DagError, GCStats};
pub use dag_manager::{
    DagManager, DagManagerConfig, DagManagerError, DagStatsSnapshot, GcStats, ParentInfo,
    ResolvedParents, WarmupStats,
};
pub use engine::{ConsensusEngine, ConsensusMessage, DagStats};
pub use folder::{ConsensusManager, DagFolder};
pub use merkle_integration::{
    compute_anchor_chain_root, compute_events_root, compute_global_state_root,
    AnchorMerkleRootsBuilder,
};
pub use recent_cache::{CacheStats, CacheStatsSnapshot, FinalizedEventMeta, RecentEventCache};
pub use root_executor::{RootExecutionResult, RootExecutorError, RootSubnetExecutor};
pub use router::{create_execution_batches, EventRouter, RoutedEvents, SubnetExecutionBatch};
pub use tee_verifier::{
    SolverInfo, SolverRegistry, TeeAttestation, TeeVerifier, VerificationResult,
};
pub use validator_set::{ElectionStrategy, ValidatorSet};
pub use vlc::VLC;

// Re-export liveness types
pub use liveness::{
    choose_index, choose_leader, create_default_election, create_election_with_contiguous_rounds,
    create_reputation_election, ConsensusFrameAggregation, ConsensusFrameMetadata,
    InMemoryMetadataBackend, LeaderReputation, MetadataBackend, ProposerAndVoterHeuristic,
    ProposerElection, ReputationConfig, ReputationHeuristic, RotatingProposer, Round, ValidatorId,
    VotingPower, VotingPowerRatio,
};
