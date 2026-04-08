//! State storage abstraction

use crate::error::{RuntimeError, RuntimeResult};
use crate::vm_object::SuiVmStoredObject;
use setu_types::{Address, CoinData, Object, ObjectId};
use std::collections::{HashMap, HashSet};

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

    /// Read a generic VM object used by the Sui subset interpreter.
    fn get_vm_object(&self, _object_id: &ObjectId) -> RuntimeResult<Option<SuiVmStoredObject>> {
        Ok(None)
    }

    /// Write a generic VM object used by the Sui subset interpreter.
    fn set_vm_object(
        &mut self,
        object_id: ObjectId,
        _object: SuiVmStoredObject,
    ) -> RuntimeResult<()> {
        Err(RuntimeError::StateError(format!(
            "VM object storage unsupported for {}",
            object_id
        )))
    }

    /// Delete a generic VM object used by the Sui subset interpreter.
    fn delete_vm_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        Err(RuntimeError::StateError(format!(
            "VM object storage unsupported for {}",
            object_id
        )))
    }

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
    /// Generic VM object storage: ObjectId -> stored object
    vm_objects: HashMap<ObjectId, SuiVmStoredObject>,
    /// Ownership index: Address -> Vec<ObjectId>
    ownership_index: HashMap<Address, Vec<ObjectId>>,
}

impl InMemoryStateStore {
    /// Create new in-memory state storage
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            vm_objects: HashMap::new(),
            ownership_index: HashMap::new(),
        }
    }

    /// Update ownership index
    fn update_ownership_index(&mut self, object_id: ObjectId, new_owner: &Address) {
        // Remove from the old owner's index
        for objects in self.ownership_index.values_mut() {
            objects.retain(|id| id != &object_id);
        }

        // Add to the new owner's index
        self.ownership_index
            .entry(new_owner.clone())
            .or_insert_with(Vec::new)
            .push(object_id);
    }

    /// Remove object from ownership index
    fn remove_from_ownership_index(&mut self, object_id: &ObjectId) {
        for objects in self.ownership_index.values_mut() {
            objects.retain(|id| id != object_id);
        }
    }

    /// Get total balance (used for testing)
    pub fn get_total_balance(&self, owner: &Address) -> u64 {
        self.get_owned_objects(owner)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.get_object(id).ok().flatten())
            .map(|obj| obj.data.balance.value())
            .sum()
    }

    /// Return whether an in-memory coin object currently shadows this ID.
    pub fn contains_object(&self, object_id: &ObjectId) -> bool {
        self.objects.contains_key(object_id)
    }

    /// Return whether an in-memory VM object currently shadows this ID.
    pub fn contains_vm_object(&self, object_id: &ObjectId) -> bool {
        self.vm_objects.contains_key(object_id)
    }

    /// Snapshot all in-memory coin objects.
    pub fn snapshot_objects(&self) -> Vec<(ObjectId, Object<CoinData>)> {
        self.objects
            .iter()
            .map(|(object_id, object)| (*object_id, object.clone()))
            .collect()
    }

    /// Snapshot all in-memory VM objects.
    pub fn snapshot_vm_objects(&self) -> Vec<(ObjectId, SuiVmStoredObject)> {
        self.vm_objects
            .iter()
            .map(|(object_id, object)| (*object_id, object.clone()))
            .collect()
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
        Ok(self.ownership_index.get(owner).cloned().unwrap_or_default())
    }

    fn get_vm_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<SuiVmStoredObject>> {
        Ok(self.vm_objects.get(object_id).cloned())
    }

    fn set_vm_object(
        &mut self,
        object_id: ObjectId,
        object: SuiVmStoredObject,
    ) -> RuntimeResult<()> {
        if let Some(owner) = &object.owner {
            self.update_ownership_index(object_id, owner);
        }

        self.vm_objects.insert(object_id, object);
        Ok(())
    }

    fn delete_vm_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.vm_objects.remove(object_id);
        self.remove_from_ownership_index(object_id);
        Ok(())
    }
}

/// Overlay state store that combines a persistent base store with
/// in-memory writes/deletes during execution.
///
/// Reads prefer the overlay first, then fall back to the base store.
/// Call `flush_to_base()` when you want durable changes to be written
/// back into the underlying store.
#[derive(Debug, Clone)]
pub struct OverlayStateStore<S: StateStore> {
    base: S,
    overlay: InMemoryStateStore,
    deleted_objects: HashSet<ObjectId>,
    deleted_vm_objects: HashSet<ObjectId>,
}

impl<S: StateStore> OverlayStateStore<S> {
    pub fn new(base: S) -> Self {
        Self {
            base,
            overlay: InMemoryStateStore::new(),
            deleted_objects: HashSet::new(),
            deleted_vm_objects: HashSet::new(),
        }
    }

    pub fn base(&self) -> &S {
        &self.base
    }

    pub fn base_mut(&mut self) -> &mut S {
        &mut self.base
    }

    pub fn overlay(&self) -> &InMemoryStateStore {
        &self.overlay
    }

    pub fn into_base(self) -> S {
        self.base
    }

    /// Persist overlay writes and deletions into the base store.
    pub fn flush_to_base(&mut self) -> RuntimeResult<()> {
        let deleted_objects: Vec<_> = self.deleted_objects.drain().collect();
        let deleted_vm_objects: Vec<_> = self.deleted_vm_objects.drain().collect();
        let overlay_objects = self.overlay.snapshot_objects();
        let overlay_vm_objects = self.overlay.snapshot_vm_objects();

        for object_id in deleted_objects {
            self.base.delete_object(&object_id)?;
        }
        for object_id in deleted_vm_objects {
            self.base.delete_vm_object(&object_id)?;
        }
        for (object_id, object) in overlay_objects {
            self.base.set_object(object_id, object)?;
        }
        for (object_id, object) in overlay_vm_objects {
            self.base.set_vm_object(object_id, object)?;
        }

        self.overlay = InMemoryStateStore::new();
        Ok(())
    }
}

impl<S: StateStore> StateStore for OverlayStateStore<S> {
    fn get_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>> {
        if self.deleted_objects.contains(object_id) {
            return Ok(None);
        }
        if self.overlay.contains_object(object_id) {
            return self.overlay.get_object(object_id);
        }
        self.base.get_object(object_id)
    }

    fn set_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()> {
        self.deleted_objects.remove(&object_id);
        self.overlay.set_object(object_id, object)
    }

    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.overlay.delete_object(object_id)?;
        self.deleted_objects.insert(*object_id);
        Ok(())
    }

    fn get_owned_objects(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>> {
        let mut object_ids = Vec::new();

        for object_id in self.base.get_owned_objects(owner)? {
            if self.deleted_objects.contains(&object_id)
                || self.deleted_vm_objects.contains(&object_id)
                || self.overlay.contains_object(&object_id)
                || self.overlay.contains_vm_object(&object_id)
            {
                continue;
            }
            object_ids.push(object_id);
        }

        for object_id in self.overlay.get_owned_objects(owner)? {
            if !object_ids.contains(&object_id) {
                object_ids.push(object_id);
            }
        }

        Ok(object_ids)
    }

    fn get_vm_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<SuiVmStoredObject>> {
        if self.deleted_vm_objects.contains(object_id) {
            return Ok(None);
        }
        if self.overlay.contains_vm_object(object_id) {
            return self.overlay.get_vm_object(object_id);
        }
        self.base.get_vm_object(object_id)
    }

    fn set_vm_object(
        &mut self,
        object_id: ObjectId,
        object: SuiVmStoredObject,
    ) -> RuntimeResult<()> {
        self.deleted_vm_objects.remove(&object_id);
        self.overlay.set_vm_object(object_id, object)
    }

    fn delete_vm_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.overlay.delete_vm_object(object_id)?;
        self.deleted_vm_objects.insert(*object_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_overlay_state_store_flushes_vm_objects_to_base() {
        let base = InMemoryStateStore::new();
        let mut store = OverlayStateStore::new(base);

        let owner = Address::from_str_id("alice");
        let object_id = ObjectId::new([0x31; 32]);
        let object = SuiVmStoredObject::new_owned(
            object_id,
            "Counter",
            owner,
            std::collections::BTreeMap::from([(
                "value".to_string(),
                crate::vm_object::SuiVmStoredValue::U64(41),
            )]),
        );

        store.set_vm_object(object_id, object).unwrap();
        assert!(store.base().get_vm_object(&object_id).unwrap().is_none());

        store.flush_to_base().unwrap();

        let persisted = store.base().get_vm_object(&object_id).unwrap().unwrap();
        assert_eq!(persisted.get_u64_field("value"), Some(41));
    }
}
