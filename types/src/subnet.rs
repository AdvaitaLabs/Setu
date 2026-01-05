//! Subnet (Sub-application/Sub-network) Types
//!
//! # Design Philosophy
//!
//! - Each subnet is an independent application with its own events and tokens
//! - Subnets are isolated: transactions within a subnet don't conflict with other subnets
//! - Users can participate in multiple subnets
//! - Routing is based on subnet ID for optimal state locality
//!
//! # Storage Strategy (Independent)
//!
//! `UserSubnetMembership` is stored **independently** from `AccountView`:
//!
//! ```text
//! ┌─────────────────────────────┐     ┌─────────────────────────────┐
//! │    UserSubnetMembership     │     │        AccountView          │
//! │  (Indexed by user/subnet)   │     │   (Profile, Coins, etc.)    │
//! ├─────────────────────────────┤     └─────────────────────────────┘
//! │ - user: Address             │              (separate)
//! │ - joined_subnets            │
//! │ - primary_subnet            │     Query independently:
//! │ - last_activity             │     - get_membership(user)
//! └─────────────────────────────┘     - get_users_in_subnet(subnet_id)
//! ```
//!
//! Benefits:
//! - Efficient subnet-based indexing (find all users in a subnet)
//! - Efficient user-based queries (find all subnets for a user)
//! - AccountView stays lightweight and focused on owned objects

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;

use crate::object::Address;

/// Unique identifier for a subnet (32 bytes)
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct SubnetId([u8; 32]);

impl SubnetId {
    /// The root/system subnet (for global operations)
    pub const ROOT: SubnetId = SubnetId([0u8; 32]);
    
    /// Create from raw bytes
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    
    /// Create from a string identifier (hashes the string)
    pub fn from_str_id(id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"SUBNET:");
        hasher.update(id.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Self(bytes)
    }
    
    /// Create from hex string
    pub fn from_hex(hex_str: &str) -> Result<Self, &'static str> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(hex_str).map_err(|_| "Invalid hex string")?;
        if bytes.len() != 32 {
            return Err("SubnetId must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
    
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    
    /// Get shard hint - first 2 bytes can be used for shard routing
    pub fn shard_hint(&self) -> u16 {
        u16::from_be_bytes([self.0[0], self.0[1]])
    }
    
    /// Check if this is the root subnet
    pub fn is_root(&self) -> bool {
        *self == Self::ROOT
    }
}

impl fmt::Display for SubnetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(&self.0[..8])) // Short display
    }
}

impl fmt::Debug for SubnetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SubnetId({})", self)
    }
}

impl From<&str> for SubnetId {
    fn from(s: &str) -> Self {
        Self::from_str_id(s)
    }
}

/// Subnet metadata/configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetConfig {
    /// Subnet identifier
    pub id: SubnetId,
    
    /// Human-readable name
    pub name: String,
    
    /// Description
    pub description: String,
    
    /// Native token symbol for this subnet (if any)
    pub native_token: Option<String>,
    
    /// Whether the subnet is active
    pub is_active: bool,
    
    /// Creation timestamp
    pub created_at: u64,
    
    /// Creator address
    pub creator: Address,
}

impl SubnetConfig {
    pub fn new(name: impl Into<String>, creator: Address) -> Self {
        let name = name.into();
        let id = SubnetId::from_str_id(&name);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        Self {
            id,
            name,
            description: String::new(),
            native_token: None,
            is_active: true,
            created_at: now,
            creator,
        }
    }
    
    pub fn with_token(mut self, symbol: impl Into<String>) -> Self {
        self.native_token = Some(symbol.into());
        self
    }
    
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }
}

/// User's subnet participation record
/// 
/// This tracks which subnets a user has joined and their status in each.
/// Can be stored as part of Profile or as a separate index.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserSubnetMembership {
    /// User's address
    pub user: Address,
    
    /// Set of subnet IDs the user has joined
    pub joined_subnets: HashSet<SubnetId>,
    
    /// Primary/default subnet for this user
    pub primary_subnet: Option<SubnetId>,
    
    /// Last activity timestamp per subnet
    pub last_activity: std::collections::HashMap<SubnetId, u64>,
}

impl UserSubnetMembership {
    pub fn new(user: Address) -> Self {
        Self {
            user,
            joined_subnets: HashSet::new(),
            primary_subnet: None,
            last_activity: std::collections::HashMap::new(),
        }
    }
    
    /// Join a subnet
    pub fn join(&mut self, subnet_id: SubnetId) {
        self.joined_subnets.insert(subnet_id);
        if self.primary_subnet.is_none() {
            self.primary_subnet = Some(subnet_id);
        }
        self.touch(subnet_id);
    }
    
    /// Leave a subnet
    pub fn leave(&mut self, subnet_id: &SubnetId) {
        self.joined_subnets.remove(subnet_id);
        self.last_activity.remove(subnet_id);
        if self.primary_subnet.as_ref() == Some(subnet_id) {
            self.primary_subnet = self.joined_subnets.iter().next().copied();
        }
    }
    
    /// Check if user is in a subnet
    pub fn is_member(&self, subnet_id: &SubnetId) -> bool {
        self.joined_subnets.contains(subnet_id)
    }
    
    /// Update last activity time
    pub fn touch(&mut self, subnet_id: SubnetId) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.last_activity.insert(subnet_id, now);
    }
    
    /// Get all joined subnets
    pub fn subnets(&self) -> impl Iterator<Item = &SubnetId> {
        self.joined_subnets.iter()
    }
    
    /// Number of subnets joined
    pub fn subnet_count(&self) -> usize {
        self.joined_subnets.len()
    }
}

/// Cross-subnet transaction marker
/// 
/// When a transaction involves multiple subnets, it needs special handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSubnetContext {
    /// Source subnet
    pub source_subnet: SubnetId,
    
    /// Target subnet(s)
    pub target_subnets: Vec<SubnetId>,
    
    /// Whether this requires 2-phase commit
    pub requires_2pc: bool,
}

impl CrossSubnetContext {
    pub fn new(source: SubnetId, targets: Vec<SubnetId>) -> Self {
        let requires_2pc = !targets.is_empty() && targets.iter().any(|t| t != &source);
        Self {
            source_subnet: source,
            target_subnets: targets,
            requires_2pc,
        }
    }
    
    /// Check if this is a single-subnet transaction
    pub fn is_single_subnet(&self) -> bool {
        self.target_subnets.is_empty() || 
        self.target_subnets.iter().all(|t| t == &self.source_subnet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_subnet_id_creation() {
        let id1 = SubnetId::from_str_id("defi-app");
        let id2 = SubnetId::from_str_id("defi-app");
        let id3 = SubnetId::from_str_id("gaming-app");
        
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
    
    #[test]
    fn test_user_membership() {
        let user = Address::from("alice");
        let mut membership = UserSubnetMembership::new(user);
        
        let defi = SubnetId::from_str_id("defi");
        let gaming = SubnetId::from_str_id("gaming");
        
        membership.join(defi);
        membership.join(gaming);
        
        assert!(membership.is_member(&defi));
        assert!(membership.is_member(&gaming));
        assert_eq!(membership.subnet_count(), 2);
        
        membership.leave(&defi);
        assert!(!membership.is_member(&defi));
        assert_eq!(membership.subnet_count(), 1);
    }
    
    #[test]
    fn test_cross_subnet_context() {
        let defi = SubnetId::from_str_id("defi");
        let gaming = SubnetId::from_str_id("gaming");
        
        // Single subnet transaction
        let ctx1 = CrossSubnetContext::new(defi, vec![defi]);
        assert!(ctx1.is_single_subnet());
        assert!(!ctx1.requires_2pc);
        
        // Cross subnet transaction
        let ctx2 = CrossSubnetContext::new(defi, vec![gaming]);
        assert!(!ctx2.is_single_subnet());
        assert!(ctx2.requires_2pc);
    }
}
