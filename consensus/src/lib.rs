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

pub mod dag;
pub mod engine;
pub mod folder;
pub mod liveness;
pub mod merkle_integration;
pub mod root_executor;
pub mod router;
pub mod validator_set;
pub mod vlc;

// Re-export main types
pub use dag::{Dag, DagError};
pub use engine::{ConsensusEngine, ConsensusMessage, DagStats};
pub use folder::{ConsensusManager, DagFolder};
pub use merkle_integration::{
    compute_events_root, compute_anchor_chain_root, compute_global_state_root,
    AnchorMerkleRootsBuilder,
};
pub use root_executor::{RootSubnetExecutor, RootExecutorError, RootExecutionResult};
pub use router::{EventRouter, RoutedEvents, SubnetExecutionBatch, create_execution_batches};
pub use validator_set::{ElectionStrategy, ValidatorSet};
pub use vlc::VLC;

// Re-export liveness types
pub use liveness::{
    choose_index, choose_leader, create_default_election,
    create_election_with_contiguous_rounds, create_reputation_election,
    ConsensusFrameAggregation, ConsensusFrameMetadata, InMemoryMetadataBackend,
    LeaderReputation, MetadataBackend, ProposerAndVoterHeuristic, ProposerElection,
    ReputationConfig, ReputationHeuristic, RotatingProposer, Round, ValidatorId,
    VotingPower, VotingPowerRatio,
};

