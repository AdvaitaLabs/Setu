//! AccountView - Application Layer Aggregated View
//! 
//! Design Philosophy:
//! - AccountView is an application layer concept, not an on-chain object
//! - Aggregates all resources owned by an Address
//! - Address is the root of ownership (no SBT intermediate layer)
//! - Provides convenient query interfaces and computed fields
//!
//! Note: Subnet membership is stored independently in `UserSubnetMembership`.
//! Query subnet info separately via the subnet module.

use serde::{Deserialize, Serialize};
use crate::{
    object::{ObjectId, Address},
    profile::{Profile, Credential},
    coin::Coin,
    relation::RelationGraph,
};

/// Account aggregated view - represents an address and all its resources
/// 
/// This is a read-only view that aggregates:
/// - Profile (optional identity display)
/// - Credentials (verifiable attestations)
/// - Coins (assets)
/// - RelationGraphs (social relationships)
///
/// Note: For subnet membership, query `UserSubnetMembership` separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountView {
    /// Owner address (root of ownership)
    pub address: Address,
    
    /// Profile (optional identity display)
    pub profile: Option<Profile>,
    
    /// All credentials
    pub credentials: Vec<Credential>,
    
    /// All owned Coin objects
    pub coins: Vec<Coin>,
    
    /// All owned RelationGraph objects
    pub graphs: Vec<RelationGraph>,
    
    // ========== Computed Fields ==========
    
    /// Total balance (sum of all Coins)
    pub total_balance: u64,
    
    /// Coin count
    pub coin_count: usize,
    
    /// Valid credential count (active and not expired)
    pub valid_credential_count: usize,
    
    /// Relationship graph count
    pub graph_count: usize,
    
    /// Total relationship count
    pub total_relations: usize,
}

impl AccountView {
    /// Build view from address and resources
    pub fn new(
        address: Address,
        profile: Option<Profile>,
        credentials: Vec<Credential>,
        coins: Vec<Coin>,
        graphs: Vec<RelationGraph>,
    ) -> Self {
        let total_balance = coins.iter().map(|c| c.value()).sum();
        let coin_count = coins.len();
        let valid_credential_count = credentials.iter()
            .filter(|c| c.data.is_valid())
            .count();
        let graph_count = graphs.len();
        let total_relations = graphs.iter()
            .map(|g| g.data.relation_count())
            .sum();
        
        Self {
            address,
            profile,
            credentials,
            coins,
            graphs,
            total_balance,
            coin_count,
            valid_credential_count,
            graph_count,
            total_relations,
        }
    }
    
    /// Create empty view for an address
    pub fn empty(address: Address) -> Self {
        Self::new(address, None, vec![], vec![], vec![])
    }
    
    /// Get display name (from profile if available)
    pub fn display_name(&self) -> Option<&String> {
        self.profile.as_ref()
            .and_then(|p| p.data.display_name.as_ref())
    }
    
    /// Get avatar URL (from profile if available)
    pub fn avatar(&self) -> Option<&String> {
        self.profile.as_ref()
            .and_then(|p| p.data.avatar_url.as_ref())
    }
    
    /// Get bio (from profile if available)
    pub fn bio(&self) -> Option<&String> {
        self.profile.as_ref()
            .and_then(|p| p.data.bio.as_ref())
    }
    
    /// Check if account has a profile
    pub fn has_profile(&self) -> bool {
        self.profile.is_some()
    }
    
    // ========== Credential Methods ==========
    
    /// Get all valid credentials
    pub fn valid_credentials(&self) -> Vec<&Credential> {
        self.credentials.iter()
            .filter(|c| c.data.is_valid())
            .collect()
    }
    
    /// Get credentials by type
    pub fn get_credentials_by_type(&self, credential_type: &str) -> Vec<&Credential> {
        self.credentials.iter()
            .filter(|c| c.data.credential_type == credential_type)
            .collect()
    }
    
    /// Check if has valid credential of type
    pub fn has_valid_credential(&self, credential_type: &str) -> bool {
        self.credentials.iter()
            .any(|c| c.data.credential_type == credential_type && c.data.is_valid())
    }
    
    /// Check if has valid KYC
    pub fn has_kyc(&self) -> bool {
        self.has_valid_credential("kyc")
    }
    
    /// Get KYC level (if any)
    pub fn kyc_level(&self) -> Option<&String> {
        self.credentials.iter()
            .find(|c| c.data.credential_type == "kyc" && c.data.is_valid())
            .and_then(|c| c.data.get_claim("level"))
    }
    
    // ========== Coin Methods ==========
    
    /// Check if has sufficient balance
    pub fn has_balance(&self, amount: u64) -> bool {
        self.total_balance >= amount
    }
    
    /// Get coins by type
    pub fn get_coins_by_type(&self, coin_type: &str) -> Vec<&Coin> {
        self.coins.iter()
            .filter(|c| c.data.coin_type.as_str() == coin_type)
            .collect()
    }
    
    /// Get total balance by coin type
    pub fn balance_by_type(&self, coin_type: &str) -> u64 {
        self.coins.iter()
            .filter(|c| c.data.coin_type.as_str() == coin_type)
            .map(|c| c.value())
            .sum()
    }
    
    /// Get list of Coins available for payment (sorted by balance, largest first)
    pub fn get_payable_coins(&self, amount: u64) -> Option<Vec<&Coin>> {
        let mut sorted_coins: Vec<&Coin> = self.coins.iter().collect();
        sorted_coins.sort_by(|a, b| b.value().cmp(&a.value()));
        
        let mut total = 0;
        let mut selected = Vec::new();
        
        for coin in sorted_coins {
            if total >= amount {
                break;
            }
            selected.push(coin);
            total += coin.value();
        }
        
        if total >= amount {
            Some(selected)
        } else {
            None
        }
    }
    
    // ========== Graph Methods ==========
    
    /// Get relationship graph by type
    pub fn get_graph_by_type(&self, graph_type: &str) -> Option<&RelationGraph> {
        self.graphs.iter()
            .find(|g| g.data.graph_type == graph_type)
    }
    
    /// Get all relationship graphs of specified type
    pub fn get_graphs_by_type(&self, graph_type: &str) -> Vec<&RelationGraph> {
        self.graphs.iter()
            .filter(|g| g.data.graph_type == graph_type)
            .collect()
    }
    
    /// Get social relationship graph
    pub fn social_graph(&self) -> Option<&RelationGraph> {
        self.get_graph_by_type("social")
    }
    
    /// Get professional relationship graph
    pub fn professional_graph(&self) -> Option<&RelationGraph> {
        self.get_graph_by_type("professional")
    }
    
    /// Count relationships by relation type
    pub fn count_relations_by_type(&self, relation_type: &str) -> usize {
        self.graphs.iter()
            .map(|g| g.data.relation_count_by_type(relation_type))
            .sum()
    }
    
    /// Get all followed addresses (from all graphs)
    pub fn following(&self) -> Vec<ObjectId> {
        self.graphs.iter()
            .flat_map(|g| g.data.get_relations_by_type("follows"))
            .map(|r| r.target_sbt.clone())
            .collect()
    }
    
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        profile::create_profile,
        profile::create_kyc_credential,
        coin::create_coin,
        relation::create_social_graph,
        object::generate_object_id,
    };
    
    #[test]
    fn test_account_view_creation() {
        let alice = Address::from_str_id("alice");
        
        let mut profile = create_profile(alice.clone());
        profile.data.set_display_name("Alice");
        
        let coins = vec![
            create_coin(alice.clone(), 1000),
            create_coin(alice.clone(), 500),
        ];
        
        let owner_id = generate_object_id(b"alice_owner");
        let graphs = vec![
            create_social_graph(owner_id, alice.clone()),
        ];
        
        let view = AccountView::new(
            alice.clone(),
            Some(profile),
            vec![],
            coins,
            graphs,
        );
        
        assert_eq!(view.address, alice);
        assert_eq!(view.display_name(), Some(&"Alice".to_string()));
        assert_eq!(view.total_balance, 1500);
        assert_eq!(view.coin_count, 2);
        assert_eq!(view.graph_count, 1);
        assert!(view.has_profile());
    }
    
    #[test]
    fn test_empty_account_view() {
        let bob = Address::from_str_id("bob");
        let view = AccountView::empty(bob.clone());
        
        assert_eq!(view.address, bob);
        assert!(view.profile.is_none());
        assert_eq!(view.total_balance, 0);
        assert_eq!(view.coin_count, 0);
        assert!(!view.has_profile());
    }
    
    #[test]
    fn test_credentials() {
        let alice = Address::from_str_id("alice");
        let issuer = Address::from_str_id("kyc_provider");
        
        let kyc = create_kyc_credential(alice.clone(), issuer, "level_2");
        
        let view = AccountView::new(
            alice.clone(),
            None,
            vec![kyc],
            vec![],
            vec![],
        );
        
        assert!(view.has_kyc());
        assert_eq!(view.kyc_level(), Some(&"level_2".to_string()));
        assert_eq!(view.valid_credential_count, 1);
    }
    
    #[test]
    fn test_has_balance() {
        let alice = Address::from_str_id("alice");
        
        let coins = vec![
            create_coin(alice.clone(), 1000),
        ];
        
        let view = AccountView::new(alice, None, vec![], coins, vec![]);
        
        assert!(view.has_balance(500));
        assert!(view.has_balance(1000));
        assert!(!view.has_balance(1001));
    }
    
    #[test]
    fn test_get_payable_coins() {
        let alice = Address::from_str_id("alice");
        
        let coins = vec![
            create_coin(alice.clone(), 1000),
            create_coin(alice.clone(), 500),
            create_coin(alice.clone(), 300),
        ];
        
        let view = AccountView::new(alice, None, vec![], coins, vec![]);
        
        // Need 800, should return [1000]
        let payable = view.get_payable_coins(800);
        assert!(payable.is_some());
        let coins = payable.unwrap();
        assert_eq!(coins.len(), 1);
        assert_eq!(coins[0].value(), 1000);
        
        // Need 1200, should return [1000, 500]
        let payable = view.get_payable_coins(1200);
        assert!(payable.is_some());
        let coins = payable.unwrap();
        assert_eq!(coins.len(), 2);
        
        // Need 2000, insufficient balance
        let payable = view.get_payable_coins(2000);
        assert!(payable.is_none());
    }
    
    #[test]
    fn test_get_graphs() {
        let alice = Address::from_str_id("alice");
        let owner_id = generate_object_id(b"alice_owner");
        let bob_id = generate_object_id(b"bob");
        
        let mut social_graph = create_social_graph(owner_id, alice.clone());
        social_graph.data.add_relation(bob_id, "follows".to_string(), 100);
        
        let graphs = vec![social_graph];
        let view = AccountView::new(alice, None, vec![], vec![], graphs);
        
        assert_eq!(view.total_relations, 1);
        assert!(view.social_graph().is_some());
        assert_eq!(view.count_relations_by_type("follows"), 1);
    }
}
