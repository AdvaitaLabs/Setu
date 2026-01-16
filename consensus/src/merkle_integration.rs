//! Merkle Integration for Consensus
//!
//! This module provides Merkle tree computation during consensus:
//! - Events Binary Merkle Tree for event commitments
//! - Anchor Chain Binary Merkle Tree
//! - Integration with GlobalStateManager for state roots

use setu_merkle::{BinaryMerkleTree, HashValue as MerkleHash, SubnetAggregationTree, SubnetStateEntry};
use setu_types::{Anchor, AnchorMerkleRoots, Event, SubnetId, HashValue as TypesHash};
use std::collections::HashMap;

/// Convert MerkleHash to TypesHash ([u8; 32])
fn to_types_hash(h: MerkleHash) -> TypesHash {
    *h.as_bytes()
}

/// Computes the events Merkle root from a list of events
///
/// According to mkt-3.md, events are committed using a Binary Merkle Tree
/// where each leaf is the hash of an event.
pub fn compute_events_root(events: &[Event]) -> MerkleHash {
    if events.is_empty() {
        return MerkleHash::zero();
    }
    
    // Build tree directly from event ID bytes
    let leaves: Vec<&[u8]> = events
        .iter()
        .map(|e| e.id.as_bytes())
        .collect();
    
    let tree = BinaryMerkleTree::build(&leaves);
    tree.root()
}

/// Computes the anchor chain root from previous anchors
///
/// The anchor chain is an append-only Binary Merkle Tree
/// that provides historical integrity proofs.
pub fn compute_anchor_chain_root(anchors: &[&Anchor]) -> MerkleHash {
    if anchors.is_empty() {
        return MerkleHash::zero();
    }
    
    let leaves: Vec<&[u8]> = anchors
        .iter()
        .map(|a| a.id.as_bytes())
        .collect();
    
    let tree = BinaryMerkleTree::build(&leaves);
    tree.root()
}

/// Computes the anchor chain root from anchor hash history
///
/// This variant takes pre-computed anchor hashes instead of anchor references.
/// Useful when maintaining an append-only history of anchor hashes.
pub fn compute_anchor_chain_root_from_hashes(anchor_hashes: &[[u8; 32]]) -> [u8; 32] {
    if anchor_hashes.is_empty() {
        return [0u8; 32];
    }
    
    let leaves: Vec<&[u8]> = anchor_hashes
        .iter()
        .map(|h| h.as_slice())
        .collect();
    
    let tree = BinaryMerkleTree::build(&leaves);
    *tree.root().as_bytes()
}

/// Computes the global state root from subnet state roots
///
/// Uses SubnetAggregationTree to combine all subnet roots into a single
/// global state root.
pub fn compute_global_state_root(subnet_roots: &HashMap<SubnetId, MerkleHash>) -> MerkleHash {
    if subnet_roots.is_empty() {
        return MerkleHash::zero();
    }
    
    let entries: Vec<SubnetStateEntry> = subnet_roots
        .iter()
        .map(|(subnet_id, root)| {
            // Convert SubnetId to [u8; 32]
            let subnet_bytes = subnet_id.to_bytes();
            SubnetStateEntry::new(subnet_bytes, *root)
        })
        .collect();
    
    let tree = SubnetAggregationTree::build(entries);
    tree.root()
}

/// Builder for constructing AnchorMerkleRoots
pub struct AnchorMerkleRootsBuilder {
    events: Vec<Event>,
    previous_anchors: Vec<Anchor>,
    subnet_roots: HashMap<SubnetId, MerkleHash>,
    current_anchor_id: u64,
}

impl AnchorMerkleRootsBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            previous_anchors: Vec::new(),
            subnet_roots: HashMap::new(),
            current_anchor_id: 0,
        }
    }
    
    /// Set the events for this anchor
    pub fn with_events(mut self, events: Vec<Event>) -> Self {
        self.events = events;
        self
    }
    
    /// Set the previous anchor chain
    pub fn with_anchor_chain(mut self, anchors: Vec<Anchor>) -> Self {
        self.previous_anchors = anchors;
        self
    }
    
    /// Add a subnet state root
    pub fn with_subnet_root(mut self, subnet_id: SubnetId, root: MerkleHash) -> Self {
        self.subnet_roots.insert(subnet_id, root);
        self
    }
    
    /// Add multiple subnet state roots
    pub fn with_subnet_roots(mut self, roots: HashMap<SubnetId, MerkleHash>) -> Self {
        self.subnet_roots.extend(roots);
        self
    }
    
    /// Set the current anchor ID for metadata
    pub fn with_anchor_id(mut self, anchor_id: u64) -> Self {
        self.current_anchor_id = anchor_id;
        self
    }
    
    /// Build the AnchorMerkleRoots
    pub fn build(self) -> AnchorMerkleRoots {
        let events_root = compute_events_root(&self.events);
        
        let anchor_refs: Vec<&Anchor> = self.previous_anchors.iter().collect();
        let anchor_chain_root = compute_anchor_chain_root(&anchor_refs);
        
        let global_state_root = compute_global_state_root(&self.subnet_roots);
        
        // Convert subnet roots to types hash format
        let subnet_roots_converted: HashMap<SubnetId, TypesHash> = self.subnet_roots
            .into_iter()
            .map(|(k, v)| (k, to_types_hash(v)))
            .collect();
        
        AnchorMerkleRoots {
            events_root: to_types_hash(events_root),
            global_state_root: to_types_hash(global_state_root),
            anchor_chain_root: to_types_hash(anchor_chain_root),
            subnet_roots: subnet_roots_converted,
        }
    }
}

impl Default for AnchorMerkleRootsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{EventType, VLCSnapshot};
    use setu_merkle::sha256;

    fn create_test_event(id: &str) -> Event {
        let mut event = Event::new(
            EventType::Transfer,
            vec![],
            VLCSnapshot::default(),
            "test".to_string(),
        );
        // Override the auto-generated ID for testing
        event.id = id.to_string();
        event
    }

    #[test]
    fn test_compute_events_root_empty() {
        let root = compute_events_root(&[]);
        assert_eq!(root, MerkleHash::zero());
    }

    #[test]
    fn test_compute_events_root_single() {
        let events = vec![create_test_event("event1")];
        let root = compute_events_root(&events);
        assert_ne!(root, MerkleHash::zero());
    }

    #[test]
    fn test_compute_events_root_multiple() {
        let events = vec![
            create_test_event("event1"),
            create_test_event("event2"),
            create_test_event("event3"),
        ];
        let root = compute_events_root(&events);
        assert_ne!(root, MerkleHash::zero());
    }

    #[test]
    fn test_anchor_merkle_roots_builder() {
        let events = vec![create_test_event("event1")];
        
        let roots = AnchorMerkleRootsBuilder::new()
            .with_events(events)
            .with_subnet_root(SubnetId::ROOT, sha256(b"state1"))
            .build();
        
        assert_ne!(roots.events_root, setu_types::ZERO_HASH);
        assert_ne!(roots.global_state_root, setu_types::ZERO_HASH);
    }

    #[test]
    fn test_global_state_root_aggregation() {
        let mut subnet_roots = HashMap::new();
        subnet_roots.insert(SubnetId::ROOT, sha256(b"root_state"));
        subnet_roots.insert(SubnetId::new_app_simple(100), sha256(b"app_state"));
        
        let global_root = compute_global_state_root(&subnet_roots);
        assert_ne!(global_root, MerkleHash::zero());
    }
}
