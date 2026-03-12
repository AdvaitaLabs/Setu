// storage/src/state/shared.rs
//
// Read-write separated GlobalStateManager wrapper.
//
// - Read path: lock-free snapshot via ArcSwap (for high-concurrency HTTP threads)
// - Write path: exclusive access via Mutex (for consensus single-writer)
// - After write completes, publish_snapshot() atomically updates the read snapshot

use arc_swap::ArcSwap;
use std::sync::{Arc, Mutex, MutexGuard};
use super::manager::GlobalStateManager;

/// Read-write separated GlobalStateManager wrapper.
///
/// - Read path: lock-free snapshot via ArcSwap, suitable for high-concurrency HTTP threads
/// - Write path: exclusive access via Mutex, suitable for consensus single-writer
/// - After write completes, call publish_snapshot() to atomically update the read snapshot
///
/// ## Thread Safety
///
/// - No contention among readers (ArcSwap::load is lock-free)
/// - Mutual exclusion among writers (Mutex)
/// - Writers do not block readers (readers hold Arc to old snapshot)
/// - Readers do not block writers (writers modify exclusive copy inside Mutex)
pub struct SharedStateManager {
    /// Read snapshot — all readers obtain an immutable GSM snapshot through this
    read_snapshot: ArcSwap<GlobalStateManager>,

    /// Write end — the sole writer (consensus thread) modifies state through this
    write_gsm: Mutex<GlobalStateManager>,
}

impl SharedStateManager {
    /// Create a new SharedStateManager.
    ///
    /// `gsm` will be cloned once: the original goes into the write Mutex,
    /// the clone goes into the read ArcSwap snapshot.
    pub fn new(gsm: GlobalStateManager) -> Self {
        let read_copy = gsm.clone_for_read_snapshot();
        Self {
            read_snapshot: ArcSwap::from_pointee(read_copy),
            write_gsm: Mutex::new(gsm),
        }
    }

    /// Read path: get the current read snapshot (lock-free, O(1)).
    ///
    /// The returned Guard holds an Arc<GSM>; data is immutable for the Guard's lifetime.
    /// Readers should reuse the same Guard within a single request for consistency.
    pub fn load_snapshot(&self) -> arc_swap::Guard<Arc<GlobalStateManager>> {
        self.read_snapshot.load()
    }

    /// Read path: get the current read snapshot as Arc (lock-free, O(1)).
    ///
    /// Costs one extra Arc clone compared to load_snapshot, but can be held longer.
    pub fn load_snapshot_arc(&self) -> Arc<GlobalStateManager> {
        self.read_snapshot.load_full()
    }

    /// Write path: acquire the write Mutex lock.
    ///
    /// Caller gets exclusive write access to GSM.
    /// After completing write operations, must call publish_snapshot() to update the read snapshot.
    pub fn lock_write(&self) -> MutexGuard<'_, GlobalStateManager> {
        self.write_gsm.lock()
            .expect("SharedStateManager write mutex poisoned")
    }

    /// Publish a new read snapshot.
    ///
    /// Should be called after every commit_build completes.
    /// Internally executes clone_for_read_snapshot() (im::HashMap O(1) + index clone)
    /// then atomically replaces the read snapshot.
    ///
    /// ## Cost
    /// - im::HashMap clone: O(1) (structural sharing)
    /// - Index clone: O(N_accounts) (currently ~200 accounts → ~100μs)
    /// - ArcSwap::store: O(1) (atomic pointer swap)
    pub fn publish_snapshot(&self, gsm: &GlobalStateManager) {
        let new_snapshot = Arc::new(gsm.clone_for_read_snapshot());
        self.read_snapshot.store(new_snapshot);
    }

    /// Execute a closure with exclusive write access to GSM.
    ///
    /// Convenience method for initialization-time operations.
    pub fn with_write_gsm<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut GlobalStateManager) -> R,
    {
        let mut guard = self.lock_write();
        f(&mut guard)
    }
}
