// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! DAG (Directed Acyclic Graph) Module
//!
//! This module provides the DAG data structure for storing and managing events
//! in the Setu consensus protocol.

use setu_types::{Event, EventId, EventStatus};
use std::collections::{HashMap, HashSet, VecDeque};

/// DAG (Directed Acyclic Graph) for storing events
///
/// The DAG maintains:
/// - All events indexed by their ID
/// - Parent-child relationships
/// - Depth information for each event
/// - Tips (events with no children)
#[derive(Debug, Clone)]
pub struct Dag {
    /// All events in the DAG
    events: HashMap<EventId, Event>,
    
    /// Children of each event (event_id -> set of child event_ids)
    children: HashMap<EventId, HashSet<EventId>>,
    
    /// Depth of each event (distance from genesis)
    depths: HashMap<EventId, u64>,
    
    /// Current tips (events with no children)
    tips: HashSet<EventId>,
    
    /// Maximum depth in the DAG
    max_depth: u64,
    
    /// Events pending confirmation
    pending: HashSet<EventId>,
}

impl Dag {
    /// Create a new empty DAG
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
            children: HashMap::new(),
            depths: HashMap::new(),
            tips: HashSet::new(),
            max_depth: 0,
            pending: HashSet::new(),
        }
    }

    /// Add an event to the DAG
    ///
    /// Returns the event ID if successful
    pub fn add_event(&mut self, event: Event) -> Result<EventId, DagError> {
        let event_id = event.id.clone();

        // Check if event already exists
        if self.events.contains_key(&event_id) {
            return Err(DagError::DuplicateEvent(event_id));
        }

        // Calculate depth based on parents
        let depth = if event.parent_ids.is_empty() {
            0 // Genesis event
        } else {
            let mut max_parent_depth = 0u64;
            for parent_id in &event.parent_ids {
                if !self.events.contains_key(parent_id) {
                    return Err(DagError::MissingParent(parent_id.clone()));
                }
                let parent_depth = self.depths.get(parent_id).copied().unwrap_or(0);
                max_parent_depth = max_parent_depth.max(parent_depth);
            }
            max_parent_depth + 1
        };

        // Update children relationships and remove parents from tips
        for parent_id in &event.parent_ids {
            self.children
                .entry(parent_id.clone())
                .or_insert_with(HashSet::new)
                .insert(event_id.clone());
            self.tips.remove(parent_id);
        }

        // Store the event
        self.events.insert(event_id.clone(), event);
        self.depths.insert(event_id.clone(), depth);
        self.tips.insert(event_id.clone());
        self.pending.insert(event_id.clone());
        self.max_depth = self.max_depth.max(depth);

        Ok(event_id)
    }

    /// Get an event by ID
    pub fn get_event(&self, event_id: &EventId) -> Option<&Event> {
        self.events.get(event_id)
    }

    /// Check if an event exists in the DAG
    pub fn contains(&self, event_id: &EventId) -> bool {
        self.events.contains_key(event_id)
    }

    /// Get a mutable reference to an event
    pub fn get_event_mut(&mut self, event_id: &EventId) -> Option<&mut Event> {
        self.events.get_mut(event_id)
    }

    /// Get the depth of an event
    pub fn get_depth(&self, event_id: &EventId) -> Option<u64> {
        self.depths.get(event_id).copied()
    }

    /// Get all tips (events with no children)
    pub fn get_tips(&self) -> Vec<EventId> {
        self.tips.iter().cloned().collect()
    }

    /// Get the maximum depth in the DAG
    pub fn max_depth(&self) -> u64 {
        self.max_depth
    }

    /// Get the number of events in the DAG
    pub fn node_count(&self) -> usize {
        self.events.len()
    }

    /// Get the number of pending events
    pub fn get_pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Get events in a depth range [from_depth, to_depth]
    pub fn get_events_in_range(&self, from_depth: u64, to_depth: u64) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|(id, _)| {
                if let Some(&depth) = self.depths.get(*id) {
                    depth >= from_depth && depth <= to_depth
                } else {
                    false
                }
            })
            .map(|(_, event)| event)
            .collect()
    }

    /// Get all events at a specific depth
    pub fn get_events_at_depth(&self, depth: u64) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|(id, _)| self.depths.get(*id).copied() == Some(depth))
            .map(|(_, event)| event)
            .collect()
    }

    /// Mark an event as confirmed
    pub fn confirm_event(&mut self, event_id: &EventId) -> bool {
        if let Some(event) = self.events.get_mut(event_id) {
            event.status = EventStatus::Confirmed;
            self.pending.remove(event_id);
            true
        } else {
            false
        }
    }

    // =========================================================================
    // GC-Related Methods (for DagManager integration)
    // =========================================================================

    /// Batch update event status to Finalized
    ///
    /// MUST be called BEFORE GC! has_active_children() depends on status == Finalized
    pub fn finalize_events(&mut self, event_ids: &[EventId]) {
        for event_id in event_ids {
            if let Some(event) = self.events.get_mut(event_id) {
                event.status = EventStatus::Finalized;
            }
            self.pending.remove(event_id);
        }
    }

    /// Check if an event has active (non-finalized) children
    ///
    /// Returns true if any child event has status != Finalized
    pub fn has_active_children(&self, event_id: &EventId) -> bool {
        self.children
            .get(event_id)
            .map(|children| {
                children.iter().any(|child_id| {
                    self.events
                        .get(child_id)
                        .map(|e| e.status != EventStatus::Finalized)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// Add event with pre-calculated depth (skips parent existence check)
    ///
    /// WARNING: This method skips parent existence check because DagManager
    /// has already validated parents through three-layer query in Phase 1.
    /// Parents may be in Cache/Store rather than DAG.
    ///
    /// Used by: DagManager.add_event() Phase 3 write stage
    /// 
    /// # Visibility
    /// 
    /// This is `pub(crate)` to prevent external crates from bypassing
    /// DagManager's parent validation. All external callers MUST use
    /// DagManager.add_event() or add_event_with_retry().
    pub(crate) fn add_event_with_depth(
        &mut self,
        event: Event,
        depth: u64,
    ) -> Result<EventId, DagError> {
        let event_id = event.id.clone();

        // Check for duplicates
        if self.events.contains_key(&event_id) {
            return Err(DagError::DuplicateEvent(event_id));
        }

        // Update children relationships (only for parents still in DAG)
        // Note: Parents may have been GC'd (now in Cache/Store)
        for parent_id in &event.parent_ids {
            // Only update if parent is still in DAG
            if self.events.contains_key(parent_id) {
                self.children
                    .entry(parent_id.clone())
                    .or_insert_with(HashSet::new)
                    .insert(event_id.clone());
                // Remove from tips (no longer a leaf)
                self.tips.remove(parent_id);
            }
            // Parents in Cache/Store:
            // - Don't need to update children (GC'd events don't maintain children)
            // - This is expected behavior, cross-CF refs are safe via depth_diff limit
        }

        // Store the event
        self.events.insert(event_id.clone(), event);
        self.depths.insert(event_id.clone(), depth);
        self.tips.insert(event_id.clone());
        self.pending.insert(event_id.clone());
        self.max_depth = self.max_depth.max(depth);

        Ok(event_id)
    }

    /// Safely remove an event (only when no active children)
    ///
    /// Returns the removed event's parent_ids for optional cascading GC.
    ///
    /// Cascading GC note:
    ///   After E5 is removed, E5's parents (E3, E4) may become removable
    ///   (if they're also finalized with no other active children).
    ///   
    ///   Caller options:
    ///   - Ignore return: Simple mode, parents handled in next GC batch
    ///   - Recursive check: Call remove_event() on returned parent_ids
    ///   
    ///   Recommended: Simple mode to avoid stack overflow from recursion.
    ///   GC is progressive; leave for next anchor finalization.
    pub fn remove_event(&mut self, event_id: &EventId) -> Option<Vec<EventId>> {
        // Check for active children
        if self.has_active_children(event_id) {
            return None; // Has active children, cannot remove
        }

        let event = self.events.remove(event_id)?;
        self.depths.remove(event_id);
        self.tips.remove(event_id);
        self.pending.remove(event_id);

        // Remove self from parent's children lists
        for parent_id in &event.parent_ids {
            if let Some(children) = self.children.get_mut(parent_id) {
                children.remove(event_id);
                // Note: Don't automatically add parent to tips
                // Parent may be finalized, shouldn't become a tip
            }
        }

        // Remove own children index
        self.children.remove(event_id);

        Some(event.parent_ids)
    }

    /// Batch remove finalized events
    ///
    /// Removes events and cascadingly checks their parents for removal.
    /// This prevents memory leaks from retained parents.
    pub fn gc_finalized_events(&mut self, event_ids: &[EventId]) -> GCStats {
        let mut removed = 0;
        let mut retained = 0;
        
        let initial_targets: HashSet<EventId> = event_ids.iter().cloned().collect();
        let mut queue: VecDeque<EventId> = event_ids.iter().cloned().collect();

        while let Some(event_id) = queue.pop_front() {
            // Note: We do NOT track 'checked' events to prevent cycles, 
            // because a parent might fail to remove initially (due to active children),
            // but succeed later in the queue when those children are removed.
            // DAG acyclic property guarantees termination.

            match self.remove_event(&event_id) {
                Some(parent_ids) => {
                    removed += 1;
                    // Successfully removed, now check parents
                    for parent_id in parent_ids {
                        // Optimization: Only queue parent if it looks removable 
                        // (exists and is Finalized)
                        if let Some(parent) = self.events.get(&parent_id) {
                            if parent.status == EventStatus::Finalized {
                                queue.push_back(parent_id.clone());
                            }
                        }
                    }
                }
                None => {
                    // Count as retained only if it was one of the explicitly requested events
                    // AND it actually still exists in the DAG (has active children).
                    // If it returned None because it was already removed (e.g. duplicate),
                    // we shouldn't count it as retained.
                    if initial_targets.contains(&event_id) && self.events.contains_key(&event_id) {
                        retained += 1;
                    }
                }
            }
        }

        GCStats { removed, retained }
    }

    // =========================================================================
    // Existing Methods
    // =========================================================================

    /// Mark events up to a certain depth as finalized
    pub fn finalize_up_to_depth(&mut self, depth: u64) {
        for (id, event) in &mut self.events {
            if let Some(&d) = self.depths.get(id) {
                if d <= depth {
                    event.status = EventStatus::Finalized;
                    self.pending.remove(id);
                }
            }
        }
    }

    /// Get children of an event
    pub fn get_children(&self, event_id: &EventId) -> Vec<EventId> {
        self.children
            .get(event_id)
            .map(|c| c.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Check if an event is an ancestor of another
    pub fn is_ancestor(&self, ancestor_id: &EventId, descendant_id: &EventId) -> bool {
        if ancestor_id == descendant_id {
            return false;
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(descendant_id.clone());

        while let Some(current) = queue.pop_front() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            if let Some(event) = self.events.get(&current) {
                for parent_id in &event.parent_ids {
                    if parent_id == ancestor_id {
                        return true;
                    }
                    queue.push_back(parent_id.clone());
                }
            }
        }

        false
    }

    /// Get all ancestors of an event
    pub fn get_ancestors(&self, event_id: &EventId) -> HashSet<EventId> {
        let mut ancestors = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(event) = self.events.get(event_id) {
            for parent_id in &event.parent_ids {
                queue.push_back(parent_id.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            if ancestors.contains(&current) {
                continue;
            }
            ancestors.insert(current.clone());

            if let Some(event) = self.events.get(&current) {
                for parent_id in &event.parent_ids {
                    queue.push_back(parent_id.clone());
                }
            }
        }

        ancestors
    }

    /// Check if the DAG is empty
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Get all events
    pub fn all_events(&self) -> impl Iterator<Item = &Event> {
        self.events.values()
    }

    /// Get pending events
    pub fn pending_events(&self) -> Vec<&Event> {
        self.pending
            .iter()
            .filter_map(|id| self.events.get(id))
            .collect()
    }
}

impl Default for Dag {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur when working with the DAG
#[derive(Debug, Clone, thiserror::Error)]
pub enum DagError {
    #[error("Duplicate event: {0}")]
    DuplicateEvent(EventId),

    #[error("Missing parent event: {0}")]
    MissingParent(EventId),

    #[error("Event not found: {0}")]
    EventNotFound(EventId),

    #[error("Invalid event: {0}")]
    InvalidEvent(String),
}

/// Statistics from a GC operation
#[derive(Debug, Default, Clone)]
pub struct GCStats {
    /// Number of events removed from DAG
    pub removed: usize,
    /// Number of events retained (have active children)
    pub retained: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_vlc::VLCSnapshot;

    fn create_event(id: &str, parents: Vec<&str>, creator: &str) -> Event {
        let parent_ids: Vec<EventId> = parents.iter().map(|s| s.to_string()).collect();
        let mut event = Event::new(
            setu_types::EventType::Transfer,
            parent_ids,
            VLCSnapshot::new(),
            creator.to_string(),
        );
        // Override ID for testing
        event.id = id.to_string();
        event
    }

    #[test]
    fn test_dag_add_genesis() {
        let mut dag = Dag::new();
        let event = create_event("genesis", vec![], "node1");
        
        let result = dag.add_event(event);
        assert!(result.is_ok());
        assert_eq!(dag.node_count(), 1);
        assert_eq!(dag.max_depth(), 0);
    }

    #[test]
    fn test_dag_add_with_parent() {
        let mut dag = Dag::new();
        
        let genesis = create_event("genesis", vec![], "node1");
        dag.add_event(genesis).unwrap();
        
        let event1 = create_event("event1", vec!["genesis"], "node1");
        dag.add_event(event1).unwrap();
        
        assert_eq!(dag.node_count(), 2);
        assert_eq!(dag.max_depth(), 1);
        assert_eq!(dag.get_depth(&"event1".to_string()), Some(1));
    }

    #[test]
    fn test_dag_missing_parent() {
        let mut dag = Dag::new();
        
        let event = create_event("event1", vec!["missing"], "node1");
        let result = dag.add_event(event);
        
        assert!(matches!(result, Err(DagError::MissingParent(_))));
    }

    #[test]
    fn test_dag_tips() {
        let mut dag = Dag::new();
        
        let genesis = create_event("genesis", vec![], "node1");
        dag.add_event(genesis).unwrap();
        
        assert!(dag.get_tips().contains(&"genesis".to_string()));
        
        let event1 = create_event("event1", vec!["genesis"], "node1");
        dag.add_event(event1).unwrap();
        
        // genesis is no longer a tip
        assert!(!dag.get_tips().contains(&"genesis".to_string()));
        assert!(dag.get_tips().contains(&"event1".to_string()));
    }

    #[test]
    fn test_dag_is_ancestor() {
        let mut dag = Dag::new();
        
        let genesis = create_event("genesis", vec![], "node1");
        dag.add_event(genesis).unwrap();
        
        let event1 = create_event("event1", vec!["genesis"], "node1");
        dag.add_event(event1).unwrap();
        
        let event2 = create_event("event2", vec!["event1"], "node1");
        dag.add_event(event2).unwrap();
        
        assert!(dag.is_ancestor(&"genesis".to_string(), &"event2".to_string()));
        assert!(dag.is_ancestor(&"event1".to_string(), &"event2".to_string()));
        assert!(!dag.is_ancestor(&"event2".to_string(), &"genesis".to_string()));
    }

    #[test]
    fn test_dag_get_events_in_range() {
        let mut dag = Dag::new();
        
        let genesis = create_event("genesis", vec![], "node1");
        dag.add_event(genesis).unwrap();
        
        let event1 = create_event("event1", vec!["genesis"], "node1");
        dag.add_event(event1).unwrap();
        
        let event2 = create_event("event2", vec!["event1"], "node1");
        dag.add_event(event2).unwrap();
        
        let events = dag.get_events_in_range(0, 1);
        assert_eq!(events.len(), 2);
        
        let events = dag.get_events_in_range(1, 2);
        assert_eq!(events.len(), 2);
    }
}
