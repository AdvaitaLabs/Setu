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

// Re-export CoinState from setu_types (single source of truth)
pub use setu_types::CoinState;

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
    /// Subnet ID that owns this coin type (1 subnet : 1 token binding)
    /// 
    /// For ROOT subnet, this is "ROOT". For other subnets, it's the subnet_id.
    /// To get the display name (e.g., "GAME"), lookup SubnetConfig.token_symbol.
    pub coin_type: String,
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
}

impl MerkleStateProvider {
    /// Create a new MerkleStateProvider
    pub fn new(state_manager: Arc<RwLock<GlobalStateManager>>) -> Self {
        Self {
            state_manager,
            default_subnet: SubnetId::ROOT,
            modification_tracker: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with a specific default subnet
    pub fn with_subnet(state_manager: Arc<RwLock<GlobalStateManager>>, subnet_id: SubnetId) -> Self {
        Self {
            state_manager,
            default_subnet: subnet_id,
            modification_tracker: Arc::new(RwLock::new(HashMap::new())),
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

    /// Get reference to modification_tracker (for batch snapshot creation)
    /// 
    /// This is used by `BatchStateSnapshot` to efficiently fetch all
    /// modification events in a single lock acquisition.
    pub(crate) fn modification_tracker(&self) -> &Arc<RwLock<HashMap<[u8; 32], String>>> {
        &self.modification_tracker
    }

    /// Register a subnet for an address (called when creating/updating coins)
    /// 
    /// Despite the name "coin_type", this stores subnet_id internally.
    /// 
    /// This updates the GlobalStateManager's authoritative index.
    /// During normal operation, apply_state_change() handles index updates,
    /// but this method is useful for manual initialization (genesis, tests).
    pub fn register_coin_type(&self, address: &str, subnet_id: &str) {
        let mut gsm = self.state_manager.write().unwrap();
        gsm.register_coin_type(address, subnet_id);
    }

    /// Get all registered subnet_ids for an address
    /// 
    /// Returns the list of subnets where this address has coins.
    /// 
    /// This method queries the GlobalStateManager's authoritative index,
    /// which is kept in sync by apply_state_change().
    pub fn get_coin_types_for_address(&self, address: &str) -> Vec<String> {
        let gsm = self.state_manager.read().unwrap();
        gsm.get_coin_types_for_address(address)
            .into_iter()
            .collect()
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
        let mut gsm = self.state_manager.write().unwrap();
        let count = gsm.rebuild_coin_type_index();
        debug!(coin_count = count, "Rebuilt coin_type_index from Merkle Tree");
        count
    }

    /// Get statistics about the index
    pub fn index_stats(&self) -> (usize, usize) {
        // Return GSM's index stats as the authoritative source
        let gsm = self.state_manager.read().unwrap();
        gsm.coin_type_index_stats()
    }

    // ------------------------------------------------------------------------
    // Helper methods
    // ------------------------------------------------------------------------

    /// Generate object ID for a coin owned by an address with specific coin type
    /// 
    /// Convention: coin_object_id = SHA256("coin:" || address || ":" || subnet_id)
    /// 
    /// Each address can hold coins in different subnets (1 subnet = 1 native token).
    /// The subnet_id is used as the coin namespace.
    /// 
    /// Examples:
    /// - ROOT subnet: SHA256("coin:alice:ROOT")
    /// - gaming subnet: SHA256("coin:alice:gaming-subnet")
    pub fn coin_object_id_with_type(address: &str, subnet_id: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"coin:");
        hasher.update(address.as_bytes());
        hasher.update(b":");
        hasher.update(subnet_id.as_bytes());
        hasher.finalize().into()
    }

    /// Generate object ID for ROOT subnet coin
    /// Format: SHA256("coin:" || address || ":ROOT")
    fn coin_object_id(address: &str) -> [u8; 32] {
        Self::coin_object_id_with_type(address, "ROOT")
    }

    /// Convert SparseMerkleProof to SimpleMerkleProof
    /// Made pub(crate) to allow access from batch_snapshot module
    pub(crate) fn convert_proof(key: &HashValue, smt_proof: &SparseMerkleProof) -> SimpleMerkleProof {
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

    /// Get object from a specific subnet SMT
    fn get_object_from_subnet(&self, object_id_bytes: &[u8; 32], subnet_id: &SubnetId) -> Option<Vec<u8>> {
        let manager = self.state_manager.read().unwrap();
        let hash = HashValue::from_slice(object_id_bytes).ok()?;
        manager.get_subnet(subnet_id)?.get(&hash).cloned()
    }

    /// Get object from the default subnet (ROOT)
    fn get_object_internal(&self, object_id_bytes: &[u8; 32]) -> Option<Vec<u8>> {
        self.get_object_from_subnet(object_id_bytes, &self.default_subnet)
    }

    /// Get Merkle proof from a specific subnet SMT
    fn get_proof_from_subnet(&self, object_id_bytes: &[u8; 32], subnet_id: &SubnetId) -> Option<SparseMerkleProof> {
        let manager = self.state_manager.read().unwrap();
        let hash = HashValue::from_slice(object_id_bytes).ok()?;
        manager.get_subnet(subnet_id).map(|smt| smt.prove(&hash))
    }

    /// Get Merkle proof from the default subnet (ROOT)
    fn get_proof_internal(&self, object_id_bytes: &[u8; 32]) -> Option<SparseMerkleProof> {
        self.get_proof_from_subnet(object_id_bytes, &self.default_subnet)
    }
    
    /// Convert subnet_id string to SubnetId
    fn resolve_subnet_id(subnet_id_str: &str) -> SubnetId {
        if subnet_id_str == "ROOT" {
            SubnetId::ROOT
        } else {
            SubnetId::from_hex(subnet_id_str).unwrap_or_else(|_| {
                SubnetId::from_str_id(subnet_id_str)
            })
        }
    }
}

impl StateProvider for MerkleStateProvider {
    fn get_coins_for_address(&self, address: &str) -> Vec<CoinInfo> {
        use setu_types::Address;
        
        // Normalize address to hex format ("0x...") for consistency.
        // All indices use hex address as the canonical key.
        let addr_hex = Address::from(address).to_string();
        
        // Use owner_coin_index to find all (object_id, coin_type) pairs for this owner.
        // This works for both deterministic init coins AND runtime split coins.
        let manager = self.state_manager.read().unwrap();
        let coin_objects = manager.get_coin_objects_for_address(&addr_hex);
        drop(manager);
        
        if coin_objects.is_empty() {
            // Fallback: try deterministic ROOT subnet coin id
            let coin_object_id = Self::coin_object_id(&addr_hex);
            let target_subnet = SubnetId::ROOT;
            if let Some(data) = self.get_object_from_subnet(&coin_object_id, &target_subnet) {
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
            debug!(address = %address, addr_hex = %addr_hex, "No coins found for address");
            return vec![];
        }

        // Look up each coin object from its subnet SMT
        let mut coins = Vec::new();
        for (object_id_bytes, coin_type) in coin_objects {
            let target_subnet = Self::resolve_subnet_id(&coin_type);
            if let Some(data) = self.get_object_from_subnet(&object_id_bytes, &target_subnet) {
                if let Some(coin_state) = CoinState::from_bytes(&data) {
                    // Only include if still owned by this address
                    if coin_state.owner == addr_hex {
                        coins.push(CoinInfo {
                            object_id: ObjectId::new(object_id_bytes),
                            owner: coin_state.owner,
                            balance: coin_state.balance,
                            version: coin_state.version,
                            coin_type: coin_state.coin_type,
                        });
                    }
                }
            }
        }

        if coins.is_empty() {
            debug!(address = %address, addr_hex = %addr_hex, "No coins found for address (all transferred?)");
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
        // First check GSM's modification_tracker (populated by apply_committed_events)
        {
            let gsm = self.state_manager.read().unwrap();
            if let Some(event_id) = gsm.get_last_modifying_event(object_id.as_bytes()) {
                return Some(event_id.clone());
            }
        }
        // Fallback to local tracker (for backward compatibility)
        let tracker = self.modification_tracker.read().unwrap();
        tracker.get(object_id.as_bytes()).cloned()
    }
}

// ============================================================================
// Utility Functions for State Initialization
// ============================================================================

/// Initialize a coin in the state (for testing/genesis)
/// Uses ROOT subnet (native SETU token)
pub fn init_coin(
    state_manager: &mut GlobalStateManager,
    owner: &str,
    balance: u64,
) -> ObjectId {
    init_coin_with_type(state_manager, owner, balance, "ROOT")
}

/// Initialize a coin with specific subnet_id in the state
/// 
/// Use this for multi-subnet scenarios where each subnet has its own token.
/// The `subnet_id` determines which subnet's native token to use.
/// 
/// Examples:
/// - "ROOT" for the root subnet's SETU token
/// - "gaming-subnet" for a gaming app's token
/// 
/// Note: This function now automatically registers the coin in GSM's index.
/// For testing without index registration, use `upsert_object` directly.
pub fn init_coin_with_type(
    state_manager: &mut GlobalStateManager,
    owner: &str,
    balance: u64,
    subnet_id: &str,
) -> ObjectId {
    use setu_types::Address;
    
    // Normalize owner to hex address format ("0x...") for consistency.
    // Runtime always uses Address::from(s).to_string() as the canonical owner format.
    // We must use the same format for:
    //   1. CoinState.owner (stored in SMT)
    //   2. coin_object_id computation
    //   3. coin_type_index key
    let owner_hex = Address::from(owner).to_string();
    
    let object_id_bytes = MerkleStateProvider::coin_object_id_with_type(&owner_hex, subnet_id);
    let coin_state = CoinState::new_with_type(owner_hex.clone(), balance, subnet_id.to_string());
    
    // Physical isolation: store coin in the correct subnet's SMT
    let target_subnet = if subnet_id == "ROOT" {
        SubnetId::ROOT
    } else {
        SubnetId::from_hex(subnet_id).unwrap_or_else(|_| {
            // If not a valid hex, create subnet from string hash
            SubnetId::from_str_id(subnet_id)
        })
    };
    
    state_manager.upsert_object(
        target_subnet,
        object_id_bytes,
        coin_state.to_bytes(),
    );
    
    // Register in GSM's coin_type_index for efficient queries
    state_manager.register_coin_object(&owner_hex, subnet_id, object_id_bytes);
    
    ObjectId::new(object_id_bytes)
}

/// Initialize a coin and register it with the provider's index
/// 
/// This is the recommended way to create coins as it ensures the index stays in sync.
/// 
/// # Arguments
/// * `provider` - The MerkleStateProvider
/// * `owner` - Owner address
/// * `balance` - Initial balance
/// * `subnet_id` - Subnet ID (determines token type, 1:1 binding)
pub fn init_coin_with_provider(
    provider: &MerkleStateProvider,
    owner: &str,
    balance: u64,
    subnet_id: &str,
) -> ObjectId {
    let object_id = {
        let state_manager = provider.state_manager();
        let mut manager = state_manager.write().unwrap();
        init_coin_with_type(&mut manager, owner, balance, subnet_id)
    };
    
    // Auto-register to index
    provider.register_coin_type(owner, subnet_id);
    
    object_id
}

/// Get or create a coin for the given address and subnet
/// 
/// This is useful for transfer operations where the recipient may not have
/// a coin object yet. Returns the existing coin or creates one with 0 balance.
/// 
/// # Arguments
/// * `provider` - The MerkleStateProvider
/// * `owner` - Owner address
/// * `subnet_id` - Subnet ID (determines token type)
/// 
/// # Returns
/// (ObjectId, is_new) - object ID and whether it was newly created
pub fn get_or_create_coin(
    provider: &MerkleStateProvider,
    owner: &str,
    subnet_id: &str,
) -> (ObjectId, bool) {
    let object_id_bytes = MerkleStateProvider::coin_object_id_with_type(owner, subnet_id);
    let object_id = ObjectId::new(object_id_bytes);
    
    // Resolve the correct subnet to query
    // Coins are stored in their respective subnet's SMT (physical isolation)
    let target_subnet = MerkleStateProvider::resolve_subnet_id(subnet_id);
    
    // Check if coin already exists in the correct subnet
    if provider.get_object_from_subnet(&object_id_bytes, &target_subnet).is_some() {
        return (object_id, false);
    }
    
    // Create new coin with 0 balance
    let new_object_id = init_coin_with_provider(provider, owner, 0, subnet_id);
    (new_object_id, true)
}

/// Mint initial supply for a subnet token to the owner
/// 
/// Called when a subnet with token is registered.
/// Uses the subnet_id as the coin type identifier.
/// 
/// # Arguments
/// * `provider` - The MerkleStateProvider
/// * `subnet_id` - The subnet ID (also used as coin namespace)
/// * `subnet_owner` - Owner address to receive the tokens
/// * `initial_supply` - Amount of tokens to mint
pub fn mint_subnet_token(
    provider: &MerkleStateProvider,
    subnet_id: &str,
    subnet_owner: &str,
    initial_supply: u64,
) -> ObjectId {
    init_coin_with_provider(provider, subnet_owner, initial_supply, subnet_id)
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

        // Initialize a coin for ROOT subnet
        {
            let mut manager = state_manager.write().unwrap();
            init_coin(&mut manager, "alice", 1000);  // Uses ROOT as default
        }

        // Create provider and query
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        // Query by ROOT subnet (not SETU)
        let coins = provider.get_coins_for_address_by_type("alice", "ROOT");
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

        // Initialize coins for different subnets
        // Note: coin_type now represents subnet_id (1:1 binding)
        {
            let mut manager = state_manager.write().unwrap();
            init_coin_with_type(&mut manager, "alice", 1000, "ROOT");       // Root subnet
            init_coin_with_type(&mut manager, "alice", 500, "defi-subnet"); // DeFi subnet
            init_coin_with_type(&mut manager, "alice", 200, "nft-subnet");  // NFT subnet
        }

        // Create provider and register subnets for this address
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));
        provider.register_coin_type("alice", "ROOT");
        provider.register_coin_type("alice", "defi-subnet");
        provider.register_coin_type("alice", "nft-subnet");

        // Query all coins for alice
        let coins = provider.get_coins_for_address("alice");
        assert_eq!(coins.len(), 3);

        // Verify each subnet's coin exists with correct balance
        let root_coin = coins.iter().find(|c| c.coin_type == "ROOT").unwrap();
        assert_eq!(root_coin.balance, 1000);

        let defi_coin = coins.iter().find(|c| c.coin_type == "defi-subnet").unwrap();
        assert_eq!(defi_coin.balance, 500);

        let nft_coin = coins.iter().find(|c| c.coin_type == "nft-subnet").unwrap();
        assert_eq!(nft_coin.balance, 200);

        // Query by specific subnet
        let root_only = provider.get_coins_for_address_by_type("alice", "ROOT");
        assert_eq!(root_only.len(), 1);
        assert_eq!(root_only[0].balance, 1000);
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
        // Note: subnet_id is first param, then owner, then amount
        let subnet_id = "my-app-subnet";
        let obj_id = mint_subnet_token(&provider, subnet_id, "subnet_owner", 1_000_000);
        
        // Query by subnet_id (not token_symbol)
        let coins = provider.get_coins_for_address_by_type("subnet_owner", subnet_id);
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].balance, 1_000_000);
        assert_eq!(coins[0].object_id, obj_id);
    }

    #[test]
    fn test_rebuild_coin_type_index() {
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));

        // Initialize multiple coins for multiple users
        // Using subnet_id as the coin namespace
        // Note: init_coin_with_type now auto-registers to GSM index
        {
            let mut manager = state_manager.write().unwrap();
            init_coin_with_type(&mut manager, "alice", 1000, "ROOT");
            init_coin_with_type(&mut manager, "alice", 500, "defi-subnet");
            init_coin_with_type(&mut manager, "bob", 800, "ROOT");
            init_coin_with_type(&mut manager, "bob", 300, "gaming-subnet");
            init_coin_with_type(&mut manager, "charlie", 100, "ROOT");
        }

        // Create provider - coins are already indexed in GSM
        let provider = MerkleStateProvider::new(Arc::clone(&state_manager));

        // Before rebuild: alice's coins ARE found (auto-registered by init_coin_with_type)
        let alice_coins_before = provider.get_coins_for_address("alice");
        assert_eq!(alice_coins_before.len(), 2); // Both ROOT and defi-subnet

        // Rebuild the index from Merkle Tree (should be idempotent)
        let indexed_count = provider.rebuild_coin_type_index();
        assert_eq!(indexed_count, 5); // 5 coins total

        // After rebuild: all coins should still be found
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
