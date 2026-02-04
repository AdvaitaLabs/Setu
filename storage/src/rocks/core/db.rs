use std::path::Path;
use std::sync::Arc;
use rocksdb::{DB, WriteBatch, IteratorMode};
use serde::{Serialize, de::DeserializeOwned};
use bincode::Encode;

use super::{StorageError, RocksDBConfig, ColumnFamily};
use super::error::Result;

/// Main database wrapper for Setu
pub struct SetuDB {
    db: Arc<DB>,
}

impl SetuDB {
    /// Open or create a new database at the given path
    pub fn open(config: RocksDBConfig) -> Result<Self> {
        let opts = config.to_options();
        let cfs = ColumnFamily::descriptors();
        
        let db = DB::open_cf_descriptors(&opts, &config.path, cfs)?;
        
        Ok(Self {
            db: Arc::new(db),
        })
    }
    
    /// Open a database at the given path with default config
    pub fn open_default(path: impl AsRef<Path>) -> Result<Self> {
        let config = RocksDBConfig::new(path.as_ref());
        Self::open(config)
    }
    
    /// Get a reference to the underlying RocksDB instance
    pub fn inner(&self) -> &DB {
        &self.db
    }
    
    /// Get a column family handle
    fn cf_handle(&self, cf: ColumnFamily) -> Result<&rocksdb::ColumnFamily> {
        self.db
            .cf_handle(cf.name())
            .ok_or_else(|| StorageError::cf_not_found(cf.name()))
    }
    
    /// Serialize a key using bincode
    fn encode_key<K: Encode>(key: &K) -> Result<Vec<u8>> {
        bincode::encode_to_vec(key, bincode::config::standard())
            .map_err(|e| StorageError::serialization(e.to_string()))
    }
    
    /// Serialize a value using BCS (Binary Canonical Serialization)
    fn encode_value<V: Serialize>(value: &V) -> Result<Vec<u8>> {
        bcs::to_bytes(value)
            .map_err(|e| StorageError::serialization(e.to_string()))
    }
    
    /// Deserialize a value using BCS
    fn decode_value<V: DeserializeOwned>(bytes: &[u8]) -> Result<V> {
        bcs::from_bytes(bytes)
            .map_err(|e| StorageError::deserialization(e.to_string()))
    }
    
    /// Put a key-value pair into a column family
    pub fn put<K, V>(&self, cf: ColumnFamily, key: &K, value: &V) -> Result<()>
    where
        K: Encode,
        V: Serialize,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        let value_bytes = Self::encode_value(value)?;
        
        self.db.put_cf(cf_handle, key_bytes, value_bytes)?;
        Ok(())
    }
    
    /// Get a value by key from a column family
    pub fn get<K, V>(&self, cf: ColumnFamily, key: &K) -> Result<Option<V>>
    where
        K: Encode,
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        
        match self.db.get_cf(cf_handle, key_bytes)? {
            Some(bytes) => Ok(Some(Self::decode_value(&bytes)?)),
            None => Ok(None),
        }
    }
    
    /// Delete a key from a column family
    pub fn delete<K>(&self, cf: ColumnFamily, key: &K) -> Result<()>
    where
        K: Encode,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        
        self.db.delete_cf(cf_handle, key_bytes)?;
        Ok(())
    }
    
    /// Check if a key exists in a column family
    pub fn exists<K>(&self, cf: ColumnFamily, key: &K) -> Result<bool>
    where
        K: Encode,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        
        Ok(self.db.get_cf(cf_handle, key_bytes)?.is_some())
    }
    
    /// Multi-get: retrieve multiple values at once
    pub fn multi_get<K, V>(
        &self,
        cf: ColumnFamily,
        keys: &[K],
    ) -> Result<Vec<Option<V>>>
    where
        K: Encode,
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes: Vec<_> = keys
            .iter()
            .map(Self::encode_key)
            .collect::<Result<_>>()?;
        
        let results = self.db.multi_get_cf(
            key_bytes.iter().map(|k| (cf_handle, k.as_slice()))
        );
        
        results
            .into_iter()
            .map(|result| match result? {
                Some(bytes) => Ok(Some(Self::decode_value(&bytes)?)),
                None => Ok(None),
            })
            .collect()
    }
    
    /// Create a new write batch
    pub fn batch(&self) -> WriteBatch {
        WriteBatch::default()
    }
    
    // ========== Raw byte key operations (for composite keys) ==========
    
    /// Put a value with raw byte key
    pub fn put_raw<V>(&self, cf: ColumnFamily, key: &[u8], value: &V) -> Result<()>
    where
        V: Serialize,
    {
        let cf_handle = self.cf_handle(cf)?;
        let value_bytes = Self::encode_value(value)?;
        self.db.put_cf(cf_handle, key, value_bytes)?;
        Ok(())
    }
    
    /// Get a value by raw byte key
    pub fn get_raw<V>(&self, cf: ColumnFamily, key: &[u8]) -> Result<Option<V>>
    where
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        match self.db.get_cf(cf_handle, key)? {
            Some(bytes) => Ok(Some(Self::decode_value(&bytes)?)),
            None => Ok(None),
        }
    }
    
    /// Delete by raw byte key
    pub fn delete_raw(&self, cf: ColumnFamily, key: &[u8]) -> Result<()> {
        let cf_handle = self.cf_handle(cf)?;
        self.db.delete_cf(cf_handle, key)?;
        Ok(())
    }
    
    /// Add a put operation to a write batch
    pub fn batch_put<K, V>(
        &self,
        batch: &mut WriteBatch,
        cf: ColumnFamily,
        key: &K,
        value: &V,
    ) -> Result<()>
    where
        K: Encode,
        V: Serialize,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        let value_bytes = Self::encode_value(value)?;
        
        batch.put_cf(cf_handle, key_bytes, value_bytes);
        Ok(())
    }
    
    /// Add a delete operation to a write batch
    pub fn batch_delete<K>(
        &self,
        batch: &mut WriteBatch,
        cf: ColumnFamily,
        key: &K,
    ) -> Result<()>
    where
        K: Encode,
    {
        let cf_handle = self.cf_handle(cf)?;
        let key_bytes = Self::encode_key(key)?;
        
        batch.delete_cf(cf_handle, key_bytes);
        Ok(())
    }
    
    /// Add a put operation to a write batch using raw byte key
    pub fn batch_put_raw<V>(
        &self,
        batch: &mut WriteBatch,
        cf: ColumnFamily,
        key: &[u8],
        value: &V,
    ) -> Result<()>
    where
        V: Serialize,
    {
        let cf_handle = self.cf_handle(cf)?;
        let value_bytes = Self::encode_value(value)?;
        batch.put_cf(cf_handle, key, value_bytes);
        Ok(())
    }
    
    /// Add a delete operation to a write batch using raw byte key
    pub fn batch_delete_raw(
        &self,
        batch: &mut WriteBatch,
        cf: ColumnFamily,
        key: &[u8],
    ) -> Result<()> {
        let cf_handle = self.cf_handle(cf)?;
        batch.delete_cf(cf_handle, key);
        Ok(())
    }
    
    /// Check if a raw byte key exists in a column family
    pub fn exists_raw(&self, cf: ColumnFamily, key: &[u8]) -> Result<bool> {
        let cf_handle = self.cf_handle(cf)?;
        Ok(self.db.get_cf(cf_handle, key)?.is_some())
    }
    
    /// Scan all keys with a prefix (returns raw keys)
    pub fn prefix_scan_keys(&self, cf: ColumnFamily, prefix: &[u8]) -> Result<Vec<Vec<u8>>> {
        let cf_handle = self.cf_handle(cf)?;
        
        let keys: Vec<Vec<u8>> = self.db
            .prefix_iterator_cf(cf_handle, prefix)
            .filter_map(|result| result.ok().map(|(k, _)| k.to_vec()))
            .collect();
        
        Ok(keys)
    }
    
    /// Write a batch atomically
    pub fn write_batch(&self, batch: WriteBatch) -> Result<()> {
        self.db.write(batch)?;
        Ok(())
    }
    
    /// Iterate over all key-value pairs in a column family
    pub fn iter<K, V>(&self, cf: ColumnFamily) -> Result<impl Iterator<Item = Result<(K, V)>> + '_>
    where
        K: DeserializeOwned + bincode::Decode<()>,
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        
        Ok(self
            .db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|result| {
                let (key_bytes, value_bytes) = result?;
                let key = bincode::decode_from_slice(&key_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::deserialization(e.to_string()))?
                    .0;
                let value = Self::decode_value(&value_bytes)?;
                Ok((key, value))
            }))
    }
    
    /// Iterate over all values in a column family (ignoring keys)
    /// Useful when you only need values and keys use different encoding
    pub fn iter_values<V>(&self, cf: ColumnFamily) -> Result<impl Iterator<Item = Result<V>> + '_>
    where
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        
        Ok(self
            .db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|result| {
                let (_key_bytes, value_bytes) = result?;
                let value = Self::decode_value(&value_bytes)?;
                Ok(value)
            }))
    }
    
    /// Iterate with a prefix
    pub fn prefix_iter<P, K, V>(
        &self,
        cf: ColumnFamily,
        prefix: &P,
    ) -> Result<impl Iterator<Item = Result<(K, V)>> + '_>
    where
        P: Encode,
        K: DeserializeOwned + bincode::Decode<()>,
        V: DeserializeOwned,
    {
        let cf_handle = self.cf_handle(cf)?;
        let prefix_bytes = Self::encode_key(prefix)?;
        
        Ok(self
            .db
            .prefix_iterator_cf(cf_handle, &prefix_bytes)
            .map(|result| {
                let (key_bytes, value_bytes) = result?;
                let key = bincode::decode_from_slice(&key_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::deserialization(e.to_string()))?
                    .0;
                let value = Self::decode_value(&value_bytes)?;
                Ok((key, value))
            }))
    }
    
    /// Flush the database to disk
    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }
    
    /// Get property value for a column family
    pub fn property_int_value(&self, cf: ColumnFamily, property: &str) -> Result<Option<u64>> {
        let cf_handle = self.cf_handle(cf)?;
        Ok(self.db.property_int_value_cf(cf_handle, property)?)
    }
    
    /// Compact a column family
    pub fn compact(&self, cf: ColumnFamily) -> Result<()> {
        let cf_handle = self.cf_handle(cf)?;
        self.db.compact_range_cf(cf_handle, None::<&[u8]>, None::<&[u8]>);
        Ok(())
    }
}

impl Clone for SetuDB {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

// Ensure thread safety
unsafe impl Send for SetuDB {}
unsafe impl Sync for SetuDB {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode)]
    struct TestKey {
        id: u64,
    }
    
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestValue {
        name: String,
        age: u32,
    }
    
    fn setup_test_db() -> (SetuDB, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db = SetuDB::open_default(temp_dir.path()).unwrap();
        (db, temp_dir)
    }
    
    #[test]
    fn test_put_get() {
        let (db, _temp) = setup_test_db();
        
        let key = TestKey { id: 1 };
        let value = TestValue {
            name: "Alice".to_string(),
            age: 30,
        };
        
        db.put(ColumnFamily::Objects, &key, &value).unwrap();
        
        let retrieved: Option<TestValue> = db.get(ColumnFamily::Objects, &key).unwrap();
        assert_eq!(retrieved, Some(value));
    }
    
    #[test]
    fn test_delete() {
        let (db, _temp) = setup_test_db();
        
        let key = TestKey { id: 1 };
        let value = TestValue {
            name: "Bob".to_string(),
            age: 25,
        };
        
        db.put(ColumnFamily::Objects, &key, &value).unwrap();
        assert!(db.exists(ColumnFamily::Objects, &key).unwrap());
        
        db.delete(ColumnFamily::Objects, &key).unwrap();
        assert!(!db.exists(ColumnFamily::Objects, &key).unwrap());
    }
    
    #[test]
    fn test_batch_write() {
        let (db, _temp) = setup_test_db();
        
        let mut batch = db.batch();
        
        for i in 0..10 {
            let key = TestKey { id: i };
            let value = TestValue {
                name: format!("User{}", i),
                age: 20 + i as u32,
            };
            db.batch_put(&mut batch, ColumnFamily::Objects, &key, &value).unwrap();
        }
        
        db.write_batch(batch).unwrap();
        
        // Verify all values were written
        for i in 0..10 {
            let key = TestKey { id: i };
            let value: Option<TestValue> = db.get(ColumnFamily::Objects, &key).unwrap();
            assert!(value.is_some());
        }
    }
    
    #[test]
    fn test_multi_get() {
        let (db, _temp) = setup_test_db();
        
        // Insert multiple values
        for i in 0..5 {
            let key = TestKey { id: i };
            let value = TestValue {
                name: format!("User{}", i),
                age: 20 + i as u32,
            };
            db.put(ColumnFamily::Objects, &key, &value).unwrap();
        }
        
        // Multi-get
        let keys: Vec<TestKey> = (0..5).map(|id| TestKey { id }).collect();
        let values: Vec<Option<TestValue>> = db.multi_get(ColumnFamily::Objects, &keys).unwrap();
        
        assert_eq!(values.len(), 5);
        assert!(values.iter().all(|v| v.is_some()));
    }
}
