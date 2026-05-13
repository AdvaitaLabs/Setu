//! State storage abstraction
//!
//! Three-level trait hierarchy:
//! - Level 0: `RawStore` — key-value bytes (module bytecode)
//! - Level 1: `ObjectStore` — ObjectEnvelope-based (Move objects)
//! - Level 2: `StateStore` — CoinData-specialized (legacy, unchanged)

use std::collections::HashMap;
use setu_types::{Object, ObjectId, Address, CoinData, ObjectEnvelope};
use crate::error::RuntimeResult;

// ─── Level 0: Raw byte storage ───

/// Raw key-value byte storage (used by Move VM to load module bytecode).
///
/// Storage key format: `"mod:{hex_addr}::{module_name}"` → compiled bytecode
pub trait RawStore: Send + Sync {
    fn get_raw(&self, key: &str) -> RuntimeResult<Option<Vec<u8>>>;
    fn set_raw(&mut self, key: &str, value: Vec<u8>) -> RuntimeResult<()>;
    fn delete_raw(&mut self, key: &str) -> RuntimeResult<()>;
}

// ─── Level 1: Generic object envelope storage ───

/// Generic object store — not bound to any concrete object type.
///
/// All objects stored as `ObjectEnvelope` (BCS) containing:
/// metadata (id, owner, version, ownership), type_tag, BCS data.
pub trait ObjectStore: RawStore {
    fn get_envelope(&self, id: &ObjectId) -> RuntimeResult<Option<ObjectEnvelope>>;
    fn set_envelope(&mut self, id: ObjectId, envelope: ObjectEnvelope) -> RuntimeResult<()>;
    fn delete_envelope(&mut self, id: &ObjectId) -> RuntimeResult<()>;
    fn get_owned_ids(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>>;
}

// ─── Level 2: Legacy CoinData-specialized storage (unchanged) ───

/// State storage trait
/// Can be replaced with persistent storage or Move VM state management in the future
pub trait StateStore {
    /// Read object
    fn get_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>>;
    
    /// Write object
    fn set_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()>;
    
    /// Delete object
    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()>;
    
    /// Get all objects owned by an address
    fn get_owned_objects(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>>;
    
    /// Read raw bytes by ObjectId (type-agnostic — works for Coin BCS, FluxState/PowerState JSON, etc.)
    fn get_raw_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Vec<u8>>>;
    
    /// Write raw bytes by ObjectId (type-agnostic)
    fn set_raw_object(&mut self, object_id: ObjectId, data: Vec<u8>) -> RuntimeResult<()>;
    
    /// Check if object exists
    fn exists(&self, object_id: &ObjectId) -> bool {
        self.get_object(object_id).ok().flatten().is_some()
    }
}

/// In-memory state storage (used for testing and simple scenarios)
#[derive(Debug, Clone)]
pub struct InMemoryStateStore {
    /// Object storage: ObjectId -> Object
    objects: HashMap<ObjectId, Object<CoinData>>,
    /// Ownership index: Address -> Vec<ObjectId>
    ownership_index: HashMap<Address, Vec<ObjectId>>,
    /// Reverse index: ObjectId -> Address (for O(1) old-owner lookup)
    object_owner: HashMap<ObjectId, Address>,
    /// Raw object storage: ObjectId -> raw bytes (for non-Coin objects like FluxState, PowerState)
    raw_objects: HashMap<ObjectId, Vec<u8>>,
}

impl InMemoryStateStore {
    /// Create new in-memory state storage
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            ownership_index: HashMap::new(),
            object_owner: HashMap::new(),
            raw_objects: HashMap::new(),
        }
    }
    
    /// Update ownership index (O(1) amortized via reverse index)
    fn update_ownership_index(&mut self, object_id: ObjectId, new_owner: &Address) {
        // Remove from the old owner's index using reverse lookup (O(1))
        if let Some(old_owner) = self.object_owner.remove(&object_id) {
            if let Some(objects) = self.ownership_index.get_mut(&old_owner) {
                objects.retain(|id| id != &object_id);
            }
        }
        
        // Add to the new owner's index
        self.ownership_index
            .entry(new_owner.clone())
            .or_insert_with(Vec::new)
            .push(object_id);
        
        // Update reverse index
        self.object_owner.insert(object_id, new_owner.clone());
    }
    
    /// Remove object from ownership index (O(1) via reverse index)
    fn remove_from_ownership_index(&mut self, object_id: &ObjectId) {
        if let Some(old_owner) = self.object_owner.remove(object_id) {
            if let Some(objects) = self.ownership_index.get_mut(&old_owner) {
                objects.retain(|id| id != object_id);
            }
        }
    }
    
    /// Get total balance (used for testing)
    pub fn get_total_balance(&self, owner: &Address) -> u64 {
        self.get_owned_objects(owner)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.get_object(id).ok().flatten())
            .fold(0u64, |acc, obj| acc.saturating_add(obj.data.balance.value()))
    }
}

impl Default for InMemoryStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StateStore for InMemoryStateStore {
    fn get_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>> {
        Ok(self.objects.get(object_id).cloned())
    }
    
    fn set_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()> {
        // Update ownership index
        if let Some(owner) = &object.metadata.owner {
            self.update_ownership_index(object_id, owner);
        }
        
        // Store object
        self.objects.insert(object_id, object);
        Ok(())
    }
    
    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.objects.remove(object_id);
        self.remove_from_ownership_index(object_id);
        Ok(())
    }
    
    fn get_owned_objects(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>> {
        Ok(self.ownership_index
            .get(owner)
            .cloned()
            .unwrap_or_default())
    }
    
    fn get_raw_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Vec<u8>>> {
        Ok(self.raw_objects.get(object_id).cloned())
    }
    
    fn set_raw_object(&mut self, object_id: ObjectId, data: Vec<u8>) -> RuntimeResult<()> {
        self.raw_objects.insert(object_id, data);
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════
// InMemoryObjectStore — generic envelope-based store (Move VM path)
// ════════════════════════════════════════════════════════════════════════════

/// Generic in-memory object store (used for TEE-internal Move VM execution).
///
/// Implements all three levels: `RawStore` + `ObjectStore` + `StateStore`.
#[derive(Debug, Clone)]
pub struct InMemoryObjectStore {
    /// ObjectId → ObjectEnvelope
    envelopes: HashMap<ObjectId, ObjectEnvelope>,
    /// Raw key-value storage (module bytecode, etc.)
    raw: HashMap<String, Vec<u8>>,
    /// Raw object storage by ObjectId (type-agnostic bytes, e.g. FluxState/PowerState JSON)
    raw_objects: HashMap<ObjectId, Vec<u8>>,
    /// Ownership index: Address → Vec<ObjectId>
    ownership_index: HashMap<Address, Vec<ObjectId>>,
    /// Reverse index: ObjectId → Address (for O(1) old-owner removal)
    object_owner: HashMap<ObjectId, Address>,
}

impl InMemoryObjectStore {
    pub fn new() -> Self {
        Self {
            envelopes: HashMap::new(),
            raw: HashMap::new(),
            raw_objects: HashMap::new(),
            ownership_index: HashMap::new(),
            object_owner: HashMap::new(),
        }
    }

    fn remove_from_ownership_index(&mut self, id: &ObjectId) {
        if let Some(old_owner) = self.object_owner.remove(id) {
            if let Some(ids) = self.ownership_index.get_mut(&old_owner) {
                ids.retain(|oid| oid != id);
            }
        }
    }
}

impl Default for InMemoryObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RawStore for InMemoryObjectStore {
    fn get_raw(&self, key: &str) -> RuntimeResult<Option<Vec<u8>>> {
        Ok(self.raw.get(key).cloned())
    }

    fn set_raw(&mut self, key: &str, value: Vec<u8>) -> RuntimeResult<()> {
        self.raw.insert(key.to_string(), value);
        Ok(())
    }

    fn delete_raw(&mut self, key: &str) -> RuntimeResult<()> {
        self.raw.remove(key);
        Ok(())
    }
}

impl ObjectStore for InMemoryObjectStore {
    fn get_envelope(&self, id: &ObjectId) -> RuntimeResult<Option<ObjectEnvelope>> {
        Ok(self.envelopes.get(id).cloned())
    }

    fn set_envelope(&mut self, id: ObjectId, envelope: ObjectEnvelope) -> RuntimeResult<()> {
        let owner = envelope.metadata.owner;
        self.remove_from_ownership_index(&id);
        self.ownership_index.entry(owner).or_default().push(id);
        self.object_owner.insert(id, owner);
        self.envelopes.insert(id, envelope);
        Ok(())
    }

    fn delete_envelope(&mut self, id: &ObjectId) -> RuntimeResult<()> {
        self.envelopes.remove(id);
        self.remove_from_ownership_index(id);
        Ok(())
    }

    fn get_owned_ids(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>> {
        Ok(self.ownership_index.get(owner).cloned().unwrap_or_default())
    }
}

/// Backward-compatible `StateStore` implementation via ObjectEnvelope.
impl StateStore for InMemoryObjectStore {
    fn get_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>> {
        match self.envelopes.get(object_id) {
            Some(env) => Ok(env.try_as_coin_object()),
            None => Ok(None),
        }
    }

    fn set_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()> {
        let envelope = ObjectEnvelope::from_coin_object(&object)
            .map_err(|e| crate::error::RuntimeError::StateError(e))?;
        self.set_envelope(object_id, envelope)
    }

    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.delete_envelope(object_id)
    }

    fn get_owned_objects(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>> {
        self.get_owned_ids(owner)
    }

    fn get_raw_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Vec<u8>>> {
        Ok(self.raw_objects.get(object_id).cloned())
    }

    fn set_raw_object(&mut self, object_id: ObjectId, data: Vec<u8>) -> RuntimeResult<()> {
        self.raw_objects.insert(object_id, data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::envelope::ENVELOPE_MAGIC;
    
    #[test]
    fn test_state_store_operations() {
        let mut store = InMemoryStateStore::new();
        
        let owner = Address::from_str_id("alice");
        let coin = setu_types::create_coin(owner.clone(), 1000);
        let coin_id = *coin.id();
        
        // Set object
        store.set_object(coin_id, coin.clone()).unwrap();
        
        // Read object
        let retrieved = store.get_object(&coin_id).unwrap().unwrap();
        assert_eq!(retrieved.id(), &coin_id);
        
        // Check ownership index
        let owned = store.get_owned_objects(&owner).unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0], coin_id);
        
        // Delete object
        store.delete_object(&coin_id).unwrap();
        assert!(store.get_object(&coin_id).unwrap().is_none());
        
        let owned = store.get_owned_objects(&owner).unwrap();
        assert_eq!(owned.len(), 0);
    }

    // ─── InMemoryObjectStore tests ───

    fn make_envelope(id_byte: u8, owner: Address, balance: u64) -> (ObjectId, ObjectEnvelope) {
        let id = ObjectId::new([id_byte; 32]);
        let coin = setu_types::create_coin_with_id(id, owner, balance, "ROOT", 0);
        let env = ObjectEnvelope::from_coin_object(&coin).unwrap();
        (id, env)
    }

    #[test]
    fn test_raw_store_operations() {
        let mut store = InMemoryObjectStore::new();

        // Get non-existent key
        assert!(store.get_raw("mod:0x1::coin").unwrap().is_none());

        // Set and get
        let bytecode = vec![0xCA, 0xFE, 0xBA, 0xBE];
        store.set_raw("mod:0x1::coin", bytecode.clone()).unwrap();
        assert_eq!(store.get_raw("mod:0x1::coin").unwrap().unwrap(), bytecode);

        // Delete
        store.delete_raw("mod:0x1::coin").unwrap();
        assert!(store.get_raw("mod:0x1::coin").unwrap().is_none());
    }

    #[test]
    fn test_object_store_operations() {
        let mut store = InMemoryObjectStore::new();
        let owner = Address::from_str_id("alice");
        let (id, env) = make_envelope(1, owner, 1000);

        // Get non-existent
        assert!(store.get_envelope(&id).unwrap().is_none());

        // Set and get
        store.set_envelope(id, env.clone()).unwrap();
        let retrieved = store.get_envelope(&id).unwrap().unwrap();
        assert_eq!(retrieved.magic, ENVELOPE_MAGIC);
        assert_eq!(retrieved.metadata.id, id);
        assert_eq!(retrieved.metadata.owner, owner);

        // Ownership index
        let owned = store.get_owned_ids(&owner).unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0], id);

        // Delete
        store.delete_envelope(&id).unwrap();
        assert!(store.get_envelope(&id).unwrap().is_none());
        assert!(store.get_owned_ids(&owner).unwrap().is_empty());
    }

    #[test]
    fn test_object_store_ownership_transfer() {
        let mut store = InMemoryObjectStore::new();
        let alice = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");

        let (id, env) = make_envelope(1, alice, 500);
        store.set_envelope(id, env).unwrap();
        assert_eq!(store.get_owned_ids(&alice).unwrap().len(), 1);

        // Re-set with new owner → ownership should transfer
        let coin_bob = setu_types::create_coin_with_id(id, bob, 500, "ROOT", 0);
        let env2 = ObjectEnvelope::from_coin_object(&coin_bob).unwrap();
        store.set_envelope(id, env2).unwrap();

        assert!(store.get_owned_ids(&alice).unwrap().is_empty());
        assert_eq!(store.get_owned_ids(&bob).unwrap().len(), 1);
    }

    #[test]
    fn test_object_store_state_store_compat() {
        let mut store = InMemoryObjectStore::new();
        let owner = Address::from_str_id("carol");
        let coin = setu_types::create_coin(owner, 750);
        let coin_id = *coin.id();

        // Use StateStore interface to set
        store.set_object(coin_id, coin.clone()).unwrap();

        // Use StateStore interface to get
        let retrieved = store.get_object(&coin_id).unwrap().unwrap();
        assert_eq!(retrieved.data.balance.value(), 750);

        // Verify ownership via ObjectStore interface
        let owned = store.get_owned_ids(&owner).unwrap();
        assert_eq!(owned.len(), 1);

        // Delete via StateStore interface
        store.delete_object(&coin_id).unwrap();
        assert!(store.get_object(&coin_id).unwrap().is_none());
    }
}
