//! Object Store trait for Coin/Profile/Credential/RelationGraph storage operations
use setu_types::{
    ObjectId, Address, SubnetId, CoinType,
    Coin, Profile, Credential, RelationGraph, AccountView,
    UserRelationNetworkObject, UserSubnetActivity,
    SetuResult,
};

/// Object store interface for storing and retrieving Setu objects
/// 
/// Address is the root of ownership. All objects are owned by an Address.
pub trait ObjectStore {
    // ========== Coin operations ==========
    
    /// Store a coin and update indexes
    fn store_coin(&self, coin: &Coin) -> SetuResult<ObjectId>;
    
    /// Get a coin by ID
    fn get_coin(&self, id: &ObjectId) -> SetuResult<Option<Coin>>;
    
    /// Get all coins owned by an address
    fn get_coins_by_owner(&self, owner: &Address) -> SetuResult<Vec<Coin>>;
    
    /// Get coins owned by an address filtered by coin type
    /// 
    /// This is essential for multi-subnet scenarios where each subnet
    /// may have its own token type.
    fn get_coins_by_owner_and_type(&self, owner: &Address, coin_type: &CoinType) -> SetuResult<Vec<Coin>>;
    
    /// Update a coin (replaces existing)
    fn update_coin(&self, coin: &Coin) -> SetuResult<()>;
    
    /// Delete a coin and clean up indexes
    fn delete_coin(&self, id: &ObjectId) -> SetuResult<()>;
    
    // ========== Profile operations ==========
    
    /// Store a profile and update indexes
    fn store_profile(&self, profile: &Profile) -> SetuResult<ObjectId>;
    
    /// Get a profile by ID
    fn get_profile(&self, id: &ObjectId) -> SetuResult<Option<Profile>>;
    
    /// Get profile by owner address (one address has at most one profile)
    fn get_profile_by_address(&self, address: &Address) -> SetuResult<Option<Profile>>;
    
    /// Update a profile (replaces existing)
    fn update_profile(&self, profile: &Profile) -> SetuResult<()>;
    
    /// Delete a profile and clean up indexes
    fn delete_profile(&self, id: &ObjectId) -> SetuResult<()>;
    
    // ========== Credential operations ==========
    
    /// Store a credential and update indexes
    fn store_credential(&self, credential: &Credential) -> SetuResult<ObjectId>;
    
    /// Get a credential by ID
    fn get_credential(&self, id: &ObjectId) -> SetuResult<Option<Credential>>;
    
    /// Get all credentials for an address (holder)
    fn get_credentials_by_holder(&self, holder: &Address) -> SetuResult<Vec<Credential>>;
    
    /// Get credentials issued by an address
    fn get_credentials_by_issuer(&self, issuer: &Address) -> SetuResult<Vec<Credential>>;
    
    /// Update a credential (replaces existing)
    fn update_credential(&self, credential: &Credential) -> SetuResult<()>;
    
    /// Delete a credential and clean up indexes
    fn delete_credential(&self, id: &ObjectId) -> SetuResult<()>;
    
    // ========== RelationGraph operations ==========
    
    /// Store a relation graph and update indexes
    fn store_graph(&self, graph: &RelationGraph) -> SetuResult<ObjectId>;
    
    /// Get a relation graph by ID
    fn get_graph(&self, id: &ObjectId) -> SetuResult<Option<RelationGraph>>;
    
    /// Get all relation graphs owned by an address
    fn get_graphs_by_owner(&self, owner: &Address) -> SetuResult<Vec<RelationGraph>>;
    
    /// Update a relation graph (replaces existing)
    fn update_graph(&self, graph: &RelationGraph) -> SetuResult<()>;
    
    /// Delete a relation graph and clean up indexes
    fn delete_graph(&self, id: &ObjectId) -> SetuResult<()>;
    
    // ========== UserRelationNetwork operations ==========
    
    /// Store a user relation network
    fn store_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<ObjectId>;
    
    /// Get a user relation network by user address
    fn get_user_relation_network(&self, user: &Address) -> SetuResult<Option<UserRelationNetworkObject>>;
    
    /// Update a user relation network
    fn update_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<()>;
    
    /// Delete a user relation network
    fn delete_user_relation_network(&self, user: &Address) -> SetuResult<()>;
    
    // ========== UserSubnetActivity operations ==========
    
    /// Store user subnet activity
    fn store_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()>;
    
    /// Get user subnet activity for a specific subnet
    fn get_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<Option<UserSubnetActivity>>;
    
    /// Get all subnet activities for a user
    fn get_user_all_subnet_activities(&self, user: &Address) -> SetuResult<Vec<UserSubnetActivity>>;
    
    /// Update user subnet activity
    fn update_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()>;
    
    /// Delete user subnet activity
    fn delete_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<()>;
    
    // ========== Aggregation ==========
    
    /// Load AccountView (aggregates Profile, Credentials, Coins, and Graphs)
    fn load_account_view(&self, address: &Address) -> SetuResult<AccountView>;
}
