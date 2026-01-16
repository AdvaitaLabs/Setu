//! Anchor Builder - Integrates DAG folding with Merkle tree state management.
//!
//! This module provides the complete flow for creating Anchors:
//! 1. Collect events from DAG
//! 2. Route events by subnet
//! 3. Apply state changes from execution results
//! 4. Compute all Merkle roots
//! 5. Create the Anchor with merkle_roots

use crate::dag::Dag;
use crate::vlc::VLC;
use crate::router::{EventRouter, RoutedEvents};
use crate::merkle_integration::{compute_events_root, compute_anchor_chain_root_from_hashes};
use setu_types::{
    Anchor, ConsensusConfig, Event, EventId, SubnetId,
};
use setu_storage::subnet_state::{GlobalStateManager, StateApplySummary, StateApplyError};

/// Complete Anchor creation result
#[derive(Debug)]
pub struct AnchorBuildResult {
    /// The created anchor
    pub anchor: Anchor,
    /// Summary of state changes applied
    pub state_summary: StateApplySummary,
    /// Events routed by subnet
    pub routed_events: RoutedEvents,
}

/// Error types for anchor building
#[derive(Debug)]
pub enum AnchorBuildError {
    /// Not enough events to fold
    InsufficientEvents { required: usize, found: usize },
    /// VLC delta not reached
    DeltaNotReached { required: u64, current: u64 },
    /// State apply failed
    StateApplyError(StateApplyError),
    /// Merkle operation failed
    MerkleError(String),
    /// No events to process
    NoEvents,
}

impl std::fmt::Display for AnchorBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorBuildError::InsufficientEvents { required, found } => {
                write!(f, "Insufficient events: required {}, found {}", required, found)
            }
            AnchorBuildError::DeltaNotReached { required, current } => {
                write!(f, "VLC delta not reached: required {}, current {}", required, current)
            }
            AnchorBuildError::StateApplyError(e) => write!(f, "State apply error: {}", e),
            AnchorBuildError::MerkleError(e) => write!(f, "Merkle error: {}", e),
            AnchorBuildError::NoEvents => write!(f, "No events to process"),
        }
    }
}

impl std::error::Error for AnchorBuildError {}

impl From<StateApplyError> for AnchorBuildError {
    fn from(e: StateApplyError) -> Self {
        AnchorBuildError::StateApplyError(e)
    }
}

impl From<setu_merkle::MerkleError> for AnchorBuildError {
    fn from(e: setu_merkle::MerkleError) -> Self {
        AnchorBuildError::MerkleError(e.to_string())
    }
}

/// Anchor Builder with integrated Merkle tree management
pub struct AnchorBuilder {
    config: ConsensusConfig,
    /// Global state manager (owns all subnet SMTs)
    state_manager: GlobalStateManager,
    /// Last created anchor
    last_anchor: Option<Anchor>,
    /// Current anchor depth
    anchor_depth: u64,
    /// Last fold VLC timestamp
    last_fold_vlc: u64,
    /// Historical anchor hashes for anchor chain tree
    anchor_history: Vec<[u8; 32]>,
}

impl AnchorBuilder {
    /// Create a new AnchorBuilder
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            config,
            state_manager: GlobalStateManager::new(),
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            anchor_history: Vec::new(),
        }
    }
    
    /// Create with an existing GlobalStateManager
    pub fn with_state_manager(config: ConsensusConfig, state_manager: GlobalStateManager) -> Self {
        Self {
            config,
            state_manager,
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
            anchor_history: Vec::new(),
        }
    }
    
    /// Check if we should attempt to fold
    pub fn should_fold(&self, current_vlc: &VLC) -> bool {
        let delta = current_vlc.logical_time().saturating_sub(self.last_fold_vlc);
        delta >= self.config.vlc_delta_threshold
    }
    
    /// Get the global state manager (read-only)
    pub fn state_manager(&self) -> &GlobalStateManager {
        &self.state_manager
    }
    
    /// Get the global state manager (mutable)
    pub fn state_manager_mut(&mut self) -> &mut GlobalStateManager {
        &mut self.state_manager
    }
    
    /// Try to build an anchor from the DAG
    ///
    /// This is the main entry point for anchor creation. It:
    /// 1. Checks if folding conditions are met
    /// 2. Collects events from DAG
    /// 3. Routes events by subnet
    /// 4. Applies state changes from execution results
    /// 5. Computes all Merkle roots
    /// 6. Creates and returns the Anchor
    pub fn try_build(
        &mut self,
        dag: &Dag,
        vlc: &VLC,
    ) -> Result<AnchorBuildResult, AnchorBuildError> {
        // Check VLC delta threshold
        let delta = vlc.logical_time().saturating_sub(self.last_fold_vlc);
        if delta < self.config.vlc_delta_threshold {
            return Err(AnchorBuildError::DeltaNotReached {
                required: self.config.vlc_delta_threshold,
                current: delta,
            });
        }
        
        // Collect events from DAG
        let from_depth = self.anchor_depth;
        let to_depth = dag.max_depth();
        let events: Vec<Event> = dag.get_events_in_range(from_depth, to_depth)
            .into_iter()
            .cloned()
            .collect();
        
        // Check minimum events
        if events.len() < self.config.min_events_per_cf {
            return Err(AnchorBuildError::InsufficientEvents {
                required: self.config.min_events_per_cf,
                found: events.len(),
            });
        }
        
        if events.is_empty() {
            return Err(AnchorBuildError::NoEvents);
        }
        
        // Build the anchor
        self.build_anchor_internal(events, vlc, to_depth)
    }
    
    /// Force build an anchor from a specific set of events
    ///
    /// This bypasses the VLC delta check and min events check.
    /// Useful for testing or when events are already validated.
    pub fn force_build(
        &mut self,
        events: Vec<Event>,
        vlc: &VLC,
        depth: u64,
    ) -> Result<AnchorBuildResult, AnchorBuildError> {
        if events.is_empty() {
            return Err(AnchorBuildError::NoEvents);
        }
        self.build_anchor_internal(events, vlc, depth)
    }
    
    fn build_anchor_internal(
        &mut self,
        events: Vec<Event>,
        vlc: &VLC,
        to_depth: u64,
    ) -> Result<AnchorBuildResult, AnchorBuildError> {
        // Route events by subnet
        let routed = EventRouter::route_events(&events);
        
        // Apply state changes from execution results
        let state_summary = self.state_manager.apply_committed_events(&events);
        
        // Compute events root (Binary Merkle Tree)
        let events_root_hash = compute_events_root(&events);
        let events_root = *events_root_hash.as_bytes();
        
        // Compute anchor chain root
        let anchor_chain_root = compute_anchor_chain_root_from_hashes(&self.anchor_history);
        
        // Build anchor merkle roots
        let merkle_roots = self.state_manager.build_anchor_roots(
            events_root,
            anchor_chain_root,
        );
        
        // Collect event IDs (with limit)
        let event_ids: Vec<EventId> = events
            .iter()
            .take(self.config.max_events_per_cf)
            .map(|e| e.id.clone())
            .collect();
        
        // Create anchor
        let anchor = Anchor::with_merkle_roots(
            event_ids,
            vlc.snapshot(),
            self.last_anchor.as_ref().map(|a| a.id.clone()),
            to_depth,
            merkle_roots,
        );
        
        // Update anchor history for anchor chain tree
        self.anchor_history.push(anchor.compute_hash());
        
        // Commit state
        let anchor_id = self.anchor_depth + 1;
        self.state_manager.commit(anchor_id)?;
        
        // Update internal state
        self.last_anchor = Some(anchor.clone());
        self.anchor_depth = to_depth + 1;
        self.last_fold_vlc = vlc.logical_time();
        
        Ok(AnchorBuildResult {
            anchor,
            state_summary,
            routed_events: routed,
        })
    }
    
    /// Get the last created anchor
    pub fn last_anchor(&self) -> Option<&Anchor> {
        self.last_anchor.as_ref()
    }
    
    /// Get the current anchor depth
    pub fn anchor_depth(&self) -> u64 {
        self.anchor_depth
    }
    
    /// Get anchor history count
    pub fn anchor_count(&self) -> usize {
        self.anchor_history.len()
    }
    
    /// Get a subnet's current state root
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<[u8; 32]> {
        self.state_manager.get_subnet_root_bytes(subnet_id)
    }
    
    /// Get the current global state root
    pub fn get_global_root(&self) -> [u8; 32] {
        let (root, _) = self.state_manager.compute_global_root_bytes();
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{EventType, ExecutionResult, StateChange};
    use setu_types::event::VLCSnapshot;
    
    fn create_vlc(node_id: &str, time: u64) -> VLC {
        let mut vlc = VLC::new(node_id.to_string());
        for _ in 0..time {
            vlc.tick();
        }
        vlc
    }
    
    fn create_event_with_result(subnet_id: SubnetId, changes: Vec<StateChange>) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        event = event.with_subnet(subnet_id);
        event.execution_result = Some(ExecutionResult {
            success: true,
            message: None,
            state_changes: changes,
        });
        event
    }
    
    #[test]
    fn test_anchor_builder_basic() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };
        
        let mut builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);
        
        // Create event with state changes
        let event = create_event_with_result(
            SubnetId::ROOT,
            vec![
                StateChange {
                    key: "balance:alice".to_string(),
                    old_value: Some(vec![0; 8]),
                    new_value: Some(vec![100; 8]),
                },
            ],
        );
        
        // Force build anchor
        let result = builder.force_build(vec![event], &vlc, 1);
        assert!(result.is_ok());
        
        let build_result = result.unwrap();
        assert_eq!(build_result.state_summary.total_events, 1);
        assert_eq!(build_result.state_summary.total_changes, 1);
        assert!(build_result.anchor.merkle_roots.is_some());
    }
    
    #[test]
    fn test_anchor_builder_multi_subnet() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };
        
        let mut builder = AnchorBuilder::new(config);
        let vlc = create_vlc("node1", 10);
        
        // Create events for different subnets
        let app_subnet = SubnetId::from_str_id("my-app");
        
        let events = vec![
            create_event_with_result(
                SubnetId::ROOT,
                vec![StateChange {
                    key: "balance:alice".to_string(),
                    old_value: None,
                    new_value: Some(vec![100; 8]),
                }],
            ),
            create_event_with_result(
                app_subnet,
                vec![StateChange {
                    key: "nft:token1".to_string(),
                    old_value: None,
                    new_value: Some(vec![1; 32]),
                }],
            ),
        ];
        
        let result = builder.force_build(events, &vlc, 1).unwrap();
        
        // Should have processed events from both subnets
        assert_eq!(result.state_summary.subnets_updated(), 2);
        assert!(result.state_summary.subnet_stats.contains_key(&SubnetId::ROOT));
        assert!(result.state_summary.subnet_stats.contains_key(&app_subnet));
        
        // Anchor should have merkle roots for both subnets
        let merkle_roots = result.anchor.merkle_roots.as_ref().unwrap();
        assert_eq!(merkle_roots.subnet_roots.len(), 2);
    }
    
    #[test]
    fn test_anchor_builder_state_persistence() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            max_events_per_cf: 100,
            ..Default::default()
        };
        
        let mut builder = AnchorBuilder::new(config);
        
        // Build first anchor
        let vlc1 = create_vlc("node1", 10);
        let event1 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:alice".to_string(),
                old_value: None,
                new_value: Some(vec![100; 8]),
            }],
        );
        builder.force_build(vec![event1], &vlc1, 1).unwrap();
        let root1 = builder.get_global_root();
        
        // Build second anchor with more changes
        let vlc2 = create_vlc("node1", 20);
        let event2 = create_event_with_result(
            SubnetId::ROOT,
            vec![StateChange {
                key: "balance:bob".to_string(),
                old_value: None,
                new_value: Some(vec![200; 8]),
            }],
        );
        builder.force_build(vec![event2], &vlc2, 2).unwrap();
        let root2 = builder.get_global_root();
        
        // State roots should be different
        assert_ne!(root1, root2);
        
        // Anchor count should be 2
        assert_eq!(builder.anchor_count(), 2);
    }
}
