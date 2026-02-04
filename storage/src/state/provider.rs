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

    // ------------------------------------------------------------------------
    // Helper methods
    // ------------------------------------------------------------------------

    /// Generate object ID for a coin owned by an address
    /// 
    /// Convention: coin_object_id = SHA256("coin:" || address)
    fn coin_object_id(address: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"coin:");
        hasher.update(address.as_bytes());
        hasher.finalize().into()
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
        // For now: single coin per address (convention-based lookup)
        // Future: implement proper coin indexing with multi-type support
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
        vec![]
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
pub fn init_coin(
    state_manager: &mut GlobalStateManager,
    owner: &str,
    balance: u64,
) -> ObjectId {
    let object_id_bytes = MerkleStateProvider::coin_object_id(owner);
    let coin_state = CoinState::new(owner.to_string(), balance);
    
    state_manager.upsert_object(
        SubnetId::ROOT,
        object_id_bytes,
        coin_state.to_bytes(),
    );
    
    ObjectId::new(object_id_bytes)
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
}
