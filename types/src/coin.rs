//! Coin Object - Transferable Asset
//! 
//! Design Philosophy:
//! - Coin is an independent object that can be owned by SBT
//! - Supports operations like split, merge, transfer
//! - Balance is a value type, not an object

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use crate::object::{Object, Address, ObjectId, generate_object_id};

/// Coin type identifier (e.g., "SUI", "USDC", "SETU")
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CoinType(pub String);

impl CoinType {
    pub const NATIVE: &'static str = "SETU";
    
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
}

/// Helper function: create native Coin
pub fn create_coin(owner: Address, value: u64) -> Coin {
    Coin::new(owner, value)
}

/// Helper function: create Coin with specific type
pub fn create_typed_coin(owner: Address, value: u64, coin_type: impl Into<String>) -> Coin {
    Coin::new_with_type(owner, value, CoinType::new(coin_type))
}

/// Generate deterministic coin ObjectId for an address and coin type
/// 
/// Convention: coin_object_id = SHA256("coin:" || address || [":" || coin_type])
/// 
/// This is the canonical ID generation used by both storage layer and runtime.
/// For backwards compatibility, "SETU" type uses legacy format without suffix.
/// 
/// # Arguments
/// * `owner` - Owner address (will be converted to hex string)
/// * `coin_type` - Coin type (e.g., "SETU", "USDC", "SUBNET_TOKEN")
/// 
/// # Returns
/// Deterministic 32-byte ObjectId
pub fn deterministic_coin_id(owner: &Address, coin_type: &str) -> ObjectId {
    let mut hasher = Sha256::new();
    hasher.update(b"coin:");
    hasher.update(owner.to_string().as_bytes());
    // For backwards compatibility: SETU uses legacy format without coin_type suffix
    if coin_type != CoinType::NATIVE {
        hasher.update(b":");
        hasher.update(coin_type.as_bytes());
    }
    ObjectId::new(hasher.finalize().into())
}

/// Generate deterministic coin ObjectId from string address
/// 
/// Convenience function for when you have a string address instead of Address.
pub fn deterministic_coin_id_from_str(owner: &str, coin_type: &str) -> ObjectId {
    let mut hasher = Sha256::new();
    hasher.update(b"coin:");
    hasher.update(owner.as_bytes());
    // For backwards compatibility: SETU uses legacy format without coin_type suffix
    if coin_type != CoinType::NATIVE {
        hasher.update(b":");
        hasher.update(coin_type.as_bytes());
    }
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
    fn test_deterministic_coin_id() {
        use crate::object::ObjectId;
        
        // Test SETU (native) coin - should use legacy format (no suffix)
        let id1 = deterministic_coin_id_from_str("alice", "SETU");
        let id2 = deterministic_coin_id_from_str("alice", "SETU");
        assert_eq!(id1, id2, "Same input should produce same ID");
        
        // Different address should produce different ID
        let id3 = deterministic_coin_id_from_str("bob", "SETU");
        assert_ne!(id1, id3, "Different address should produce different ID");
        
        // Different coin type should produce different ID
        let id4 = deterministic_coin_id_from_str("alice", "USDC");
        assert_ne!(id1, id4, "Different coin type should produce different ID");
        
        // Test Address-based version matches string-based version
        let addr = Address::from_str_id("alice");
        let id5 = deterministic_coin_id(&addr, "SETU");
        // Note: Address::from_str_id hashes the string, so direct comparison won't work
        // But same Address should produce same ID
        let id6 = deterministic_coin_id(&addr, "SETU");
        assert_eq!(id5, id6, "Same Address should produce same ID");
        
        // Non-SETU coins should have different format from SETU
        let id_setu = deterministic_coin_id_from_str("alice", "SETU");
        let id_other = deterministic_coin_id_from_str("alice", "OTHER");
        assert_ne!(id_setu, id_other, "SETU and non-SETU should have different IDs");
    }
}
