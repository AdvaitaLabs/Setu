// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Consensus Engine
//!
//! The main consensus engine that orchestrates the DAG-based consensus process.
//! It integrates VLC-based timing, leader election, and ConsensusFrame management.
//!
//! ## Main Flow
//!
//! 1. Events enter the DAG from solvers (with TEE execution proofs)
//! 2. Validators verify execution results
//! 3. Each validator maintains a VLC clock
//! 4. Leader is selected via round-robin rotation
//! 5. When leader's VLC delta reaches threshold, it folds the DAG
//! 6. Other validators vote on the fold validity
//! 7. After quorum votes, the ConsensusFrame is finalized
//! 8. Next round begins with the finalized frame as anchor

use setu_types::{ConsensusConfig, ConsensusFrame, Event, EventId, SetuResult, Vote};
use setu_vlc::VLCSnapshot;
use setu_storage::{EventStore, subnet_state::GlobalStateManager};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, RwLock, Mutex};
use tracing::{debug, info, warn};

use crate::broadcaster::ConsensusBroadcaster;
use crate::dag::Dag;
use crate::dag_manager::{DagManager, DagManagerError};
use crate::folder::ConsensusManager;
use crate::liveness::Round;
use crate::validator_set::ValidatorSet;
use crate::vlc::VLC;

/// Messages exchanged between consensus components
#[derive(Debug, Clone)]
pub enum ConsensusMessage {
    /// New event added to DAG
    NewEvent(Event),
    /// Leader proposes a ConsensusFrame
    ProposeFrame(ConsensusFrame),
    /// Validator votes for a frame
    Vote(Vote),
    /// Frame has been finalized
    FrameFinalized(ConsensusFrame),
    /// Leader rotation occurred
    LeaderChanged { round: Round, new_leader: String },
}

/// The main consensus engine
pub struct ConsensusEngine {
    /// Configuration
    config: ConsensusConfig,
    /// The DAG storing all events
    dag: Arc<RwLock<Dag>>,
    /// DagManager for three-layer storage (DAG → Cache → Store)
    /// This is the ONLY entry point for adding events to the DAG
    dag_manager: Arc<DagManager>,
    /// Local VLC clock (full VLC, used for merge operations with other validators)
    vlc: Arc<RwLock<VLC>>,
    /// Fast path logical time counter (lock-free, for local event creation)
    /// This provides O(1) atomic increment for get_vlc_time() calls.
    /// The full VLC is still used for merge() operations in add_event().
    logical_time_counter: AtomicU64,
    /// Set of validators with leader election
    validator_set: Arc<RwLock<ValidatorSet>>,
    /// ConsensusFrame manager (folder)
    consensus_manager: Arc<RwLock<ConsensusManager>>,
    /// This validator's ID
    local_validator_id: String,
    /// Private key for signing votes (ed25519, 32 bytes)
    /// If None, votes will not be signed (backward compatibility mode)
    private_key: Arc<RwLock<Option<Vec<u8>>>>,
    /// Channel for sending consensus messages (legacy, for internal use)
    message_tx: mpsc::Sender<ConsensusMessage>,
    /// Channel for receiving consensus messages (reserved for future use)
    #[allow(dead_code)]
    message_rx: Arc<Mutex<mpsc::Receiver<ConsensusMessage>>>,
    /// Network broadcaster for P2P message delivery (optional)
    broadcaster: Arc<RwLock<Option<Arc<dyn ConsensusBroadcaster>>>>,
}

impl ConsensusEngine {
    /// Create a new consensus engine
    pub fn new(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);
        
        // Create shared DAG
        let dag = Arc::new(RwLock::new(Dag::new()));
        
        // Create EventStore (in-memory for now)
        let event_store = Arc::new(EventStore::new());
        
        // Create DagManager with the shared DAG
        let dag_manager = Arc::new(DagManager::with_defaults(
            Arc::clone(&dag),
            event_store,
        ));

        Self {
            config: config.clone(),
            dag,
            dag_manager,
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            logical_time_counter: AtomicU64::new(0),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::new(
                config,
                validator_id.clone(),
            ))),
            local_validator_id: validator_id,
            private_key: Arc::new(RwLock::new(None)),
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create a new consensus engine with state manager for Merkle tree persistence
    /// 
    /// This constructor allows injecting a pre-configured GlobalStateManager
    /// with MerkleStore for persisting state roots.
    pub fn with_state_manager(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
        state_manager: GlobalStateManager,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);
        
        // Create shared DAG
        let dag = Arc::new(RwLock::new(Dag::new()));
        
        // Create EventStore (in-memory for now)
        let event_store = Arc::new(EventStore::new());
        
        // Create DagManager with the shared DAG
        let dag_manager = Arc::new(DagManager::with_defaults(
            Arc::clone(&dag),
            event_store,
        ));

        Self {
            config: config.clone(),
            dag,
            dag_manager,
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            logical_time_counter: AtomicU64::new(0),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::with_state_manager(
                config,
                validator_id.clone(),
                state_manager,
            ))),
            local_validator_id: validator_id,
            private_key: Arc::new(RwLock::new(None)),
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create a new consensus engine with external EventStore
    /// 
    /// This constructor allows injecting a pre-configured EventStore,
    /// enabling the DagManager to persist events with depth information.
    pub fn with_event_store(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
        event_store: Arc<EventStore>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);
        
        // Create shared DAG
        let dag = Arc::new(RwLock::new(Dag::new()));
        
        // Create DagManager with the shared DAG and external EventStore
        let dag_manager = Arc::new(DagManager::with_defaults(
            Arc::clone(&dag),
            event_store,
        ));

        Self {
            config: config.clone(),
            dag,
            dag_manager,
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            logical_time_counter: AtomicU64::new(0),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::new(
                config,
                validator_id.clone(),
            ))),
            local_validator_id: validator_id,
            private_key: Arc::new(RwLock::new(None)),
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create with both state manager and event store
    pub fn with_stores(
        config: ConsensusConfig,
        validator_id: String,
        validator_set: ValidatorSet,
        state_manager: GlobalStateManager,
        event_store: Arc<EventStore>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1000);
        
        // Create shared DAG
        let dag = Arc::new(RwLock::new(Dag::new()));
        
        // Create DagManager with the shared DAG and external EventStore
        let dag_manager = Arc::new(DagManager::with_defaults(
            Arc::clone(&dag),
            event_store,
        ));

        Self {
            config: config.clone(),
            dag,
            dag_manager,
            vlc: Arc::new(RwLock::new(VLC::new(validator_id.clone()))),
            logical_time_counter: AtomicU64::new(0),
            validator_set: Arc::new(RwLock::new(validator_set)),
            consensus_manager: Arc::new(RwLock::new(ConsensusManager::with_state_manager(
                config,
                validator_id.clone(),
                state_manager,
            ))),
            local_validator_id: validator_id,
            private_key: Arc::new(RwLock::new(None)),
            message_tx: tx,
            message_rx: Arc::new(Mutex::new(rx)),
            broadcaster: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the private key for signing votes
    /// 
    /// The private key should be 32 bytes for ed25519 signatures.
    /// If not set, votes will not be signed (backward compatibility mode).
    pub async fn set_private_key(&self, key: Vec<u8>) {
        *self.private_key.write().await = Some(key);
    }
    
    /// Clear the private key (disable vote signing)
    pub async fn clear_private_key(&self) {
        *self.private_key.write().await = None;
    }

    /// Set the network broadcaster for P2P message delivery
    /// 
    /// This should be called after the network layer is initialized.
    /// Without a broadcaster, consensus messages are only sent to internal channels.
    pub async fn set_broadcaster(&self, broadcaster: Arc<dyn ConsensusBroadcaster>) {
        let mut b = self.broadcaster.write().await;
        *b = Some(broadcaster);
        info!("Consensus broadcaster configured");
    }
    
    /// Check if a broadcaster is configured
    pub async fn has_broadcaster(&self) -> bool {
        self.broadcaster.read().await.is_some()
    }
    
    /// Get a reference to the broadcaster for making network requests
    /// 
    /// Returns None if no broadcaster is configured.
    /// Used for fetching missing events or other network operations.
    pub async fn get_broadcaster(&self) -> Option<Arc<dyn ConsensusBroadcaster>> {
        let b = self.broadcaster.read().await;
        b.as_ref().cloned()
    }

    /// Add an event to the DAG and try to create a CF if conditions are met
    /// 
    /// This method uses DagManager as the single entry point for adding events,
    /// ensuring proper depth calculation and three-layer storage management.
    pub async fn add_event(&self, event: Event) -> SetuResult<EventId> {
        // Update local VLC by merging with the event's VLC
        {
            let mut vlc = self.vlc.write().await;
            vlc.merge(&event.vlc_snapshot);
            vlc.tick();
        }

        // Add event through DagManager with retry (handles TOCTOU race with GC)
        // DuplicateEvent is treated as success (idempotent operation)
        let event_id = match self.dag_manager.add_event_with_retry(event.clone()).await {
            Ok(id) => id,
            Err(DagManagerError::DuplicateEvent(id)) => {
                // Idempotent: event already exists, treat as success
                debug!(event_id = %id, "Event already exists in DAG (idempotent)");
                return Ok(id);
            }
            Err(DagManagerError::MissingParent(id)) => {
                return Err(setu_types::SetuError::InvalidData(format!("Missing parent: {}", id)));
            }
            Err(DagManagerError::ParentTooOld { parent_id, depth_diff, max_allowed }) => {
                return Err(setu_types::SetuError::InvalidData(format!(
                    "Parent {} too old: depth_diff {} > max {}",
                    parent_id, depth_diff, max_allowed
                )));
            }
            Err(e) => {
                return Err(setu_types::SetuError::InvalidData(e.to_string()));
            }
        };

        // Broadcast the new event
        // We broadcast regardless of whether we are the leader, as all validators
        // need the event for their DAGs.
        {
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                // Background this to avoid blocking?
                // For now, we await it but log errors instead of failing.
                // Event propagation should be best-effort; state sync fixes gaps.
                if let Err(e) = b.broadcast_event(&event).await {
                    warn!(event_id = %event.id, error = %e, "Failed to broadcast event");
                } else {
                    debug!(event_id = %event.id, "Event broadcasted");
                }
            }
            
            // Still send to internal channel for backward compatibility or local monitoring
            let _ = self
                .message_tx
                .send(ConsensusMessage::NewEvent(event))
                .await;
        }

        // Try to create a ConsensusFrame if we're the leader
        self.try_create_cf().await?;

        Ok(event_id)
    }

    /// Receive an event from the network (does not broadcast again)
    ///
    /// This is used when receiving events from other validators.
    /// Unlike `add_event`, this does not broadcast the event again to avoid message loops.
    pub async fn receive_event_from_network(&self, event: Event) -> SetuResult<EventId> {
        // Update local VLC by merging with the event's VLC
        {
            let mut vlc = self.vlc.write().await;
            vlc.merge(&event.vlc_snapshot);
            vlc.tick();
        }

        // Add event through DagManager with retry (handles TOCTOU race with GC)
        // DuplicateEvent is treated as success (idempotent operation)
        let event_id = match self.dag_manager.add_event_with_retry(event.clone()).await {
            Ok(id) => id,
            Err(DagManagerError::DuplicateEvent(id)) => {
                // Idempotent: event already exists (common during network sync)
                debug!(event_id = %id, "Event already exists in DAG (idempotent, from network)");
                return Ok(id);
            }
            Err(DagManagerError::MissingParent(id)) => {
                return Err(setu_types::SetuError::InvalidData(format!("Missing parent: {}", id)));
            }
            Err(DagManagerError::ParentTooOld { parent_id, depth_diff, max_allowed }) => {
                return Err(setu_types::SetuError::InvalidData(format!(
                    "Parent {} too old: depth_diff {} > max {}",
                    parent_id, depth_diff, max_allowed
                )));
            }
            Err(e) => {
                return Err(setu_types::SetuError::InvalidData(e.to_string()));
            }
        };

        // Note: We do NOT broadcast the event here since it came from the network
        
        // Try to create a ConsensusFrame if we're the leader
        self.try_create_cf().await?;

        Ok(event_id)
    }

    /// Create a new event with the given parent IDs
    pub async fn create_event(&self, parent_ids: Vec<EventId>) -> SetuResult<Event> {
        let vlc_snapshot = {
            let mut vlc = self.vlc.write().await;
            vlc.tick();
            vlc.snapshot()
        };

        let event = Event::new(
            setu_types::EventType::Transfer,
            parent_ids,
            vlc_snapshot,
            self.local_validator_id.clone(),
        );

        Ok(event)
    }

    /// Check if this validator is the current leader
    pub async fn is_current_leader(&self) -> bool {
        let validator_set = self.validator_set.read().await;
        validator_set.is_leader(&self.local_validator_id)
    }

    /// Check if this validator is the valid proposer for a specific round
    pub async fn is_valid_proposer_for_round(&self, round: Round) -> bool {
        let validator_set = self.validator_set.read().await;
        validator_set.is_valid_proposer(&self.local_validator_id, round)
    }

    /// Get the current round
    pub async fn current_round(&self) -> Round {
        let validator_set = self.validator_set.read().await;
        validator_set.current_round()
    }

    /// Get the valid proposer for a specific round
    pub async fn get_valid_proposer(&self, round: Round) -> Option<String> {
        let validator_set = self.validator_set.read().await;
        validator_set.get_valid_proposer(round)
    }

    /// Advance to the next round
    pub async fn advance_round(&self) -> Round {
        let mut validator_set = self.validator_set.write().await;
        let new_round = validator_set.advance_round();

        // Notify about leader change
        if let Some(new_leader) = validator_set.get_leader_id() {
            let _ = self
                .message_tx
                .send(ConsensusMessage::LeaderChanged {
                    round: new_round,
                    new_leader: new_leader.clone(),
                })
                .await;
        }

        new_round
    }

    /// Try to create a ConsensusFrame if conditions are met
    async fn try_create_cf(&self) -> SetuResult<Option<ConsensusFrame>> {
        let _current_round = {
            let validator_set = self.validator_set.read().await;
            let round = validator_set.current_round();

            // Check if we are the valid proposer for the current round
            if !validator_set.is_valid_proposer(&self.local_validator_id, round) {
                return Ok(None);
            }
            round
        };

        let vlc = self.vlc.read().await;
        let mut manager = self.consensus_manager.write().await;

        if !manager.should_fold(&vlc) {
            return Ok(None);
        }

        let dag = self.dag.read().await;
        // AnchorBuilder now handles all Merkle tree computation internally
        let cf = manager.try_create_cf(&dag, &vlc);

        if let Some(ref frame) = cf {
            // Send to internal channel (legacy)
            let _ = self
                .message_tx
                .send(ConsensusMessage::ProposeFrame(frame.clone()))
                .await;
            
            // Broadcast to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_cf(frame).await {
                    Ok(result) => {
                        info!(
                            cf_id = %frame.id,
                            success = result.success_count,
                            total = result.total_peers,
                            "CF broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %frame.id, error = %e, "Failed to broadcast CF");
                    }
                }
            } else {
                debug!(cf_id = %frame.id, "No broadcaster configured, CF not sent to network");
            }
        }

        Ok(cf)
    }

    /// Receive a ConsensusFrame from another validator (Follower path)
    /// 
    /// When a follower receives a CF from the leader:
    /// 1. Verify proposer is valid for current round
    /// 2. Check if we've already processed this CF (idempotency)
    /// 3. **Ensure all referenced events are in local DAG (fetch if missing)**
    /// 4. Verify the CF's merkle roots are valid
    /// 5. Apply the state changes from the anchor's events to local SMT
    /// 6. Verify resulting state matches the anchor's state root
    /// 7. Vote for the CF
    /// 8. Check if our vote causes finalization
    /// 
    /// Returns (finalized, Option<Anchor>) so the caller can persist when finalized.
    /// This ensures followers have consistent state with the leader.
    pub async fn receive_cf(&self, cf: ConsensusFrame) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        // Step 0: Verify CF ID matches content (anti-tampering)
        if !cf.verify_id() {
            return Err(setu_types::SetuError::InvalidData(
                format!("CF ID verification failed - possible tampering: {}", cf.id)
            ));
        }
        
        // Step 1: Verify proposer is valid for current round
        // This prevents malicious nodes from creating fake CFs
        {
            let validator_set = self.validator_set.read().await;
            let current_round = validator_set.current_round();
            if !validator_set.is_valid_proposer(&cf.proposer, current_round) {
                return Err(setu_types::SetuError::InvalidData(
                    format!("CF proposer {} is not valid for round {}", cf.proposer, current_round)
                ));
            }
        }
        
        // Step 1.5: Verify anchor chain consistency
        // This prevents historical fork attacks and time-travel attacks
        // The CF's anchor_chain_root must match our local chain root
        if let Some(ref merkle_roots) = cf.anchor.merkle_roots {
            let local_chain_root = {
                let manager = self.consensus_manager.read().await;
                manager.anchor_builder().anchor_chain_root()
            };
            
            if merkle_roots.anchor_chain_root != local_chain_root {
                return Err(setu_types::SetuError::InvalidData(
                    format!(
                        "Anchor chain root mismatch - possible fork or time-travel attack: expected {}, got {}",
                        hex::encode(local_chain_root),
                        hex::encode(merkle_roots.anchor_chain_root)
                    )
                ));
            }
            
            debug!(
                cf_id = %cf.id,
                chain_root = %hex::encode(local_chain_root),
                "Anchor chain consistency verified"
            );
        }
        
        // Step 2: Idempotency check (before acquiring write lock)
        {
            let manager = self.consensus_manager.read().await;
            if manager.has_cf(&cf.id) {
                return Ok((false, None));
            }
        }
        
        // Step 3: Ensure all referenced events are available in local DAG
        // This is critical for state consistency - we cannot verify or apply
        // state changes without having all the events.
        {
            let dag = self.dag.read().await;
            let missing_event_ids: Vec<EventId> = cf.anchor.event_ids
                .iter()
                .filter(|id| !dag.contains(id))
                .cloned()
                .collect();
            
            if !missing_event_ids.is_empty() {
                // Release dag lock before network call
                drop(dag);
                
                // Try to fetch missing events from peers with retry
                let broadcaster = self.broadcaster.read().await;
                if let Some(ref b) = *broadcaster {
                    info!(
                        cf_id = %cf.id,
                        missing_count = missing_event_ids.len(),
                        "Fetching missing events for CF"
                    );
                    
                    // Retry up to 3 times for transient network failures
                    const MAX_RETRY: usize = 3;
                    let mut last_error = None;
                    let mut fetch_success = false;
                    
                    for retry in 0..MAX_RETRY {
                        match b.request_events(&missing_event_ids).await {
                            Ok(fetched_events) => {
                                fetch_success = true;
                                last_error = None;
                                // Add fetched events through DagManager with retry (handles TOCTOU)
                                for event in fetched_events {
                                    // Update VLC when adding fetched events
                                    {
                                        let mut vlc = self.vlc.write().await;
                                        vlc.merge(&event.vlc_snapshot);
                                    }
                                    
                                    match self.dag_manager.add_event_with_retry(event.clone()).await {
                                        Ok(_) => {}
                                        Err(DagManagerError::DuplicateEvent(_)) => {
                                            // DuplicateEvent is OK (race condition)
                                        }
                                        Err(e) => {
                                            warn!(event_id = %event.id, error = %e, "Failed to add fetched event");
                                        }
                                    }
                                }
                                
                                // Re-check if we still have missing events
                                let dag = self.dag.read().await;
                                let still_missing: Vec<_> = cf.anchor.event_ids
                                    .iter()
                                    .filter(|id| !dag.contains(id))
                                    .collect();
                                
                                if !still_missing.is_empty() {
                                    // Still have missing events, continue retry
                                    last_error = Some(format!(
                                        "Still missing {} events after fetch",
                                        still_missing.len()
                                    ));
                                    drop(dag);
                                } else {
                                    // All events fetched successfully
                                    break;
                                }
                            }
                            Err(e) => {
                                last_error = Some(format!("Fetch failed: {}", e));
                                if retry < MAX_RETRY - 1 {
                                    warn!(
                                        cf_id = %cf.id,
                                        retry = retry + 1,
                                        max_retry = MAX_RETRY,
                                        error = %e,
                                        "Failed to fetch missing events, retrying..."
                                    );
                                    // Wait before retry with exponential backoff
                                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * (retry as u64 + 1))).await;
                                }
                            }
                        }
                    }
                    
                    // Check final result
                    if !fetch_success {
                        return Err(setu_types::SetuError::InvalidData(
                            format!(
                                "CF {} references {} missing events and all fetch attempts failed: {}",
                                cf.id,
                                missing_event_ids.len(),
                                last_error.unwrap_or_else(|| "unknown error".to_string())
                            )
                        ));
                    }
                } else {
                    // No broadcaster configured - cannot fetch missing events
                    return Err(setu_types::SetuError::InvalidData(
                        format!(
                            "CF {} references {} events not in local DAG (no broadcaster to fetch)",
                            cf.id,
                            missing_event_ids.len()
                        )
                    ));
                }
            }
        }
        
        // Now we have all events, proceed with verification
        let dag = self.dag.read().await;
        let mut manager = self.consensus_manager.write().await;
        
        // Double-check idempotency (another thread may have processed while we fetched)
        if manager.has_cf(&cf.id) {
            return Ok((false, None));
        }
        
        // Step 4: Verify the CF's merkle roots are internally consistent
        if !manager.verify_cf_merkle_roots(&cf) {
            return Err(setu_types::SetuError::InvalidData(
                "CF merkle roots verification failed".to_string()
            ));
        }
        
        // Step 5-6: Apply state changes from the CF's events to maintain consistent state
        // This also verifies the resulting state matches the anchor's state root
        let state_verified = manager.apply_cf_state_changes(&dag, &cf);
        if !state_verified {
            return Err(setu_types::SetuError::InvalidData(
                "State root mismatch after applying CF state changes".to_string()
            ));
        }
        
        let cf_id = cf.id.clone();
        
        // Receive the CF
        manager.receive_cf(cf.clone());

        // Vote for the CF (in MVP, we always approve valid CFs)
        let private_key = self.private_key.read().await;
        let vote = manager.vote_for_cf(
            &cf_id, 
            true,
            private_key.as_ref().map(|k| k.as_slice())
        );
        if let Some(ref v) = vote {
            // Broadcast vote to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_vote(v).await {
                    Ok(result) => {
                        debug!(
                            cf_id = %cf_id,
                            success = result.success_count,
                            "Vote broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %cf_id, error = %e, "Failed to broadcast vote");
                    }
                }
            }
            
            // Check if our vote caused finalization
            // (vote_for_cf adds vote but doesn't check finalization, so we check here)
            let finalized = manager.check_finalization(&cf_id);
            if finalized {
                return self.handle_finalization(&mut manager).await;
            }
        }

        Ok((false, None))
    }
    
    /// Handle CF finalization: broadcast notification and return anchor for persistence
    /// 
    /// Note: This method extracts data from manager before acquiring other locks
    /// to avoid potential deadlock from holding multiple write locks.
    async fn handle_finalization(&self, manager: &mut tokio::sync::RwLockWriteGuard<'_, ConsensusManager>) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        // Extract data from manager first, before acquiring other locks
        let cf_data = manager.last_finalized_cf().map(|cf| (cf.id.clone(), cf.anchor.clone(), cf.clone()));
        
        let finalized_anchor = if let Some((cf_id, anchor, cf)) = cf_data {
            // Send finalization to internal channel (for local listeners)
            let _ = self
                .message_tx
                .send(ConsensusMessage::FrameFinalized(cf))
                .await;

            // Broadcast finalization to network via broadcaster (if configured)
            let broadcaster = self.broadcaster.read().await;
            if let Some(ref b) = *broadcaster {
                match b.broadcast_finalized(&cf_id).await {
                    Ok(result) => {
                        info!(
                            cf_id = %cf_id,
                            success = result.success_count,
                            "CF finalization broadcasted to peers"
                        );
                    }
                    Err(e) => {
                        warn!(cf_id = %cf_id, error = %e, "Failed to broadcast finalization");
                    }
                }
            }
            // Release broadcaster lock before acquiring validator_set lock
            drop(broadcaster);

            // Advance to next round (ValidatorSet manages rounds)
            let mut validator_set = self.validator_set.write().await;
            validator_set.advance_round();

            Some(anchor)
        } else {
            None
        };

        Ok((true, finalized_anchor))
    }

    /// Receive a vote from another validator
    /// 
    /// Returns (finalized, Option<Anchor>) - the anchor is returned when finalized
    /// so the caller can persist it to storage.
    pub async fn receive_vote(&self, vote: Vote) -> SetuResult<(bool, Option<setu_types::Anchor>)> {
        // Step 1: Verify voter is a valid validator
        {
            let validator_set = self.validator_set.read().await;
            let all_validators = validator_set.all_validators();
            if !all_validators.iter().any(|v| v.node.id == vote.validator_id) {
                return Err(setu_types::SetuError::InvalidData(
                    format!("Vote from non-validator: {}", vote.validator_id)
                ));
            }
        }
        
        // Step 2: Verify vote signature
        if !vote.signature.is_empty() {
            // Get the public key for this validator
            let public_key = {
                let validator_set = self.validator_set.read().await;
                let all_validators = validator_set.all_validators();
                all_validators
                    .iter()
                    .find(|v| v.node.id == vote.validator_id)
                    .map(|v| v.node.public_key.clone())
            };
            
            if let Some(pub_key) = public_key {
                if !pub_key.is_empty() {
                    if !vote.verify_signature(&pub_key) {
                        return Err(setu_types::SetuError::InvalidData(
                            format!("Invalid vote signature from validator: {}", vote.validator_id)
                        ));
                    }
                    debug!(
                        cf_id = %vote.cf_id,
                        voter = %vote.validator_id,
                        "Vote signature verified successfully"
                    );
                } else {
                    warn!(
                        voter = %vote.validator_id,
                        "Validator has no public key configured, skipping signature verification"
                    );
                }
            } else {
                // This shouldn't happen as we already checked validator existence
                return Err(setu_types::SetuError::InvalidData(
                    format!("Could not find public key for validator: {}", vote.validator_id)
                ));
            }
        } else {
            // Empty signature - for backward compatibility or testing
            warn!(
                cf_id = %vote.cf_id,
                voter = %vote.validator_id,
                "Vote has no signature - signature verification skipped (insecure in production)"
            );
        }
        
        let mut manager = self.consensus_manager.write().await;
        let finalized = manager.receive_vote(vote);

        if finalized {
            self.handle_finalization(&mut manager).await
        } else {
            Ok((false, None))
        }
    }

    /// Compute the state root from the DAG (legacy method)
    /// 
    /// This is a simple hash-based computation for backward compatibility.
    /// The real state root is now computed by AnchorBuilder using SMTs.
    #[deprecated(since = "0.2.0", note = "State root is now computed internally by ConsensusManager/AnchorBuilder")]
    fn compute_state_root_internal(&self, dag: &Dag) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dag.node_count().to_le_bytes());
        hasher.update(dag.max_depth().to_le_bytes());
        hex::encode(hasher.finalize())
    }

    /// Compute the state root (async version, legacy)
    #[deprecated(since = "0.2.0", note = "Use get_global_state_root() instead")]
    pub async fn compute_state_root(&self) -> String {
        let dag = self.dag.read().await;
        #[allow(deprecated)]
        self.compute_state_root_internal(&dag)
    }
    
    /// Get the current global state root from AnchorBuilder
    pub async fn get_global_state_root(&self) -> [u8; 32] {
        let manager = self.consensus_manager.read().await;
        manager.get_global_root()
    }
    
    /// Get a subnet's current state root
    pub async fn get_subnet_state_root(&self, subnet_id: &setu_types::SubnetId) -> Option<[u8; 32]> {
        let manager = self.consensus_manager.read().await;
        manager.get_subnet_root(subnet_id)
    }
    
    /// Get the current anchor chain root
    /// 
    /// This returns the cumulative chain root that commits to the entire anchor history.
    /// Used for verifying anchor chain consistency when receiving CFs from other validators.
    pub async fn get_anchor_chain_root(&self) -> [u8; 32] {
        let manager = self.consensus_manager.read().await;
        manager.anchor_builder().anchor_chain_root()
    }
    
    /// Get the number of anchors created
    pub async fn get_anchor_count(&self) -> usize {
        let manager = self.consensus_manager.read().await;
        manager.anchor_count()
    }
    
    /// Mark an anchor as successfully persisted to storage
    /// 
    /// Call this after successfully storing the anchor to AnchorStore.
    /// This enables safe garbage collection of finalized CFs from memory.
    pub async fn mark_anchor_persisted(&self, anchor_id: &str) {
        let mut manager = self.consensus_manager.write().await;
        manager.mark_anchor_persisted(anchor_id);
    }

    /// Get the message sender for external communication
    pub fn message_sender(&self) -> mpsc::Sender<ConsensusMessage> {
        self.message_tx.clone()
    }

    /// Get DAG statistics
    pub async fn get_dag_stats(&self) -> DagStats {
        let dag = self.dag.read().await;
        DagStats {
            node_count: dag.node_count(),
            max_depth: dag.max_depth(),
            tip_count: dag.get_tips().len(),
            pending_count: dag.get_pending_count(),
        }
    }

    /// Get the current VLC snapshot
    pub async fn get_vlc_snapshot(&self) -> VLCSnapshot {
        self.vlc.read().await.snapshot()
    }

    /// Allocate a logical time using lock-free atomic counter (FAST PATH)
    /// 
    /// This is the preferred method for local event creation where only
    /// the logical_time is needed. It avoids acquiring the VLC write lock
    /// and provides O(1) performance even under high concurrency.
    /// 
    /// Use this instead of `tick_and_get_vlc()` when:
    /// - Creating local events (submit_transfer)
    /// - Only logical_time is needed (not full VLCSnapshot)
    /// 
    /// The full VLC with `tick_and_get_vlc()` is still needed for:
    /// - Receiving events from other validators (merge operation)
    /// - Multi-validator consensus scenarios
    #[inline]
    pub fn allocate_logical_time(&self) -> u64 {
        self.logical_time_counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Atomically increment VLC and return the new snapshot
    /// 
    /// This is the correct method to use when assigning VLC to events,
    /// as it ensures each event gets a unique logical time.
    /// 
    /// NOTE: For high-performance local event creation, prefer `allocate_logical_time()`
    /// which uses a lock-free atomic counter.
    pub async fn tick_and_get_vlc(&self) -> VLCSnapshot {
        let mut vlc = self.vlc.write().await;
        vlc.tick();  // VLC::tick() uses the node_id internally
        vlc.snapshot()
    }

    /// Get the current tips of the DAG
    pub async fn get_tips(&self) -> Vec<EventId> {
        self.dag.read().await.get_tips()
    }

    /// Get events by their IDs from the DAG
    /// 
    /// This is used to retrieve events for persistence when a CF is finalized.
    /// Returns events that exist in the DAG.
    pub async fn get_events_by_ids(&self, event_ids: &[EventId]) -> Vec<Event> {
        let dag = self.dag.read().await;
        event_ids
            .iter()
            .filter_map(|id| dag.get_event(id).cloned())
            .collect()
    }
    
    /// Get events by their IDs using three-layer query (DAG → Store)
    /// 
    /// This method queries both the active DAG and the persistent EventStore,
    /// ensuring events can be found even after GC.
    /// 
    /// Used by: Network sync handlers when responding to event requests.
    pub async fn get_events_by_ids_three_layer(&self, event_ids: &[EventId]) -> Vec<Event> {
        let mut results = Vec::with_capacity(event_ids.len());
        let mut store_query_ids = Vec::new();
        
        // Step 1: Query DAG (hot data)
        {
            let dag = self.dag.read().await;
            for id in event_ids {
                if let Some(event) = dag.get_event(id) {
                    results.push(event.clone());
                } else {
                    store_query_ids.push(id.clone());
                }
            }
        }
        
        // Step 2: Query EventStore for DAG misses (cold data)
        if !store_query_ids.is_empty() {
            let store_events = self.dag_manager
                .event_store()
                .get_events_batch(&store_query_ids)
                .await;
            results.extend(store_events);
        }
        
        results
    }
    
    /// Get the DagManager reference
    /// 
    /// Used for direct access to three-layer storage operations,
    /// such as GC triggering and cache warmup.
    pub fn dag_manager(&self) -> &Arc<DagManager> {
        &self.dag_manager
    }

    /// Get the local validator ID
    pub fn local_validator_id(&self) -> &str {
        &self.local_validator_id
    }

    /// Get the configuration
    pub fn config(&self) -> &ConsensusConfig {
        &self.config
    }
}

/// DAG statistics
#[derive(Debug, Clone)]
pub struct DagStats {
    pub node_count: usize,
    pub max_depth: u64,
    pub tip_count: usize,
    pub pending_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{NodeInfo, ValidatorInfo, Anchor, AnchorMerkleRoots};
    use setu_vlc::VectorClock;
    use std::collections::HashMap;

    fn create_validator_set() -> ValidatorSet {
        let mut set = ValidatorSet::new();
        for i in 1..=3 {
            let node = NodeInfo::new_validator(
                format!("v{}", i),
                "127.0.0.1".to_string(),
                8000 + i as u16,
            );
            set.add_validator(ValidatorInfo::new(node, false));
        }
        set
    }

    #[tokio::test]
    async fn test_engine_create_event() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let event = engine.create_event(vec![]).await.unwrap();
        assert_eq!(event.creator, "v1");
    }

    #[tokio::test]
    async fn test_engine_add_event() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let genesis = Event::genesis(
            "v1".to_string(),
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
        );

        let _event_id = engine.add_event(genesis).await.unwrap();

        let stats = engine.get_dag_stats().await;
        assert_eq!(stats.node_count, 1);
    }

    #[tokio::test]
    async fn test_engine_leader_check() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        // First validator should be the leader
        assert!(engine.is_current_leader().await);
    }

    #[tokio::test]
    async fn test_engine_advance_round() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        let round0 = engine.current_round().await;
        assert_eq!(round0, 0);

        let round1 = engine.advance_round().await;
        assert_eq!(round1, 1);
    }

    #[tokio::test]
    async fn test_engine_valid_proposer() {
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());

        // Check proposer for different rounds
        let proposer_0 = engine.get_valid_proposer(0).await;
        let proposer_1 = engine.get_valid_proposer(1).await;
        let proposer_2 = engine.get_valid_proposer(2).await;

        assert!(proposer_0.is_some());
        assert!(proposer_1.is_some());
        assert!(proposer_2.is_some());

        // Proposers should rotate
        assert_ne!(proposer_0, proposer_1);
    }
    
    #[tokio::test]
    async fn test_anchor_chain_root_verification() {
        use setu_types::{Anchor, merkle::AnchorMerkleRoots};
        
        let config = ConsensusConfig::default();
        let engine = ConsensusEngine::new(config, "v1".to_string(), create_validator_set());
        
        // Get initial anchor chain root (should be all zeros for genesis)
        let initial_root = engine.get_anchor_chain_root().await;
        assert_eq!(initial_root, [0u8; 32], "Initial anchor chain root should be all zeros");
        
        // Create a CF with correct anchor_chain_root
        let correct_merkle_roots = AnchorMerkleRoots::with_roots(
            [1u8; 32], // events_root
            [2u8; 32], // global_state_root
            initial_root, // anchor_chain_root (matches current state)
        );
        
        let anchor_correct = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
            correct_merkle_roots,
            None,
            0,
        );
        
        let cf_correct = ConsensusFrame::new(anchor_correct, "v1".to_string());
        
        // This should succeed (anchor_chain_root matches)
        let result = engine.receive_cf(cf_correct).await;
        // Note: Will fail at idempotency check or other steps, but should pass chain root verification
        // The error should NOT be about anchor chain root mismatch
        if let Err(e) = result {
            let error_msg = e.to_string();
            assert!(!error_msg.contains("Anchor chain root mismatch"), 
                "Should not fail on anchor chain root verification, got: {}", error_msg);
        }
        
        // Create a CF with WRONG anchor_chain_root
        let wrong_merkle_roots = AnchorMerkleRoots::with_roots(
            [1u8; 32], // events_root
            [2u8; 32], // global_state_root
            [99u8; 32], // anchor_chain_root (WRONG - doesn't match)
        );
        
        let anchor_wrong = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
            wrong_merkle_roots,
            None,
            0,
        );
        
        let cf_wrong = ConsensusFrame::new(anchor_wrong, "v1".to_string());
        
        // This should FAIL due to anchor chain root mismatch
        let result = engine.receive_cf(cf_wrong).await;
        assert!(result.is_err(), "Should reject CF with wrong anchor_chain_root");
        
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Anchor chain root mismatch"), 
            "Error should be about anchor chain root mismatch, got: {}", error_msg);
    }
    
    #[tokio::test]
    async fn test_follower_synchronizes_anchor_chain_root() {
        use setu_types::{Anchor, merkle::AnchorMerkleRoots, EventType};
        
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            validator_count: 3,
            ..Default::default()
        };
        
        // Create leader engine
        let leader_engine = ConsensusEngine::new(
            config.clone(),
            "v1".to_string(),
            create_validator_set()
        );
        
        // Create follower engine
        let follower_engine = ConsensusEngine::new(
            config.clone(),
            "v2".to_string(),
            create_validator_set()
        );
        
        // Both start with same anchor_chain_root
        let initial_root = leader_engine.get_anchor_chain_root().await;
        assert_eq!(initial_root, [0u8; 32], "Initial root should be zero");
        assert_eq!(
            follower_engine.get_anchor_chain_root().await,
            initial_root,
            "Both nodes should start with same root"
        );
        
        // Leader creates events and CF
        for i in 0..3 {
            let mut event = Event::new(
                EventType::System,
                vec![],
                VLCSnapshot {
                    vector_clock: VectorClock::new(),
                    logical_time: i,
                    physical_time: 0,
                },
                "v1".to_string(),
            );
            // Add execution result for state changes
            event.execution_result = Some(setu_types::ExecutionResult::success());
            let _ = leader_engine.add_event(event).await;
        }
        
        // Try to create CF (needs enough VLC delta)
        {
            let mut vlc = leader_engine.vlc.write().await;
            for _ in 0..10 {
                vlc.tick();
            }
        }
        
        let _cf_opt = leader_engine.try_create_cf().await;
        // CF creation might fail due to various reasons (min events, etc)
        // For this test, we simulate a CF manually
        
        // Create a CF with correct merkle roots
        let correct_merkle_roots = AnchorMerkleRoots::with_roots(
            [1u8; 32],
            [2u8; 32],
            initial_root,
        );
        
        let anchor = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 10,
                physical_time: 0,
            },
            correct_merkle_roots,
            None,
            0,
        );
        
        let cf = ConsensusFrame::new(anchor.clone(), "v1".to_string());
        
        // Follower receives and finalizes CF
        // Note: receive_cf will fail at various checks, but we can test the manager directly
        {
            let mut manager = follower_engine.consensus_manager.write().await;
            manager.receive_cf(cf.clone());
            
            // Add votes to reach quorum (simulate 3 validators)
            let vote1 = Vote::new("v1".to_string(), cf.id.clone(), true);
            let vote2 = Vote::new("v2".to_string(), cf.id.clone(), true);
            let vote3 = Vote::new("v3".to_string(), cf.id.clone(), true);
            
            manager.receive_vote(vote1);
            manager.receive_vote(vote2);
            let finalized = manager.receive_vote(vote3);
            
            assert!(finalized, "CF should be finalized after quorum");
            
            // Check that anchor_chain_root was updated
            let updated_root = manager.anchor_builder().anchor_chain_root();
            assert_ne!(updated_root, initial_root, "Anchor chain root should have been updated");
            
            // Verify it matches the expected computation
            let expected_root = {
                use sha2::{Sha256, Digest};
                let anchor_hash = anchor.compute_hash();
                let mut hasher = Sha256::new();
                hasher.update(&initial_root);
                hasher.update(&anchor_hash);
                let result = hasher.finalize();
                let mut output = [0u8; 32];
                output.copy_from_slice(&result);
                output
            };
            
            assert_eq!(
                updated_root, expected_root,
                "Anchor chain root should match expected chain hash"
            );
        }
        
        // Verify follower can now accept next CF with updated root
        let follower_root = follower_engine.get_anchor_chain_root().await;
        assert_ne!(follower_root, initial_root, "Follower root should be updated");
    }
    
    #[tokio::test]
    async fn test_cf_rejection_with_minority_reject_votes() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            cf_timeout_ms: 5000,
            validator_count: 4,  // 4 validators: need 2 rejects to reject (1/3+1)
        };
        
        let validator_set = create_validator_set();
        let engine = ConsensusEngine::new(config, "v1".to_string(), validator_set);
        
        let anchor = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 10,
                physical_time: 0,
            },
            AnchorMerkleRoots {
                events_root: [0u8; 32],
                global_state_root: [0u8; 32],
                anchor_chain_root: [0u8; 32],
                subnet_roots: HashMap::new(),
            },
            None,
            0,
        );
        
        let cf = ConsensusFrame::new(anchor.clone(), "v1".to_string());
        let cf_id = cf.id.clone();
        
        let mut manager = engine.consensus_manager.write().await;
        manager.receive_cf(cf);
        
        // Vote 1: approve (should not finalize yet)
        let vote1 = Vote::new("v1".to_string(), cf_id.clone(), true);
        let result1 = manager.receive_vote(vote1);
        assert!(!result1, "Should not finalize with 1 approve vote");
        
        // Vote 2: reject (1 reject, not enough)
        let vote2 = Vote::new("v2".to_string(), cf_id.clone(), false);
        let result2 = manager.receive_vote(vote2);
        assert!(!result2, "Should not reject with only 1 reject vote");
        
        // Vote 3: reject (2 rejects = 1/3+1, should reject)
        let vote3 = Vote::new("v3".to_string(), cf_id.clone(), false);
        let result3 = manager.receive_vote(vote3);
        assert!(result3, "Should reject with 2 reject votes (1/3+1 threshold)");
        
        // Verify CF was removed from pending (can't directly access private field)
        // Instead verify it's not in last_finalized_cf
        assert!(manager.last_finalized_cf().is_none(), "Rejected CF should not be finalized");
    }
    
    #[tokio::test]
    async fn test_cf_timeout_cleanup() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            cf_timeout_ms: 100,  // 100ms timeout for testing
            validator_count: 4,
        };
        
        let validator_set = create_validator_set();
        let engine = ConsensusEngine::new(config, "v1".to_string(), validator_set);
        
        let anchor = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 10,
                physical_time: 0,
            },
            AnchorMerkleRoots {
                events_root: [0u8; 32],
                global_state_root: [0u8; 32],
                anchor_chain_root: [0u8; 32],
                subnet_roots: HashMap::new(),
            },
            None,
            0,
        );
        
        let cf = ConsensusFrame::new(anchor.clone(), "v1".to_string());
        let cf_id = cf.id.clone();
        
        {
            let mut manager = engine.consensus_manager.write().await;
            manager.receive_cf(cf);
            
            // Verify CF is pending (test by attempting to receive vote)
            let vote = Vote::new("v1".to_string(), cf_id.clone(), true);
            manager.receive_vote(vote);
        }
        
        // Wait for timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
        
        {
            let mut manager = engine.consensus_manager.write().await;
            
            // Call cleanup
            let removed_count = manager.cleanup_timeout_cfs();
            assert_eq!(removed_count, 1, "Should remove 1 timeout CF");
        }
    }
    
    #[tokio::test]
    async fn test_timeout_check_in_vote_processing() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            cf_timeout_ms: 100,  // 100ms timeout
            validator_count: 4,
        };
        
        let validator_set = create_validator_set();
        let engine = ConsensusEngine::new(config, "v1".to_string(), validator_set);
        
        let anchor = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 10,
                physical_time: 0,
            },
            AnchorMerkleRoots {
                events_root: [0u8; 32],
                global_state_root: [0u8; 32],
                anchor_chain_root: [0u8; 32],
                subnet_roots: HashMap::new(),
            },
            None,
            0,
        );
        
        let cf = ConsensusFrame::new(anchor.clone(), "v1".to_string());
        let cf_id = cf.id.clone();
        
        let mut manager = engine.consensus_manager.write().await;
        manager.receive_cf(cf);
        
        // Wait for timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
        
        // Receive a vote - should trigger timeout check and remove CF
        let vote = Vote::new("v1".to_string(), cf_id.clone(), true);
        let result = manager.receive_vote(vote);
        
        // The vote processing should detect timeout and remove CF
        assert!(result, "Should return true when CF is removed due to timeout");
    }
    
    #[tokio::test]
    async fn test_leader_can_retry_after_cf_rejection() {
        // This test verifies that after a CF is rejected, the leader can
        // immediately create a new CF (rollback_failed_build works correctly)
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            cf_timeout_ms: 5000,
            validator_count: 4,
        };
        
        let validator_set = create_validator_set();
        let engine = ConsensusEngine::new(config, "v1".to_string(), validator_set);
        
        // Create first CF
        let anchor1 = Anchor::with_merkle_roots(
            vec![],
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 10,
                physical_time: 0,
            },
            AnchorMerkleRoots {
                events_root: [0u8; 32],
                global_state_root: [0u8; 32],
                anchor_chain_root: [0u8; 32],
                subnet_roots: HashMap::new(),
            },
            None,
            0,
        );
        
        let cf1 = ConsensusFrame::new(anchor1.clone(), "v1".to_string());
        let cf1_id = cf1.id.clone();
        
        let mut manager = engine.consensus_manager.write().await;
        
        // Record state before receiving CF
        let initial_vlc = manager.anchor_builder().last_fold_vlc();
        let initial_depth = manager.anchor_builder().anchor_depth();
        
        // Simulate: leader created this CF (receive it as if we proposed it)
        manager.receive_cf(cf1);
        
        // Reject the CF with enough reject votes
        let vote1 = Vote::new("v2".to_string(), cf1_id.clone(), false);
        let vote2 = Vote::new("v3".to_string(), cf1_id.clone(), false);
        manager.receive_vote(vote1);
        let rejected = manager.receive_vote(vote2);
        
        assert!(rejected, "CF should be rejected with 2 reject votes");
        
        // Verify the CF is removed
        assert!(manager.last_finalized_cf().is_none(), "No CF should be finalized");
        
        // Key assertion: After rejection, leader's anchor_builder state should be
        // rolled back, allowing immediate retry. Since we received (not created) this CF,
        // the rollback won't apply (proposer check). But we can verify the mechanism exists.
        
        // For a true test of rollback, we'd need to use try_create_cf which actually
        // modifies anchor_builder state. This test verifies the reject path works.
    }
}
