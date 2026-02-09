//! In-memory implementation of ObjectStore
//!
//! This implementation uses DashMap for lock-free concurrent access,
//! making it suitable for testing and development scenarios.
//!
//! # Example
//! ```rust,ignore
//! use setu_storage::{MemoryObjectStore, ObjectStore};
//!
//! let store = MemoryObjectStore::new();
//! store.store_coin(&coin)?;
//! let retrieved = store.get_coin(&coin.metadata.id)?;
//! ```

use crate::backends::object::ObjectStore;
use dashmap::DashMap;
use setu_types::{
    Address, AccountView, Coin, CoinType, Credential, ObjectId,
    Profile, RelationGraph, SetuError, SetuResult, SubnetId,
    UserRelationNetworkObject, UserSubnetActivity,
};
use std::sync::Arc;

/// In-memory object store using DashMap for concurrent access
#[derive(Debug)]
pub struct MemoryObjectStore {
    // Primary data storage
    coins: Arc<DashMap<ObjectId, Coin>>,
    profiles: Arc<DashMap<ObjectId, Profile>>,
    credentials: Arc<DashMap<ObjectId, Credential>>,
    graphs: Arc<DashMap<ObjectId, RelationGraph>>,
    user_relation_networks: Arc<DashMap<Address, UserRelationNetworkObject>>,
    // Keyed by (user, subnet_id) composite
    user_subnet_activities: Arc<DashMap<(Address, SubnetId), UserSubnetActivity>>,

    // Secondary indexes
    coins_by_owner: Arc<DashMap<Address, Vec<ObjectId>>>,
    // Index for (owner, coin_type) -> Vec<ObjectId>
    coins_by_owner_and_type: Arc<DashMap<(Address, CoinType), Vec<ObjectId>>>,
    profile_by_address: Arc<DashMap<Address, ObjectId>>,
    credentials_by_holder: Arc<DashMap<Address, Vec<ObjectId>>>,
    credentials_by_issuer: Arc<DashMap<Address, Vec<ObjectId>>>,
    graphs_by_owner: Arc<DashMap<Address, Vec<ObjectId>>>,
    // Index for user -> Vec<SubnetId> for fast subnet activity lookup
    subnet_ids_by_user: Arc<DashMap<Address, Vec<SubnetId>>>,
}

impl Default for MemoryObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryObjectStore {
    pub fn new() -> Self {
        Self {
            coins: Arc::new(DashMap::new()),
            profiles: Arc::new(DashMap::new()),
            credentials: Arc::new(DashMap::new()),
            graphs: Arc::new(DashMap::new()),
            user_relation_networks: Arc::new(DashMap::new()),
            user_subnet_activities: Arc::new(DashMap::new()),
            coins_by_owner: Arc::new(DashMap::new()),
            coins_by_owner_and_type: Arc::new(DashMap::new()),
            profile_by_address: Arc::new(DashMap::new()),
            credentials_by_holder: Arc::new(DashMap::new()),
            credentials_by_issuer: Arc::new(DashMap::new()),
            graphs_by_owner: Arc::new(DashMap::new()),
            subnet_ids_by_user: Arc::new(DashMap::new()),
        }
    }

    /// Get count of stored coins
    pub fn coin_count(&self) -> usize {
        self.coins.len()
    }

    /// Get count of stored profiles
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }

    /// Get count of stored credentials
    pub fn credential_count(&self) -> usize {
        self.credentials.len()
    }

    /// Get count of stored graphs
    pub fn graph_count(&self) -> usize {
        self.graphs.len()
    }

    // ========== Helper methods for index operations ==========

    fn add_to_index(map: &DashMap<Address, Vec<ObjectId>>, key: &Address, id: &ObjectId) {
        let mut entry = map.entry(key.clone()).or_insert_with(Vec::new);
        if !entry.contains(id) {
            entry.push(*id);
        }
    }

    fn remove_from_index(map: &DashMap<Address, Vec<ObjectId>>, key: &Address, id: &ObjectId) {
        if let Some(mut entry) = map.get_mut(key) {
            entry.retain(|existing| existing != id);
            if entry.is_empty() {
                drop(entry);
                map.remove(key);
            }
        }
    }

    fn get_from_index(map: &DashMap<Address, Vec<ObjectId>>, key: &Address) -> Vec<ObjectId> {
        map.get(key).map(|v| v.clone()).unwrap_or_default()
    }
}

impl ObjectStore for MemoryObjectStore {
    // ========== Coin operations ==========

    fn store_coin(&self, coin: &Coin) -> SetuResult<ObjectId> {
        let id = *coin.id();

        // Store primary data
        self.coins.insert(id, coin.clone());

        // Update indexes
        if let Some(owner) = coin.owner() {
            Self::add_to_index(&self.coins_by_owner, owner, &id);
            
            // Update (owner, coin_type) index
            self.coins_by_owner_and_type
                .entry((owner.clone(), coin.coin_type().clone()))
                .or_insert_with(Vec::new)
                .push(id);
        }

        Ok(id)
    }

    fn get_coin(&self, id: &ObjectId) -> SetuResult<Option<Coin>> {
        Ok(self.coins.get(id).map(|c| c.clone()))
    }

    fn get_coins_by_owner(&self, owner: &Address) -> SetuResult<Vec<Coin>> {
        let ids = Self::get_from_index(&self.coins_by_owner, owner);
        let coins: Vec<Coin> = ids
            .iter()
            .filter_map(|id| self.coins.get(id).map(|c| c.clone()))
            .collect();
        Ok(coins)
    }

    fn get_coins_by_owner_and_type(&self, owner: &Address, coin_type: &CoinType) -> SetuResult<Vec<Coin>> {
        let key = (owner.clone(), coin_type.clone());
        let ids = self.coins_by_owner_and_type
            .get(&key)
            .map(|v| v.clone())
            .unwrap_or_default();
        
        let coins: Vec<Coin> = ids
            .iter()
            .filter_map(|id| self.coins.get(id).map(|c| c.clone()))
            .collect();
        Ok(coins)
    }

    fn update_coin(&self, coin: &Coin) -> SetuResult<()> {
        let id = *coin.id();
        
        // Get old coin to update indexes if owner or coin_type changed
        if let Some(old_coin) = self.coins.get(&id) {
            let old_owner = old_coin.owner().cloned();
            let new_owner = coin.owner().cloned();
            let old_coin_type = old_coin.coin_type().clone();
            let new_coin_type = coin.coin_type().clone();
            
            // If owner changed, update both indexes
            if old_owner != new_owner {
                // Remove from old owner's index
                if let Some(old_owner) = &old_owner {
                    Self::remove_from_index(&self.coins_by_owner, old_owner, &id);
                    
                    let old_key = (old_owner.clone(), old_coin_type.clone());
                    if let Some(mut entry) = self.coins_by_owner_and_type.get_mut(&old_key) {
                        entry.retain(|existing| *existing != id);
                        if entry.is_empty() {
                            drop(entry);
                            self.coins_by_owner_and_type.remove(&old_key);
                        }
                    }
                }
                
                // Add to new owner's index
                if let Some(new_owner) = &new_owner {
                    Self::add_to_index(&self.coins_by_owner, new_owner, &id);
                    
                    self.coins_by_owner_and_type
                        .entry((new_owner.clone(), new_coin_type.clone()))
                        .or_insert_with(Vec::new)
                        .push(id);
                }
            }
            // Handle coin_type change (rare but possible) - same owner
            else if old_coin_type != new_coin_type {
                if let Some(owner) = &new_owner {
                    // Remove from old (owner, coin_type) index
                    let old_key = (owner.clone(), old_coin_type);
                    if let Some(mut entry) = self.coins_by_owner_and_type.get_mut(&old_key) {
                        entry.retain(|existing| *existing != id);
                        if entry.is_empty() {
                            drop(entry);
                            self.coins_by_owner_and_type.remove(&old_key);
                        }
                    }
                    
                    // Add to new (owner, coin_type) index
                    self.coins_by_owner_and_type
                        .entry((owner.clone(), new_coin_type))
                        .or_insert_with(Vec::new)
                        .push(id);
                }
            }
        }
        
        // Update coin data
        self.coins.insert(id, coin.clone());
        Ok(())
    }

    fn delete_coin(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some((_, coin)) = self.coins.remove(id) {
            if let Some(owner) = coin.owner() {
                Self::remove_from_index(&self.coins_by_owner, owner, id);
                
                let key = (owner.clone(), coin.coin_type().clone());
                if let Some(mut entry) = self.coins_by_owner_and_type.get_mut(&key) {
                    entry.retain(|existing| *existing != *id);
                    if entry.is_empty() {
                        drop(entry);
                        self.coins_by_owner_and_type.remove(&key);
                    }
                }
            }
        }
        Ok(())
    }

    // ========== Profile operations ==========

    fn store_profile(&self, profile: &Profile) -> SetuResult<ObjectId> {
        let id = *profile.id();
        let owner = profile.data.owner.clone();

        // Atomic check-and-insert using entry API to prevent TOCTOU race
        use dashmap::mapref::entry::Entry;
        match self.profile_by_address.entry(owner.clone()) {
            Entry::Occupied(_) => {
                return Err(SetuError::StorageError(format!(
                    "Profile already exists for address {}",
                    owner
                )));
            }
            Entry::Vacant(entry) => {
                // Store primary data first
                self.profiles.insert(id, profile.clone());
                // Then update index atomically
                entry.insert(id);
            }
        }

        Ok(id)
    }

    fn get_profile(&self, id: &ObjectId) -> SetuResult<Option<Profile>> {
        Ok(self.profiles.get(id).map(|p| p.clone()))
    }

    fn get_profile_by_address(&self, address: &Address) -> SetuResult<Option<Profile>> {
        if let Some(id) = self.profile_by_address.get(address) {
            Ok(self.profiles.get(&*id).map(|p| p.clone()))
        } else {
            Ok(None)
        }
    }

    fn update_profile(&self, profile: &Profile) -> SetuResult<()> {
        let id = *profile.id();

        // Check if profile exists
        if !self.profiles.contains_key(&id) {
            return Err(SetuError::StorageError(format!(
                "Profile {} not found",
                id
            )));
        }

        // Update profile
        self.profiles.insert(id, profile.clone());
        Ok(())
    }

    fn delete_profile(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some((_, profile)) = self.profiles.remove(id) {
            self.profile_by_address.remove(&profile.data.owner);
        }
        Ok(())
    }

    // ========== Credential operations ==========

    fn store_credential(&self, credential: &Credential) -> SetuResult<ObjectId> {
        let id = *credential.id();

        // Store primary data
        self.credentials.insert(id, credential.clone());

        // Update indexes
        Self::add_to_index(&self.credentials_by_holder, &credential.data.holder, &id);
        Self::add_to_index(&self.credentials_by_issuer, &credential.data.issuer, &id);

        Ok(id)
    }

    fn get_credential(&self, id: &ObjectId) -> SetuResult<Option<Credential>> {
        Ok(self.credentials.get(id).map(|c| c.clone()))
    }

    fn get_credentials_by_holder(&self, holder: &Address) -> SetuResult<Vec<Credential>> {
        let ids = Self::get_from_index(&self.credentials_by_holder, holder);
        let credentials: Vec<Credential> = ids
            .iter()
            .filter_map(|id| self.credentials.get(id).map(|c| c.clone()))
            .collect();
        Ok(credentials)
    }

    fn get_credentials_by_issuer(&self, issuer: &Address) -> SetuResult<Vec<Credential>> {
        let ids = Self::get_from_index(&self.credentials_by_issuer, issuer);
        let credentials: Vec<Credential> = ids
            .iter()
            .filter_map(|id| self.credentials.get(id).map(|c| c.clone()))
            .collect();
        Ok(credentials)
    }

    fn update_credential(&self, credential: &Credential) -> SetuResult<()> {
        let id = *credential.id();

        // Get old credential to update indexes if holder/issuer changed
        if let Some(old_cred) = self.credentials.get(&id) {
            // Handle holder change
            if old_cred.data.holder != credential.data.holder {
                Self::remove_from_index(&self.credentials_by_holder, &old_cred.data.holder, &id);
                Self::add_to_index(&self.credentials_by_holder, &credential.data.holder, &id);
            }
            // Handle issuer change
            if old_cred.data.issuer != credential.data.issuer {
                Self::remove_from_index(&self.credentials_by_issuer, &old_cred.data.issuer, &id);
                Self::add_to_index(&self.credentials_by_issuer, &credential.data.issuer, &id);
            }
        } else {
            return Err(SetuError::StorageError(format!(
                "Credential {} not found",
                id
            )));
        }

        self.credentials.insert(id, credential.clone());
        Ok(())
    }

    fn delete_credential(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some((_, credential)) = self.credentials.remove(id) {
            Self::remove_from_index(&self.credentials_by_holder, &credential.data.holder, id);
            Self::remove_from_index(&self.credentials_by_issuer, &credential.data.issuer, id);
        }
        Ok(())
    }

    // ========== RelationGraph operations ==========

    fn store_graph(&self, graph: &RelationGraph) -> SetuResult<ObjectId> {
        let id = *graph.id();

        // Store primary data
        self.graphs.insert(id, graph.clone());

        // Update index
        Self::add_to_index(&self.graphs_by_owner, &graph.data.owner_address, &id);

        Ok(id)
    }

    fn get_graph(&self, id: &ObjectId) -> SetuResult<Option<RelationGraph>> {
        Ok(self.graphs.get(id).map(|g| g.clone()))
    }

    fn get_graphs_by_owner(&self, owner: &Address) -> SetuResult<Vec<RelationGraph>> {
        let ids = Self::get_from_index(&self.graphs_by_owner, owner);
        let graphs: Vec<RelationGraph> = ids
            .iter()
            .filter_map(|id| self.graphs.get(id).map(|g| g.clone()))
            .collect();
        Ok(graphs)
    }

    fn update_graph(&self, graph: &RelationGraph) -> SetuResult<()> {
        let id = *graph.id();

        // Get old graph to update indexes if owner changed
        if let Some(old_graph) = self.graphs.get(&id) {
            // Handle owner change
            if old_graph.data.owner_address != graph.data.owner_address {
                Self::remove_from_index(&self.graphs_by_owner, &old_graph.data.owner_address, &id);
                Self::add_to_index(&self.graphs_by_owner, &graph.data.owner_address, &id);
            }
        } else {
            return Err(SetuError::StorageError(format!(
                "RelationGraph {} not found",
                id
            )));
        }

        self.graphs.insert(id, graph.clone());
        Ok(())
    }

    fn delete_graph(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some((_, graph)) = self.graphs.remove(id) {
            Self::remove_from_index(&self.graphs_by_owner, &graph.data.owner_address, id);
        }
        Ok(())
    }

    // ========== UserRelationNetwork operations ==========

    fn store_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<ObjectId> {
        let id = *network.id();
        let user = network.data.user.clone();
        
        // Atomic check-and-insert using entry API to prevent TOCTOU race
        use dashmap::mapref::entry::Entry;
        match self.user_relation_networks.entry(user.clone()) {
            Entry::Occupied(_) => {
                return Err(SetuError::StorageError(format!(
                    "User relation network already exists for user {}",
                    user
                )));
            }
            Entry::Vacant(entry) => {
                entry.insert(network.clone());
            }
        }
        
        Ok(id)
    }

    fn get_user_relation_network(&self, user: &Address) -> SetuResult<Option<UserRelationNetworkObject>> {
        Ok(self.user_relation_networks.get(user).map(|n| n.clone()))
    }

    fn update_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<()> {
        let user = &network.data.user;
        
        if !self.user_relation_networks.contains_key(user) {
            return Err(SetuError::StorageError(format!(
                "User relation network not found for user {}",
                user
            )));
        }

        self.user_relation_networks.insert(user.clone(), network.clone());
        Ok(())
    }

    fn delete_user_relation_network(&self, user: &Address) -> SetuResult<()> {
        self.user_relation_networks.remove(user);
        Ok(())
    }

    // ========== UserSubnetActivity operations ==========

    fn store_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()> {
        let key = (activity.user.clone(), activity.subnet_id);
        
        // Check if already exists to avoid duplicate index entries
        let is_new = !self.user_subnet_activities.contains_key(&key);
        
        // Store activity
        self.user_subnet_activities.insert(key, activity.clone());
        
        // Update user -> subnet_ids index only for new entries
        if is_new {
            self.subnet_ids_by_user
                .entry(activity.user.clone())
                .or_insert_with(Vec::new)
                .push(activity.subnet_id);
        }

        Ok(())
    }

    fn get_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<Option<UserSubnetActivity>> {
        let key = (user.clone(), *subnet_id);
        Ok(self.user_subnet_activities.get(&key).map(|a| a.clone()))
    }

    fn get_user_all_subnet_activities(&self, user: &Address) -> SetuResult<Vec<UserSubnetActivity>> {
        let subnet_ids = self.subnet_ids_by_user
            .get(user)
            .map(|v| v.clone())
            .unwrap_or_default();
        
        let activities: Vec<UserSubnetActivity> = subnet_ids
            .iter()
            .filter_map(|subnet_id| {
                let key = (user.clone(), *subnet_id);
                self.user_subnet_activities.get(&key).map(|a| a.clone())
            })
            .collect();
        
        Ok(activities)
    }

    fn update_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()> {
        let key = (activity.user.clone(), activity.subnet_id);
        
        if !self.user_subnet_activities.contains_key(&key) {
            return Err(SetuError::StorageError(format!(
                "User subnet activity not found for user {} and subnet {}",
                activity.user, activity.subnet_id
            )));
        }

        self.user_subnet_activities.insert(key, activity.clone());
        Ok(())
    }

    fn delete_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<()> {
        let key = (user.clone(), *subnet_id);
        
        if self.user_subnet_activities.remove(&key).is_some() {
            // Update user -> subnet_ids index
            if let Some(mut entry) = self.subnet_ids_by_user.get_mut(user) {
                entry.retain(|id| *id != *subnet_id);
                if entry.is_empty() {
                    drop(entry);
                    self.subnet_ids_by_user.remove(user);
                }
            }
        }
        
        Ok(())
    }

    // ========== Aggregation ==========

    fn load_account_view(&self, address: &Address) -> SetuResult<AccountView> {
        let profile = self.get_profile_by_address(address)?;
        let credentials = self.get_credentials_by_holder(address)?;
        let coins = self.get_coins_by_owner(address)?;
        let graphs = self.get_graphs_by_owner(address)?;

        Ok(AccountView::new(
            address.clone(),
            profile,
            credentials,
            coins,
            graphs,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::ObjectId;

    #[test]
    fn test_coin_crud() {
        let store = MemoryObjectStore::new();
        let owner = Address::from_str_id("alice");
        let coin = Coin::new(owner.clone(), 1000);
        let id = *coin.id();

        // Store
        let stored_id = store.store_coin(&coin).unwrap();
        assert_eq!(stored_id, id);
        assert_eq!(store.coin_count(), 1);

        // Get
        let retrieved = store.get_coin(&id).unwrap().unwrap();
        assert_eq!(retrieved.value(), 1000);

        // Get by owner
        let by_owner = store.get_coins_by_owner(&owner).unwrap();
        assert_eq!(by_owner.len(), 1);

        // Get by owner and type
        let by_type = store.get_coins_by_owner_and_type(&owner, &CoinType::native()).unwrap();
        assert_eq!(by_type.len(), 1);

        // Update
        let mut updated_coin = coin.clone();
        updated_coin.data.balance = setu_types::Balance::new(2000);
        store.update_coin(&updated_coin).unwrap();
        let retrieved = store.get_coin(&id).unwrap().unwrap();
        assert_eq!(retrieved.value(), 2000);

        // Delete
        store.delete_coin(&id).unwrap();
        assert!(store.get_coin(&id).unwrap().is_none());
        assert_eq!(store.coin_count(), 0);
    }

    #[test]
    fn test_coin_by_type_filter() {
        let store = MemoryObjectStore::new();
        let owner = Address::from_str_id("alice");
        
        // Store coins with different types
        store.store_coin(&Coin::new(owner.clone(), 100)).unwrap();
        store.store_coin(&Coin::new(owner.clone(), 200)).unwrap();
        store.store_coin(&Coin::new_with_type(owner.clone(), 300, CoinType::new("OTHER"))).unwrap();

        let native_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::native()).unwrap();
        assert_eq!(native_coins.len(), 2);
        
        let other_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::new("OTHER")).unwrap();
        assert_eq!(other_coins.len(), 1);

        let all_coins = store.get_coins_by_owner(&owner).unwrap();
        assert_eq!(all_coins.len(), 3);
    }

    #[test]
    fn test_profile_crud() {
        let store = MemoryObjectStore::new();
        let address = Address::from_str_id("alice");
        let profile = Profile::new(address.clone());
        let id = *profile.id();

        // Store
        let stored_id = store.store_profile(&profile).unwrap();
        assert_eq!(stored_id, id);
        assert_eq!(store.profile_count(), 1);

        // Get by ID
        let retrieved = store.get_profile(&id).unwrap().unwrap();
        assert_eq!(retrieved.data.owner, address);

        // Get by address
        let by_address = store.get_profile_by_address(&address).unwrap().unwrap();
        assert_eq!(*by_address.id(), id);

        // Cannot store duplicate profile for same address
        let profile2 = Profile::new(address.clone());
        assert!(store.store_profile(&profile2).is_err());

        // Delete
        store.delete_profile(&id).unwrap();
        assert!(store.get_profile(&id).unwrap().is_none());
        assert!(store.get_profile_by_address(&address).unwrap().is_none());
    }

    #[test]
    fn test_credential_crud() {
        let store = MemoryObjectStore::new();
        let holder = Address::from_str_id("alice");
        let issuer = Address::from_str_id("issuer");
        let credential = Credential::new(holder.clone(), "kyc", issuer.clone());
        let id = *credential.id();

        // Store
        let stored_id = store.store_credential(&credential).unwrap();
        assert_eq!(stored_id, id);
        assert_eq!(store.credential_count(), 1);

        // Get
        let retrieved = store.get_credential(&id).unwrap().unwrap();
        assert_eq!(retrieved.data.credential_type, "kyc");

        // Get by holder
        let by_holder = store.get_credentials_by_holder(&holder).unwrap();
        assert_eq!(by_holder.len(), 1);

        // Get by issuer
        let by_issuer = store.get_credentials_by_issuer(&issuer).unwrap();
        assert_eq!(by_issuer.len(), 1);

        // Delete
        store.delete_credential(&id).unwrap();
        assert!(store.get_credential(&id).unwrap().is_none());
        assert!(store.get_credentials_by_holder(&holder).unwrap().is_empty());
    }

    #[test]
    fn test_graph_crud() {
        let store = MemoryObjectStore::new();
        let owner = Address::from_str_id("alice");
        let graph = RelationGraph::new(ObjectId::random(), owner.clone(), "social".to_string());
        let id = *graph.id();

        // Store
        let stored_id = store.store_graph(&graph).unwrap();
        assert_eq!(stored_id, id);
        assert_eq!(store.graph_count(), 1);

        // Get
        let retrieved = store.get_graph(&id).unwrap().unwrap();
        assert_eq!(retrieved.data.graph_type, "social");

        // Get by owner
        let by_owner = store.get_graphs_by_owner(&owner).unwrap();
        assert_eq!(by_owner.len(), 1);

        // Delete
        store.delete_graph(&id).unwrap();
        assert!(store.get_graph(&id).unwrap().is_none());
    }

    #[test]
    fn test_load_account_view() {
        let store = MemoryObjectStore::new();
        let address = Address::from_str_id("alice");

        // Create profile
        store.store_profile(&Profile::new(address.clone())).unwrap();

        // Create coins
        store.store_coin(&Coin::new(address.clone(), 100)).unwrap();
        store.store_coin(&Coin::new(address.clone(), 200)).unwrap();

        // Create credential (holder)
        let issuer = Address::from_str_id("issuer");
        store.store_credential(&Credential::new(address.clone(), "kyc", issuer)).unwrap();

        // Create graph
        store.store_graph(&RelationGraph::new(ObjectId::random(), address.clone(), "social".to_string())).unwrap();

        // Load account view
        let view = store.load_account_view(&address).unwrap();
        
        assert!(view.profile.is_some());
        assert_eq!(view.coins.len(), 2);
        assert_eq!(view.credentials.len(), 1);
        assert_eq!(view.graphs.len(), 1);
        assert_eq!(view.total_balance, 300);
    }

    #[test]
    fn test_user_subnet_activity() {
        let store = MemoryObjectStore::new();
        let user = Address::from_str_id("alice");
        let subnet_id = SubnetId::new_system(1);

        let activity = UserSubnetActivity::new(user.clone(), subnet_id);

        // Store
        store.store_user_subnet_activity(&activity).unwrap();

        // Get specific
        let retrieved = store.get_user_subnet_activity(&user, &subnet_id).unwrap().unwrap();
        assert_eq!(retrieved.total_interaction_count, 0);

        // Get all for user
        let all = store.get_user_all_subnet_activities(&user).unwrap();
        assert_eq!(all.len(), 1);

        // Update
        let mut updated = activity.clone();
        updated.total_interaction_count = 20;
        store.update_user_subnet_activity(&updated).unwrap();
        let retrieved = store.get_user_subnet_activity(&user, &subnet_id).unwrap().unwrap();
        assert_eq!(retrieved.total_interaction_count, 20);

        // Delete
        store.delete_user_subnet_activity(&user, &subnet_id).unwrap();
        assert!(store.get_user_subnet_activity(&user, &subnet_id).unwrap().is_none());
    }

    #[test]
    fn test_concurrent_coin_operations() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(MemoryObjectStore::new());
        let owner = Address::from_str_id("alice");
        
        let mut handles = vec![];
        
        // Spawn 10 threads, each storing 10 coins
        for i in 0..10 {
            let store = Arc::clone(&store);
            let owner = owner.clone();
            handles.push(thread::spawn(move || {
                for j in 0..10 {
                    let coin_type = CoinType::new(format!("TYPE_{}", i));
                    let coin = Coin::new_with_type(
                        owner.clone(),
                        (i * 10 + j) as u64,
                        coin_type,
                    );
                    store.store_coin(&coin).unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(store.coin_count(), 100);
        
        let all_coins = store.get_coins_by_owner(&owner).unwrap();
        assert_eq!(all_coins.len(), 100);
    }

    #[test]
    fn test_store_user_subnet_activity_idempotent() {
        // Test that storing the same activity multiple times doesn't create duplicate index entries
        let store = MemoryObjectStore::new();
        let user = Address::from_str_id("alice");
        let subnet_id = SubnetId::new_system(1);

        let activity = UserSubnetActivity::new(user.clone(), subnet_id);

        // Store same activity multiple times
        store.store_user_subnet_activity(&activity).unwrap();
        store.store_user_subnet_activity(&activity).unwrap();
        store.store_user_subnet_activity(&activity).unwrap();

        // Should only have one entry in the index
        let all = store.get_user_all_subnet_activities(&user).unwrap();
        assert_eq!(all.len(), 1, "Index should not have duplicates");
    }

    #[test]
    fn test_update_coin_type_change() {
        // Test that changing coin_type updates the index correctly
        let store = MemoryObjectStore::new();
        let owner = Address::from_str_id("alice");
        
        // Create a coin with native type
        let mut coin = Coin::new(owner.clone(), 1000);
        let id = *coin.id();
        store.store_coin(&coin).unwrap();

        // Verify initial index state
        let native_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::native()).unwrap();
        assert_eq!(native_coins.len(), 1);
        
        let other_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::new("OTHER")).unwrap();
        assert_eq!(other_coins.len(), 0);

        // Update coin type (this is a rare operation but should be handled)
        coin.data.coin_type = CoinType::new("OTHER");
        store.update_coin(&coin).unwrap();

        // Verify index updated correctly
        let native_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::native()).unwrap();
        assert_eq!(native_coins.len(), 0, "Old type index should be empty");
        
        let other_coins = store.get_coins_by_owner_and_type(&owner, &CoinType::new("OTHER")).unwrap();
        assert_eq!(other_coins.len(), 1, "New type index should have the coin");

        // Verify coin data is correct
        let retrieved = store.get_coin(&id).unwrap().unwrap();
        assert_eq!(retrieved.coin_type().as_str(), "OTHER");
    }

    #[test]
    fn test_update_credential_holder_change() {
        // Test that changing holder updates the index correctly
        let store = MemoryObjectStore::new();
        let holder1 = Address::from_str_id("alice");
        let holder2 = Address::from_str_id("bob");
        let issuer = Address::from_str_id("issuer");

        // Create a credential
        let mut credential = Credential::new(holder1.clone(), "kyc", issuer.clone());
        store.store_credential(&credential).unwrap();

        // Verify initial index state
        let alice_creds = store.get_credentials_by_holder(&holder1).unwrap();
        assert_eq!(alice_creds.len(), 1);
        let bob_creds = store.get_credentials_by_holder(&holder2).unwrap();
        assert_eq!(bob_creds.len(), 0);

        // Update holder
        credential.data.holder = holder2.clone();
        store.update_credential(&credential).unwrap();

        // Verify index updated correctly
        let alice_creds = store.get_credentials_by_holder(&holder1).unwrap();
        assert_eq!(alice_creds.len(), 0, "Old holder index should be empty");
        let bob_creds = store.get_credentials_by_holder(&holder2).unwrap();
        assert_eq!(bob_creds.len(), 1, "New holder index should have the credential");
    }

    #[test]
    fn test_update_graph_owner_change() {
        // Test that changing owner updates the index correctly
        let store = MemoryObjectStore::new();
        let owner1 = Address::from_str_id("alice");
        let owner2 = Address::from_str_id("bob");

        // Create a graph
        let mut graph = RelationGraph::new(ObjectId::random(), owner1.clone(), "social".to_string());
        store.store_graph(&graph).unwrap();

        // Verify initial index state
        let alice_graphs = store.get_graphs_by_owner(&owner1).unwrap();
        assert_eq!(alice_graphs.len(), 1);
        let bob_graphs = store.get_graphs_by_owner(&owner2).unwrap();
        assert_eq!(bob_graphs.len(), 0);

        // Update owner
        graph.data.owner_address = owner2.clone();
        store.update_graph(&graph).unwrap();

        // Verify index updated correctly
        let alice_graphs = store.get_graphs_by_owner(&owner1).unwrap();
        assert_eq!(alice_graphs.len(), 0, "Old owner index should be empty");
        let bob_graphs = store.get_graphs_by_owner(&owner2).unwrap();
        assert_eq!(bob_graphs.len(), 1, "New owner index should have the graph");
    }
}
