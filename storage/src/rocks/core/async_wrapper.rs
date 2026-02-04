//! Async wrappers for blocking RocksDB operations
//!
//! RocksDB operations are inherently blocking (they perform disk I/O).
//! When called from an async context, they can block the executor thread.
//! This module provides utilities to safely run RocksDB operations
//! on Tokio's blocking thread pool using `spawn_blocking`.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::rocks::async_wrapper::spawn_db_op;
//!
//! let result = spawn_db_op(move || {
//!     db.get(cf, &key)
//! }).await?;
//! ```
//!
//! ## Performance Considerations
//!
//! - `spawn_blocking` has overhead (~1-2μs per call)
//! - For very fast operations, the overhead may be significant
//! - Batch operations are preferred to reduce spawn_blocking calls
//! - Use for: writes, iterations, flushes, compactions
//! - Consider direct calls for: simple single-key reads with hot cache

use tokio::task::JoinHandle;

/// Spawn a blocking database operation on Tokio's blocking thread pool.
///
/// This is the recommended way to perform RocksDB operations from async code.
/// It prevents blocking the async runtime's worker threads.
///
/// # Arguments
/// * `f` - A closure that performs the blocking database operation
///
/// # Returns
/// * A future that resolves to the result of the operation
///
/// # Example
/// ```ignore
/// let anchor: Option<Anchor> = spawn_db_op(move || {
///     db.get_raw(ColumnFamily::Anchors, &key).ok().flatten()
/// }).await;
/// ```
pub async fn spawn_db_op<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("Blocking task panicked")
}

/// Spawn a blocking database operation that may fail.
///
/// Similar to `spawn_db_op` but for operations that return `Result`.
///
/// # Example
/// ```ignore
/// let result = spawn_db_op_result(move || {
///     db.put(cf, &key, &value)
/// }).await?;
/// ```
pub async fn spawn_db_op_result<F, T, E>(f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("Blocking task panicked")
}

/// A wrapper that holds state needed for spawning blocking operations.
///
/// This is useful when you need to spawn multiple related operations
/// and want to avoid cloning the database handle repeatedly.
///
/// # Example
/// ```ignore
/// let wrapper = BlockingDbWrapper::new(db.clone());
/// let value = wrapper.run(|db| db.get(cf, &key)).await;
/// ```
pub struct BlockingDbWrapper<D: Clone + Send + 'static> {
    inner: D,
}

impl<D: Clone + Send + 'static> BlockingDbWrapper<D> {
    /// Create a new wrapper around a cloneable database handle
    pub fn new(inner: D) -> Self {
        Self { inner }
    }

    /// Run a blocking operation on the wrapped database
    pub async fn run<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&D) -> T + Send + 'static,
        T: Send + 'static,
    {
        let db = self.inner.clone();
        spawn_db_op(move || f(&db)).await
    }

    /// Run a blocking operation that returns a Result
    pub async fn run_result<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&D) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
    {
        let db = self.inner.clone();
        spawn_db_op_result(move || f(&db)).await
    }
}

/// Trait extension for running multiple operations as a batch
pub trait BatchBlockingOps {
    /// The database type
    type Db;
    
    /// Run a batch of operations that should be executed together
    fn run_batch<F, T>(&self, f: F) -> JoinHandle<T>
    where
        F: FnOnce(&Self::Db) -> T + Send + 'static,
        T: Send + 'static;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn test_spawn_db_op() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        
        let result = spawn_db_op(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            42u64
        }).await;
        
        assert_eq!(result, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_spawn_db_op_result() {
        let result: Result<u64, &str> = spawn_db_op_result(|| {
            Ok(42)
        }).await;
        
        assert_eq!(result, Ok(42));
        
        let err_result: Result<u64, &str> = spawn_db_op_result(|| {
            Err("failed")
        }).await;
        
        assert_eq!(err_result, Err("failed"));
    }

    #[tokio::test]
    async fn test_blocking_wrapper() {
        let counter = Arc::new(AtomicU64::new(0));
        let wrapper = BlockingDbWrapper::new(counter.clone());
        
        let result = wrapper.run(|c| {
            c.fetch_add(10, Ordering::SeqCst);
            c.load(Ordering::SeqCst)
        }).await;
        
        assert_eq!(result, 10);
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn test_multiple_concurrent_ops() {
        let counter = Arc::new(AtomicU64::new(0));
        
        let mut handles = Vec::new();
        for _ in 0..10 {
            let c = counter.clone();
            handles.push(tokio::spawn(async move {
                spawn_db_op(move || {
                    c.fetch_add(1, Ordering::SeqCst)
                }).await
            }));
        }
        
        for handle in handles {
            handle.await.unwrap();
        }
        
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }
}
