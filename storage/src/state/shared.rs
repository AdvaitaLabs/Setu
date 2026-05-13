// storage/src/state/shared.rs
//
// Read-write separated GlobalStateManager wrapper.
//
// - Read path: lock-free snapshot via ArcSwap (for high-concurrency HTTP threads)
// - Write path: exclusive access via Mutex (for consensus single-writer)
// - After write completes, publish_snapshot() atomically updates the read snapshot

use arc_swap::ArcSwap;
use std::sync::{Arc, Mutex, MutexGuard};
use setu_merkle::HashValue;
use setu_types::{event::StateChange, SubnetId};
use super::manager::GlobalStateManager;
use super::speculative_overlay::{
    OverlayClearStats, OverlayStats, SpeculativeOverlay, StageError,
};

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

    /// Speculative overlay for MoveCall pre-apply.
    ///
    /// Reads go through `OverlayView` which merges overlay + SMT snapshot;
    /// writes are attached to a CF event_id and cleared on CF finalization.
    /// See `storage/src/state/speculative_overlay.rs` and
    /// `docs/feat/move-call-speculative-overlay/design.md`.
    overlay: Arc<SpeculativeOverlay>,
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
            overlay: Arc::new(SpeculativeOverlay::new()),
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

    // ============================================================================
    // Speculative Overlay API (MoveCall pre-apply path)
    // ============================================================================

    /// Read path with speculative overlay merged on top of the SMT snapshot.
    ///
    /// All MerkleStateProvider reads should go through this to preserve
    /// read-your-writes semantics during the pre-apply → CF finalize window.
    pub fn load_overlay_view(&self) -> OverlayView {
        OverlayView {
            snapshot: self.read_snapshot.load_full(),
            overlay: Arc::clone(&self.overlay),
        }
    }

    /// Stage a MoveCall event's state_changes into the speculative overlay.
    ///
    /// See [`SpeculativeOverlay::stage`]: returns `Err(StageError::MalformedKey)`
    /// if any change's key is not `"oid:{64-hex}"`.
    pub fn stage_overlay(
        &self,
        event_id: &str,
        event_subnet: SubnetId,
        changes: &[StateChange],
    ) -> Result<(), StageError> {
        self.overlay.stage(event_id, event_subnet, changes)
    }

    /// Clear all overlay entries owned by the given event ids.
    ///
    /// Called after CF finalization (both applied and stale_read outcomes).
    pub fn clear_overlay_events(&self, event_ids: &[String]) -> OverlayClearStats {
        self.overlay.clear_events(event_ids)
    }

    /// Debug / metric: current overlay size and oldest entry age.
    pub fn overlay_stats(&self) -> OverlayStats {
        self.overlay.stats()
    }
}

// ============================================================================
// OverlayView — merged read over (SMT snapshot + speculative overlay)
// ============================================================================

/// A single-use merged read view.
///
/// Holds an Arc to the current SMT snapshot and a reference to the
/// overlay. Lookups check the overlay first; on miss, fall back to the
/// SMT snapshot.
///
/// Reuse within a single HTTP request for time-point consistency.
pub struct OverlayView {
    snapshot: Arc<GlobalStateManager>,
    overlay: Arc<SpeculativeOverlay>,
}

impl OverlayView {
    /// Lookup an object (32-byte id) under a specific subnet.
    ///
    /// Returns:
    /// - `Some(bytes)` if overlay has Insert/Update OR SMT has the key
    /// - `None` if overlay has pre-Delete OR the key is absent in both
    pub fn get_subnet_object(
        &self,
        subnet: &SubnetId,
        oid_bytes: &[u8; 32],
    ) -> Option<Vec<u8>> {
        let hv = HashValue::from_slice(oid_bytes).ok()?;
        match self.overlay.get(subnet, &hv) {
            Some(Some(bytes)) => Some(bytes),
            Some(None) => None,
            None => self.snapshot.get_subnet(subnet)?.get(&hv).cloned(),
        }
    }

    /// Lookup by raw string key (e.g., module bytecode `"mod:0x..::Name"`).
    ///
    /// Overlay and SMT both key by `BLAKE3(key)` in the ROOT subnet.
    pub fn get_raw_data(&self, key: &str) -> Option<Vec<u8>> {
        let hash = setu_types::hash_utils::setu_hash(key.as_bytes());
        let hv = HashValue::from_slice(&hash).ok()?;
        match self.overlay.get(&SubnetId::ROOT, &hv) {
            Some(Some(bytes)) => Some(bytes),
            Some(None) => None,
            None => self
                .snapshot
                .get_subnet(&SubnetId::ROOT)?
                .get(&hv)
                .cloned(),
        }
    }
}

// ============================================================================
// Tests — OverlayView + overlay API on SharedStateManager
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn oid_bytes(b: u8) -> [u8; 32] {
        [b; 32]
    }

    fn oid_key_str(b: u8) -> String {
        format!("oid:{}", hex::encode(oid_bytes(b)))
    }

    fn fresh_shared() -> Arc<SharedStateManager> {
        Arc::new(SharedStateManager::new(GlobalStateManager::new()))
    }

    #[test]
    fn overlay_view_smt_fallback() {
        // Seed SMT with a value via write path, then verify OverlayView
        // returns the SMT value when overlay is empty.
        let shared = fresh_shared();
        let hv = HashValue::from_slice(&oid_bytes(0x10)).unwrap();
        {
            let mut w = shared.lock_write();
            let smt = w.get_subnet_mut(SubnetId::ROOT);
            smt.upsert(hv, b"smt_value".to_vec());
        }
        {
            let w = shared.lock_write();
            shared.publish_snapshot(&w);
        }

        let view = shared.load_overlay_view();
        assert_eq!(
            view.get_subnet_object(&SubnetId::ROOT, &oid_bytes(0x10)),
            Some(b"smt_value".to_vec())
        );
    }

    #[test]
    fn overlay_view_overlay_hit() {
        let shared = fresh_shared();
        let key = oid_key_str(0x11);
        shared
            .stage_overlay(
                "E1",
                SubnetId::ROOT,
                &[StateChange::insert(key, b"overlay_value".to_vec())],
            )
            .unwrap();

        let view = shared.load_overlay_view();
        assert_eq!(
            view.get_subnet_object(&SubnetId::ROOT, &oid_bytes(0x11)),
            Some(b"overlay_value".to_vec())
        );
    }

    #[test]
    fn overlay_view_overlay_delete_masks_smt() {
        let shared = fresh_shared();
        let hv = HashValue::from_slice(&oid_bytes(0x12)).unwrap();
        {
            let mut w = shared.lock_write();
            let smt = w.get_subnet_mut(SubnetId::ROOT);
            smt.upsert(hv, b"smt_value".to_vec());
        }
        {
            let w = shared.lock_write();
            shared.publish_snapshot(&w);
        }

        // Stage a Delete on top — overlay should mask SMT
        shared
            .stage_overlay(
                "E1",
                SubnetId::ROOT,
                &[StateChange::delete(
                    oid_key_str(0x12),
                    b"smt_value".to_vec(),
                )],
            )
            .unwrap();

        let view = shared.load_overlay_view();
        assert_eq!(
            view.get_subnet_object(&SubnetId::ROOT, &oid_bytes(0x12)),
            None
        );
    }

    #[test]
    fn clear_overlay_exposes_smt() {
        let shared = fresh_shared();
        let hv = HashValue::from_slice(&oid_bytes(0x13)).unwrap();
        {
            let mut w = shared.lock_write();
            let smt = w.get_subnet_mut(SubnetId::ROOT);
            smt.upsert(hv, b"smt_original".to_vec());
        }
        {
            let w = shared.lock_write();
            shared.publish_snapshot(&w);
        }

        // Stage a new value
        shared
            .stage_overlay(
                "E1",
                SubnetId::ROOT,
                &[StateChange::update(
                    oid_key_str(0x13),
                    b"smt_original".to_vec(),
                    b"overlay_new".to_vec(),
                )],
            )
            .unwrap();

        // Overlay hit
        assert_eq!(
            shared
                .load_overlay_view()
                .get_subnet_object(&SubnetId::ROOT, &oid_bytes(0x13)),
            Some(b"overlay_new".to_vec())
        );

        // Clear — overlay empties, SMT exposed
        let stats = shared.clear_overlay_events(&["E1".to_string()]);
        assert_eq!(stats.cleared, 1);
        assert_eq!(
            shared
                .load_overlay_view()
                .get_subnet_object(&SubnetId::ROOT, &oid_bytes(0x13)),
            Some(b"smt_original".to_vec())
        );
    }
}
