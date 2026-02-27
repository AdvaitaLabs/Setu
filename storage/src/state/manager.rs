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
//!
//! # B4 Scheme: Batch Delayed Persistence
//!
//! The B4 scheme delays SMT leaf persistence until Anchor commit:
//! - Dirty leaves are tracked during transaction execution
//! - At Anchor commit, all changes are persisted atomically via WriteBatch
//! - Recovery reconstructs SMT from persisted leaf data

use setu_merkle::{
    HashValue, IncrementalSparseMerkleTree, LeafChanges, SparseMerkleProof,
    B4Store, MerkleStore,
    SubnetAggregationTree, SubnetStateEntry,
};
use setu_types::{SubnetId, AnchorMerkleRoots};
use setu_types::event::{Event, StateChange, ExecutionResult};
use setu_types::coin::CoinState;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use sha2::{Sha256, Digest};

/// Manages Object State SMT for a single subnet
#[derive(Clone)]
pub struct SubnetStateSMT {
    /// The subnet this SMT belongs to
    subnet_id: SubnetId,
    /// The underlying Sparse Merkle Tree (incremental O(log N) updates)
    tree: IncrementalSparseMerkleTree,
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
            tree: IncrementalSparseMerkleTree::new(),
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

    /// Iterate over all objects in this subnet.
    ///
    /// Returns an iterator of (object_id_bytes, value_bytes).
    /// Useful for rebuilding indexes at startup.
    pub fn iter_objects(&self) -> impl Iterator<Item = ([u8; 32], &Vec<u8>)> {
        self.tree.iter_leaves().map(|(k, v)| (*k.as_bytes(), v))
    }

    /// Get all objects as a vector of (object_id_bytes, value_bytes).
    pub fn all_objects(&self) -> Vec<([u8; 32], Vec<u8>)> {
        self.tree.iter_leaves().map(|(k, v)| (*k.as_bytes(), v.clone())).collect()
    }

    // =========================================================================
    // B4 Scheme: Dirty Data Tracking
    // =========================================================================

    /// Check if there are uncommitted changes in this subnet's SMT.
    pub fn has_pending_changes(&self) -> bool {
        self.tree.has_pending_changes()
    }

    /// Take all pending changes from the SMT (clears dirty tracking).
    /// 
    /// Returns `LeafChanges` containing:
    /// - `upserts`: All leaves that were inserted or updated
    /// - `deletes`: All keys that were deleted
    pub fn take_changes(&mut self) -> LeafChanges {
        self.tree.take_changes()
    }

    /// Get the number of pending upserts.
    pub fn pending_upsert_count(&self) -> usize {
        self.tree.pending_upsert_count()
    }

    /// Get the number of pending deletes.
    pub fn pending_delete_count(&self) -> usize {
        self.tree.pending_delete_count()
    }

    /// Reconstruct this SMT from persisted leaf data (B4 recovery).
    ///
    /// This creates a new SMT from a HashMap of (object_id -> value),
    /// typically loaded from the MerkleLeaves column family.
    pub fn from_persisted_leaves(subnet_id: SubnetId, leaves: HashMap<HashValue, Vec<u8>>) -> Self {
        let tree = IncrementalSparseMerkleTree::from_leaves(leaves);
        let object_count = tree.leaf_count() as u64;
        
        Self {
            subnet_id,
            tree,
            object_count,
            last_updated_anchor: 0,
        }
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
///
/// ## B4 Scheme: Batch Delayed Persistence
///
/// The manager uses the B4 scheme for SMT persistence:
/// - State changes are tracked as dirty during execution
/// - All dirty leaves are persisted atomically at Anchor commit
/// - Recovery reconstructs SMT from persisted leaf data
pub struct GlobalStateManager {
    /// Per-subnet SMT instances
    subnet_states: HashMap<SubnetId, SubnetStateSMT>,
    /// Storage backend implementing B4Store for atomic batch persistence
    #[allow(dead_code)]
    store: Option<Arc<dyn B4StoreExt>>,
    /// Current anchor ID
    current_anchor: u64,
    /// Coin type index: address -> set of subnet_ids (coin types)
    /// 
    /// This index is updated synchronously when state changes are applied,
    /// enabling efficient coin queries by address. The index maps owner
    /// addresses to the set of subnet IDs where they have coins.
    coin_type_index: HashMap<String, HashSet<String>>,
    /// Owner coin index: owner_address -> set of (object_id, subnet_id) pairs
    /// 
    /// This index tracks which object IDs belong to each owner, enabling
    /// efficient coin lookups even for coins created by runtime split operations
    /// (whose object_ids are not deterministic).
    owner_coin_index: HashMap<String, HashSet<([u8; 32], String)>>,
    /// Modification tracker: object_id -> last modifying event_id
    /// 
    /// Updated during apply_committed_events to track which event last modified
    /// each object. Used by TaskPreparer to derive DAG parent_ids for causal ordering.
    modification_tracker: HashMap<[u8; 32], String>,
}

/// Extended B4Store trait that combines all required storage capabilities.
///
/// The B4 scheme requires:
/// - B4Store: For atomic batch operations (WriteBatch support)
/// - MerkleStore: For persisting tree nodes and roots
pub trait B4StoreExt: B4Store + MerkleStore {}

// Blanket implementation for any type implementing all required traits
impl<T: B4Store + MerkleStore> B4StoreExt for T {}

/// ⚠️ **WARNING**: Cloning `GlobalStateManager` clears all dirty tracking!
///
/// The cloned instance:
/// - Contains a snapshot of all SMT data (leaves, nodes, roots)
/// - Has `store` set to `None` (cannot commit)
/// - Has empty dirty/deleted tracking (pending changes are lost)
///
/// **Safe use cases**:
/// - Computing state roots without modifying original state
/// - Temporary calculations for validation
/// - Read-only operations
///
/// **Unsafe use case** (will lose data):
/// ```ignore
/// let mut manager = GlobalStateManager::new();
/// manager.upsert_object(subnet, key, value);  // Marks as dirty
/// let cloned = manager.clone();               // Dirty tracking lost!
/// // cloned.commit() would NOT persist the upsert
/// ```
impl Clone for GlobalStateManager {
    fn clone(&self) -> Self {
        Self {
            subnet_states: self.subnet_states.clone(),
            store: None,  // Don't clone store - clones are for temporary calculations
            current_anchor: self.current_anchor,
            // Don't clone indices - clones are for temporary state root calculations only
            coin_type_index: HashMap::new(),
            owner_coin_index: HashMap::new(),
            modification_tracker: HashMap::new(),
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
            coin_type_index: HashMap::new(),
            owner_coin_index: HashMap::new(),
            modification_tracker: HashMap::new(),
        }
    }
    
    /// Create with a storage backend for persistence (B4 scheme).
    ///
    /// The store must implement B4Store + MerkleStore.
    pub fn with_store(store: Arc<dyn B4StoreExt>) -> Self {
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
    
    /// Commit current state for an anchor (B4 scheme: atomic batch persistence).
    ///
    /// This method:
    /// 1. Collects all dirty leaves from all subnets' SMTs
    /// 2. Persists leaves + roots + metadata atomically via single WriteBatch
    /// 3. Updates internal anchor tracking
    ///
    /// ## Atomicity Guarantee
    ///
    /// The B4 scheme ensures that either all changes are persisted or none:
    /// - All dirty leaves are written to MerkleLeaves CF
    /// - All deleted keys are removed from MerkleLeaves CF  
    /// - All subnet roots are written to MerkleRoots CF
    /// - Global root is written to MerkleRoots CF
    /// - Metadata (last anchor, subnet registry) is updated
    ///
    /// All operations use a **single WriteBatch** to guarantee atomicity.
    pub fn commit(&mut self, anchor_id: u64) -> setu_merkle::MerkleResult<()> {
        // Update last anchor for all subnets
        for smt in self.subnet_states.values_mut() {
            smt.set_last_anchor(anchor_id);
        }
        
        // Persist to storage if available (B4 scheme)
        if let Some(ref store) = self.store {
            // ⭐ Create a SINGLE WriteBatch for all operations
            let mut batch = store.begin_batch()?;
            
            // Phase 1: Persist all dirty leaves for each subnet
            for (subnet_id, smt) in &mut self.subnet_states {
                if smt.has_pending_changes() {
                    let changes = smt.take_changes();
                    
                    // Batch put upserted leaves (into WriteBatch, not committed yet)
                    if !changes.upserts.is_empty() {
                        let upserts: Vec<_> = changes.upserts
                            .iter()
                            .map(|(k, v)| (k, v.as_slice()))
                            .collect();
                        store.batch_put_leaves_to_batch(
                            &mut batch,
                            subnet_id.as_bytes(),
                            &upserts,
                        )?;
                    }
                    
                    // Batch delete removed leaves (into WriteBatch, not committed yet)
                    if !changes.deletes.is_empty() {
                        let deletes: Vec<_> = changes.deletes.iter().collect();
                        store.batch_delete_leaves_to_batch(
                            &mut batch,
                            subnet_id.as_bytes(),
                            &deletes,
                        )?;
                    }
                }
                
                // Register subnet if it has data (into WriteBatch)
                if smt.object_count() > 0 {
                    store.batch_register_subnet(&mut batch, subnet_id.as_bytes())?;
                }
                
                // Store subnet root (into WriteBatch)
                store.batch_put_subnet_root(
                    &mut batch,
                    subnet_id.as_bytes(),
                    anchor_id,
                    &smt.root(),
                )?;
                
                // Store last anchor for subnet (into WriteBatch)
                store.batch_set_last_anchor(&mut batch, subnet_id.as_bytes(), anchor_id)?;
            }
            
            // Phase 2: Store global root (into WriteBatch)
            let (global_root, _) = self.compute_global_root();
            store.batch_put_global_root(&mut batch, anchor_id, &global_root)?;
            
            // ⭐ Atomic commit: all or nothing
            store.commit_batch(batch)?;
            
            tracing::debug!(
                anchor_id,
                subnet_count = self.subnet_states.len(),
                "B4 commit completed atomically"
            );
        }
        
        self.current_anchor = anchor_id;
        Ok(())
    }
    
    /// Recover state from persisted data (B4 scheme: startup recovery).
    ///
    /// This method reconstructs all subnet SMTs from persisted leaf data.
    /// It should be called during node startup before processing any new events.
    ///
    /// ## Recovery Process
    ///
    /// 1. List all registered subnets from MerkleMeta
    /// 2. For each subnet, load all leaves from MerkleLeaves CF
    /// 3. Reconstruct SMT from leaves using `IncrementalSparseMerkleTree::from_leaves()`
    /// 4. Verify reconstructed root matches persisted root (consistency check)
    /// 5. Restore last anchor info from MerkleMeta
    ///
    /// ## Returns
    ///
    /// Returns a `RecoverySummary` with statistics about the recovery.
    pub fn recover(&mut self) -> setu_merkle::MerkleResult<RecoverySummary> {
        let store = match &self.store {
            Some(s) => s,
            None => {
                tracing::warn!("recover() called without storage backend");
                return Ok(RecoverySummary::default());
            }
        };
        
        let mut summary = RecoverySummary::default();
        
        // List all registered subnets
        let subnet_ids = store.list_registered_subnets()?;
        tracing::info!(subnet_count = subnet_ids.len(), "Recovering subnets from storage");
        
        for subnet_id_bytes in subnet_ids {
            let subnet_id = SubnetId::new(subnet_id_bytes);
            
            // Load all leaves for this subnet
            let leaves = store.load_all_leaves(&subnet_id_bytes)?;
            let leaf_count = leaves.len();
            
            if leaf_count == 0 {
                tracing::debug!(?subnet_id, "Skipping empty subnet");
                continue;
            }
            
            // Reconstruct SMT from leaves
            let mut smt = SubnetStateSMT::from_persisted_leaves(subnet_id, leaves);
            
            // Restore last anchor
            let mut last_anchor = 0u64;
            if let Some(anchor) = store.get_last_anchor(&subnet_id_bytes)? {
                last_anchor = anchor;
                smt.set_last_anchor(anchor);
                if anchor > self.current_anchor {
                    self.current_anchor = anchor;
                }
            }
            
            // ⭐ P1-6: Verify root hash consistency
            if last_anchor > 0 {
                if let Some(expected_root) = store.get_subnet_root(&subnet_id_bytes, last_anchor)? {
                    let actual_root = smt.root();
                    if actual_root != expected_root {
                        tracing::error!(
                            ?subnet_id,
                            expected = %expected_root,
                            actual = %actual_root,
                            "Root hash mismatch during recovery!"
                        );
                        return Err(setu_merkle::MerkleError::ConsistencyError(format!(
                            "Root hash mismatch for subnet {:?}: expected {}, got {}",
                            subnet_id, expected_root, actual_root
                        )));
                    }
                    tracing::debug!(
                        ?subnet_id,
                        root = %actual_root,
                        "Root hash verified"
                    );
                }
            }
            
            tracing::debug!(
                ?subnet_id,
                leaf_count,
                root = %smt.root(),
                "Recovered subnet SMT"
            );
            
            summary.subnets_recovered += 1;
            summary.total_leaves += leaf_count;
            
            self.subnet_states.insert(subnet_id, smt);
        }
        
        // Ensure ROOT subnet exists
        if !self.subnet_states.contains_key(&SubnetId::ROOT) {
            self.subnet_states.insert(SubnetId::ROOT, SubnetStateSMT::new(SubnetId::ROOT));
        }
        
        tracing::info!(
            subnets = summary.subnets_recovered,
            leaves = summary.total_leaves,
            anchor = self.current_anchor,
            "Recovery complete"
        );
        
        Ok(summary)
    }

    /// Check if any subnet has uncommitted changes.
    pub fn has_pending_changes(&self) -> bool {
        self.subnet_states.values().any(|smt| smt.has_pending_changes())
    }

    /// Get total pending changes count across all subnets.
    pub fn total_pending_changes(&self) -> (usize, usize) {
        let mut upserts = 0;
        let mut deletes = 0;
        for smt in self.subnet_states.values() {
            upserts += smt.pending_upsert_count();
            deletes += smt.pending_delete_count();
        }
        (upserts, deletes)
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

    /// Iterate over all objects across all subnets.
    ///
    /// Returns an iterator of (subnet_id, object_id_bytes, value_bytes).
    /// Useful for rebuilding indexes at startup.
    pub fn iter_all_objects(&self) -> impl Iterator<Item = (SubnetId, [u8; 32], &Vec<u8>)> {
        self.subnet_states.iter().flat_map(|(subnet_id, smt)| {
            smt.iter_objects().map(move |(obj_id, value)| (*subnet_id, obj_id, value))
        })
    }

    /// Get all objects across all subnets as a vector.
    pub fn all_objects(&self) -> Vec<(SubnetId, [u8; 32], Vec<u8>)> {
        self.iter_all_objects()
            .map(|(subnet_id, obj_id, value)| (subnet_id, obj_id, value.clone()))
            .collect()
    }

    // =========================================================================
    // Coin Type Index Methods
    // =========================================================================

    /// Get all coin types (subnet IDs) for an address.
    /// 
    /// This is used by MerkleStateProvider to efficiently query coins
    /// without scanning the entire SMT.
    /// 
    /// # Returns
    /// Set of subnet IDs where the address has coins.
    pub fn get_coin_types_for_address(&self, address: &str) -> HashSet<String> {
        self.coin_type_index
            .get(address)
            .cloned()
            .unwrap_or_default()
    }

    /// Register a coin type for an address (manual registration).
    /// 
    /// This is primarily used during initialization (e.g., genesis, tests).
    /// During normal operation, apply_state_change() handles index updates.
    pub fn register_coin_type(&mut self, address: &str, coin_type: &str) {
        self.coin_type_index
            .entry(address.to_string())
            .or_default()
            .insert(coin_type.to_string());
    }
    
    /// Register a coin object for an address (includes object_id tracking).
    /// 
    /// This tracks both the coin type and the specific object_id,
    /// enabling efficient lookup of coins even after runtime split operations.
    pub fn register_coin_object(&mut self, address: &str, coin_type: &str, object_id: [u8; 32]) {
        self.register_coin_type(address, coin_type);
        self.owner_coin_index
            .entry(address.to_string())
            .or_default()
            .insert((object_id, coin_type.to_string()));
    }
    
    /// Get all object IDs owned by an address
    /// 
    /// Returns (object_id, coin_type) pairs for all coins owned by the address.
    pub fn get_coin_objects_for_address(&self, address: &str) -> Vec<([u8; 32], String)> {
        self.owner_coin_index
            .get(address)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Rebuild the coin_type_index by scanning all objects in all SMTs.
    /// 
    /// This should be called at startup after recovering SMT state from storage.
    /// It ensures the index is consistent with the actual state.
    /// 
    /// # Performance
    /// - O(n) where n is total number of objects
    /// - ~1 second per 1M objects (in-memory scan)
    /// 
    /// # Returns
    /// Number of coins indexed.
    pub fn rebuild_coin_type_index(&mut self) -> usize {
        self.coin_type_index.clear();
        self.owner_coin_index.clear();
        
        // First collect all coin data to avoid borrow conflicts
        let coin_data: Vec<(String, String, [u8; 32])> = self.iter_all_objects()
            .filter_map(|(_subnet_id, object_id, value)| {
                CoinState::from_bytes(value)
                    .map(|cs| (cs.owner.clone(), cs.coin_type.clone(), object_id))
            })
            .collect();
        
        // Then update the indices
        for (owner, coin_type, object_id) in coin_data {
            self.coin_type_index
                .entry(owner.clone())
                .or_default()
                .insert(coin_type.clone());
            
            self.owner_coin_index
                .entry(owner)
                .or_default()
                .insert((object_id, coin_type));
        }
        
        self.coin_type_index.values().map(|v| v.len()).sum()
    }

    /// Get index statistics for debugging/monitoring.
    /// 
    /// # Returns
    /// (number of addresses, total coin type entries)
    pub fn coin_type_index_stats(&self) -> (usize, usize) {
        let address_count = self.coin_type_index.len();
        let total_entries: usize = self.coin_type_index.values().map(|v| v.len()).sum();
        (address_count, total_entries)
    }

    // =========================================================================
    // Modification Tracking (object_id → last modifying event_id)
    // =========================================================================

    /// Get the last event that modified a given object
    pub fn get_last_modifying_event(&self, object_id: &[u8; 32]) -> Option<&String> {
        self.modification_tracker.get(object_id)
    }

    /// Record that an event modified specific objects
    /// 
    /// This is called during genesis initialization and after state changes
    /// are applied via apply_committed_events.
    pub fn record_modification(&mut self, event_id: &str, object_id: [u8; 32]) {
        self.modification_tracker.insert(object_id, event_id.to_string());
    }
    
    // =========================================================================
    // StateChange Application Methods (Connecting Solver Output to SMT)
    // =========================================================================
    
    /// Apply a single StateChange to a subnet's SMT
    ///
    /// This is called when processing Solver execution results.
    /// 
    /// ## Key Format Support
    /// 
    /// - `"oid:{hex}"`: Direct ObjectId hex (from TEE output) → decode directly
    /// - Other formats: Hash the key to create a 32-byte ObjectId (legacy)
    /// 
    /// The "oid:" prefix allows TEE outputs to specify exact SMT keys,
    /// ensuring state changes are applied to the correct objects.
    pub fn apply_state_change(
        &mut self,
        subnet_id: SubnetId,
        change: &StateChange,
    ) -> ApplyResult {
        let object_id = Self::parse_state_change_key(&change.key);
        let smt = self.get_subnet_mut(subnet_id);
        
        match &change.new_value {
            Some(value) => {
                // Insert or update
                let root = smt.upsert(object_id, value.clone());
                
                // Update coin_type_index and owner_coin_index if this is a CoinState
                // Parse the BCS-serialized CoinState to extract owner and coin_type
                if let Some(coin_state) = CoinState::from_bytes(value) {
                    self.coin_type_index
                        .entry(coin_state.owner.clone())
                        .or_default()
                        .insert(coin_state.coin_type.clone());
                    
                    // Track owner → object_id mapping
                    self.owner_coin_index
                        .entry(coin_state.owner.clone())
                        .or_default()
                        .insert((*object_id.as_bytes(), coin_state.coin_type.clone()));
                }
                
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
        
        'event_loop: for event in &sorted_events {
            let subnet_id = event.get_subnet_id();
            
            if let Some(result) = &event.execution_result {
                if !result.success {
                    // Skip failed executions
                    summary.failed_events.push(event.id.clone());
                    continue;
                }
                
                // Conflict detection: verify old_value matches current SMT state
                // This prevents double-spend where two events read the same stale state.
                // If any state_change has an old_value that doesn't match current state,
                // the event's read_set was stale → skip entire event.
                let smt = self.get_subnet_mut(subnet_id);
                for change in &result.state_changes {
                    if let Some(ref expected_old) = change.old_value {
                        let object_id = Self::parse_state_change_key(&change.key);
                        let current_value = smt.get(&object_id).cloned();
                        if current_value.as_ref() != Some(expected_old) {
                            // Stale read detected: current state differs from what
                            // the Solver saw when it executed this event.
                            // This is the double-spend safety net.
                            tracing::warn!(
                                event_id = %event.id,
                                key = %change.key,
                                "Conflict detected: old_value mismatch, skipping event (stale read)"
                            );
                            summary.conflicted_events.push(event.id.clone());
                            continue 'event_loop;
                        }
                    }
                }
                
                let changes_count = result.state_changes.len();
                
                // Apply all state changes for this event
                let new_root = self.apply_execution_result(subnet_id, result);
                
                // Update modification_tracker: record event_id for each modified object
                for change in &result.state_changes {
                    let object_id = Self::parse_state_change_key(&change.key);
                    self.modification_tracker.insert(*object_id.as_bytes(), event.id.clone());
                }
                
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
    
    /// Parse a StateChange key to extract the ObjectId for SMT storage
    /// 
    /// ## Supported Formats
    /// 
    /// - `"oid:{hex}"`: Direct ObjectId hex from TEE output → decode directly
    /// - Other: Hash the key with SHA-256 (legacy fallback)
    /// 
    /// ## Example
    /// 
    /// ```ignore
    /// // TEE output key (new format)
    /// parse_state_change_key("oid:abcd1234...") → HashValue([0xab, 0xcd, ...])
    /// 
    /// // Legacy key (hashed)
    /// parse_state_change_key("event:some-id") → SHA256("event:some-id")
    /// ```
    fn parse_state_change_key(key: &str) -> HashValue {
        if let Some(hex_str) = key.strip_prefix("oid:") {
            // Direct ObjectId hex → decode to bytes
            if let Ok(bytes) = hex::decode(hex_str) {
                if bytes.len() == 32 {
                    return HashValue::from_slice(&bytes).expect("32 bytes");
                }
            }
            // If decode fails, fall through to SHA256
            tracing::warn!(
                key = %key,
                "Invalid oid: format, falling back to SHA256"
            );
        }
        // Legacy: hash the key
        Self::key_to_object_id(key)
    }
    
    /// Convert a string key to a 32-byte ObjectId using SHA-256 (legacy)
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

/// Summary of B4 recovery operation
#[derive(Debug, Clone, Default)]
pub struct RecoverySummary {
    /// Number of subnets recovered
    pub subnets_recovered: usize,
    /// Total number of leaves loaded
    pub total_leaves: usize,
}

/// Summary of state changes applied during anchor processing
#[derive(Debug, Clone, Default)]
pub struct StateApplySummary {
    /// Changes per subnet: (event_count, total_changes, final_root)
    pub subnet_stats: HashMap<SubnetId, SubnetApplyStats>,
    /// Events that failed execution (skipped)
    pub failed_events: Vec<String>,
    /// Events rejected due to stale read (old_value mismatch with current state)
    pub conflicted_events: Vec<String>,
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
    
    #[test]
    fn test_parse_state_change_key_oid_format() {
        // Test "oid:{hex}" format - should decode directly to ObjectId
        let object_id_bytes = [
            0xab, 0xcd, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
        ];
        let hex_str = hex::encode(object_id_bytes);
        let key = format!("oid:{}", hex_str);
        
        let parsed = GlobalStateManager::parse_state_change_key(&key);
        assert_eq!(parsed.as_bytes(), &object_id_bytes);
    }
    
    #[test]
    fn test_parse_state_change_key_legacy_format() {
        // Test legacy format - should hash the key
        let key = "event:some-event-id";
        let parsed = GlobalStateManager::parse_state_change_key(key);
        
        // Verify it matches SHA256 hash
        let expected = GlobalStateManager::key_to_object_id(key);
        assert_eq!(parsed, expected);
    }
    
    #[test]
    fn test_parse_state_change_key_invalid_oid() {
        // Test invalid oid format - should fall back to SHA256
        let key = "oid:not-valid-hex";
        let parsed = GlobalStateManager::parse_state_change_key(key);
        
        // Should hash the whole key as fallback
        let expected = GlobalStateManager::key_to_object_id(key);
        assert_eq!(parsed, expected);
    }
    
    #[test]
    fn test_apply_committed_events_conflict_detection() {
        use setu_types::event::{Event, EventType, ExecutionResult, StateChange, VLCSnapshot};
        
        let mut manager = GlobalStateManager::new();
        
        // Setup: insert a coin (object) into ROOT subnet
        let coin_bytes = [0xAA; 32];
        let coin_key = format!("oid:{}", hex::encode(coin_bytes));
        let initial_value = vec![1u8; 64]; // balance = 1000 (conceptual)
        manager.upsert_object(SubnetId::ROOT, coin_bytes, initial_value.clone());
        
        // Create two events that both read the SAME initial state (double-spend)
        let new_value_t1 = vec![2u8; 64]; // balance = 500 after T1
        let new_value_t2 = vec![3u8; 64]; // balance = 300 after T2
        
        // Bob's coin created by T1
        let bob_coin_bytes = [0xBB; 32];
        let bob_coin_key = format!("oid:{}", hex::encode(bob_coin_bytes));
        let bob_value = vec![4u8; 64];
        
        // Charlie's coin created by T2
        let charlie_coin_bytes = [0xCC; 32];
        let charlie_coin_key = format!("oid:{}", hex::encode(charlie_coin_bytes));
        let charlie_value = vec![5u8; 64];
        
        // Event T1: coin 1000 → 500, create bob's coin (vlc=1)
        let mut vlc1 = VLCSnapshot::new();
        vlc1.logical_time = 1;
        let mut event1 = Event::new(EventType::Transfer, vec![], vlc1, "validator-1".to_string());
        event1.set_execution_result(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![
                StateChange::update(coin_key.clone(), initial_value.clone(), new_value_t1.clone()),
                StateChange::insert(bob_coin_key.clone(), bob_value.clone()),
            ],
        });
        event1.status = setu_types::event::EventStatus::Executed;
        
        // Event T2: coin 1000 → 300, create charlie's coin (vlc=2)
        // T2 sees the SAME old_value (initial_value) because coin was released early
        let mut vlc2 = VLCSnapshot::new();
        vlc2.logical_time = 2;
        let mut event2 = Event::new(EventType::Transfer, vec![], vlc2, "validator-1".to_string());
        event2.set_execution_result(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![
                StateChange::update(coin_key.clone(), initial_value.clone(), new_value_t2.clone()),
                StateChange::insert(charlie_coin_key.clone(), charlie_value.clone()),
            ],
        });
        event2.status = setu_types::event::EventStatus::Executed;
        
        // Apply both events (sorted by VLC: T1 first, then T2)
        let summary = manager.apply_committed_events(&[event1.clone(), event2.clone()]);
        
        // T1 should succeed
        assert_eq!(summary.total_events, 1, "Only T1 should be applied");
        assert_eq!(summary.total_changes, 2, "T1 has 2 state changes");
        
        // T2 should be detected as conflicted (old_value mismatch)
        assert_eq!(summary.conflicted_events.len(), 1, "T2 should be conflicted");
        assert_eq!(summary.conflicted_events[0], event2.id);
        
        // Verify final state: only T1's changes applied
        let smt = manager.root_subnet();
        let coin_oid = HashValue::from_slice(&coin_bytes).unwrap();
        let bob_oid = HashValue::from_slice(&bob_coin_bytes).unwrap();
        let charlie_oid = HashValue::from_slice(&charlie_coin_bytes).unwrap();
        
        assert_eq!(smt.get(&coin_oid), Some(&new_value_t1), "Coin should have T1's value (500)");
        assert_eq!(smt.get(&bob_oid), Some(&bob_value), "Bob's coin should exist");
        assert_eq!(smt.get(&charlie_oid), None, "Charlie's coin should NOT exist (T2 rejected)");
    }
    
    #[test]
    fn test_apply_committed_events_no_conflict_when_different_objects() {
        use setu_types::event::{Event, EventType, ExecutionResult, StateChange, VLCSnapshot};
        
        let mut manager = GlobalStateManager::new();
        
        // Setup: two different coins
        let coin_a_bytes = [0xAA; 32];
        let coin_b_bytes = [0xBB; 32];
        let coin_a_key = format!("oid:{}", hex::encode(coin_a_bytes));
        let coin_b_key = format!("oid:{}", hex::encode(coin_b_bytes));
        let value_a = vec![1u8; 64];
        let value_b = vec![2u8; 64];
        
        manager.upsert_object(SubnetId::ROOT, coin_a_bytes, value_a.clone());
        manager.upsert_object(SubnetId::ROOT, coin_b_bytes, value_b.clone());
        
        let new_a = vec![10u8; 64];
        let new_b = vec![20u8; 64];
        
        // Event 1: modify coin_a
        let mut vlc1 = VLCSnapshot::new();
        vlc1.logical_time = 1;
        let mut event1 = Event::new(EventType::Transfer, vec![], vlc1, "v1".to_string());
        event1.set_execution_result(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![StateChange::update(coin_a_key, value_a, new_a.clone())],
        });
        event1.status = setu_types::event::EventStatus::Executed;
        
        // Event 2: modify coin_b (no conflict)
        let mut vlc2 = VLCSnapshot::new();
        vlc2.logical_time = 2;
        let mut event2 = Event::new(EventType::Transfer, vec![], vlc2, "v1".to_string());
        event2.set_execution_result(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![StateChange::update(coin_b_key, value_b, new_b.clone())],
        });
        event2.status = setu_types::event::EventStatus::Executed;
        
        let summary = manager.apply_committed_events(&[event1, event2]);
        
        // Both should succeed - no conflicts
        assert_eq!(summary.total_events, 2);
        assert!(summary.conflicted_events.is_empty(), "No conflicts expected");
        
        let smt = manager.root_subnet();
        let oid_a = HashValue::from_slice(&coin_a_bytes).unwrap();
        let oid_b = HashValue::from_slice(&coin_b_bytes).unwrap();
        assert_eq!(smt.get(&oid_a), Some(&new_a));
        assert_eq!(smt.get(&oid_b), Some(&new_b));
    }
    
    #[test]
    fn test_apply_committed_events_insert_no_false_conflict() {
        use setu_types::event::{Event, EventType, ExecutionResult, StateChange, VLCSnapshot};
        
        let mut manager = GlobalStateManager::new();
        
        // Event with only inserts (old_value = None) should never conflict
        let coin_bytes = [0xDD; 32];
        let coin_key = format!("oid:{}", hex::encode(coin_bytes));
        let value = vec![7u8; 64];
        
        let mut vlc = VLCSnapshot::new();
        vlc.logical_time = 1;
        let mut event = Event::new(EventType::Transfer, vec![], vlc, "v1".to_string());
        event.set_execution_result(ExecutionResult {
            success: true,
            message: None,
            state_changes: vec![StateChange::insert(coin_key, value.clone())],
        });
        event.status = setu_types::event::EventStatus::Executed;
        
        let summary = manager.apply_committed_events(&[event]);
        
        assert_eq!(summary.total_events, 1);
        assert!(summary.conflicted_events.is_empty());
        
        let smt = manager.root_subnet();
        let oid = HashValue::from_slice(&coin_bytes).unwrap();
        assert_eq!(smt.get(&oid), Some(&value));
    }
}
