//! RelationGraph Object - Social Relationship Graph
//! 
//! Design Philosophy:
//! - RelationGraph is a resource object owned by Address
//! - One Address can have multiple RelationGraphs (friend circle, work circle, etc.)
//! - RelationGraph stores relationships to other users
//! - UserRelationNetwork is a specialized structure for user registration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::object::{Object, ObjectId, Address, generate_object_id};
use crate::subnet::SubnetId;

// ============================================================================
// Relation Type Constants
// ============================================================================

/// Predefined relation type constants
pub mod relation_type {
    /// Invited by - who invited this user to register
    pub const INVITED_BY: &str = "invited_by";
    /// Invited - users this user has invited
    pub const INVITED: &str = "invited";
    /// Chatted - users this user has chatted with
    pub const CHATTED: &str = "chatted";
    /// Trusted - users this user trusts
    pub const TRUSTED: &str = "trusted";
    /// Follows - users this user follows
    pub const FOLLOWS: &str = "follows";
    /// Friend - mutual friendship (bidirectional)
    pub const FRIEND: &str = "friend";
    /// Collaborated - users collaborated with in tasks
    pub const COLLABORATED: &str = "collaborated";
    /// Traded - users traded with
    pub const TRADED: &str = "traded";
}

/// Relationship edge
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Relation {
    /// Target SBT's ID
    pub target_sbt: ObjectId,
    
    /// Relationship type
    pub relation_type: String,
    
    /// Relationship weight (used for algorithms)
    pub weight: u32,
    
    /// Creation time
    pub created_at: u64,
    
    /// Metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// Relationship graph data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationGraphData {
    /// Owner (SBT's ID)
    pub owner_sbt: ObjectId,
    
    /// Owner address (for convenience)
    pub owner_address: Address,
    
    /// Graph type/name
    pub graph_type: String,
    
    /// Relationship list
    pub relations: Vec<Relation>,
    
    /// Creation time
    pub created_at: u64,
    
    /// Update time
    pub updated_at: u64,
}

/// RelationGraph type alias
pub type RelationGraph = Object<RelationGraphData>;

impl RelationGraphData {
    /// Create a new relationship graph
    pub fn new(owner_sbt: ObjectId, owner_address: Address, graph_type: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        Self {
            owner_sbt,
            owner_address,
            graph_type,
            relations: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
    
    /// Add relationship
    pub fn add_relation(&mut self, target_sbt: ObjectId, relation_type: String, weight: u32) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        let relation = Relation {
            target_sbt,
            relation_type,
            weight,
            created_at: now,
            metadata: std::collections::HashMap::new(),
        };
        
        self.relations.push(relation);
        self.touch();
    }
    
    /// Remove relationship
    pub fn remove_relation(&mut self, target_sbt: &ObjectId, relation_type: &str) -> bool {
        let initial_len = self.relations.len();
        self.relations.retain(|r| {
            !(r.target_sbt == *target_sbt && r.relation_type == relation_type)
        });
        
        if self.relations.len() < initial_len {
            self.touch();
            true
        } else {
            false
        }
    }
    
    /// Get all relationships of specified type
    pub fn get_relations_by_type(&self, relation_type: &str) -> Vec<&Relation> {
        self.relations
            .iter()
            .filter(|r| r.relation_type == relation_type)
            .collect()
    }
    
    /// Get relationship to specified target
    pub fn get_relation(&self, target_sbt: &ObjectId, relation_type: &str) -> Option<&Relation> {
        self.relations
            .iter()
            .find(|r| r.target_sbt == *target_sbt && r.relation_type == relation_type)
    }
    
    /// Update relationship weight
    pub fn update_weight(&mut self, target_sbt: &ObjectId, relation_type: &str, weight: u32) -> bool {
        if let Some(relation) = self.relations
            .iter_mut()
            .find(|r| r.target_sbt == *target_sbt && r.relation_type == relation_type) 
        {
            relation.weight = weight;
            self.touch();
            true
        } else {
            false
        }
    }
    
    /// Get relationship count
    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }
    
    /// Get relationship count by type
    pub fn relation_count_by_type(&self, relation_type: &str) -> usize {
        self.relations
            .iter()
            .filter(|r| r.relation_type == relation_type)
            .count()
    }
    
    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }
}

impl RelationGraph {
    /// Create a new relationship graph object
    pub fn new(owner_sbt: ObjectId, owner_address: Address, graph_type: String) -> Self {
        let id = generate_object_id(
            format!("graph:{}:{}", owner_sbt, graph_type).as_bytes()
        );
        let data = RelationGraphData::new(owner_sbt, owner_address, graph_type);
        
        Object::new_owned(id, owner_address, data)
    }
}

/// Helper function: create social relationship graph
pub fn create_social_graph(owner_sbt: ObjectId, owner_address: Address) -> RelationGraph {
    RelationGraph::new(owner_sbt, owner_address, "social".to_string())
}

/// Helper function: create professional relationship graph
pub fn create_professional_graph(owner_sbt: ObjectId, owner_address: Address) -> RelationGraph {
    RelationGraph::new(owner_sbt, owner_address, "professional".to_string())
}

// ============================================================================
// User Relation Network - Specialized for User Registration
// ============================================================================

/// Subnet interaction summary - tracks user interactions within a subnet
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubnetInteractionSummary {
    /// Number of unique users interacted with
    pub interaction_count: u64,
    /// Recent users interacted with (limited to last N)
    pub recent_interactions: Vec<Address>,
    /// Last interaction timestamp
    pub last_interaction: u64,
    /// Total interaction events count
    pub total_events: u64,
}

impl SubnetInteractionSummary {
    /// Create a new empty summary
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Record an interaction with a user
    pub fn record_interaction(&mut self, user: Address) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // Check if this is a new user
        if !self.recent_interactions.contains(&user) {
            self.interaction_count += 1;
            // Keep only last 100 recent interactions
            if self.recent_interactions.len() >= 100 {
                self.recent_interactions.remove(0);
            }
            self.recent_interactions.push(user);
        }
        
        self.total_events += 1;
        self.last_interaction = now;
    }
}

/// User Relation Network - stores all relationship data for a user
/// 
/// This is created when a user registers and tracks:
/// - Who invited this user
/// - All relationships built over time
/// - Subnet-specific interaction summaries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRelationNetwork {
    /// User's address (owner of this network)
    pub user: Address,
    
    /// Inviter's address (who invited this user to register)
    pub invited_by: Option<Address>,
    
    /// Invite code used during registration
    pub invite_code: Option<String>,
    
    /// Main relation graph (stores detailed relationships)
    pub relation_graph: RelationGraphData,
    
    /// Per-subnet interaction summaries
    pub subnet_interactions: HashMap<SubnetId, SubnetInteractionSummary>,
    
    /// Total number of users this user has invited
    pub invite_count: u64,
    
    /// Creation timestamp
    pub created_at: u64,
    
    /// Last update timestamp
    pub updated_at: u64,
}

/// UserRelationNetwork as an Object
pub type UserRelationNetworkObject = Object<UserRelationNetwork>;

impl UserRelationNetwork {
    /// Create a new user relation network
    pub fn new(user: Address, invited_by: Option<Address>, invite_code: Option<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // Create a default relation graph for this user
        let owner_id = generate_object_id(format!("user_network:{}", user).as_bytes());
        let relation_graph = RelationGraphData::new(owner_id, user, "user_network".to_string());
        
        Self {
            user,
            invited_by,
            invite_code,
            relation_graph,
            subnet_interactions: HashMap::new(),
            invite_count: 0,
            created_at: now,
            updated_at: now,
        }
    }
    
    /// Add a relationship to another user
    pub fn add_relation(&mut self, target: Address, relation_type: &str, weight: u32) {
        let target_id = generate_object_id(format!("user:{}", target).as_bytes());
        self.relation_graph.add_relation(target_id, relation_type.to_string(), weight);
        self.touch();
    }
    
    /// Remove a relationship
    pub fn remove_relation(&mut self, target: Address, relation_type: &str) -> bool {
        let target_id = generate_object_id(format!("user:{}", target).as_bytes());
        let removed = self.relation_graph.remove_relation(&target_id, relation_type);
        if removed {
            self.touch();
        }
        removed
    }
    
    /// Record an interaction in a subnet
    pub fn record_subnet_interaction(&mut self, subnet_id: SubnetId, with_user: Address) {
        self.subnet_interactions
            .entry(subnet_id)
            .or_insert_with(SubnetInteractionSummary::new)
            .record_interaction(with_user);
        self.touch();
    }
    
    /// Increment invite count (when this user invites someone)
    pub fn increment_invite_count(&mut self) {
        self.invite_count += 1;
        self.touch();
    }
    
    /// Get total relation count
    pub fn relation_count(&self) -> usize {
        self.relation_graph.relation_count()
    }
    
    /// Get relations by type
    pub fn get_relations_by_type(&self, relation_type: &str) -> Vec<&Relation> {
        self.relation_graph.get_relations_by_type(relation_type)
    }
    
    /// Get subnet interaction summary
    pub fn get_subnet_summary(&self, subnet_id: &SubnetId) -> Option<&SubnetInteractionSummary> {
        self.subnet_interactions.get(subnet_id)
    }
    
    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }
}

/// Helper function: create a user relation network object
pub fn create_user_relation_network(
    user: Address,
    invited_by: Option<Address>,
    invite_code: Option<String>,
) -> UserRelationNetworkObject {
    let id = generate_object_id(format!("user_relation_network:{}", user).as_bytes());
    let data = UserRelationNetwork::new(user, invited_by, invite_code);
    Object::new_owned(id, user, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_relation_graph() {
        let owner_sbt = generate_object_id(b"sbt_alice");
        let owner_address = Address::from_str_id("alice");
        let graph = create_social_graph(owner_sbt, owner_address);
        
        assert_eq!(graph.data.owner_sbt, owner_sbt);
        assert_eq!(graph.data.graph_type, "social");
        assert_eq!(graph.data.relation_count(), 0);
    }
    
    #[test]
    fn test_add_relation() {
        let owner_sbt = generate_object_id(b"sbt_alice");
        let owner_address = Address::from_str_id("alice");
        let mut data = RelationGraphData::new(owner_sbt, owner_address, "social".to_string());
        
        let bob_sbt = generate_object_id(b"sbt_bob");
        let charlie_sbt = generate_object_id(b"sbt_charlie");
        
        data.add_relation(bob_sbt, "follows".to_string(), 100);
        data.add_relation(charlie_sbt, "trusts".to_string(), 80);
        
        assert_eq!(data.relation_count(), 2);
        assert_eq!(data.relation_count_by_type("follows"), 1);
        assert_eq!(data.relation_count_by_type("trusts"), 1);
    }
    
    #[test]
    fn test_remove_relation() {
        let owner_sbt = generate_object_id(b"sbt_alice");
        let owner_address = Address::from_str_id("alice");
        let mut data = RelationGraphData::new(owner_sbt, owner_address, "social".to_string());
        
        let bob_sbt = generate_object_id(b"sbt_bob");
        let charlie_sbt = generate_object_id(b"sbt_charlie");
        
        data.add_relation(bob_sbt, "follows".to_string(), 100);
        data.add_relation(charlie_sbt, "trusts".to_string(), 80);
        
        let removed = data.remove_relation(&bob_sbt, "follows");
        assert!(removed);
        assert_eq!(data.relation_count(), 1);
        
        let not_removed = data.remove_relation(&bob_sbt, "follows");
        assert!(!not_removed);
    }
    
    #[test]
    fn test_get_relations_by_type() {
        let owner_sbt = generate_object_id(b"sbt_alice");
        let owner_address = Address::from_str_id("alice");
        let mut data = RelationGraphData::new(owner_sbt, owner_address, "social".to_string());
        
        let bob_sbt = generate_object_id(b"sbt_bob");
        let charlie_sbt = generate_object_id(b"sbt_charlie");
        let dave_sbt = generate_object_id(b"sbt_dave");
        
        data.add_relation(bob_sbt, "follows".to_string(), 100);
        data.add_relation(charlie_sbt, "follows".to_string(), 90);
        data.add_relation(dave_sbt, "trusts".to_string(), 80);
        
        let follows = data.get_relations_by_type("follows");
        assert_eq!(follows.len(), 2);
        
        let trusts = data.get_relations_by_type("trusts");
        assert_eq!(trusts.len(), 1);
    }
    
    #[test]
    fn test_update_weight() {
        let owner_sbt = generate_object_id(b"sbt_alice");
        let owner_address = Address::from_str_id("alice");
        let mut data = RelationGraphData::new(owner_sbt, owner_address, "social".to_string());
        
        let bob_sbt = generate_object_id(b"sbt_bob");
        data.add_relation(bob_sbt, "follows".to_string(), 100);
        
        let updated = data.update_weight(&bob_sbt, "follows", 150);
        assert!(updated);
        
        let relation = data.get_relation(&bob_sbt, "follows").unwrap();
        assert_eq!(relation.weight, 150);
    }
}
