//! RocksDB implementation of ObjectStore

use crate::backends::object::ObjectStore;
use crate::rocks::core::{SetuDB, ColumnFamily, StorageOperation, IntoSetuResult};
use rocksdb::WriteBatch;
use setu_types::{
    ObjectId, Address, SubnetId, CoinType,
    Coin, Profile, Credential, RelationGraph, AccountView,
    UserRelationNetworkObject, UserSubnetActivity,
    SetuResult, SetuError,
};
use tracing::{debug, warn, error, info, instrument};

pub struct RocksObjectStore {
    db: SetuDB,
}

impl RocksObjectStore {
    pub fn new(db: SetuDB) -> Self {
        Self { db }
    }
    
    pub fn open(path: impl AsRef<std::path::Path>) -> SetuResult<Self> {
        let db = SetuDB::open_default(&path)
            .map_err(|e| e.with_operation(StorageOperation::Open))
            .into_setu_result()?;
        Ok(Self::new(db))
    }
    
    fn add_to_index(&self, cf: ColumnFamily, key: &Address, object_id: &ObjectId) -> SetuResult<()> {
        let mut ids: Vec<ObjectId> = self.db.get(cf, key)
            .map_err(|e| e.with_operation(StorageOperation::Get).with_cf(cf.name()))
            .into_setu_result()?
            .unwrap_or_default();
        if !ids.contains(object_id) {
            ids.push(*object_id);
            self.db.put(cf, key, &ids)
                .map_err(|e| e.with_operation(StorageOperation::Put).with_cf(cf.name()))
                .into_setu_result()?;
        }
        Ok(())
    }
    
    fn remove_from_index(&self, cf: ColumnFamily, key: &Address, object_id: &ObjectId) -> SetuResult<()> {
        let mut ids: Vec<ObjectId> = self.db.get(cf, key)
            .map_err(|e| e.with_operation(StorageOperation::Get).with_cf(cf.name()))
            .into_setu_result()?
            .unwrap_or_default();
        let original_len = ids.len();
        ids.retain(|id| id != object_id);
        if ids.len() < original_len {
            if ids.is_empty() {
                self.db.delete(cf, key)
                    .map_err(|e| e.with_operation(StorageOperation::Delete).with_cf(cf.name()))
                    .into_setu_result()?;
            } else {
                self.db.put(cf, key, &ids)
                    .map_err(|e| e.with_operation(StorageOperation::Put).with_cf(cf.name()))
                    .into_setu_result()?;
            }
        }
        Ok(())
    }
    
    fn get_index(&self, cf: ColumnFamily, key: &Address) -> SetuResult<Vec<ObjectId>> {
        self.db.get(cf, key)
            .map_err(|e| e.with_operation(StorageOperation::Get).with_cf(cf.name()))
            .into_setu_result()
            .map(|opt| opt.unwrap_or_default())
    }
    
    /// Create a composite key for subnet activity: user_bytes + subnet_bytes
    fn make_subnet_activity_key(user: &Address, subnet_id: &SubnetId) -> Vec<u8> {
        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(user.as_bytes());
        key.extend_from_slice(subnet_id.as_bytes());
        key
    }
    
    /// Add a subnet to user's activity index
    fn add_subnet_to_user_index(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<()> {
        let cf = ColumnFamily::UserSubnetActivitiesByUser;
        let mut subnet_ids: Vec<SubnetId> = self.db.get(cf, user)
            .map_err(|e| e.with_operation(StorageOperation::Get).with_cf(cf.name()))
            .into_setu_result()?
            .unwrap_or_default();
        if !subnet_ids.contains(subnet_id) {
            subnet_ids.push(*subnet_id);
            self.db.put(cf, user, &subnet_ids)
                .map_err(|e| e.with_operation(StorageOperation::Put).with_cf(cf.name()))
                .into_setu_result()?;
        }
        Ok(())
    }
    
    /// Remove a subnet from user's activity index
    fn remove_subnet_from_user_index(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<()> {
        let cf = ColumnFamily::UserSubnetActivitiesByUser;
        let mut subnet_ids: Vec<SubnetId> = self.db.get(cf, user)
            .map_err(|e| e.with_operation(StorageOperation::Get).with_cf(cf.name()))
            .into_setu_result()?
            .unwrap_or_default();
        let original_len = subnet_ids.len();
        subnet_ids.retain(|id| id != subnet_id);
        if subnet_ids.len() < original_len {
            if subnet_ids.is_empty() {
                self.db.delete(cf, user)
                    .map_err(|e| e.with_operation(StorageOperation::Delete).with_cf(cf.name()))
                    .into_setu_result()?;
            } else {
                self.db.put(cf, user, &subnet_ids)
                    .map_err(|e| e.with_operation(StorageOperation::Put).with_cf(cf.name()))
                    .into_setu_result()?;
            }
        }
        Ok(())
    }
    
    /// Create a composite key for (owner, coin_type) index
    fn make_owner_cointype_key(owner: &Address, coin_type: &CoinType) -> Vec<u8> {
        let mut key = Vec::with_capacity(64 + coin_type.as_str().len());
        key.extend_from_slice(owner.as_bytes());
        key.push(b':'); // separator
        key.extend_from_slice(coin_type.as_str().as_bytes());
        key
    }
    
    /// Add coin to (owner, coin_type) index
    fn add_to_owner_type_index(&self, owner: &Address, coin_type: &CoinType, object_id: &ObjectId) -> SetuResult<()> {
        let key = Self::make_owner_cointype_key(owner, coin_type);
        let mut ids: Vec<ObjectId> = self.db.get_raw(ColumnFamily::CoinsByOwnerAndType, &key)
            .map_err(|e| {
                error!(
                    owner = %owner,
                    coin_type = %coin_type,
                    object_id = %object_id,
                    error = %e,
                    "Failed to read CoinsByOwnerAndType index"
                );
                SetuError::StorageError(e.to_string())
            })?
            .unwrap_or_default();
        if !ids.contains(object_id) {
            ids.push(*object_id);
            self.db.put_raw(ColumnFamily::CoinsByOwnerAndType, &key, &ids)
                .map_err(|e| {
                    error!(
                        owner = %owner,
                        coin_type = %coin_type,
                        object_id = %object_id,
                        index_size = ids.len(),
                        error = %e,
                        "Failed to update CoinsByOwnerAndType index"
                    );
                    SetuError::StorageError(e.to_string())
                })?;
            debug!(
                owner = %owner,
                coin_type = %coin_type,
                object_id = %object_id,
                "Added coin to owner+type index"
            );
        }
        Ok(())
    }
    
    /// Remove coin from (owner, coin_type) index
    /// 
    /// Note: This non-batch version is kept for potential use in index rebuild operations.
    /// For normal operations, use batch_remove_from_owner_type_index for atomicity.
    #[allow(dead_code)]
    fn remove_from_owner_type_index(&self, owner: &Address, coin_type: &CoinType, object_id: &ObjectId) -> SetuResult<()> {
        let key = Self::make_owner_cointype_key(owner, coin_type);
        let mut ids: Vec<ObjectId> = self.db.get_raw(ColumnFamily::CoinsByOwnerAndType, &key)
            .map_err(|e| {
                error!(
                    owner = %owner,
                    coin_type = %coin_type,
                    object_id = %object_id,
                    error = %e,
                    "Failed to read CoinsByOwnerAndType index for removal"
                );
                SetuError::StorageError(e.to_string())
            })?
            .unwrap_or_default();
        let original_len = ids.len();
        ids.retain(|id| id != object_id);
        if ids.len() < original_len {
            if ids.is_empty() {
                self.db.delete_raw(ColumnFamily::CoinsByOwnerAndType, &key)
                    .map_err(|e| {
                        error!(
                            owner = %owner,
                            coin_type = %coin_type,
                            object_id = %object_id,
                            error = %e,
                            "Failed to delete empty CoinsByOwnerAndType index entry"
                        );
                        SetuError::StorageError(e.to_string())
                    })?;
            } else {
                self.db.put_raw(ColumnFamily::CoinsByOwnerAndType, &key, &ids)
                    .map_err(|e| {
                        error!(
                            owner = %owner,
                            coin_type = %coin_type,
                            object_id = %object_id,
                            remaining_count = ids.len(),
                            error = %e,
                            "Failed to update CoinsByOwnerAndType index after removal"
                        );
                        SetuError::StorageError(e.to_string())
                    })?;
            }
            debug!(
                owner = %owner,
                coin_type = %coin_type,
                object_id = %object_id,
                remaining_count = ids.len(),
                "Removed coin from owner+type index"
            );
        } else {
            warn!(
                owner = %owner,
                coin_type = %coin_type,
                object_id = %object_id,
                "Coin not found in owner+type index during removal"
            );
        }
        Ok(())
    }
    
    /// Get coins from (owner, coin_type) index
    fn get_owner_type_index(&self, owner: &Address, coin_type: &CoinType) -> SetuResult<Vec<ObjectId>> {
        let key = Self::make_owner_cointype_key(owner, coin_type);
        self.db.get_raw(ColumnFamily::CoinsByOwnerAndType, &key)
            .map_err(|e| SetuError::StorageError(e.to_string()))
            .map(|opt| opt.unwrap_or_default())
    }
    
    // ========== Batched Index Operations (for atomic writes) ==========
    
    /// Add to index in batch (for atomic writes)
    fn batch_add_to_index(&self, batch: &mut WriteBatch, cf: ColumnFamily, key: &Address, object_id: &ObjectId) -> SetuResult<()> {
        let mut ids: Vec<ObjectId> = self.db.get(cf, key)
            .map_err(|e| SetuError::StorageError(e.to_string()))?
            .unwrap_or_default();
        if !ids.contains(object_id) {
            ids.push(*object_id);
            self.db.batch_put(batch, cf, key, &ids)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
        }
        Ok(())
    }
    
    /// Remove from index in batch (for atomic writes)
    fn batch_remove_from_index(&self, batch: &mut WriteBatch, cf: ColumnFamily, key: &Address, object_id: &ObjectId) -> SetuResult<()> {
        let mut ids: Vec<ObjectId> = self.db.get(cf, key)
            .map_err(|e| SetuError::StorageError(e.to_string()))?
            .unwrap_or_default();
        let original_len = ids.len();
        ids.retain(|id| id != object_id);
        if ids.len() < original_len {
            if ids.is_empty() {
                self.db.batch_delete(batch, cf, key)
                    .map_err(|e| SetuError::StorageError(e.to_string()))?;
            } else {
                self.db.batch_put(batch, cf, key, &ids)
                    .map_err(|e| SetuError::StorageError(e.to_string()))?;
            }
        }
        Ok(())
    }
    
    /// Add coin to (owner, coin_type) index in batch (for atomic writes)
    fn batch_add_to_owner_type_index(&self, batch: &mut WriteBatch, owner: &Address, coin_type: &CoinType, object_id: &ObjectId) -> SetuResult<()> {
        let key = Self::make_owner_cointype_key(owner, coin_type);
        let mut ids: Vec<ObjectId> = self.db.get_raw(ColumnFamily::CoinsByOwnerAndType, &key)
            .map_err(|e| SetuError::StorageError(e.to_string()))?
            .unwrap_or_default();
        if !ids.contains(object_id) {
            ids.push(*object_id);
            self.db.batch_put_raw(batch, ColumnFamily::CoinsByOwnerAndType, &key, &ids)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            debug!(
                owner = %owner,
                coin_type = %coin_type,
                object_id = %object_id,
                "Added coin to owner+type index (batch)"
            );
        }
        Ok(())
    }
    
    /// Remove coin from (owner, coin_type) index in batch (for atomic writes)
    fn batch_remove_from_owner_type_index(&self, batch: &mut WriteBatch, owner: &Address, coin_type: &CoinType, object_id: &ObjectId) -> SetuResult<()> {
        let key = Self::make_owner_cointype_key(owner, coin_type);
        let mut ids: Vec<ObjectId> = self.db.get_raw(ColumnFamily::CoinsByOwnerAndType, &key)
            .map_err(|e| SetuError::StorageError(e.to_string()))?
            .unwrap_or_default();
        let original_len = ids.len();
        ids.retain(|id| id != object_id);
        if ids.len() < original_len {
            if ids.is_empty() {
                self.db.batch_delete_raw(batch, ColumnFamily::CoinsByOwnerAndType, &key)
                    .map_err(|e| SetuError::StorageError(e.to_string()))?;
            } else {
                self.db.batch_put_raw(batch, ColumnFamily::CoinsByOwnerAndType, &key, &ids)
                    .map_err(|e| SetuError::StorageError(e.to_string()))?;
            }
            debug!(
                owner = %owner,
                coin_type = %coin_type,
                object_id = %object_id,
                remaining_count = ids.len(),
                "Removed coin from owner+type index (batch)"
            );
        }
        Ok(())
    }
    
    // ========== Index Rebuild Tools ==========
    
    /// Rebuild the CoinsByOwnerAndType index from existing Coin data.
    /// 
    /// This is useful for:
    /// - Data migration after adding the new index
    /// - Recovery from index corruption
    /// - Initial index population for existing databases
    /// 
    /// # Returns
    /// The number of coins processed and indexed.
    /// 
    /// # Example
    /// ```rust,ignore
    /// let store = RocksObjectStore::open("./db")?;
    /// let count = store.rebuild_coin_type_index()?;
    /// println!("Rebuilt index for {} coins", count);
    /// ```
    #[instrument(skip(self), name = "rebuild_coin_type_index")]
    pub fn rebuild_coin_type_index(&self) -> SetuResult<RebuildIndexResult> {
        info!("Starting CoinsByOwnerAndType index rebuild");
        let mut result = RebuildIndexResult::default();
        
        // Iterate over all coins
        let iter = self.db.iter_values::<Coin>(ColumnFamily::Coins)
            .map_err(|e| {
                error!(error = %e, "Failed to create coin iterator for index rebuild");
                SetuError::StorageError(e.to_string())
            })?;
        
        for coin_result in iter {
            match coin_result {
                Ok(coin) => {
                    if let Some(owner) = &coin.metadata.owner {
                        match self.add_to_owner_type_index(owner, &coin.data.coin_type, &coin.metadata.id) {
                            Ok(_) => result.success += 1,
                            Err(e) => {
                                result.failed += 1;
                                let err_msg = format!(
                                    "Failed to index coin {}: {}",
                                    coin.metadata.id, e
                                );
                                warn!(
                                    coin_id = %coin.metadata.id,
                                    owner = %owner,
                                    coin_type = %coin.data.coin_type,
                                    error = %e,
                                    "Failed to index coin during rebuild"
                                );
                                result.errors.push(err_msg);
                            }
                        }
                    } else {
                        result.skipped += 1; // Coin without owner
                        debug!(coin_id = %coin.metadata.id, "Skipped coin without owner");
                    }
                }
                Err(e) => {
                    result.failed += 1;
                    let err_msg = format!("Failed to deserialize coin: {}", e);
                    warn!(error = %e, "Failed to deserialize coin during index rebuild");
                    result.errors.push(err_msg);
                }
            }
        }
        
        info!(
            success = result.success,
            skipped = result.skipped,
            failed = result.failed,
            total = result.total(),
            "Completed CoinsByOwnerAndType index rebuild"
        );
        
        Ok(result)
    }
    
    /// Clear and rebuild the CoinsByOwnerAndType index.
    /// 
    /// This first clears all entries in the index, then rebuilds from scratch.
    /// Use this when the index might contain stale or corrupt data.
    #[instrument(skip(self), name = "clear_and_rebuild_coin_type_index")]
    pub fn clear_and_rebuild_coin_type_index(&self) -> SetuResult<RebuildIndexResult> {
        info!("Starting clear and rebuild of CoinsByOwnerAndType index");
        
        // Clear existing index by iterating and deleting
        // Note: RocksDB doesn't have a direct "clear column family" API,
        // so we iterate and delete, or we could drop and recreate the CF
        let iter = self.db.iter_values::<Coin>(ColumnFamily::Coins)
            .map_err(|e| {
                error!(error = %e, "Failed to create coin iterator for index clear");
                SetuError::StorageError(e.to_string())
            })?;
        
        // Collect all unique (owner, coin_type) keys to clear
        let mut keys_to_clear = std::collections::HashSet::new();
        for coin_result in iter {
            if let Ok(coin) = coin_result {
                if let Some(owner) = &coin.metadata.owner {
                    let key = Self::make_owner_cointype_key(owner, &coin.data.coin_type);
                    keys_to_clear.insert(key);
                }
            }
        }
        
        let keys_count = keys_to_clear.len();
        debug!(keys_count = keys_count, "Clearing existing index entries");
        
        // Delete all existing index entries
        let mut cleared = 0u64;
        for key in keys_to_clear {
            if let Err(e) = self.db.delete_raw(ColumnFamily::CoinsByOwnerAndType, &key) {
                warn!(error = %e, "Failed to delete index entry during clear");
            } else {
                cleared += 1;
            }
        }
        
        info!(cleared = cleared, "Cleared existing index entries, now rebuilding");
        
        // Now rebuild
        self.rebuild_coin_type_index()
    }
}

/// Result of index rebuild operation
#[derive(Debug, Default)]
pub struct RebuildIndexResult {
    /// Number of coins successfully indexed
    pub success: u64,
    /// Number of coins skipped (e.g., no owner)
    pub skipped: u64,
    /// Number of coins that failed to index
    pub failed: u64,
    /// Error messages for failed operations
    pub errors: Vec<String>,
}

impl RebuildIndexResult {
    pub fn total(&self) -> u64 {
        self.success + self.skipped + self.failed
    }
    
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

impl std::fmt::Display for RebuildIndexResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Index rebuild: {} success, {} skipped, {} failed (total: {})",
            self.success, self.skipped, self.failed, self.total()
        )
    }
}

impl ObjectStore for RocksObjectStore {
    /// Store a coin with atomic index updates
    /// 
    /// Uses WriteBatch to ensure the coin data and all indexes are updated atomically.
    /// If any operation fails, all changes are rolled back.
    fn store_coin(&self, coin: &Coin) -> SetuResult<ObjectId> {
        let id = coin.metadata.id;
        let mut batch = self.db.batch();
        
        // Add coin data to batch
        self.db.batch_put(&mut batch, ColumnFamily::Coins, &id, coin)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Add index entries to batch
        if let Some(owner) = &coin.metadata.owner {
            self.batch_add_to_index(&mut batch, ColumnFamily::CoinsByOwner, owner, &id)?;
            self.batch_add_to_owner_type_index(&mut batch, owner, &coin.data.coin_type, &id)?;
        }
        
        // Commit all changes atomically
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        Ok(id)
    }
    
    fn get_coin(&self, id: &ObjectId) -> SetuResult<Option<Coin>> {
        self.db.get(ColumnFamily::Coins, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn get_coins_by_owner(&self, owner: &Address) -> SetuResult<Vec<Coin>> {
        let ids = self.get_index(ColumnFamily::CoinsByOwner, owner)?;
        let coins: Vec<Option<Coin>> = self.db.multi_get(ColumnFamily::Coins, &ids)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(coins.into_iter().flatten().collect())
    }
    
    fn get_coins_by_owner_and_type(&self, owner: &Address, coin_type: &CoinType) -> SetuResult<Vec<Coin>> {
        let ids = self.get_owner_type_index(owner, coin_type)?;
        let coins: Vec<Option<Coin>> = self.db.multi_get(ColumnFamily::Coins, &ids)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(coins.into_iter().flatten().collect())
    }
    
    fn update_coin(&self, coin: &Coin) -> SetuResult<()> {
        let mut batch = self.db.batch();
        
        if let Some(old_coin) = self.get_coin(&coin.metadata.id)? {
            // Handle owner change
            if old_coin.metadata.owner != coin.metadata.owner {
                if let Some(old_owner) = &old_coin.metadata.owner {
                    self.batch_remove_from_index(&mut batch, ColumnFamily::CoinsByOwner, old_owner, &coin.metadata.id)?;
                    self.batch_remove_from_owner_type_index(&mut batch, old_owner, &old_coin.data.coin_type, &coin.metadata.id)?;
                }
                if let Some(new_owner) = &coin.metadata.owner {
                    self.batch_add_to_index(&mut batch, ColumnFamily::CoinsByOwner, new_owner, &coin.metadata.id)?;
                    self.batch_add_to_owner_type_index(&mut batch, new_owner, &coin.data.coin_type, &coin.metadata.id)?;
                }
            }
            // Handle coin_type change (rare but possible)
            else if old_coin.data.coin_type != coin.data.coin_type {
                if let Some(owner) = &coin.metadata.owner {
                    self.batch_remove_from_owner_type_index(&mut batch, owner, &old_coin.data.coin_type, &coin.metadata.id)?;
                    self.batch_add_to_owner_type_index(&mut batch, owner, &coin.data.coin_type, &coin.metadata.id)?;
                }
            }
        }
        
        // Add coin update to batch
        self.db.batch_put(&mut batch, ColumnFamily::Coins, &coin.metadata.id, coin)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Commit all changes atomically
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn delete_coin(&self, id: &ObjectId) -> SetuResult<()> {
        let mut batch = self.db.batch();
        
        if let Some(coin) = self.get_coin(id)? {
            if let Some(owner) = &coin.metadata.owner {
                self.batch_remove_from_index(&mut batch, ColumnFamily::CoinsByOwner, owner, id)?;
                self.batch_remove_from_owner_type_index(&mut batch, owner, &coin.data.coin_type, id)?;
            }
        }
        
        // Add coin deletion to batch
        self.db.batch_delete(&mut batch, ColumnFamily::Coins, id)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        
        // Commit all changes atomically
        self.db.write_batch(batch)
            .map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn store_profile(&self, profile: &Profile) -> SetuResult<ObjectId> {
        let id = profile.metadata.id;
        self.db.put(ColumnFamily::Profiles, &id, profile).map_err(|e| SetuError::StorageError(e.to_string()))?;
        self.db.put(ColumnFamily::ProfileByAddress, &profile.data.owner, &id).map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(id)
    }
    
    fn get_profile(&self, id: &ObjectId) -> SetuResult<Option<Profile>> {
        self.db.get(ColumnFamily::Profiles, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn get_profile_by_address(&self, address: &Address) -> SetuResult<Option<Profile>> {
        let profile_id: Option<ObjectId> = self.db.get(ColumnFamily::ProfileByAddress, address)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        match profile_id {
            Some(id) => self.get_profile(&id),
            None => Ok(None),
        }
    }
    
    fn update_profile(&self, profile: &Profile) -> SetuResult<()> {
        if let Some(old_profile) = self.get_profile(&profile.metadata.id)? {
            if old_profile.data.owner != profile.data.owner {
                self.db.delete(ColumnFamily::ProfileByAddress, &old_profile.data.owner)
                    .map_err(|e| SetuError::StorageError(e.to_string()))?;
            }
        }
        self.store_profile(profile)?;
        Ok(())
    }
    
    fn delete_profile(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some(profile) = self.get_profile(id)? {
            self.db.delete(ColumnFamily::ProfileByAddress, &profile.data.owner)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
        }
        self.db.delete(ColumnFamily::Profiles, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn store_credential(&self, credential: &Credential) -> SetuResult<ObjectId> {
        let id = credential.metadata.id;
        self.db.put(ColumnFamily::Credentials, &id, credential).map_err(|e| SetuError::StorageError(e.to_string()))?;
        self.add_to_index(ColumnFamily::CredentialsByHolder, &credential.data.holder, &id)?;
        self.add_to_index(ColumnFamily::CredentialsByIssuer, &credential.data.issuer, &id)?;
        Ok(id)
    }
    
    fn get_credential(&self, id: &ObjectId) -> SetuResult<Option<Credential>> {
        self.db.get(ColumnFamily::Credentials, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn get_credentials_by_holder(&self, holder: &Address) -> SetuResult<Vec<Credential>> {
        let ids = self.get_index(ColumnFamily::CredentialsByHolder, holder)?;
        let creds: Vec<Option<Credential>> = self.db.multi_get(ColumnFamily::Credentials, &ids)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(creds.into_iter().flatten().collect())
    }
    
    fn get_credentials_by_issuer(&self, issuer: &Address) -> SetuResult<Vec<Credential>> {
        let ids = self.get_index(ColumnFamily::CredentialsByIssuer, issuer)?;
        let creds: Vec<Option<Credential>> = self.db.multi_get(ColumnFamily::Credentials, &ids)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(creds.into_iter().flatten().collect())
    }
    
    fn update_credential(&self, credential: &Credential) -> SetuResult<()> {
        if let Some(old_cred) = self.get_credential(&credential.metadata.id)? {
            if old_cred.data.holder != credential.data.holder {
                self.remove_from_index(ColumnFamily::CredentialsByHolder, &old_cred.data.holder, &credential.metadata.id)?;
                self.add_to_index(ColumnFamily::CredentialsByHolder, &credential.data.holder, &credential.metadata.id)?;
            }
            if old_cred.data.issuer != credential.data.issuer {
                self.remove_from_index(ColumnFamily::CredentialsByIssuer, &old_cred.data.issuer, &credential.metadata.id)?;
                self.add_to_index(ColumnFamily::CredentialsByIssuer, &credential.data.issuer, &credential.metadata.id)?;
            }
        }
        self.db.put(ColumnFamily::Credentials, &credential.metadata.id, credential).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn delete_credential(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some(cred) = self.get_credential(id)? {
            self.remove_from_index(ColumnFamily::CredentialsByHolder, &cred.data.holder, id)?;
            self.remove_from_index(ColumnFamily::CredentialsByIssuer, &cred.data.issuer, id)?;
        }
        self.db.delete(ColumnFamily::Credentials, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn store_graph(&self, graph: &RelationGraph) -> SetuResult<ObjectId> {
        let id = graph.metadata.id;
        self.db.put(ColumnFamily::RelationGraphs, &id, graph).map_err(|e| SetuError::StorageError(e.to_string()))?;
        if let Some(owner) = &graph.metadata.owner {
            self.add_to_index(ColumnFamily::GraphsByOwner, owner, &id)?;
        }
        Ok(id)
    }
    
    fn get_graph(&self, id: &ObjectId) -> SetuResult<Option<RelationGraph>> {
        self.db.get(ColumnFamily::RelationGraphs, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn get_graphs_by_owner(&self, owner: &Address) -> SetuResult<Vec<RelationGraph>> {
        let ids = self.get_index(ColumnFamily::GraphsByOwner, owner)?;
        let graphs: Vec<Option<RelationGraph>> = self.db.multi_get(ColumnFamily::RelationGraphs, &ids)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(graphs.into_iter().flatten().collect())
    }
    
    fn update_graph(&self, graph: &RelationGraph) -> SetuResult<()> {
        if let Some(old_graph) = self.get_graph(&graph.metadata.id)? {
            if old_graph.metadata.owner != graph.metadata.owner {
                if let Some(old_owner) = &old_graph.metadata.owner {
                    self.remove_from_index(ColumnFamily::GraphsByOwner, old_owner, &graph.metadata.id)?;
                }
                if let Some(new_owner) = &graph.metadata.owner {
                    self.add_to_index(ColumnFamily::GraphsByOwner, new_owner, &graph.metadata.id)?;
                }
            }
        }
        self.db.put(ColumnFamily::RelationGraphs, &graph.metadata.id, graph).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn delete_graph(&self, id: &ObjectId) -> SetuResult<()> {
        if let Some(graph) = self.get_graph(id)? {
            if let Some(owner) = &graph.metadata.owner {
                self.remove_from_index(ColumnFamily::GraphsByOwner, owner, id)?;
            }
        }
        self.db.delete(ColumnFamily::RelationGraphs, id).map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn load_account_view(&self, address: &Address) -> SetuResult<AccountView> {
        let profile = self.get_profile_by_address(address)?;
        let credentials = self.get_credentials_by_holder(address)?;
        let coins = self.get_coins_by_owner(address)?;
        let graphs = self.get_graphs_by_owner(address)?;
        Ok(AccountView::new(address.clone(), profile, credentials, coins, graphs))
    }
    
    // ========== UserRelationNetwork operations ==========
    
    fn store_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<ObjectId> {
        let id = network.metadata.id;
        let user = &network.data.user;
        self.db.put(ColumnFamily::UserRelationNetworks, &id, network)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        // Index by user address
        self.db.put(ColumnFamily::UserRelationNetworkByUser, user, &id)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        Ok(id)
    }
    
    fn get_user_relation_network(&self, user: &Address) -> SetuResult<Option<UserRelationNetworkObject>> {
        // First get the object ID from the user index
        let network_id: Option<ObjectId> = self.db.get(ColumnFamily::UserRelationNetworkByUser, user)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        match network_id {
            Some(id) => self.db.get(ColumnFamily::UserRelationNetworks, &id)
                .map_err(|e| SetuError::StorageError(e.to_string())),
            None => Ok(None),
        }
    }
    
    fn update_user_relation_network(&self, network: &UserRelationNetworkObject) -> SetuResult<()> {
        self.db.put(ColumnFamily::UserRelationNetworks, &network.metadata.id, network)
            .map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn delete_user_relation_network(&self, user: &Address) -> SetuResult<()> {
        if let Some(network) = self.get_user_relation_network(user)? {
            self.db.delete(ColumnFamily::UserRelationNetworks, &network.metadata.id)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
            self.db.delete(ColumnFamily::UserRelationNetworkByUser, user)
                .map_err(|e| SetuError::StorageError(e.to_string()))?;
        }
        Ok(())
    }
    
    // ========== UserSubnetActivity operations ==========
    
    fn store_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()> {
        // Create a composite key: user + subnet_id
        let key = Self::make_subnet_activity_key(&activity.user, &activity.subnet_id);
        self.db.put(ColumnFamily::UserSubnetActivities, &key, activity)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        // Add to user's activity index
        self.add_subnet_to_user_index(&activity.user, &activity.subnet_id)?;
        Ok(())
    }
    
    fn get_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<Option<UserSubnetActivity>> {
        let key = Self::make_subnet_activity_key(user, subnet_id);
        self.db.get(ColumnFamily::UserSubnetActivities, &key)
            .map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn get_user_all_subnet_activities(&self, user: &Address) -> SetuResult<Vec<UserSubnetActivity>> {
        // Get all subnet IDs for this user
        let subnet_ids: Vec<SubnetId> = self.db.get(ColumnFamily::UserSubnetActivitiesByUser, user)
            .map_err(|e| SetuError::StorageError(e.to_string()))?
            .unwrap_or_default();
        
        let mut activities = Vec::new();
        for subnet_id in subnet_ids {
            if let Some(activity) = self.get_user_subnet_activity(user, &subnet_id)? {
                activities.push(activity);
            }
        }
        Ok(activities)
    }
    
    fn update_user_subnet_activity(&self, activity: &UserSubnetActivity) -> SetuResult<()> {
        let key = Self::make_subnet_activity_key(&activity.user, &activity.subnet_id);
        self.db.put(ColumnFamily::UserSubnetActivities, &key, activity)
            .map_err(|e| SetuError::StorageError(e.to_string()))
    }
    
    fn delete_user_subnet_activity(&self, user: &Address, subnet_id: &SubnetId) -> SetuResult<()> {
        let key = Self::make_subnet_activity_key(user, subnet_id);
        self.db.delete(ColumnFamily::UserSubnetActivities, &key)
            .map_err(|e| SetuError::StorageError(e.to_string()))?;
        self.remove_subnet_from_user_index(user, subnet_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use setu_types::{create_profile, create_kyc_credential, create_social_graph, generate_object_id};
    
    fn setup_test_store() -> (RocksObjectStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = RocksObjectStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }
    
    #[test]
    fn test_coin_crud() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let coin = Coin::new(alice, 1000);
        let coin_id = coin.metadata.id;
        store.store_coin(&coin).unwrap();
        let retrieved = store.get_coin(&coin_id).unwrap().unwrap();
        assert_eq!(retrieved.value(), 1000);
        let coins = store.get_coins_by_owner(&alice).unwrap();
        assert_eq!(coins.len(), 1);
        store.delete_coin(&coin_id).unwrap();
        assert!(store.get_coin(&coin_id).unwrap().is_none());
    }
    
    #[test]
    fn test_profile_crud() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let mut profile = create_profile(alice);
        profile.data.set_display_name("Alice");
        let profile_id = profile.metadata.id;
        store.store_profile(&profile).unwrap();
        let retrieved = store.get_profile(&profile_id).unwrap().unwrap();
        assert_eq!(retrieved.data.display_name, Some("Alice".to_string()));
        let by_addr = store.get_profile_by_address(&alice).unwrap().unwrap();
        assert_eq!(by_addr.metadata.id, profile_id);
        store.delete_profile(&profile_id).unwrap();
        assert!(store.get_profile(&profile_id).unwrap().is_none());
    }
    
    #[test]
    fn test_credential_crud() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let issuer = Address::from_str_id("kyc_provider");
        let cred = create_kyc_credential(alice, issuer, "level_2");
        let cred_id = cred.metadata.id;
        store.store_credential(&cred).unwrap();
        let retrieved = store.get_credential(&cred_id).unwrap().unwrap();
        assert_eq!(retrieved.data.credential_type, "kyc");
        let by_holder = store.get_credentials_by_holder(&alice).unwrap();
        assert_eq!(by_holder.len(), 1);
        let by_issuer = store.get_credentials_by_issuer(&issuer).unwrap();
        assert_eq!(by_issuer.len(), 1);
        store.delete_credential(&cred_id).unwrap();
        assert!(store.get_credential(&cred_id).unwrap().is_none());
    }
    
    #[test]
    fn test_load_account_view() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let issuer = Address::from_str_id("kyc_provider");
        let mut profile = create_profile(alice);
        profile.data.set_display_name("Alice");
        store.store_profile(&profile).unwrap();
        let kyc = create_kyc_credential(alice, issuer, "level_2");
        store.store_credential(&kyc).unwrap();
        let coin1 = Coin::new(alice, 1000);
        let coin2 = Coin::new(alice, 500);
        store.store_coin(&coin1).unwrap();
        store.store_coin(&coin2).unwrap();
        let owner_id = generate_object_id(b"alice_owner");
        let graph = create_social_graph(owner_id, alice);
        store.store_graph(&graph).unwrap();
        let view = store.load_account_view(&alice).unwrap();
        assert_eq!(view.display_name(), Some(&"Alice".to_string()));
        assert_eq!(view.total_balance, 1500);
        assert_eq!(view.coin_count, 2);
        assert_eq!(view.valid_credential_count, 1);
        assert_eq!(view.graph_count, 1);
        assert!(view.has_kyc());
    }
    
    #[test]
    fn test_coins_by_owner_and_type() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        
        // Create coins with different types
        let setu_coin = Coin::new(alice, 1000); // Default SETU type
        let usdc_coin = Coin::new_with_type(alice, 500, CoinType::new("USDC"));
        let flux_coin = Coin::new_with_type(alice, 200, CoinType::new("FLUX"));
        let usdc_coin2 = Coin::new_with_type(alice, 300, CoinType::new("USDC"));
        
        store.store_coin(&setu_coin).unwrap();
        store.store_coin(&usdc_coin).unwrap();
        store.store_coin(&flux_coin).unwrap();
        store.store_coin(&usdc_coin2).unwrap();
        
        // Test get_coins_by_owner returns all
        let all_coins = store.get_coins_by_owner(&alice).unwrap();
        assert_eq!(all_coins.len(), 4);
        
        // Test get_coins_by_owner_and_type
        let setu_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::native()).unwrap();
        assert_eq!(setu_coins.len(), 1);
        assert_eq!(setu_coins[0].value(), 1000);
        
        let usdc_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::new("USDC")).unwrap();
        assert_eq!(usdc_coins.len(), 2);
        let usdc_total: u64 = usdc_coins.iter().map(|c| c.value()).sum();
        assert_eq!(usdc_total, 800);
        
        let flux_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::new("FLUX")).unwrap();
        assert_eq!(flux_coins.len(), 1);
        
        // Non-existent type returns empty
        let btc_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::new("BTC")).unwrap();
        assert!(btc_coins.is_empty());
    }
    
    #[test]
    fn test_coin_type_index_update_on_owner_change() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");
        
        // Create a USDC coin owned by alice
        let mut coin = Coin::new_with_type(alice, 1000, CoinType::new("USDC"));
        let coin_id = coin.metadata.id;
        store.store_coin(&coin).unwrap();
        
        // Verify alice has the coin
        let alice_usdc = store.get_coins_by_owner_and_type(&alice, &CoinType::new("USDC")).unwrap();
        assert_eq!(alice_usdc.len(), 1);
        
        // Transfer coin to bob (update owner)
        coin.metadata.owner = Some(bob);
        store.update_coin(&coin).unwrap();
        
        // Alice should have no USDC now
        let alice_usdc = store.get_coins_by_owner_and_type(&alice, &CoinType::new("USDC")).unwrap();
        assert!(alice_usdc.is_empty());
        
        // Bob should have the USDC
        let bob_usdc = store.get_coins_by_owner_and_type(&bob, &CoinType::new("USDC")).unwrap();
        assert_eq!(bob_usdc.len(), 1);
        assert_eq!(bob_usdc[0].metadata.id, coin_id);
    }
    
    #[test]
    fn test_rebuild_coin_type_index() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");
        
        // Store coins (this automatically builds the index)
        let coin1 = Coin::new(alice, 1000);
        let coin2 = Coin::new_with_type(alice, 500, CoinType::new("USDC"));
        let coin3 = Coin::new_with_type(bob, 200, CoinType::new("USDC"));
        
        store.store_coin(&coin1).unwrap();
        store.store_coin(&coin2).unwrap();
        store.store_coin(&coin3).unwrap();
        
        // Rebuild index (should be idempotent)
        let result = store.rebuild_coin_type_index().unwrap();
        assert_eq!(result.success, 3);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
        assert!(result.is_success());
        
        // Verify index still works correctly
        let alice_setu = store.get_coins_by_owner_and_type(&alice, &CoinType::native()).unwrap();
        assert_eq!(alice_setu.len(), 1);
        
        let alice_usdc = store.get_coins_by_owner_and_type(&alice, &CoinType::new("USDC")).unwrap();
        assert_eq!(alice_usdc.len(), 1);
        
        let bob_usdc = store.get_coins_by_owner_and_type(&bob, &CoinType::new("USDC")).unwrap();
        assert_eq!(bob_usdc.len(), 1);
    }
    
    #[test]
    fn test_clear_and_rebuild_coin_type_index() {
        let (store, _temp) = setup_test_store();
        let alice = Address::from_str_id("alice");
        
        // Store coins
        let coin1 = Coin::new(alice, 1000);
        let coin2 = Coin::new_with_type(alice, 500, CoinType::new("FLUX"));
        
        store.store_coin(&coin1).unwrap();
        store.store_coin(&coin2).unwrap();
        
        // Clear and rebuild
        let result = store.clear_and_rebuild_coin_type_index().unwrap();
        assert_eq!(result.total(), 2);
        assert!(result.is_success());
        println!("{}", result); // Test Display impl
        
        // Verify index works
        let setu_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::native()).unwrap();
        assert_eq!(setu_coins.len(), 1);
        
        let flux_coins = store.get_coins_by_owner_and_type(&alice, &CoinType::new("FLUX")).unwrap();
        assert_eq!(flux_coins.len(), 1);
    }
}
