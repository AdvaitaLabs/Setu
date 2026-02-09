//! Per-subnet state management with SMT integration.
//!
//! This module provides the `GlobalStateManager` which manages Sparse Merkle Trees
//! for each subnet, enabling per-subnet state isolation and efficient state proofs.
//!
//! # Design
//!
//! Based on mkt-3.md:
//! - Each subnet maintains its own Object State SMT
//! - ROOT subnet (SubnetId=0) always exists
//! - All subnet roots are aggregated into a global state root
//! - State changes are batched and committed at each anchor

use setu_merkle::{
    HashValue, SparseMerkleTree, SparseMerkleProof,
    MerkleStore, SubnetAggregationTree, SubnetStateEntry,
};
use setu_types::{SubnetId, AnchorMerkleRoots};
use setu_types::event::{Event, StateChange, ExecutionResult};
use std::collections::HashMap;
use std::sync::Arc;
use sha2::{Sha256, Digest};

/// Manages Object State SMT for a single subnet
#[derive(Clone)]
pub struct SubnetStateSMT {
    /// The subnet this SMT belongs to
    subnet_id: SubnetId,
    /// The underlying Sparse Merkle Tree
    tree: SparseMerkleTree,
    /// Number of objects in this subnet
    object_count: u64,
    /// Last anchor where this subnet was updated
    last_updated_anchor: u64,
}

impl SubnetStateSMT {
    /// Create a new empty subnet state SMT
    pub fn new(subnet_id: SubnetId) -> Self {
        Self {
            subnet_id,
            tree: SparseMerkleTree::new(),
            object_count: 0,
            last_updated_anchor: 0,
        }
    }
    
    /// Get the subnet ID
    pub fn subnet_id(&self) -> SubnetId {
        self.subnet_id
    }
    
    /// Insert or update an object in the SMT
    /// Returns the new root hash
    pub fn upsert(&mut self, object_id: HashValue, value_hash: Vec<u8>) -> HashValue {
        let existing = self.tree.get(&object_id);
        if existing.is_none() {
            self.object_count += 1;
        }
        self.tree.insert(object_id, value_hash);
        self.tree.root()
    }
    
    /// Insert with raw 32-byte key and value
    pub fn upsert_raw(&mut self, object_id: [u8; 32], value: Vec<u8>) -> [u8; 32] {
        let key = HashValue::from_slice(&object_id).expect("valid 32-byte key");
        let root = self.upsert(key, value);
        *root.as_bytes()
    }
    
    /// Get an object's value hash from the SMT
    pub fn get(&self, object_id: &HashValue) -> Option<&Vec<u8>> {
        self.tree.get(object_id)
    }
    
    /// Delete an object from the SMT
    pub fn delete(&mut self, object_id: &HashValue) -> Option<Vec<u8>> {
        let removed = self.tree.remove(object_id);
        if removed.is_some() {
            self.object_count = self.object_count.saturating_sub(1);
        }
        removed
    }
    
    /// Get current state root
    pub fn root(&self) -> HashValue {
        self.tree.root()
    }
    
    /// Get root as raw bytes
    pub fn root_bytes(&self) -> [u8; 32] {
        *self.tree.root().as_bytes()
    }
    
    /// Generate inclusion/non-inclusion proof for an object
    pub fn prove(&self, object_id: &HashValue) -> SparseMerkleProof {
        self.tree.get_proof(object_id)
    }
    
    /// Batch update multiple objects
    pub fn batch_update(&mut self, updates: Vec<(HashValue, Vec<u8>)>) -> HashValue {
        for (object_id, value) in updates {
            self.upsert(object_id, value);
        }
        self.tree.root()
    }
    
    /// Get the number of objects in this subnet
    pub fn object_count(&self) -> u64 {
        self.object_count
    }
    
    /// Set the last updated anchor
    pub fn set_last_anchor(&mut self, anchor_id: u64) {
        self.last_updated_anchor = anchor_id;
    }
    
    /// Get the last updated anchor
    pub fn last_anchor(&self) -> u64 {
        self.last_updated_anchor
    }
    
    /// Check if the subnet is empty
    pub fn is_empty(&self) -> bool {
        self.object_count == 0
    }
}

/// Global state manager handling all subnets' SMTs.
///
/// This is the main interface for managing state across all subnets.
/// It maintains per-subnet SMTs and provides aggregation to a global root.
///
/// ## Clone Behavior
/// 
/// Clone creates a deep copy of all subnet SMTs. This is used for:
/// - Computing pending state roots without modifying the original state
/// - Temporary calculations in deferred commit mode
/// 
/// Note: The `store` field is not cloned (set to None in clones) since
/// cloned instances are for temporary calculations only.
pub struct GlobalStateManager {
    /// Per-subnet SMT instances
    subnet_states: HashMap<SubnetId, SubnetStateSMT>,
    /// Storage backend (optional, for persistence)
    /// Note: Not cloned - temporary clones don't need persistence
    #[allow(dead_code)]
    store: Option<Arc<dyn MerkleStore>>,
    /// Current anchor ID
    current_anchor: u64,
}

impl Clone for GlobalStateManager {
    fn clone(&self) -> Self {
        Self {
            subnet_states: self.subnet_states.clone(),
            store: None,  // Don't clone store - clones are for temporary calculations
            current_anchor: self.current_anchor,
        }
    }
}

impl GlobalStateManager {
    /// Create a new global state manager
    pub fn new() -> Self {
        let mut subnet_states = HashMap::new();
        // Always initialize ROOT subnet
        subnet_states.insert(SubnetId::ROOT, SubnetStateSMT::new(SubnetId::ROOT));
        
        Self {
            subnet_states,
            store: None,
            current_anchor: 0,
        }
    }
    
    /// Create with a storage backend for persistence
    pub fn with_store(store: Arc<dyn MerkleStore>) -> Self {
        let mut manager = Self::new();
        manager.store = Some(store);
        manager
    }
    
    /// Get or create a subnet's SMT
    pub fn get_subnet_mut(&mut self, subnet_id: SubnetId) -> &mut SubnetStateSMT {
        self.subnet_states
            .entry(subnet_id)
            .or_insert_with(|| SubnetStateSMT::new(subnet_id))
    }
    
    /// Get a subnet's SMT (read-only)
    pub fn get_subnet(&self, subnet_id: &SubnetId) -> Option<&SubnetStateSMT> {
        self.subnet_states.get(subnet_id)
    }
    
    /// Get ROOT subnet SMT
    pub fn root_subnet(&self) -> &SubnetStateSMT {
        self.subnet_states.get(&SubnetId::ROOT)
            .expect("ROOT subnet always exists")
    }
    
    /// Get ROOT subnet SMT (mutable)
    pub fn root_subnet_mut(&mut self) -> &mut SubnetStateSMT {
        self.get_subnet_mut(SubnetId::ROOT)
    }
    
    /// Get a subnet's current root hash
    pub fn get_subnet_root(&self, subnet_id: &SubnetId) -> Option<HashValue> {
        self.subnet_states.get(subnet_id).map(|s| s.root())
    }
    
    /// Get a subnet's current root as raw bytes
    pub fn get_subnet_root_bytes(&self, subnet_id: &SubnetId) -> Option<[u8; 32]> {
        self.subnet_states.get(subnet_id).map(|s| s.root_bytes())
    }
    
    /// Insert or update an object in a subnet
    pub fn upsert_object(
        &mut self,
        subnet_id: SubnetId,
        object_id: [u8; 32],
        value: Vec<u8>,
    ) -> [u8; 32] {
        self.get_subnet_mut(subnet_id).upsert_raw(object_id, value)
    }
    
    /// Compute global state root by aggregating all subnets
    pub fn compute_global_root(&self) -> (HashValue, HashMap<SubnetId, HashValue>) {
        let entries: Vec<SubnetStateEntry> = self.subnet_states
            .iter()
            .map(|(id, smt)| SubnetStateEntry::new(*id.as_bytes(), smt.root()))
            .collect();
        
        if entries.is_empty() {
            return (HashValue::zero(), HashMap::new());
        }
        
        let tree = SubnetAggregationTree::build(entries.clone());
        let global_root = tree.root();
        
        let subnet_roots: HashMap<SubnetId, HashValue> = self.subnet_states
            .iter()
            .map(|(id, smt)| (*id, smt.root()))
            .collect();
        
        (global_root, subnet_roots)
    }
    
    /// Compute global state root as raw bytes
    pub fn compute_global_root_bytes(&self) -> ([u8; 32], HashMap<SubnetId, [u8; 32]>) {
        let (global_root, subnet_roots) = self.compute_global_root();
        let subnet_roots_bytes: HashMap<SubnetId, [u8; 32]> = subnet_roots
            .into_iter()
            .map(|(k, v)| (k, *v.as_bytes()))
            .collect();
        (*global_root.as_bytes(), subnet_roots_bytes)
    }
    
    /// Build AnchorMerkleRoots from current state
    /// 
    /// Note: events_root and anchor_chain_root must be provided externally
    pub fn build_anchor_roots(
        &self,
        events_root: [u8; 32],
        anchor_chain_root: [u8; 32],
    ) -> AnchorMerkleRoots {
        let (global_root, subnet_roots) = self.compute_global_root_bytes();
        
        AnchorMerkleRoots {
            events_root,
            global_state_root: global_root,
            anchor_chain_root,
            subnet_roots,
        }
    }
    
    /// Commit current state for an anchor
    pub fn commit(&mut self, anchor_id: u64) -> setu_merkle::MerkleResult<()> {
        // Update last anchor for all subnets
        for smt in self.subnet_states.values_mut() {
            smt.set_last_anchor(anchor_id);
        }
        
        // Persist to storage if available
        if let Some(ref store) = self.store {
            for (subnet_id, smt) in &self.subnet_states {
                store.put_subnet_root(
                    subnet_id.as_bytes(),
                    anchor_id,
                    &smt.root(),
                )?;
            }
            
            let (global_root, _) = self.compute_global_root();
            store.put_global_root(anchor_id, &global_root)?;
        }
        
        self.current_anchor = anchor_id;
        Ok(())
    }
    
    /// Get the current anchor ID
    pub fn current_anchor(&self) -> u64 {
        self.current_anchor
    }
    
    /// Get all subnet IDs
    pub fn subnet_ids(&self) -> Vec<SubnetId> {
        self.subnet_states.keys().copied().collect()
    }
    
    /// Get the number of subnets (including ROOT)
    pub fn subnet_count(&self) -> usize {
        self.subnet_states.len()
    }
    
    /// Check if a subnet exists
    pub fn has_subnet(&self, subnet_id: &SubnetId) -> bool {
        self.subnet_states.contains_key(subnet_id)
    }
    
    /// Remove a subnet (cannot remove ROOT)
    pub fn remove_subnet(&mut self, subnet_id: &SubnetId) -> bool {
        if subnet_id.is_root() {
            return false; // Cannot remove ROOT
        }
        self.subnet_states.remove(subnet_id).is_some()
    }
    
    // =========================================================================
    // StateChange Application Methods (Connecting Solver Output to SMT)
    // =========================================================================
    
    /// Apply a single StateChange to a subnet's SMT
    ///
    /// This is called when processing Solver execution results.
    /// The key is hashed to create a 32-byte ObjectId for the SMT.
    pub fn apply_state_change(
        &mut self,
        subnet_id: SubnetId,
        change: &StateChange,
    ) -> ApplyResult {
        let object_id = Self::key_to_object_id(&change.key);
        let smt = self.get_subnet_mut(subnet_id);
        
        match &change.new_value {
            Some(value) => {
                // Insert or update
                let root = smt.upsert(object_id, value.clone());
                ApplyResult::Updated {
                    object_id: *object_id.as_bytes(),
                    new_root: *root.as_bytes(),
                }
            }
            None => {
                // Delete
                let removed = smt.delete(&object_id);
                ApplyResult::Deleted {
                    object_id: *object_id.as_bytes(),
                    existed: removed.is_some(),
                }
            }
        }
    }
    
    /// Apply all state changes from an ExecutionResult to a subnet
    ///
    /// Returns the new subnet root after applying all changes.
    pub fn apply_execution_result(
        &mut self,
        subnet_id: SubnetId,
        result: &ExecutionResult,
    ) -> [u8; 32] {
        for change in &result.state_changes {
            self.apply_state_change(subnet_id, change);
        }
        self.get_subnet_mut(subnet_id).root_bytes()
    }
    
    /// Apply all committed events' execution results to the state
    ///
    /// This is the main entry point called during Anchor creation.
    /// It processes all events, extracts their execution results,
    /// and applies the state changes to the appropriate subnet SMTs.
    ///
    /// # Event Ordering (Critical for Determinism)
    ///
    /// Events are sorted by VLC before applying state changes.
    /// This ensures all validators apply changes in the same order
    /// and arrive at the same final state root.
    /// Sort order: VLC.logical_time (ascending), then event_id (lexicographic)
    ///
    /// # Returns
    /// A summary of all state changes applied, grouped by subnet.
    pub fn apply_committed_events(
        &mut self,
        events: &[Event],
    ) -> StateApplySummary {
        let mut summary = StateApplySummary::new();
        
        // Sort events by VLC for deterministic ordering
        let mut sorted_events = events.to_vec();
        sorted_events.sort_by(|a, b| {
            // Primary sort by VLC logical_time
            match a.vlc_snapshot.logical_time.cmp(&b.vlc_snapshot.logical_time) {
                std::cmp::Ordering::Equal => {
                    // Tie-breaker: sort by event_id lexicographically
                    a.id.cmp(&b.id)
                }
                other => other,
            }
        });
        
        for event in &sorted_events {
            let subnet_id = event.get_subnet_id();
            
            if let Some(result) = &event.execution_result {
                if !result.success {
                    // Skip failed executions
                    summary.failed_events.push(event.id.clone());
                    continue;
                }
                
                let changes_count = result.state_changes.len();
                
                // Apply all state changes for this event
                let new_root = self.apply_execution_result(subnet_id, result);
                
                // Track in summary
                summary.record_event(
                    subnet_id,
                    &event.id,
                    changes_count,
                    new_root,
                );
            }
        }
        
        summary
    }
    
    /// Apply events and compute final anchor merkle roots
    ///
    /// This is the complete flow for Anchor creation:
    /// 1. Apply all event state changes
    /// 2. Compute global state root
    /// 3. Build AnchorMerkleRoots
    pub fn process_anchor(
        &mut self,
        events: &[Event],
        events_root: [u8; 32],
        anchor_chain_root: [u8; 32],
        anchor_id: u64,
    ) -> Result<(AnchorMerkleRoots, StateApplySummary), StateApplyError> {
        // Apply all state changes
        let summary = self.apply_committed_events(events);
        
        // Build anchor roots
        let anchor_roots = self.build_anchor_roots(events_root, anchor_chain_root);
        
        // Commit state
        self.commit(anchor_id)
            .map_err(|e| StateApplyError::CommitFailed(e.to_string()))?;
        
        Ok((anchor_roots, summary))
    }
    
    /// Convert a string key to a 32-byte ObjectId using SHA-256
    fn key_to_object_id(key: &str) -> HashValue {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let result = hasher.finalize();
        HashValue::from_slice(&result).expect("SHA-256 produces 32 bytes")
    }
}

/// Result of applying a single StateChange
#[derive(Debug, Clone)]
pub enum ApplyResult {
    Updated {
        object_id: [u8; 32],
        new_root: [u8; 32],
    },
    Deleted {
        object_id: [u8; 32],
        existed: bool,
    },
}

/// Error type for state application
#[derive(Debug, Clone)]
pub enum StateApplyError {
    CommitFailed(String),
    InvalidStateChange(String),
}

impl std::fmt::Display for StateApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateApplyError::CommitFailed(msg) => write!(f, "Commit failed: {}", msg),
            StateApplyError::InvalidStateChange(msg) => write!(f, "Invalid state change: {}", msg),
        }
    }
}

impl std::error::Error for StateApplyError {}

/// Summary of state changes applied during anchor processing
#[derive(Debug, Clone, Default)]
pub struct StateApplySummary {
    /// Changes per subnet: (event_count, total_changes, final_root)
    pub subnet_stats: HashMap<SubnetId, SubnetApplyStats>,
    /// Events that failed execution (skipped)
    pub failed_events: Vec<String>,
    /// Total events processed
    pub total_events: usize,
    /// Total state changes applied
    pub total_changes: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SubnetApplyStats {
    pub event_count: usize,
    pub change_count: usize,
    pub event_ids: Vec<String>,
    pub final_root: [u8; 32],
}

impl StateApplySummary {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn record_event(
        &mut self,
        subnet_id: SubnetId,
        event_id: &str,
        changes_count: usize,
        new_root: [u8; 32],
    ) {
        self.total_events += 1;
        self.total_changes += changes_count;
        
        let stats = self.subnet_stats.entry(subnet_id).or_default();
        stats.event_count += 1;
        stats.change_count += changes_count;
        stats.event_ids.push(event_id.to_string());
        stats.final_root = new_root;
    }
    
    pub fn subnets_updated(&self) -> usize {
        self.subnet_stats.len()
    }
}

impl Default for GlobalStateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_subnet_state_smt() {
        let mut smt = SubnetStateSMT::new(SubnetId::ROOT);
        assert!(smt.is_empty());
        
        let object_id = HashValue::from_slice(&[1u8; 32]).unwrap();
        let value = vec![2u8; 32];
        
        smt.upsert(object_id, value.clone());
        assert_eq!(smt.object_count(), 1);
        assert!(!smt.is_empty());
        
        assert_eq!(smt.get(&object_id), Some(&value));
        
        let root1 = smt.root();
        assert_ne!(root1, HashValue::zero());
        
        // Update same object
        let new_value = vec![3u8; 32];
        smt.upsert(object_id, new_value);
        assert_eq!(smt.object_count(), 1); // Count unchanged
        
        let root2 = smt.root();
        assert_ne!(root1, root2);
        
        // Delete object
        smt.delete(&object_id);
        assert!(smt.is_empty());
    }
    
    #[test]
    fn test_global_state_manager() {
        let mut manager = GlobalStateManager::new();
        
        // ROOT subnet always exists
        assert!(manager.has_subnet(&SubnetId::ROOT));
        assert_eq!(manager.subnet_count(), 1);
        
        // Add object to ROOT subnet
        let object_id = [1u8; 32];
        let value = vec![2u8; 32];
        manager.upsert_object(SubnetId::ROOT, object_id, value);
        
        // Create app subnet
        let app_subnet = SubnetId::from_str_id("my-app");
        manager.upsert_object(app_subnet, [3u8; 32], vec![4u8; 32]);
        
        assert_eq!(manager.subnet_count(), 2);
        
        // Compute global root
        let (global_root, subnet_roots) = manager.compute_global_root();
        assert_ne!(global_root, HashValue::zero());
        assert_eq!(subnet_roots.len(), 2);
        assert!(subnet_roots.contains_key(&SubnetId::ROOT));
        assert!(subnet_roots.contains_key(&app_subnet));
        
        // Build anchor roots
        let events_root = [5u8; 32];
        let anchor_chain_root = [6u8; 32];
        let anchor_roots = manager.build_anchor_roots(events_root, anchor_chain_root);
        
        assert_eq!(anchor_roots.events_root, events_root);
        assert_eq!(anchor_roots.global_state_root, *global_root.as_bytes());
        assert_eq!(anchor_roots.subnet_roots.len(), 2);
    }
    
    #[test]
    fn test_cannot_remove_root_subnet() {
        let mut manager = GlobalStateManager::new();
        assert!(!manager.remove_subnet(&SubnetId::ROOT));
        assert!(manager.has_subnet(&SubnetId::ROOT));
    }
    
    #[test]
    fn test_batch_update() {
        let mut smt = SubnetStateSMT::new(SubnetId::ROOT);
        
        let updates: Vec<(HashValue, Vec<u8>)> = (0..10)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i;
                (HashValue::from_slice(&key).unwrap(), vec![i; 32])
            })
            .collect();
        
        smt.batch_update(updates);
        assert_eq!(smt.object_count(), 10);
    }
}
