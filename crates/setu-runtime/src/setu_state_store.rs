use std::path::Path;
use std::sync::{Arc, RwLock};

use setu_storage::{
    B4StoreExt, GlobalStateManager, MerkleStateProvider, RocksDBMerkleStore, SetuDB, StateProvider,
};
use setu_types::{object_key, Address, CoinData, Object, ObjectId, StateChange, SubnetId};

use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::vm_object::SuiVmStoredObject;

/// StateStore adapter backed by Setu's Merkle state manager and provider.
///
/// This lets RuntimeExecutor operate against Setu's persistent storage
/// without hard-coding storage details into examples or VM code.
pub struct SetuMerkleStateStore {
    state_manager: Arc<RwLock<GlobalStateManager>>,
    state_provider: MerkleStateProvider,
    subnet_id: SubnetId,
    next_anchor: u64,
}

impl SetuMerkleStateStore {
    /// Open a Setu-backed state store for the ROOT subnet.
    pub fn open_root(db_path: &Path) -> RuntimeResult<Self> {
        Self::open(db_path, SubnetId::ROOT)
    }

    /// Open a Setu-backed state store for the given subnet.
    pub fn open(db_path: &Path, subnet_id: SubnetId) -> RuntimeResult<Self> {
        let db = Arc::new(SetuDB::open_default(db_path).map_err(|e| {
            RuntimeError::StateError(format!(
                "Failed to open SetuDB at {}: {}",
                db_path.display(),
                e
            ))
        })?);
        let merkle_store: Arc<dyn B4StoreExt> = Arc::new(RocksDBMerkleStore::from_shared(db));
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::with_store(merkle_store)));

        {
            let mut manager = state_manager.write().map_err(|_| {
                RuntimeError::StateError(
                    "GlobalStateManager lock poisoned during open".to_string(),
                )
            })?;
            manager.recover().map_err(|e| {
                RuntimeError::StateError(format!("Failed to recover Setu state: {}", e))
            })?;
            manager.rebuild_coin_type_index();
        }

        let next_anchor = state_manager
            .read()
            .map_err(|_| {
                RuntimeError::StateError(
                    "GlobalStateManager lock poisoned during read".to_string(),
                )
            })?
            .current_anchor()
            + 1;
        let state_provider = MerkleStateProvider::with_subnet(Arc::clone(&state_manager), subnet_id);

        Ok(Self {
            state_manager,
            state_provider,
            subnet_id,
            next_anchor,
        })
    }

    /// Commit pending state changes as the next anchor.
    pub fn commit_pending(&mut self) -> RuntimeResult<u64> {
        let anchor_id = self.next_anchor;
        self.state_manager
            .write()
            .map_err(|_| {
                RuntimeError::StateError(
                    "GlobalStateManager lock poisoned during commit".to_string(),
                )
            })?
            .commit(anchor_id)
            .map_err(|e| {
                RuntimeError::StateError(format!(
                    "Failed to commit Setu state at anchor {}: {}",
                    anchor_id, e
                ))
            })?;
        self.next_anchor += 1;
        Ok(anchor_id)
    }

    /// Return the current subnet root bytes.
    pub fn state_root(&self) -> [u8; 32] {
        self.state_provider.get_state_root()
    }

    /// Return the raw bytes persisted for an object, if present.
    pub fn get_object_bytes(&self, object_id: &ObjectId) -> Option<Vec<u8>> {
        self.state_provider.get_object(object_id)
    }

    /// Access the configured subnet.
    pub fn subnet_id(&self) -> SubnetId {
        self.subnet_id
    }

    /// Access the underlying state provider.
    pub fn state_provider(&self) -> &MerkleStateProvider {
        &self.state_provider
    }

    /// Access the underlying state manager handle.
    pub fn state_manager(&self) -> &Arc<RwLock<GlobalStateManager>> {
        &self.state_manager
    }
}

impl StateStore for SetuMerkleStateStore {
    fn get_object(&self, _object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>> {
        Ok(None)
    }

    fn set_object(
        &mut self,
        object_id: ObjectId,
        _object: Object<CoinData>,
    ) -> RuntimeResult<()> {
        Err(RuntimeError::StateError(format!(
            "Coin storage is unsupported in SetuMerkleStateStore for {}",
            object_id
        )))
    }

    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        Err(RuntimeError::StateError(format!(
            "Coin storage is unsupported in SetuMerkleStateStore for {}",
            object_id
        )))
    }

    fn get_owned_objects(&self, _owner: &Address) -> RuntimeResult<Vec<ObjectId>> {
        Ok(vec![])
    }

    fn get_vm_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<SuiVmStoredObject>> {
        self.get_object_bytes(object_id)
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|e| {
                    RuntimeError::StateError(format!(
                        "Failed to decode VM object {} from Setu state: {}",
                        object_id, e
                    ))
                })
            })
            .transpose()
    }

    fn set_vm_object(
        &mut self,
        object_id: ObjectId,
        object: SuiVmStoredObject,
    ) -> RuntimeResult<()> {
        let bytes = serde_json::to_vec(&object)
            .map_err(|e| RuntimeError::StateError(format!("Failed to encode VM object: {}", e)))?;
        self.state_manager
            .write()
            .map_err(|_| RuntimeError::StateError("GlobalStateManager lock poisoned".to_string()))?
            .upsert_object(self.subnet_id, *object_id.as_bytes(), bytes);
        Ok(())
    }

    fn delete_vm_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        let old_value = self.get_object_bytes(object_id);
        let change = StateChange {
            key: object_key(object_id),
            old_value,
            new_value: None,
        };
        self.state_manager
            .write()
            .map_err(|_| RuntimeError::StateError("GlobalStateManager lock poisoned".to_string()))?
            .apply_state_change(self.subnet_id, &change);
        Ok(())
    }
}
