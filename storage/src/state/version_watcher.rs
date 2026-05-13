//! Per-object version watcher for `wait_min_version` long-poll API (B1).
//!
//! Design: [`docs/feat/pwoo-r6-wait-api/design.md`](../../../../docs/feat/pwoo-r6-wait-api/design.md).
//!
//! ## Architecture
//!
//! - `WatcherRegistry` holds an `Arc<Notify>` per object id (32-byte SMT key).
//! - HTTP handlers acquire a `WaitGuard` (RAII) before parking; `Drop`
//!   atomically decrements the per-object and global counters.
//! - `apply_committed_events` calls [`WatcherRegistry::notify_objects`]
//!   AFTER the SMT write commits within the same write-lock scope (design §4.4 A').
//! - Notifier uses the canonical "pre-arm `notified()`, then check version,
//!   then await" pattern to eliminate lost-wakeup races. Therefore no version
//!   number is shipped through the channel — waiters re-read state and decide
//!   whether the bumped version is high enough.
//!
//! ## Caps (OWASP A04)
//!
//! `WatcherCaps` bounds both per-object and global outstanding waiters.
//! Overshoots compensate via `fetch_sub` to keep counters honest under
//! concurrent registration.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::Notify;

/// Bounds on outstanding waiters; rejected with [`WatcherError`] when exceeded.
#[derive(Debug, Clone, Copy)]
pub struct WatcherCaps {
    pub per_object: u32,
    pub global: u32,
}

impl Default for WatcherCaps {
    fn default() -> Self {
        // Defaults match design §4.2.
        Self { per_object: 32, global: 1024 }
    }
}

/// Errors returned by [`WatcherRegistry::register`].
#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("global waiter cap exceeded ({0})")]
    GlobalCapExceeded(u32),
    #[error("per-object waiter cap exceeded ({0})")]
    PerObjectCapExceeded(u32),
}

/// Per-object slot: a `Notify` plus a refcount of outstanding waiters.
struct ObjectSlot {
    notify: Notify,
    waiters: AtomicU32,
}

impl ObjectSlot {
    fn new() -> Self {
        Self { notify: Notify::new(), waiters: AtomicU32::new(0) }
    }
}

/// Object key — 32-byte SMT key (same shape as `parse_state_change_key`'s
/// output for `oid:` prefixes).
pub type ObjKey = [u8; 32];

/// Registry of pending version waiters, owned by `GlobalStateManager` and
/// shared with the API layer through an `Arc`.
pub struct WatcherRegistry {
    per_object: DashMap<ObjKey, Arc<ObjectSlot>>,
    global_count: AtomicU32,
    caps: WatcherCaps,
}

impl WatcherRegistry {
    pub fn new(caps: WatcherCaps) -> Arc<Self> {
        Arc::new(Self {
            per_object: DashMap::new(),
            global_count: AtomicU32::new(0),
            caps,
        })
    }

    /// Register a waiter for `oid`. Returns a `WaitGuard` whose `Drop`
    /// decrements the counters atomically (success path AND any unwind /
    /// client-disconnect path).
    ///
    /// On overshoot, both counters are restored via compensating `fetch_sub`
    /// before the error is returned (design §4.2).
    pub fn register(self: &Arc<Self>, oid: ObjKey) -> Result<WaitGuard, WatcherError> {
        // Reserve a global slot first.
        let prev_global = self.global_count.fetch_add(1, Ordering::AcqRel);
        if prev_global >= self.caps.global {
            self.global_count.fetch_sub(1, Ordering::AcqRel);
            return Err(WatcherError::GlobalCapExceeded(self.caps.global));
        }

        // Get or create the per-object slot.
        let slot = self
            .per_object
            .entry(oid)
            .or_insert_with(|| Arc::new(ObjectSlot::new()))
            .clone();

        let prev_obj = slot.waiters.fetch_add(1, Ordering::AcqRel);
        if prev_obj >= self.caps.per_object {
            slot.waiters.fetch_sub(1, Ordering::AcqRel);
            self.global_count.fetch_sub(1, Ordering::AcqRel);
            return Err(WatcherError::PerObjectCapExceeded(self.caps.per_object));
        }

        Ok(WaitGuard { registry: Arc::clone(self), oid, slot })
    }

    /// Wake every waiter on each `oid` in `oids`. Called from
    /// `apply_committed_events` after the SMT writes for the events commit.
    pub fn notify_objects<I: IntoIterator<Item = ObjKey>>(&self, oids: I) {
        for oid in oids {
            if let Some(slot) = self.per_object.get(&oid) {
                slot.notify.notify_waiters();
            }
        }
    }

    /// Test/diagnostic: current outstanding global waiter count.
    pub fn global_waiters(&self) -> u32 {
        self.global_count.load(Ordering::Acquire)
    }
}

/// RAII waiter handle. Drop decrements both per-object and global counters.
pub struct WaitGuard {
    registry: Arc<WatcherRegistry>,
    oid: ObjKey,
    slot: Arc<ObjectSlot>,
}

impl std::fmt::Debug for WaitGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitGuard")
            .field("oid", &hex::encode(self.oid))
            .finish()
    }
}

impl WaitGuard {
    /// Returns a future that resolves when the next `notify_objects` call
    /// targets this object. The caller MUST construct this future BEFORE
    /// re-checking the materialized state, or a wakeup that races with the
    /// check could be lost.
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.slot.notify.notified()
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.slot.waiters.fetch_sub(1, Ordering::AcqRel);
        self.registry.global_count.fetch_sub(1, Ordering::AcqRel);
        // GC: if this was the last waiter on the object, remove the slot.
        // Safe because any concurrent `register` holding an Arc<ObjectSlot>
        // keeps it alive, and the next `register` for this oid will create
        // a fresh slot — `notify_objects` arriving on the old slot harmlessly
        // wakes nobody.
        if self.slot.waiters.load(Ordering::Acquire) == 0 {
            self.registry.per_object.remove_if(&self.oid, |_, slot| {
                slot.waiters.load(Ordering::Acquire) == 0
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key(b: u8) -> ObjKey {
        [b; 32]
    }

    #[test]
    fn register_drop_decrements_counters() {
        let reg = WatcherRegistry::new(WatcherCaps::default());
        assert_eq!(reg.global_waiters(), 0);
        {
            let _g1 = reg.register(key(1)).unwrap();
            assert_eq!(reg.global_waiters(), 1);
            let _g2 = reg.register(key(1)).unwrap();
            assert_eq!(reg.global_waiters(), 2);
            let _g3 = reg.register(key(2)).unwrap();
            assert_eq!(reg.global_waiters(), 3);
        }
        assert_eq!(reg.global_waiters(), 0);
        // GC: slot for key(1) and key(2) should be removed when waiters hit 0.
        assert_eq!(reg.per_object.len(), 0);
    }

    #[test]
    fn per_object_cap_exceeded() {
        let reg = WatcherRegistry::new(WatcherCaps { per_object: 2, global: 16 });
        let _g1 = reg.register(key(1)).unwrap();
        let _g2 = reg.register(key(1)).unwrap();
        match reg.register(key(1)) {
            Err(WatcherError::PerObjectCapExceeded(2)) => {}
            other => panic!("expected PerObjectCapExceeded, got {:?}", other),
        }
        // Global counter must not leak — overshoot was compensated.
        assert_eq!(reg.global_waiters(), 2);
    }

    #[test]
    fn global_cap_exceeded() {
        let reg = WatcherRegistry::new(WatcherCaps { per_object: 8, global: 2 });
        let _g1 = reg.register(key(1)).unwrap();
        let _g2 = reg.register(key(2)).unwrap();
        match reg.register(key(3)) {
            Err(WatcherError::GlobalCapExceeded(2)) => {}
            other => panic!("expected GlobalCapExceeded, got {:?}", other),
        }
        assert_eq!(reg.global_waiters(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_wakes_waiter() {
        let reg = WatcherRegistry::new(WatcherCaps::default());
        let guard = reg.register(key(7)).unwrap();
        let notified = guard.notified();
        let reg2 = Arc::clone(&reg);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            reg2.notify_objects([key(7)]);
        });
        tokio::time::timeout(Duration::from_millis(500), notified)
            .await
            .expect("wakeup before timeout");
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_arm_no_lost_wakeup() {
        // Canonical race: notify fires AFTER notified() future is constructed
        // but BEFORE .await. The future MUST still resolve.
        let reg = WatcherRegistry::new(WatcherCaps::default());
        let guard = reg.register(key(9)).unwrap();
        let notified = guard.notified();
        // Fire notify synchronously *before* we await — this is the key race.
        reg.notify_objects([key(9)]);
        tokio::time::timeout(Duration::from_millis(200), notified)
            .await
            .expect("pre-armed Notified must resolve");
    }

    #[test]
    fn drop_under_load_no_leak() {
        let reg = WatcherRegistry::new(WatcherCaps::default());
        for _ in 0..100 {
            let g = reg.register(key(42)).unwrap();
            drop(g);
        }
        assert_eq!(reg.global_waiters(), 0);
        assert_eq!(reg.per_object.len(), 0);
    }
}
