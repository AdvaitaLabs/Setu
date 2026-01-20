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

/// Subnet type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubnetType {
    /// ROOT subnet (SubnetId = 0)
    Root,
    /// System reserved subnets (type byte = 0x01)
    SystemReserved,
    /// Application subnets (type byte = 0x02)
    App,
    /// Organization subnet (for a company/DAO)
    Organization,
    /// Personal subnet (for individual users)
    Personal,
    /// Unknown/invalid type
    Unknown,
}

impl Default for SubnetType {
    fn default() -> Self {
        SubnetType::App
    }
}

impl std::fmt::Display for SubnetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubnetType::Root => write!(f, "root"),
            SubnetType::SystemReserved => write!(f, "system"),
            SubnetType::App => write!(f, "application"),
            SubnetType::Organization => write!(f, "organization"),
            SubnetType::Personal => write!(f, "personal"),
            SubnetType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Unique identifier for a subnet (32 bytes)
/// 
/// # Encoding
/// 
/// SubnetId uses first byte as type marker:
/// - `0x00`: ROOT subnet (all zeros)
/// - `0x01`: System reserved subnets
/// - `0x02`: Application subnets
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct SubnetId([u8; 32]);

impl SubnetId {
    /// The root/system subnet (for global operations)
    pub const ROOT: SubnetId = SubnetId([0u8; 32]);
    
    /// Type byte for system reserved subnets
    pub const SYSTEM_PREFIX: u8 = 0x01;
    
    /// Type byte for application subnets
    pub const APP_PREFIX: u8 = 0x02;
    
    /// Create from raw bytes
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    
    /// Create from a string identifier (hashes the string)
    /// Note: This creates an APP type subnet by default
    pub fn from_str_id(id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"SUBNET:");
        hasher.update(id.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        // Mark as APP subnet
        bytes[0] = Self::APP_PREFIX;
        Self(bytes)
    }
    
    /// Create a new app subnet ID from creator address, name and nonce
    pub fn new_app(creator: &Address, name: &str, nonce: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(&[Self::APP_PREFIX]);
        hasher.update(creator.as_bytes());
        hasher.update(name.as_bytes());
        hasher.update(nonce.to_le_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes[0] = Self::APP_PREFIX;
        bytes[1..].copy_from_slice(&result[..31]);
        Self(bytes)
    }
    
    /// Create a simple app subnet for testing (uses id as seed)
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_app_simple(id: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(&[Self::APP_PREFIX]);
        hasher.update(id.to_le_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes[0] = Self::APP_PREFIX;
        bytes[1..].copy_from_slice(&result[..31]);
        Self(bytes)
    }
    
    /// Create a system reserved subnet
    pub fn new_system(id: u8) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0] = Self::SYSTEM_PREFIX;
        bytes[1] = id;
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
    
    /// Get owned copy of bytes
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
    
    /// Get shard hint - first 2 bytes can be used for shard routing
    pub fn shard_hint(&self) -> u16 {
        u16::from_be_bytes([self.0[0], self.0[1]])
    }
    
    /// Check if this is the root subnet
    pub fn is_root(&self) -> bool {
        *self == Self::ROOT
    }
    
    /// Check if this is a system/reserved subnet (type byte = 0x00 or 0x01)
    pub fn is_system(&self) -> bool {
        self.0[0] <= Self::SYSTEM_PREFIX
    }
    
    /// Check if this is an app subnet (type byte = 0x02)
    pub fn is_app(&self) -> bool {
        self.0[0] == Self::APP_PREFIX
    }
    
    /// Get the type of this subnet
    pub fn subnet_type(&self) -> SubnetType {
        match self.0[0] {
            0x00 if *self == Self::ROOT => SubnetType::Root,
            0x00 | 0x01 => SubnetType::SystemReserved,
            0x02 => SubnetType::App,
            _ => SubnetType::Unknown,
        }
    }
    
    /// Get the type byte
    pub fn type_byte(&self) -> u8 {
        self.0[0]
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

// ============================================================================
// Subnet Interaction Tracking
// ============================================================================

/// Interaction type within a subnet
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionType {
    /// Chat/message interaction
    Chat,
    /// Trade/exchange interaction
    Trade,
    /// Collaboration on a task
    Collaborate,
    /// Following another user
    Follow,
    /// Endorsement/recommendation
    Endorse,
    /// Custom interaction type
    Custom(String),
}

impl InteractionType {
    pub fn as_str(&self) -> &str {
        match self {
            InteractionType::Chat => "chat",
            InteractionType::Trade => "trade",
            InteractionType::Collaborate => "collaborate",
            InteractionType::Follow => "follow",
            InteractionType::Endorse => "endorse",
            InteractionType::Custom(s) => s.as_str(),
        }
    }
}

impl From<&str> for InteractionType {
    fn from(s: &str) -> Self {
        match s {
            "chat" => InteractionType::Chat,
            "trade" => InteractionType::Trade,
            "collaborate" => InteractionType::Collaborate,
            "follow" => InteractionType::Follow,
            "endorse" => InteractionType::Endorse,
            other => InteractionType::Custom(other.to_string()),
        }
    }
}

/// A single interaction record within a subnet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetInteraction {
    /// The other user involved in this interaction
    pub with_user: Address,
    /// Type of interaction
    pub interaction_type: InteractionType,
    /// Timestamp of the interaction
    pub timestamp: u64,
    /// Optional metadata (e.g., message hash, trade details)
    pub metadata: Option<String>,
    /// Event ID that created this interaction (for traceability)
    pub event_id: Option<String>,
}

impl SubnetInteraction {
    pub fn new(with_user: Address, interaction_type: InteractionType) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        Self {
            with_user,
            interaction_type,
            timestamp: now,
            metadata: None,
            event_id: None,
        }
    }
    
    pub fn with_metadata(mut self, metadata: String) -> Self {
        self.metadata = Some(metadata);
        self
    }
    
    pub fn with_event_id(mut self, event_id: String) -> Self {
        self.event_id = Some(event_id);
        self
    }
}

/// Local relation built within a subnet (to be synced to global relation network)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRelation {
    /// Target user address
    pub target: Address,
    /// Relation type (e.g., "friend", "trusted", "followed")
    pub relation_type: String,
    /// Relation weight/strength (0-100)
    pub weight: u32,
    /// When this relation was established
    pub established_at: u64,
    /// Whether this has been synced to the global UserRelationNetwork
    pub synced_to_global: bool,
    /// Source interactions that led to this relation
    pub source_interactions: Vec<String>, // event_ids
}

impl LocalRelation {
    pub fn new(target: Address, relation_type: impl Into<String>, weight: u32) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        Self {
            target,
            relation_type: relation_type.into(),
            weight,
            established_at: now,
            synced_to_global: false,
            source_interactions: Vec::new(),
        }
    }
    
    pub fn mark_synced(&mut self) {
        self.synced_to_global = true;
    }
    
    pub fn add_source_interaction(&mut self, event_id: String) {
        self.source_interactions.push(event_id);
    }
}

/// Extended user subnet membership with interaction tracking
/// 
/// This extends the basic UserSubnetMembership with:
/// - Detailed interaction history within each subnet
/// - Local relations built through interactions
/// - Sync status for relation extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSubnetActivity {
    /// User's address
    pub user: Address,
    /// Subnet ID
    pub subnet_id: SubnetId,
    /// When the user joined this subnet
    pub joined_at: u64,
    /// Recent interactions (limited to last N for storage efficiency)
    pub recent_interactions: Vec<SubnetInteraction>,
    /// Local relations built in this subnet
    pub local_relations: Vec<LocalRelation>,
    /// Total interaction count (even if not all stored)
    pub total_interaction_count: u64,
    /// Unique users interacted with
    pub unique_users_count: u64,
    /// Last activity timestamp
    pub last_activity: u64,
}

impl UserSubnetActivity {
    /// Maximum number of recent interactions to store
    const MAX_RECENT_INTERACTIONS: usize = 100;
    
    pub fn new(user: Address, subnet_id: SubnetId) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        Self {
            user,
            subnet_id,
            joined_at: now,
            recent_interactions: Vec::new(),
            local_relations: Vec::new(),
            total_interaction_count: 0,
            unique_users_count: 0,
            last_activity: now,
        }
    }
    
    /// Record a new interaction
    pub fn record_interaction(&mut self, interaction: SubnetInteraction) {
        // Check if this is a new unique user
        let is_new_user = !self.recent_interactions
            .iter()
            .any(|i| i.with_user == interaction.with_user);
        
        if is_new_user {
            self.unique_users_count += 1;
        }
        
        // Add to recent interactions (with limit)
        if self.recent_interactions.len() >= Self::MAX_RECENT_INTERACTIONS {
            self.recent_interactions.remove(0);
        }
        self.recent_interactions.push(interaction);
        
        self.total_interaction_count += 1;
        self.touch();
    }
    
    /// Add or update a local relation
    pub fn add_local_relation(&mut self, relation: LocalRelation) {
        // Check if relation already exists
        if let Some(existing) = self.local_relations
            .iter_mut()
            .find(|r| r.target == relation.target && r.relation_type == relation.relation_type)
        {
            // Update weight if new is higher
            if relation.weight > existing.weight {
                existing.weight = relation.weight;
            }
            existing.synced_to_global = false; // Mark for re-sync
        } else {
            self.local_relations.push(relation);
        }
        self.touch();
    }
    
    /// Get unsynced local relations
    pub fn get_unsynced_relations(&self) -> Vec<&LocalRelation> {
        self.local_relations
            .iter()
            .filter(|r| !r.synced_to_global)
            .collect()
    }
    
    /// Mark all relations as synced
    pub fn mark_all_synced(&mut self) {
        for relation in &mut self.local_relations {
            relation.synced_to_global = true;
        }
    }
    
    /// Get interactions with a specific user
    pub fn get_interactions_with(&self, user: &Address) -> Vec<&SubnetInteraction> {
        self.recent_interactions
            .iter()
            .filter(|i| &i.with_user == user)
            .collect()
    }
    
    /// Get interactions by type
    pub fn get_interactions_by_type(&self, interaction_type: &InteractionType) -> Vec<&SubnetInteraction> {
        self.recent_interactions
            .iter()
            .filter(|i| &i.interaction_type == interaction_type)
            .collect()
    }
    
    fn touch(&mut self) {
        self.last_activity = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
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
