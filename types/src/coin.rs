//! Coin Object - Transferable Asset
//! 
//! Design Philosophy:
//! - Coin is an independent object that can be owned by SBT
//! - Supports operations like split, merge, transfer
//! - Balance is a value type, not an object
//! 
//! ## Storage Format
//! 
//! Coins are stored in the Merkle tree using `CoinState` format (BCS serialized).
//! This is the canonical storage format used across all components:
//! - Runtime outputs `CoinState` via `Coin::to_coin_state()`
//! - Storage layer reads/writes `CoinState` directly
//! - Enclave passes through the bytes without parsing

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use crate::object::{Object, Address, ObjectId, generate_object_id};

// ============================================================================
// CoinState - Canonical Storage Format
// ============================================================================

/// Storage-layer representation of a Coin.
/// 
/// This is the canonical format stored in the Merkle tree (BCS serialized).
/// All components must use this format for state persistence.
/// 
/// ## Why BCS?
/// - More compact than JSON (~2-3x smaller)
/// - Faster serialization/deserialization
/// - Deterministic byte representation (important for Merkle proofs)
/// 
/// ## Relationship to Object<CoinData>
/// - `Object<CoinData>` is the in-memory runtime representation
/// - `CoinState` is the storage format
/// - Use `Coin::to_coin_state()` to convert for storage
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoinState {
    /// Owner address as hex string
    pub owner: String,
    /// Balance amount
    pub balance: u64,
    /// Version number (for optimistic concurrency)
    pub version: u64,
    /// Subnet ID that owns this coin type (1 subnet : 1 token)
    /// For ROOT subnet, this is "ROOT". For other subnets, it's the subnet_id.
    pub coin_type: String,
}

impl CoinState {
    /// Create a new CoinState for ROOT subnet
    pub fn new(owner: String, balance: u64) -> Self {
        Self::new_with_type(owner, balance, "ROOT".to_string())
    }
    
    /// Create a new CoinState with specific subnet_id/coin_type
    pub fn new_with_type(owner: String, balance: u64, coin_type: String) -> Self {
        Self {
            owner,
            balance,
            version: 1,
            coin_type,
        }
    }
    
    /// Serialize to BCS bytes for storage
    pub fn to_bytes(&self) -> Vec<u8> {
        bcs::to_bytes(self).expect("CoinState BCS serialization should not fail")
    }
    
    /// Deserialize from BCS bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bcs::from_bytes(bytes).ok()
    }
}

// ============================================================================
// CoinType - Token Identifier
// ============================================================================

/// Coin type identifier - stores subnet_id internally
/// 
/// Note: This stores the subnet_id (e.g., "ROOT", "gaming-subnet"), not the display name.
/// For display name (e.g., "SETU", "GAME"), lookup SubnetConfig.token_symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CoinType(pub String);

impl CoinType {
    /// Root subnet ID (internal identifier, not display name)
    /// Display name "SETU" is stored in SubnetConfig.token_symbol
    pub const NATIVE: &'static str = "ROOT";
    
    pub fn native() -> Self {
        Self(Self::NATIVE.to_string())
    }
    
    pub fn new(coin_type: impl Into<String>) -> Self {
        Self(coin_type.into())
    }
    
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CoinType {
    fn default() -> Self {
        Self::native()
    }
}

impl std::fmt::Display for CoinType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Balance is a value type that encapsulates token amount
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Balance {
    value: u64,
}

impl Balance {
    /// Create a new Balance
    pub fn new(value: u64) -> Self {
        Self { value }
    }
    
    /// Get the balance value
    pub fn value(&self) -> u64 {
        self.value
    }
    
    /// Withdraw a specified amount, returns the withdrawn Balance
    pub fn withdraw(&mut self, amount: u64) -> Result<Balance, String> {
        if self.value < amount {
            return Err(format!(
                "Insufficient balance: have {}, need {}",
                self.value, amount
            ));
        }
        self.value -= amount;
        Ok(Balance::new(amount))
    }
    
    /// Deposit Balance
    pub fn deposit(&mut self, balance: Balance) -> Result<(), String> {
        self.value = self.value.checked_add(balance.value)
            .ok_or("Balance overflow")?;
        Ok(())
    }
    
    /// Destroy Balance (used for merging)
    pub fn destroy(self) -> u64 {
        self.value
    }
}

/// Coin object data - represents transferable tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinData {
    /// Coin type (e.g., "SETU", "USDC")
    pub coin_type: CoinType,
    /// Balance amount
    pub balance: Balance,
}

/// Coin type alias
pub type Coin = Object<CoinData>;

impl Coin {
    /// Create a new Coin object with native coin type
    /// 
    /// # Parameters
    /// - `owner`: Owner of the Coin (usually an SBT's Address)
    /// - `value`: Initial balance
    pub fn new(owner: Address, value: u64) -> Self {
        Self::new_with_type(owner, value, CoinType::native())
    }
    
    /// Create a new Coin object with specified coin type
    /// 
    /// # Parameters
    /// - `owner`: Owner of the Coin
    /// - `value`: Initial balance
    /// - `coin_type`: Type of the coin
    pub fn new_with_type(owner: Address, value: u64, coin_type: CoinType) -> Self {
        let id = generate_object_id(format!("coin:{}:{}:{}", owner, coin_type, value).as_bytes());
        let data = CoinData {
            coin_type,
            balance: Balance::new(value),
        };
        Object::new_owned(id, owner, data)
    }
    
    /// Get coin type
    pub fn coin_type(&self) -> &CoinType {
        &self.data.coin_type
    }
    
    /// Get balance
    pub fn value(&self) -> u64 {
        self.data.balance.value()
    }
    
    /// Split a specified amount into a new Coin
    /// 
    /// # Parameters
    /// - `amount`: Amount to split
    /// - `new_owner`: Owner of the new Coin
    /// 
    /// # Returns
    /// Returns the newly created Coin object
    pub fn split(&mut self, amount: u64, new_owner: Address) -> Result<Coin, String> {
        let withdrawn = self.data.balance.withdraw(amount)?;
        let new_coin = Coin::new_with_type(new_owner, withdrawn.value(), self.data.coin_type.clone());
        self.increment_version();
        Ok(new_coin)
    }
    
    /// Merge another Coin into the current Coin
    /// 
    /// # Parameters
    /// - `other`: The Coin object to merge (must be same coin type)
    pub fn merge(&mut self, other: Coin) -> Result<(), String> {
        if self.data.coin_type != other.data.coin_type {
            return Err(format!(
                "Cannot merge different coin types: {} vs {}",
                self.data.coin_type, other.data.coin_type
            ));
        }
        self.data.balance.deposit(other.data.balance)?;
        self.increment_version();
        Ok(())
    }
    
    /// Transfer ownership of the Coin
    /// 
    /// # Parameters
    /// - `new_owner`: New owner
    pub fn transfer(&mut self, new_owner: Address) {
        self.transfer_to(new_owner);
    }
    
    /// Convert to storage-compatible CoinState format (BCS serialized)
    /// 
    /// This is the canonical method for preparing Coin data for Merkle tree storage.
    /// All runtime state changes should use this method instead of JSON serialization.
    /// 
    /// # Returns
    /// BCS-serialized bytes of CoinState
    /// 
    /// # Example
    /// ```ignore
    /// let coin = create_coin(owner, 1000);
    /// let state_bytes = coin.to_coin_state_bytes();
    /// // Use state_bytes in StateChange.new_state
    /// ```
    pub fn to_coin_state(&self) -> CoinState {
        CoinState {
            owner: self.metadata.owner
                .as_ref()
                .map(|a| a.to_string())
                .unwrap_or_default(),
            balance: self.data.balance.value(),
            version: self.metadata.version,
            coin_type: self.data.coin_type.as_str().to_string(),
        }
    }
    
    /// Convert to BCS-serialized bytes for storage
    /// 
    /// Convenience method that combines `to_coin_state()` and serialization.
    pub fn to_coin_state_bytes(&self) -> Vec<u8> {
        self.to_coin_state().to_bytes()
    }
}

/// Helper function: create native Coin
pub fn create_coin(owner: Address, value: u64) -> Coin {
    Coin::new(owner, value)
}

/// Helper function: create Coin with specific type
pub fn create_typed_coin(owner: Address, value: u64, coin_type: impl Into<String>) -> Coin {
    Coin::new_with_type(owner, value, CoinType::new(coin_type))
}

/// Generate deterministic coin ObjectId for an address and subnet
/// 
/// Convention: coin_object_id = SHA256("coin:" || address || ":" || subnet_id)
/// 
/// This is the canonical ID generation used by both storage layer and runtime.
/// Each subnet has exactly one native token (1:1 binding), so subnet_id
/// uniquely identifies the token type.
/// 
/// # Arguments
/// * `owner` - Owner address (will be converted to hex string)
/// * `subnet_id` - Subnet ID (e.g., "ROOT", "gaming-subnet")
/// 
/// # Returns
/// Deterministic 32-byte ObjectId
pub fn deterministic_coin_id(owner: &Address, subnet_id: &str) -> ObjectId {
    let mut hasher = Sha256::new();
    hasher.update(b"coin:");
    hasher.update(owner.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(subnet_id.as_bytes());
    ObjectId::new(hasher.finalize().into())
}

/// Generate deterministic coin ObjectId from string address
/// 
/// Convenience function for when you have a string address instead of Address.
pub fn deterministic_coin_id_from_str(owner: &str, subnet_id: &str) -> ObjectId {
    let mut hasher = Sha256::new();
    hasher.update(b"coin:");
    hasher.update(owner.as_bytes());
    hasher.update(b":");
    hasher.update(subnet_id.as_bytes());
    ObjectId::new(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_balance_operations() {
        let mut balance = Balance::new(1000);
        
        // Withdraw
        let withdrawn = balance.withdraw(300).unwrap();
        assert_eq!(balance.value(), 700);
        assert_eq!(withdrawn.value(), 300);
        
        // Deposit
        balance.deposit(withdrawn).unwrap();
        assert_eq!(balance.value(), 1000);
        
        // Insufficient balance
        assert!(balance.withdraw(2000).is_err());
    }
    
    #[test]
    fn test_balance_overflow() {
        let mut balance = Balance::new(u64::MAX - 100);
        let to_add = Balance::new(200);
        assert!(balance.deposit(to_add).is_err());
    }
    
    #[test]
    fn test_coin_creation() {
        let owner = Address::from_str_id("sbt_alice");
        let coin = Coin::new(owner, 1000);
        
        assert_eq!(coin.value(), 1000);
        assert_eq!(coin.metadata.owner.as_ref().unwrap(), &owner);
        assert_eq!(coin.metadata.version, 1);
        assert_eq!(coin.coin_type().as_str(), CoinType::NATIVE);
    }
    
    #[test]
    fn test_coin_with_type() {
        let owner = Address::from_str_id("sbt_alice");
        let coin = Coin::new_with_type(owner, 1000, CoinType::new("USDC"));
        
        assert_eq!(coin.value(), 1000);
        assert_eq!(coin.coin_type().as_str(), "USDC");
    }
    
    #[test]
    fn test_coin_split() {
        let owner = Address::from_str_id("sbt_alice");
        let mut coin = Coin::new(owner, 1000);
        
        let new_owner = Address::from_str_id("sbt_bob");
        let new_coin = coin.split(300, new_owner).unwrap();
        
        assert_eq!(coin.value(), 700);
        assert_eq!(new_coin.value(), 300);
        assert_eq!(coin.metadata.version, 2);
        assert_eq!(new_coin.metadata.version, 1);
        assert_eq!(new_coin.metadata.owner.as_ref().unwrap(), &new_owner);
        // Split should preserve coin type
        assert_eq!(new_coin.coin_type(), coin.coin_type());
    }
    
    #[test]
    fn test_coin_split_insufficient() {
        let owner = Address::from_str_id("sbt_alice");
        let mut coin = Coin::new(owner, 100);
        
        let result = coin.split(200, Address::from_str_id("sbt_bob"));
        assert!(result.is_err());
    }
    
    #[test]
    fn test_coin_merge() {
        let owner = Address::from_str_id("sbt_alice");
        let mut coin1 = Coin::new(owner, 1000);
        let coin2 = Coin::new(owner, 500);
        
        coin1.merge(coin2).unwrap();
        
        assert_eq!(coin1.value(), 1500);
        assert_eq!(coin1.metadata.version, 2);
    }
    
    #[test]
    fn test_coin_merge_different_types() {
        let owner = Address::from_str_id("sbt_alice");
        let mut coin1 = Coin::new(owner, 1000);
        let coin2 = Coin::new_with_type(owner, 500, CoinType::new("USDC"));
        
        // Cannot merge different coin types
        assert!(coin1.merge(coin2).is_err());
    }
    
    #[test]
    fn test_coin_transfer() {
        let owner = Address::from_str_id("sbt_alice");
        let mut coin = Coin::new(owner, 1000);
        
        let new_owner = Address::from_str_id("sbt_bob");
        coin.transfer(new_owner);
        
        assert_eq!(coin.metadata.owner.as_ref().unwrap(), &new_owner);
        assert_eq!(coin.metadata.version, 2);
    }
    
    #[test]
    fn test_coin_to_coin_state() {
        // Test conversion from Coin to CoinState
        let owner = Address::from_str_id("sbt_alice");
        let coin = Coin::new_with_type(owner, 1000, CoinType::new("ROOT"));
        
        let state = coin.to_coin_state();
        
        assert_eq!(state.owner, owner.to_string());
        assert_eq!(state.balance, 1000);
        assert_eq!(state.version, 1);
        assert_eq!(state.coin_type, "ROOT");
    }
    
    #[test]
    fn test_coin_state_bcs_roundtrip() {
        // Test that CoinState can be serialized and deserialized with BCS
        let state = CoinState::new_with_type("alice".to_string(), 5000, "gaming-subnet".to_string());
        
        let bytes = state.to_bytes();
        let recovered = CoinState::from_bytes(&bytes).expect("Should deserialize");
        
        assert_eq!(recovered.owner, "alice");
        assert_eq!(recovered.balance, 5000);
        assert_eq!(recovered.version, 1);
        assert_eq!(recovered.coin_type, "gaming-subnet");
    }
    
    #[test]
    fn test_coin_to_coin_state_bytes_compatibility() {
        // Critical test: Verify runtime output (Coin -> BCS) can be parsed by storage layer
        let owner = Address::from_str_id("sbt_bob");
        let coin = Coin::new_with_type(owner, 2500, CoinType::new("ROOT"));
        
        // This is what runtime outputs to StateChange.new_state
        let bytes = coin.to_coin_state_bytes();
        
        // This is how storage layer parses it
        let parsed = CoinState::from_bytes(&bytes).expect("Storage should parse runtime output");
        
        // Verify all fields match
        assert_eq!(parsed.owner, owner.to_string());
        assert_eq!(parsed.balance, 2500);
        assert_eq!(parsed.version, coin.metadata.version);
        assert_eq!(parsed.coin_type, "ROOT");
    }
    
    #[test]
    fn test_deterministic_coin_id() {
        use crate::object::ObjectId;
        
        // Test deterministic ID generation
        let id1 = deterministic_coin_id_from_str("alice", "ROOT");
        let id2 = deterministic_coin_id_from_str("alice", "ROOT");
        assert_eq!(id1, id2, "Same input should produce same ID");
        
        // Different address should produce different ID
        let id3 = deterministic_coin_id_from_str("bob", "ROOT");
        assert_ne!(id1, id3, "Different address should produce different ID");
        
        // Different subnet should produce different ID
        let id4 = deterministic_coin_id_from_str("alice", "gaming-subnet");
        assert_ne!(id1, id4, "Different subnet should produce different ID");
        
        // Test Address-based version produces consistent IDs
        let addr = Address::from_str_id("alice");
        let id5 = deterministic_coin_id(&addr, "ROOT");
        let id6 = deterministic_coin_id(&addr, "ROOT");
        assert_eq!(id5, id6, "Same Address should produce same ID");
        
        // Different subnets produce different IDs
        let id_root = deterministic_coin_id_from_str("alice", "ROOT");
        let id_other = deterministic_coin_id_from_str("alice", "OTHER");
        assert_ne!(id_root, id_other, "Different subnets should have different IDs");
    }
}
