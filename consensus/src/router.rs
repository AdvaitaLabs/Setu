//! Event Router for Subnet-based Event Distribution
//!
//! This module implements the event routing logic as defined in mkt-3.md:
//! - Routes events to their target subnets based on subnet_id
//! - Separates ROOT subnet events (validator-executed) from App subnet events (solver/TEE-executed)
//! - Provides batch processing for efficient DAG folding

use setu_types::{Event, SubnetId};
use std::collections::HashMap;

/// Result of routing events by subnet
#[derive(Debug, Clone, Default)]
pub struct RoutedEvents {
    /// Events for the ROOT subnet (SubnetId=0), executed by validators
    pub root_events: Vec<Event>,
    
    /// Events for App subnets, keyed by SubnetId, executed by solvers/TEE
    pub app_events: HashMap<SubnetId, Vec<Event>>,
    
    /// Events without a specified subnet (legacy or malformed)
    pub unrouted_events: Vec<Event>,
}

impl RoutedEvents {
    /// Create a new empty RoutedEvents
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Total number of events across all categories
    pub fn total_count(&self) -> usize {
        self.root_events.len() 
            + self.app_events.values().map(|v| v.len()).sum::<usize>()
            + self.unrouted_events.len()
    }
    
    /// Get all subnet IDs that have events
    pub fn active_subnets(&self) -> Vec<SubnetId> {
        let mut subnets: Vec<SubnetId> = self.app_events.keys().cloned().collect();
        if !self.root_events.is_empty() {
            subnets.insert(0, SubnetId::ROOT);
        }
        subnets
    }
    
    /// Check if there are any ROOT subnet events
    pub fn has_root_events(&self) -> bool {
        !self.root_events.is_empty()
    }
    
    /// Check if there are any App subnet events
    pub fn has_app_events(&self) -> bool {
        !self.app_events.is_empty()
    }
}

/// Routes events to their target subnets based on event.subnet_id
///
/// According to mkt-3.md:
/// - ROOT subnet (SubnetId=0) events are executed by validators
/// - App subnet events are executed by solvers with TEE verification
pub struct EventRouter;

impl EventRouter {
    /// Route a batch of events by their subnet assignment
    ///
    /// # Arguments
    /// * `events` - The events to route
    ///
    /// # Returns
    /// A RoutedEvents struct containing events organized by subnet
    pub fn route_events(events: &[Event]) -> RoutedEvents {
        let mut result = RoutedEvents::new();
        
        for event in events {
            // First check if the event has an explicit subnet assignment
            if !event.has_subnet() {
                // Events without explicit subnet assignment go to unrouted
                result.unrouted_events.push(event.clone());
                continue;
            }
            
            let subnet_id = event.get_subnet_id();
            
            if event.is_validator_executed() || subnet_id.is_root() {
                result.root_events.push(event.clone());
            } else if subnet_id.is_app() {
                result.app_events
                    .entry(subnet_id)
                    .or_insert_with(Vec::new)
                    .push(event.clone());
            } else {
                // Events with unknown subnet type go to unrouted
                result.unrouted_events.push(event.clone());
            }
        }
        
        result
    }
    
    /// Route a single event to its target subnet
    pub fn route_single(event: &Event) -> SubnetId {
        event.get_subnet_id()
    }
    
    /// Check if an event should be executed by validators (ROOT subnet)
    pub fn is_validator_executed(event: &Event) -> bool {
        event.is_validator_executed()
    }
    
    /// Check if an event should be executed by solvers/TEE (App subnet)
    pub fn is_solver_executed(event: &Event) -> bool {
        let subnet_id = event.get_subnet_id();
        subnet_id.is_app()
    }
}

/// Execution context for a batch of events
#[derive(Debug, Clone)]
pub struct SubnetExecutionBatch {
    /// The target subnet
    pub subnet_id: SubnetId,
    
    /// Events to execute in this batch
    pub events: Vec<Event>,
    
    /// Whether this batch requires validator execution
    pub validator_executed: bool,
    
    /// Whether this batch requires TEE verification
    pub requires_tee: bool,
}

impl SubnetExecutionBatch {
    /// Create a new execution batch for a subnet
    pub fn new(subnet_id: SubnetId, events: Vec<Event>) -> Self {
        let validator_executed = subnet_id.is_system();
        let requires_tee = subnet_id.is_app();
        
        Self {
            subnet_id,
            events,
            validator_executed,
            requires_tee,
        }
    }
    
    /// Number of events in this batch
    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

/// Convert RoutedEvents into execution batches
pub fn create_execution_batches(routed: RoutedEvents) -> Vec<SubnetExecutionBatch> {
    let mut batches = Vec::new();
    
    // ROOT subnet batch
    if !routed.root_events.is_empty() {
        batches.push(SubnetExecutionBatch::new(
            SubnetId::ROOT,
            routed.root_events,
        ));
    }
    
    // App subnet batches
    for (subnet_id, events) in routed.app_events {
        batches.push(SubnetExecutionBatch::new(subnet_id, events));
    }
    
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::EventType;

    fn create_test_event(subnet_id: Option<SubnetId>) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            Default::default(),
            "test_node".to_string(),
        );
        if let Some(id) = subnet_id {
            event = event.with_subnet(id);
        }
        event
    }

    #[test]
    fn test_route_root_events() {
        let events = vec![
            create_test_event(Some(SubnetId::ROOT)),
            create_test_event(Some(SubnetId::ROOT)),
        ];
        
        let routed = EventRouter::route_events(&events);
        
        assert_eq!(routed.root_events.len(), 2);
        assert!(routed.app_events.is_empty());
        assert!(routed.unrouted_events.is_empty());
    }

    #[test]
    fn test_route_app_events() {
        let app_subnet = SubnetId::new_app_simple(123);
        let events = vec![
            create_test_event(Some(app_subnet)),
            create_test_event(Some(app_subnet)),
        ];
        
        let routed = EventRouter::route_events(&events);
        
        assert!(routed.root_events.is_empty());
        assert_eq!(routed.app_events.len(), 1);
        assert_eq!(routed.app_events.get(&app_subnet).unwrap().len(), 2);
    }

    #[test]
    fn test_route_mixed_events() {
        let app_subnet1 = SubnetId::new_app_simple(100);
        let app_subnet2 = SubnetId::new_app_simple(200);
        
        let events = vec![
            create_test_event(Some(SubnetId::ROOT)),
            create_test_event(Some(app_subnet1)),
            create_test_event(Some(app_subnet2)),
            create_test_event(None), // unrouted
        ];
        
        let routed = EventRouter::route_events(&events);
        
        assert_eq!(routed.root_events.len(), 1);
        assert_eq!(routed.app_events.len(), 2);
        assert_eq!(routed.unrouted_events.len(), 1);
        assert_eq!(routed.total_count(), 4);
    }

    #[test]
    fn test_create_execution_batches() {
        let app_subnet = SubnetId::new_app_simple(100);
        let mut routed = RoutedEvents::new();
        routed.root_events.push(create_test_event(Some(SubnetId::ROOT)));
        routed.app_events.insert(app_subnet, vec![
            create_test_event(Some(app_subnet)),
        ]);
        
        let batches = create_execution_batches(routed);
        
        assert_eq!(batches.len(), 2);
        
        // Find ROOT batch
        let root_batch = batches.iter().find(|b| b.subnet_id.is_root()).unwrap();
        assert!(root_batch.validator_executed);
        assert!(!root_batch.requires_tee);
        
        // Find App batch
        let app_batch = batches.iter().find(|b| b.subnet_id.is_app()).unwrap();
        assert!(!app_batch.validator_executed);
        assert!(app_batch.requires_tee);
    }
}
