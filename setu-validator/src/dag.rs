//! DAG (Directed Acyclic Graph) management for Validator
//!
//! This module provides a wrapper around the consensus crate's DAG implementation,
//! adapting it for the validator's async workflow.
//!
//! ## Design Decisions
//!
//! - **Core DAG**: Uses `consensus::Dag` as the underlying implementation
//! - **DagManager**: Thin wrapper providing logging and async interface
//! - **DagNode**: View struct for compatibility with existing code
//! - **Error Handling**: Re-exports `DagError` for type consistency

use consensus::{Dag, DagError};
use setu_types::event::{Event, EventId};
use tracing::{debug, info, warn};

// Re-export DagError for type consistency
pub use consensus::DagError as DagManagerError;

/// DAG node representing an event (re-exported for compatibility)
#[derive(Debug, Clone)]
pub struct DagNode {
    /// Event ID
    pub event_id: EventId,
    /// Parent event IDs (dependencies)
    pub parents: Vec<EventId>,
    /// Child event IDs (dependents)
    pub children: Vec<EventId>,
    /// Causal depth (distance from genesis)
    pub depth: u64,
    /// Whether this event is finalized
    pub finalized: bool,
}

impl DagNode {
    /// Create from an event and DAG context
    fn from_event(event: &Event, dag: &Dag) -> Self {
        let depth = dag.get_depth(&event.id).unwrap_or(0);
        let children = dag.get_children(&event.id);
        let finalized = matches!(event.status, setu_types::event::EventStatus::Finalized);

        Self {
            event_id: event.id.clone(),
            parents: event.parent_ids.clone(),
            children,
            depth,
            finalized,
        }
    }
}

/// DAG manager for maintaining the event graph
///
/// This is a wrapper around `consensus::Dag` that provides an async interface
/// and tracks the node ID for logging purposes.
pub struct DagManager {
    node_id: String,
    /// The underlying DAG from the consensus crate
    dag: Dag,
}

impl DagManager {
    /// Create a new DAG manager
    pub fn new(node_id: String) -> Self {
        info!(
            node_id = %node_id,
            "Creating DAG manager (using consensus::Dag)"
        );

        Self {
            node_id,
            dag: Dag::new(),
        }
    }

    /// Add an event to the DAG
    ///
    /// Returns the event ID on success, or an error if:
    /// - The event already exists (DuplicateEvent)
    /// - A parent event is missing (MissingParent)
    pub fn add_event(&mut self, event: Event) -> Result<EventId, DagManagerError> {
        info!(
            node_id = %self.node_id,
            event_id = %event.id,
            parent_count = event.parent_ids.len(),
            "Adding event to DAG"
        );

        match self.dag.add_event(event) {
            Ok(event_id) => {
                debug!(
                    event_id = %event_id,
                    total_nodes = self.dag.node_count(),
                    "Event added to DAG"
                );
                Ok(event_id)
            }
            Err(e) => {
                warn!(error = %e, "Failed to add event to DAG");
                Err(e)
            }
        }
    }

    /// Add an event to the DAG, treating duplicates as success (idempotent)
    ///
    /// This is useful for network scenarios where events may be received multiple times.
    pub fn add_event_idempotent(&mut self, event: Event) -> Result<Option<EventId>, DagManagerError> {
        match self.add_event(event) {
            Ok(id) => Ok(Some(id)),
            Err(DagError::DuplicateEvent(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get a node from the DAG
    pub fn get_node(&self, event_id: &EventId) -> Option<DagNode> {
        self.dag
            .get_event(event_id)
            .map(|event| DagNode::from_event(event, &self.dag))
    }

    /// Check if an event exists in the DAG
    pub fn contains(&self, event_id: &EventId) -> bool {
        self.dag.get_event(event_id).is_some()
    }

    /// Get all genesis events
    pub fn genesis_events(&self) -> Vec<EventId> {
        self.dag
            .get_events_at_depth(0)
            .into_iter()
            .map(|e| e.id.clone())
            .collect()
    }

    /// Get all tip events (events with no children)
    pub fn tips(&self) -> Vec<EventId> {
        self.dag.get_tips()
    }

    /// Get the total number of events in the DAG
    pub fn size(&self) -> usize {
        self.dag.node_count()
    }

    /// Get the maximum depth in the DAG
    pub fn max_depth(&self) -> u64 {
        self.dag.max_depth()
    }

    /// Check if event A happens before event B (causal ordering)
    pub fn happens_before(&self, event_a: &EventId, event_b: &EventId) -> bool {
        debug!(
            event_a = %event_a,
            event_b = %event_b,
            "Checking causal ordering"
        );
        self.dag.is_ancestor(event_a, event_b)
    }

    /// Mark an event as finalized
    pub fn finalize_event(&mut self, event_id: &EventId) -> Result<(), DagManagerError> {
        debug!(
            node_id = %self.node_id,
            event_id = %event_id,
            "Finalizing event"
        );

        if self.dag.confirm_event(event_id) {
            info!(event_id = %event_id, "Event finalized");
            Ok(())
        } else {
            Err(DagError::EventNotFound(event_id.clone()))
        }
    }

    /// Get statistics about the DAG
    pub fn stats(&self) -> DagStats {
        let finalized_count = self
            .dag
            .all_events()
            .filter(|e| matches!(e.status, setu_types::event::EventStatus::Finalized))
            .count();

        DagStats {
            total_events: self.dag.node_count(),
            genesis_count: self.genesis_events().len(),
            tip_count: self.dag.get_tips().len(),
            max_depth: self.dag.max_depth(),
            finalized_count,
        }
    }

    /// Get the underlying DAG (for advanced operations)
    pub fn inner(&self) -> &Dag {
        &self.dag
    }

    /// Get a mutable reference to the underlying DAG
    pub fn inner_mut(&mut self) -> &mut Dag {
        &mut self.dag
    }
}

/// Statistics about the DAG
#[derive(Debug, Clone)]
pub struct DagStats {
    pub total_events: usize,
    pub genesis_count: usize,
    pub tip_count: usize,
    pub max_depth: u64,
    pub finalized_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::event::{Event, EventType};
    use setu_vlc::VLCSnapshot;

    fn create_vlc_snapshot() -> VLCSnapshot {
        use setu_vlc::VectorClock;
        VLCSnapshot {
            vector_clock: VectorClock::new(),
            logical_time: 1,
            physical_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    fn create_genesis_event() -> Event {
        Event::genesis("solver-1".to_string(), create_vlc_snapshot())
    }

    fn create_child_event(parent_id: String) -> Event {
        Event::new(
            EventType::Transfer,
            vec![parent_id],
            create_vlc_snapshot(),
            "solver-1".to_string(),
        )
    }

    #[test]
    fn test_dag_creation() {
        let dag = DagManager::new("test-validator".to_string());
        assert_eq!(dag.size(), 0);
    }

    #[test]
    fn test_add_genesis_event() {
        let mut dag = DagManager::new("test-validator".to_string());
        let event = create_genesis_event();
        let event_id = event.id.clone();

        let result = dag.add_event(event);
        assert!(result.is_ok());
        assert_eq!(dag.size(), 1);
        assert_eq!(dag.genesis_events().len(), 1);
        assert!(dag.contains(&event_id));
    }

    #[test]
    fn test_add_child_event() {
        let mut dag = DagManager::new("test-validator".to_string());

        // Add genesis
        let genesis = create_genesis_event();
        let genesis_id = genesis.id.clone();
        dag.add_event(genesis).unwrap();

        // Add child
        let child = create_child_event(genesis_id.clone());
        let child_id = child.id.clone();
        dag.add_event(child).unwrap();

        assert_eq!(dag.size(), 2);

        let child_node = dag.get_node(&child_id).unwrap();
        assert_eq!(child_node.depth, 1);
    }

    #[test]
    fn test_happens_before() {
        let mut dag = DagManager::new("test-validator".to_string());

        // Create chain: e1 -> e2 -> e3
        let e1 = create_genesis_event();
        let e1_id = e1.id.clone();
        dag.add_event(e1).unwrap();

        let e2 = create_child_event(e1_id.clone());
        let e2_id = e2.id.clone();
        dag.add_event(e2).unwrap();

        let e3 = create_child_event(e2_id.clone());
        let e3_id = e3.id.clone();
        dag.add_event(e3).unwrap();

        // Test causal ordering
        assert!(dag.happens_before(&e1_id, &e2_id));
        assert!(dag.happens_before(&e1_id, &e3_id));
        assert!(dag.happens_before(&e2_id, &e3_id));

        // Test reverse (should be false)
        assert!(!dag.happens_before(&e2_id, &e1_id));
    }

    #[test]
    fn test_finalize_event() {
        let mut dag = DagManager::new("test-validator".to_string());
        let event = create_genesis_event();
        let event_id = event.id.clone();

        dag.add_event(event).unwrap();
        dag.finalize_event(&event_id).unwrap();

        // The event should be confirmed (finalize uses confirm_event internally)
        assert!(dag.contains(&event_id));
    }

    #[test]
    fn test_dag_stats() {
        let dag = DagManager::new("test-validator".to_string());
        let stats = dag.stats();

        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.genesis_count, 0);
        assert_eq!(stats.max_depth, 0);
    }
}
