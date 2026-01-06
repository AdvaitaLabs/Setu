//! State storage abstraction

use std::collections::HashMap;
use setu_types::{Object, ObjectId, Address, CoinData};
use crate::error::RuntimeResult;

/// State storage trait
/// 未来可以替换为持久化存储或 Move VM 的状态管理
pub trait StateStore {
    /// 读取对象
    fn get_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>>;
    
    /// 写入对象
    fn set_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()>;
    
    /// 删除对象
    fn delete_object(&mut self, object_id: &ObjectId) -> RuntimeResult<()>;
    
    /// 获取地址拥有的所有对象
    fn get_owned_objects(&self, owner: &Address) -> RuntimeResult<Vec<ObjectId>>;
    
    /// 检查对象是否存在
    fn exists(&self, object_id: &ObjectId) -> bool {
        self.get_object(object_id).ok().flatten().is_some()
    }
}

/// 内存状态存储（用于测试和简单场景）
#[derive(Debug, Clone)]
pub struct InMemoryStateStore {
    /// 对象存储: ObjectId -> Object
    objects: HashMap<ObjectId, Object<CoinData>>,
    /// 所有权索引: Address -> Vec<ObjectId>
    ownership_index: HashMap<Address, Vec<ObjectId>>,
}

impl InMemoryStateStore {
    /// 创建新的内存状态存储
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            ownership_index: HashMap::new(),
        }
    }
    
    /// 更新所有权索引
    fn update_ownership_index(&mut self, object_id: ObjectId, new_owner: &Address) {
        // 从旧所有者的索引中移除
        for objects in self.ownership_index.values_mut() {
            objects.retain(|id| id != &object_id);
        }
        
        // 添加到新所有者的索引
        self.ownership_index
            .entry(new_owner.clone())
            .or_insert_with(Vec::new)
            .push(object_id);
    }
    
    /// 从所有权索引中移除对象
    fn remove_from_ownership_index(&mut self, object_id: &ObjectId) {
        for objects in self.ownership_index.values_mut() {
            objects.retain(|id| id != object_id);
        }
    }
    
    /// 获取总余额（用于测试）
    pub fn get_total_balance(&self, owner: &Address) -> u64 {
        self.get_owned_objects(owner)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.get_object(id).ok().flatten())
            .map(|obj| obj.data.balance.value())
            .sum()
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
        // 更新所有权索引
        if let Some(owner) = &object.metadata.owner {
            self.update_ownership_index(object_id, owner);
        }
        
        // 存储对象
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
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_state_store_operations() {
        let mut store = InMemoryStateStore::new();
        
        let owner = Address::from("alice");
        let coin = setu_types::create_coin(owner.clone(), 1000);
        let coin_id = *coin.id();
        
        // 设置对象
        store.set_object(coin_id, coin.clone()).unwrap();
        
        // 读取对象
        let retrieved = store.get_object(&coin_id).unwrap().unwrap();
        assert_eq!(retrieved.id(), &coin_id);
        
        // 检查所有权索引
        let owned = store.get_owned_objects(&owner).unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0], coin_id);
        
        // 删除对象
        store.delete_object(&coin_id).unwrap();
        assert!(store.get_object(&coin_id).unwrap().is_none());
        
        let owned = store.get_owned_objects(&owner).unwrap();
        assert_eq!(owned.len(), 0);
    }
}
