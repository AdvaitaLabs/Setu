//! StateProvider trait and MerkleStateProvider implementation.
//!
//! This module provides the abstraction for reading blockchain state,
//! with a production implementation backed by GlobalStateManager/SparseMerkleTree.
//!
//! ## Design
//!
//! The `StateProvider` trait is defined here (in storage) to avoid circular dependencies:
//! - storage depends on setu-merkle, setu-types
//! - validator depends on storage (can use StateProvider)
//! - This avoids validator -> storage -> validator cycles
//!
//! ## Usage
//!
//! ```rust,ignore
//! // For testing: use TaskPreparer::new_for_testing() which creates
//! // MerkleStateProvider with pre-initialized accounts (alice, bob, charlie)
//! let preparer = TaskPreparer::new_for_testing("validator-1".to_string());
//!
//! // For production: create MerkleStateProvider with your own GlobalStateManager
//! let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
//! let provider = MerkleStateProvider::new(state_manager);
//! ```

use crate::state::manager::GlobalStateManager;
use setu_merkle::{HashValue, SparseMerkleProof};
use setu_types::{ObjectId, SubnetId};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::debug;

// ============================================================================
// Core Types
// ============================================================================

/// Coin information retrieved from state
#[derive(Debug, Clone)]
pub struct CoinInfo {
    pub object_id: ObjectId,
    pub owner: String,
    pub balance: u64,
    pub version: u64,
    /// Coin type (e.g., "SETU", "USDC") - supports multi-subnet token types
    pub coin_type: String,
}

/// Coin state as stored in the Merkle tree
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoinState {
    pub owner: String,
    pub balance: u64,
    pub version: u64,
    /// Coin type identifier (e.g., "SETU", "USDC")
    #[serde(default = "default_coin_type")]
    pub coin_type: String,
}

fn default_coin_type() -> String {
    "SETU".to_string()
}

impl CoinState {
    pub fn new(owner: String, balance: u64) -> Self {
        Self::new_with_type(owner, balance, "SETU".to_string())
    }
    
    pub fn new_with_type(owner: String, balance: u64, coin_type: String) -> Self {
        Self {
            owner,
            balance,
            version: 1,
            coin_type,
        }
    }

    /// Serialize for storage (using BCS for consistency with other storage)
    pub fn to_bytes(&self) -> Vec<u8> {
        bcs::to_bytes(self).expect("CoinState serialization should not fail")
    }

    /// Deserialize from storage
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bcs::from_bytes(bytes).ok()
    }
}

/// Merkle proof in a simple, serializable format
/// 
/// This is the format used for passing proofs between components.
/// It's simpler than SparseMerkleProof and easily serializable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SimpleMerkleProof {
    /// Sibling hashes on the path from leaf to root
    pub siblings: Vec<[u8; 32]>,
    /// Bit path (true = right, false = left)
    pub path_bits: Vec<bool>,
    /// The leaf key (for verification)
    pub leaf_key: [u8; 32],
    /// Whether the key exists in the tree
    pub exists: bool,
}

impl SimpleMerkleProof {
    /// Create an empty proof (for development/mock)
    pub fn empty() -> Self {
        Self {
            siblings: vec![],
            path_bits: vec![],
            leaf_key: [0u8; 32],
            exists: false,
        }
    }
}

// ============================================================================
// StateProvider Trait
// ============================================================================

/// Trait for reading current blockchain state.
///
/// This trait abstracts over the underlying state storage, allowing:
/// - Mock implementations for testing
/// - Real Merkle tree implementations for production
///
/// ## Thread Safety
/// Implementations must be Send + Sync for use across async boundaries.
pub trait StateProvider: Send + Sync {
    /// Get all coins owned by an address (all types)
    fn get_coins_for_address(&self, address: &str) -> Vec<CoinInfo>;
    
    /// Get coins owned by an address filtered by coin type
    /// 
    /// This is essential for multi-subnet scenarios where each subnet
    /// application may have its own token type.
    fn get_coins_for_address_by_type(&self, address: &str, coin_type: &str) -> Vec<CoinInfo> {
        // Default implementation: filter from all coins
        self.get_coins_for_address(address)
            .into_iter()
            .filter(|c| c.coin_type == coin_type)
            .collect()
    }

    /// Get object data by ID
    fn get_object(&self, object_id: &ObjectId) -> Option<Vec<u8>>;

    /// Get current global state root
    fn get_state_root(&self) -> [u8; 32];

    /// Get Merkle proof for an object
    fn get_merkle_proof(&self, object_id: &ObjectId) -> Option<SimpleMerkleProof>;

    /// Get the event ID that last modified an object
    ///
    /// Used for deriving event dependencies from input objects.
    /// Returns None for genesis objects or if tracking is not available.
    fn get_last_modifying_event(&self, object_id: &ObjectId) -> Option<String>;
    
    /// Get object with its proof (convenience method)
    fn get_object_with_proof(&self, object_id: &ObjectId) -> Option<(Vec<u8>, SimpleMerkleProof)> {
        let data = self.get_object(object_id)?;
        let proof = self.get_merkle_proof(object_id)?;
        Some((data, proof))
    }
}

// ============================================================================
// MerkleStateProvider Implementation
// ============================================================================

/// Production StateProvider backed by GlobalStateManager.
///
/// This implementation reads state from the actual Merkle trees,
/// providing real proofs and state data.
pub struct MerkleStateProvider {
    /// Global state manager with all subnet SMTs
    state_manager: Arc<RwLock<GlobalStateManager>>,

    /// Default subnet to operate on (usually ROOT)
    default_subnet: SubnetId,

    /// Object modification tracking (event_id -> object_ids modified)
    /// Simple in-memory tracking for development; can be enhanced later
    modification_tracker: Arc<RwLock<HashMap<[u8; 32], String>>>,

    /// Address to coin types index: address -> set of coin_types
    /// Enables querying all coins for an address across multiple token types
    coin_type_index: Arc<RwLock<HashMap<String, std::collections::HashSet<String>>>>,
}

impl MerkleStateProvider {
    /// Create a new MerkleStateProvider
    pub fn new(state_manager: Arc<RwLock<GlobalStateManager>>) -> Self {
        Self {
            state_manager,
            default_subnet: SubnetId::ROOT,
            modification_tracker: Arc::new(RwLock::new(HashMap::new())),
            coin_type_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with a specific default subnet
    pub fn with_subnet(state_manager: Arc<RwLock<GlobalStateManager>>, subnet_id: SubnetId) -> Self {
        Self {
            state_manager,
            default_subnet: subnet_id,
            modification_tracker: Arc::new(RwLock::new(HashMap::new())),
            coin_type_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the underlying state manager (for direct access if needed)
    pub fn state_manager(&self) -> Arc<RwLock<GlobalStateManager>> {
        Arc::clone(&self.state_manager)
    }

    /// Record that an event modified objects
    ///
    /// Call this after an anchor is committed to track object→event mapping.
    pub fn record_modifications(&self, event_id: &str, object_ids: &[[u8; 32]]) {
        let mut tracker = self.modification_tracker.write().unwrap();
        for object_id in object_ids {
            tracker.insert(*object_id, event_id.to_string());
        }
    }

    /// Clear modification tracking (e.g., after pruning)
    pub fn clear_modifications(&self) {
        let mut tracker = self.modification_tracker.write().unwrap();
        tracker.clear();
    }

    /// Register a coin type for an address (called when creating/updating coins)
    pub fn register_coin_type(&self, address: &str, coin_type: &str) {
        let mut index = self.coin_type_index.write().unwrap();
        index
            .entry(address.to_string())
            .or_insert_with(std::collections::HashSet::new)
            .insert(coin_type.to_string());
    }

    /// Get all registered coin types for an address
    pub fn get_coin_types_for_address(&self, address: &str) -> Vec<String> {
        let index = self.coin_type_index.read().unwrap();
        index
            .get(address)
            .map(|types| types.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Rebuild the coin_type_index by scanning all objects in the Merkle Tree.
    ///
    /// This should be called at startup to restore the index from persisted state.
    /// 
    /// # Performance
    /// - O(n) where n is total number of objects
    /// - Very fast: just iterates in-memory HashMap, no disk I/O
    /// - 1M objects ≈ 1 second
    /// 
    /// # Returns
    /// Number of coin entries indexed
    pub fn rebuild_coin_type_index(&self) -> usize {
        let state_manager = self.state_manager.read().unwrap();
        let mut index = self.coin_type_index.write().unwrap();
        
        // Clear existing index
        index.clear();
        
        let mut count = 0;
        
        // Scan all objects in all subnets
        for (_subnet_id, _object_id, value) in state_manager.iter_all_objects() {
            // Try to deserialize as CoinState
            if let Some(coin_state) = CoinState::from_bytes(value) {
                // Register this coin type for the owner
                index
                    .entry(coin_state.owner.clone())
                    .or_insert_with(std::collections::HashSet::new)
                    .insert(coin_state.coin_type.clone());
                count += 1;
            }
            // Non-CoinState objects are skipped (they don't need indexing)
        }
        
        debug!(
            coin_count = count,
            address_count = index.len(),
            "Rebuilt coin_type_index from Merkle Tree"
        );
        
        count
    }

    /// Get statistics about the index
    pub fn index_stats(&self) -> (usize, usize) {
        let index = self.coin_type_index.read().unwrap();
        let address_count = index.len();
        let total_entries: usize = index.values().map(|v| v.len()).sum();
        (address_count, total_entries)
    }

    // ------------------------------------------------------------------------
    // Helper methods
    // ------------------------------------------------------------------------

    /// Generate object ID for a coin owned by an address with specific coin type
    /// 
    /// Convention: coin_object_id = SHA256("coin:" || address || ":" || coin_type)
    /// This allows each address to hold multiple coin types (e.g., SETU, USDC, subnet tokens)
    /// 
    /// Note: For backwards compatibility, "SETU" type uses the legacy format without suffix.
    pub fn coin_object_id_with_type(address: &str, coin_type: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"coin:");
        hasher.update(address.as_bytes());
        // For backwards compatibility: SETU uses legacy format without coin_type suffix
        if coin_type != "SETU" {
            hasher.update(b":");
            hasher.update(coin_type.as_bytes());
        }
        hasher.finalize().into()
    }

    /// Generate object ID for default SETU coin (backwards compatible)
    /// Uses legacy format: SHA256("coin:" || address)
    fn coin_object_id(address: &str) -> [u8; 32] {
        Self::coin_object_id_with_type(address, "SETU")
    }

    /// Convert SparseMerkleProof to SimpleMerkleProof
    fn convert_proof(key: &HashValue, smt_proof: &SparseMerkleProof) -> SimpleMerkleProof {
        // Extract siblings from the proof
        // SparseMerkleProof stores siblings top-down (root to leaf)
        let siblings: Vec<[u8; 32]> = smt_proof
            .sibling_hashes()
            .iter()
            .map(|h| *h.as_bytes())
            .collect();

        // Compute path bits from the key
        // Each bit determines left (0) or right (1) at each level
        let depth = siblings.len();
        let path_bits: Vec<bool> = (0..depth).map(|i| key.bit(i)).collect();

        SimpleMerkleProof {
            siblings,
            path_bits,
            leaf_key: *key.as_bytes(),
            exists: smt_proof.is_inclusion(),
        }
    }

    /// Get object from the default subnet
    fn get_object_internal(&self, object_id_bytes: &[u8; 32]) -> Option<Vec<u8>> {
        let manager = self.state_manager.read().unwrap();
        let hash = HashValue::from_slice(object_id_bytes).ok()?;
        manager.get_subnet(&self.default_subnet)?.get(&hash).cloned()
    }

    /// Get Merkle proof from the default subnet
    fn get_proof_internal(&self, object_id_bytes: &[u8; 32]) -> Option<SparseMerkleProof> {
        let manager = self.state_manager.read().unwrap();
        let hash = HashValue::from_slice(object_id_bytes).ok()?;
        manager.get_subnet(&self.default_subnet).map(|smt| smt.prove(&hash))
    }
}

impl StateProvider for MerkleStateProvider {
    fn get_coins_for_address(&self, address: &str) -> Vec<CoinInfo> {
        // Query all registered coin types for this address
        let coin_types = self.get_coin_types_for_address(address);
        
        if coin_types.is_empty() {
            // Fallback: try default SETU type for backwards compatibility
            let coin_object_id = Self::coin_object_id(address);
            if let Some(data) = self.get_object_internal(&coin_object_id) {
                if let Some(coin_state) = CoinState::from_bytes(&data) {
                    return vec![CoinInfo {
                        object_id: ObjectId::new(coin_object_id),
                        owner: coin_state.owner,
                        balance: coin_state.balance,
                        version: coin_state.version,
                        coin_type: coin_state.coin_type,
                    }];
                }
            }
            debug!(address = %address, "No coins found for address");
            return vec![];
        }

        // Collect coins for all registered types
        let mut coins = Vec::new();
        for coin_type in coin_types {
            let coin_object_id = Self::coin_object_id_with_type(address, &coin_type);
            if let Some(data) = self.get_object_internal(&coin_object_id) {
                if let Some(coin_state) = CoinState::from_bytes(&data) {
                    coins.push(CoinInfo {
                        object_id: ObjectId::new(coin_object_id),
                        owner: coin_state.owner,
                        balance: coin_state.balance,
                        version: coin_state.version,
                        coin_type: coin_state.coin_type,
                    });
                }
            }
        }

        if coins.is_empty() {
            debug!(address = %address, "No coins found for address");
        }
        coins
    }

    fn get_object(&self, object_id: &ObjectId) -> Option<Vec<u8>> {
        self.get_object_internal(object_id.as_bytes())
    }

    fn get_state_root(&self) -> [u8; 32] {
        let manager = self.state_manager.read().unwrap();
        let (root, _) = manager.compute_global_root_bytes();
        root
    }

    fn get_merkle_proof(&self, object_id: &ObjectId) -> Option<SimpleMerkleProof> {
        let key = HashValue::from_slice(object_id.as_bytes()).ok()?;
        let proof = self.get_proof_internal(object_id.as_bytes())?;
        Some(Self::convert_proof(&key, &proof))
    }

    fn get_last_modifying_event(&self, object_id: &ObjectId) -> Option<String> {
        let tracker = self.modification_tracker.read().unwrap();
        tracker.get(object_id.as_bytes()).cloned()
    }
}

// ============================================================================
// Utility Functions for State Initialization
// ============================================================================

/// Initialize a coin in the state (for testing/genesis)
/// Uses default "SETU" coin type
pub fn init_coin(
    state_manager: &mut GlobalStateManager,
    owner: &str,
    balance: u64,
) -> ObjectId {
    init_coin_with_type(state_manager, owner, balance, "SETU")
}

/// Initialize a coin with specific type in the state
/// 
/// Use this for multi-subnet scenarios where each subnet has its own token.
/// Example coin_types: "SETU" (main), "SUBNET_A_TOKEN", "USDC", etc.
/// 
/// Note: This only writes to the Merkle tree. If using MerkleStateProvider,
/// also call `provider.register_coin_type()` or use `init_coin_with_provider()`.
pub fn init_coin_with_type(
    state_manager: &mut GlobalStateManager,
    owner: &str,
    balance: u64,
    coin_type: &str,
) -> ObjectId {
    let object_id_bytes = MerkleStateProvider::coin_object_id_with_type(owner, coin_type);
    let coin_state = CoinState::new_with_type(owner.to_string(), balance, coin_type.to_string());
    
    state_manager.upsert_object(
        SubnetId::ROOT,
        object_id_bytes,
        coin_state.to_bytes(),
    );
    
    ObjectId::new(object_id_bytes)
}

/// Initialize a coin and register it with the provider's index
/// 
/// This is the recommended way to create coins as it ensures the index stays in sync.
pub fn init_coin_with_provider(
    provider: &MerkleStateProvider,
    owner: &str,
    balance: u64,
    coin_type: &str,
) -> ObjectId {
    let object_id = {
        let state_manager = provider.state_manager();
        let mut manager = state_manager.write().unwrap();
        init_coin_with_type(&mut manager, owner, balance, coin_type)
    };
    
    // Auto-register to index
    provider.register_coin_type(owner, coin_type);
    
    object_id
}

/// Get or create a coin for the given address and type
/// 
/// This is useful for transfer operations where the recipient may not have
/// a coin object yet. Returns the existing coin or creates one with 0 balance.
/// 
/// # Returns
/// (ObjectId, is_new) - object ID and whether it was newly created
pub fn get_or_create_coin(
    provider: &MerkleStateProvider,
    owner: &str,
    coin_type: &str,
) -> (ObjectId, bool) {
    let object_id_bytes = MerkleStateProvider::coin_object_id_with_type(owner, coin_type);
    let object_id = ObjectId::new(object_id_bytes);
    
    // Check if coin already exists
    if provider.get_object(&object_id).is_some() {
        return (object_id, false);
    }
    
    // Create new coin with 0 balance
    let new_object_id = init_coin_with_provider(provider, owner, 0, coin_type);
    (new_object_id, true)
}

/// Mint initial supply for a subnet token to the owner
/// 
/// Called when a subnet with token is registered.
pub fn mint_subnet_token(
    provider: &MerkleStateProvider,
    subnet_owner: &str,
    token_symbol: &str,
    initial_supply: u64,
) -> ObjectId {
    init_coin_with_provider(provider, subnet_owner, initial_supply, token_symbol)
}

/// Get coin state from raw bytes
pub fn get_coin_state(data: &[u8]) -> Option<CoinState> {
    CoinState::from_bytes(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coin_state_serialization() {
        let coin = CoinState::new("alice".to_string(), 1000);
        let bytes = coin.to_bytes();
        let decoded = CoinState::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.owner, "alice");
        assert_eq!(decoded.balance, 1000);
        assert_eq!(decoded.version, 1);
    }

    #[test]
    fn test_merkle_state_provider() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));

        // Initialize a coin
        {
            let mut manager = state_manager.write().unwrap();
            init_coin(&mut manager, "alice", 1000);
        }

        // Create provider and query
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        let coins = provider.get_coins_for_address("alice");
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].owner, "alice");
        assert_eq!(coins[0].balance, 1000);

        // Verify we can get proof
        let proof = provider.get_merkle_proof(&coins[0].object_id);
        assert!(proof.is_some());

        // Verify state root is non-zero
        let root = provider.get_state_root();
        assert_ne!(root, [0u8; 32]);
    }

    #[test]
    fn test_modification_tracking() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = MerkleStateProvider::new(state_manager);

        let object_id = ObjectId::new([1u8; 32]);

        // Initially no tracking
        assert!(provider.get_last_modifying_event(&object_id).is_none());

        // Record modification
        provider.record_modifications("event-123", &[*object_id.as_bytes()]);

        // Now should return the event
        assert_eq!(
            provider.get_last_modifying_event(&object_id),
            Some("event-123".to_string())
        );
    }

    #[test]
    fn test_multi_coin_types() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));

        // Initialize multiple coin types for alice
        {
            let mut manager = state_manager.write().unwrap();
            init_coin_with_type(&mut manager, "alice", 1000, "SETU");
            init_coin_with_type(&mut manager, "alice", 500, "USDC");
            init_coin_with_type(&mut manager, "alice", 200, "SUBNET_A_TOKEN");
        }

        // Create provider and register coin types
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));
        provider.register_coin_type("alice", "SETU");
        provider.register_coin_type("alice", "USDC");
        provider.register_coin_type("alice", "SUBNET_A_TOKEN");

        // Query all coins for alice
        let coins = provider.get_coins_for_address("alice");
        assert_eq!(coins.len(), 3);

        // Verify each coin type exists with correct balance
        let setu_coin = coins.iter().find(|c| c.coin_type == "SETU").unwrap();
        assert_eq!(setu_coin.balance, 1000);

        let usdc_coin = coins.iter().find(|c| c.coin_type == "USDC").unwrap();
        assert_eq!(usdc_coin.balance, 500);

        let subnet_coin = coins.iter().find(|c| c.coin_type == "SUBNET_A_TOKEN").unwrap();
        assert_eq!(subnet_coin.balance, 200);

        // Query by specific type
        let setu_only = provider.get_coins_for_address_by_type("alice", "SETU");
        assert_eq!(setu_only.len(), 1);
        assert_eq!(setu_only[0].balance, 1000);
    }

    #[test]
    fn test_get_or_create_coin() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        // First call should create a new coin
        let (object_id, is_new) = get_or_create_coin(&provider, "bob", "MYTOKEN");
        assert!(is_new);
        
        // Verify coin was created with 0 balance
        let coins = provider.get_coins_for_address_by_type("bob", "MYTOKEN");
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].balance, 0);
        assert_eq!(coins[0].object_id, object_id);

        // Second call should return existing coin
        let (object_id2, is_new2) = get_or_create_coin(&provider, "bob", "MYTOKEN");
        assert!(!is_new2);
        assert_eq!(object_id, object_id2);
    }

    #[test]
    fn test_mint_subnet_token() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        // Mint initial supply to subnet owner
        let obj_id = mint_subnet_token(&provider, "subnet_owner", "MYAPP", 1_000_000);
        
        let coins = provider.get_coins_for_address_by_type("subnet_owner", "MYAPP");
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].balance, 1_000_000);
        assert_eq!(coins[0].object_id, obj_id);
    }

    #[test]
    fn test_rebuild_coin_type_index() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));

        // Initialize multiple coins for multiple users
        {
            let mut manager = state_manager.write().unwrap();
            init_coin_with_type(&mut manager, "alice", 1000, "SETU");
            init_coin_with_type(&mut manager, "alice", 500, "USDC");
            init_coin_with_type(&mut manager, "bob", 800, "SETU");
            init_coin_with_type(&mut manager, "bob", 300, "GAMING_TOKEN");
            init_coin_with_type(&mut manager, "charlie", 100, "SETU");
        }

        // Create provider WITHOUT registering coin types (simulating restart)
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        // Before rebuild: alice's coins not found (except SETU fallback)
        let alice_coins_before = provider.get_coins_for_address("alice");
        assert_eq!(alice_coins_before.len(), 1); // Only SETU via fallback
        assert_eq!(alice_coins_before[0].coin_type, "SETU");

        // Rebuild the index from Merkle Tree
        let indexed_count = provider.rebuild_coin_type_index();
        assert_eq!(indexed_count, 5); // 5 coins total

        // After rebuild: all coins should be found
        let alice_coins_after = provider.get_coins_for_address("alice");
        assert_eq!(alice_coins_after.len(), 2);

        let bob_coins = provider.get_coins_for_address("bob");
        assert_eq!(bob_coins.len(), 2);

        let charlie_coins = provider.get_coins_for_address("charlie");
        assert_eq!(charlie_coins.len(), 1);

        // Verify index stats
        let (address_count, entry_count) = provider.index_stats();
        assert_eq!(address_count, 3); // alice, bob, charlie
        assert_eq!(entry_count, 5);   // total coin type entries
    }

    #[test]
    fn test_rebuild_index_empty_tree() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        let provider = MerkleStateProvider::new(state_manager);

        // Rebuild on empty tree should work without error
        let count = provider.rebuild_coin_type_index();
        assert_eq!(count, 0);

        let (address_count, entry_count) = provider.index_stats();
        assert_eq!(address_count, 0);
        assert_eq!(entry_count, 0);
    }
}
