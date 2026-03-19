//! Coin Reservation Manager
//!
//! Lightweight coin reservation mechanism to prevent double-spending across concurrent batches.
//! Uses DashMap for lock-free concurrent access with minimal contention.
//!
//! ## Design Principles
//!
//! 1. **No global lock**: DashMap uses sharded locks (~100ns per operation)
//! 2. **TTL protection**: Reservations auto-expire to prevent deadlocks
//! 3. **Hot-switch**: Can be disabled at runtime for emergency rollback
//!
//! ## Performance Characteristics
//!
//! | Operation | Overhead | Notes |
//! |-----------|----------|-------|
//! | `try_reserve()` | ~50-100ns | DashMap sharded lock |
//! | `release()` | ~30-50ns | Simple deletion |
//! | Memory | ~100 bytes/reservation | ObjectId(32) + Reservation(~60) |

use dashmap::DashMap;
use setu_types::ObjectId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::warn;
use uuid::Uuid;

/// Lightweight Coin Reservation Manager
///
/// Prevents double-spending across concurrent batches by reserving coins
/// before TEE task submission.
///
/// ## Thread Safety
///
/// Uses DashMap for lock-free concurrent access. Each operation only locks
/// a single shard, minimizing contention even under high concurrency.
///
/// ## Example
///
/// ```rust,ignore
/// let mgr = CoinReservationManager::default();
///
/// // Try to reserve a coin
/// if let Some(handle) = mgr.try_reserve(&coin_id, 100, "tx-123") {
///     // Reservation successful - execute TEE task
///     // ...
///     mgr.release(&handle);  // Release after completion
/// } else {
///     // Coin already reserved - try another coin or fail
/// }
/// ```
pub struct CoinReservationManager {
    /// coin_id → reservation info (DashMap: sharded locks, minimal contention)
    reservations: DashMap<ObjectId, Reservation>,
    /// Reservation timeout (default: 30s)
    ttl: Duration,
    /// Enable/disable flag (hot-switch)
    enabled: AtomicBool,
}

/// Reservation information
struct Reservation {
    /// Unique reservation identifier
    id: Uuid,
    /// Reserved amount
    amount: u64,
    /// Creation time (for TTL check)
    created_at: Instant,
    /// Associated transfer_id (for debugging)
    transfer_id: String,
}

/// Reservation handle returned on successful reservation
///
/// Must be passed to `release()` after TEE task completion.
#[derive(Debug, Clone)]
pub struct ReservationHandle {
    /// Unique reservation identifier
    pub reservation_id: Uuid,
    /// Reserved coin's object ID
    pub coin_id: ObjectId,
}

impl CoinReservationManager {
    /// Create a new reservation manager
    ///
    /// # Arguments
    /// * `ttl` - Reservation timeout; expired reservations are automatically invalid
    pub fn new(ttl: Duration) -> Self {
        Self {
            reservations: DashMap::new(),
            ttl,
            enabled: AtomicBool::new(true),
        }
    }

    /// Create with default configuration (TTL = 30s)
    pub fn with_default_ttl() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Try to reserve a coin
    ///
    /// Returns `Some(ReservationHandle)` on success, `None` if coin is already reserved.
    ///
    /// ## Thread Safety
    ///
    /// Uses DashMap::entry() for atomic check-and-insert, avoiding TOCTOU races.
    ///
    /// ## Arguments
    /// * `coin_id` - The coin's object ID
    /// * `amount` - Amount to reserve
    /// * `transfer_id` - Associated transfer ID (for debugging)
    pub fn try_reserve(
        &self,
        coin_id: &ObjectId,
        amount: u64,
        transfer_id: &str,
    ) -> Option<ReservationHandle> {
        // Hot-switch: when disabled, always succeed
        if !self.enabled.load(Ordering::Relaxed) {
            return Some(ReservationHandle {
                reservation_id: Uuid::nil(),
                coin_id: coin_id.clone(),
            });
        }

        let reservation_id = Uuid::new_v4();

        // DashMap::entry() only locks a single shard - minimal contention
        use dashmap::mapref::entry::Entry;
        match self.reservations.entry(coin_id.clone()) {
            Entry::Vacant(e) => {
                // No existing reservation - insert new one
                e.insert(Reservation {
                    id: reservation_id,
                    amount,
                    created_at: Instant::now(),
                    transfer_id: transfer_id.to_string(),
                });
                Some(ReservationHandle {
                    reservation_id,
                    coin_id: coin_id.clone(),
                })
            }
            Entry::Occupied(mut e) => {
                // Check if existing reservation has expired
                if e.get().created_at.elapsed() > self.ttl {
                    // Expired - replace it
                    warn!(
                        old_transfer = %e.get().transfer_id,
                        new_transfer = %transfer_id,
                        coin_id = %hex::encode(coin_id.as_bytes()),
                        "Replacing expired coin reservation"
                    );
                    e.insert(Reservation {
                        id: reservation_id,
                        amount,
                        created_at: Instant::now(),
                        transfer_id: transfer_id.to_string(),
                    });
                    Some(ReservationHandle {
                        reservation_id,
                        coin_id: coin_id.clone(),
                    })
                } else {
                    // Valid reservation exists - reject
                    None
                }
            }
        }
    }

    /// Release a reservation
    ///
    /// Only releases if the reservation ID matches (prevents releasing another thread's reservation).
    /// Must be called after TEE task completion (success or failure).
    pub fn release(&self, handle: &ReservationHandle) {
        // Hot-switch: when disabled, no-op
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        // Nil UUID means reservation was bypassed (disabled mode)
        if handle.reservation_id.is_nil() {
            return;
        }

        // Only release if reservation ID matches (prevent mis-release)
        self.reservations
            .remove_if(&handle.coin_id, |_, r| r.id == handle.reservation_id);
    }

    /// Background cleanup of expired reservations
    ///
    /// Optional: call periodically (e.g., every 60s) to clean up
    /// reservations from abnormally terminated tasks.
    /// Returns the number of cleaned reservations.
    pub fn cleanup_expired(&self) -> usize {
        let mut removed = 0;
        self.reservations.retain(|_, r| {
            let expired = r.created_at.elapsed() > self.ttl;
            if expired {
                warn!(
                    transfer_id = %r.transfer_id,
                    age_secs = r.created_at.elapsed().as_secs(),
                    "Cleaning up expired coin reservation"
                );
                removed += 1;
            }
            !expired
        });
        removed
    }

    /// Hot-switch: enable/disable reservation mechanism
    ///
    /// When disabled, all `try_reserve()` calls immediately succeed (pass-through).
    /// Use for emergency rollback or performance comparison testing.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            // Clear all reservations when disabled
            self.reservations.clear();
        }
    }

    /// Check if reservation is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Get current reservation count (for monitoring)
    pub fn reservation_count(&self) -> usize {
        self.reservations.len()
    }

    /// Get TTL duration
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Try to atomically reserve multiple coins (all-or-nothing).
    ///
    /// On success returns handles for every coin. On failure (any coin already
    /// reserved) all previously-acquired reservations in this batch are rolled
    /// back and `None` is returned.
    ///
    /// ## Thread Safety
    ///
    /// Each individual `try_reserve` is atomic (DashMap entry API), and rollback
    /// uses `release` which is also shard-level. A tiny race window exists where
    /// a competing caller could see an intermediate state, but that is benign:
    /// the competing caller would simply fail its own `try_reserve` for the same
    /// coin and retry later.
    pub fn try_reserve_batch(
        &self,
        coins: &[(&ObjectId, u64)],
        transfer_id: &str,
    ) -> Option<Vec<ReservationHandle>> {
        let mut handles = Vec::with_capacity(coins.len());

        for &(coin_id, amount) in coins {
            match self.try_reserve(coin_id, amount, transfer_id) {
                Some(handle) => handles.push(handle),
                None => {
                    // Rollback all previously acquired reservations
                    for h in &handles {
                        self.release(h);
                    }
                    return None;
                }
            }
        }

        Some(handles)
    }

    /// Release all handles in a batch.
    pub fn release_batch(&self, handles: &[ReservationHandle]) {
        for h in handles {
            self.release(h);
        }
    }
}

impl Default for CoinReservationManager {
    fn default() -> Self {
        Self::with_default_ttl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_coin_id(n: u8) -> ObjectId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        ObjectId::from_bytes(&bytes).expect("test coin id should be valid")
    }

    #[test]
    fn test_basic_reserve_release() {
        let mgr = CoinReservationManager::default();
        let coin = test_coin_id(1);

        // First reservation should succeed
        let handle = mgr.try_reserve(&coin, 100, "tx-1").unwrap();
        assert_eq!(mgr.reservation_count(), 1);

        // Second reservation for same coin should fail
        assert!(mgr.try_reserve(&coin, 50, "tx-2").is_none());

        // Release
        mgr.release(&handle);
        assert_eq!(mgr.reservation_count(), 0);

        // Now reservation should succeed again
        assert!(mgr.try_reserve(&coin, 100, "tx-3").is_some());
    }

    #[test]
    fn test_different_coins() {
        let mgr = CoinReservationManager::default();
        let coin1 = test_coin_id(1);
        let coin2 = test_coin_id(2);

        // Both should succeed
        let h1 = mgr.try_reserve(&coin1, 100, "tx-1").unwrap();
        let h2 = mgr.try_reserve(&coin2, 100, "tx-2").unwrap();
        assert_eq!(mgr.reservation_count(), 2);

        mgr.release(&h1);
        mgr.release(&h2);
        assert_eq!(mgr.reservation_count(), 0);
    }

    #[test]
    fn test_expired_reservation() {
        let mgr = CoinReservationManager::new(Duration::from_millis(10));
        let coin = test_coin_id(1);

        // Reserve
        let _h1 = mgr.try_reserve(&coin, 100, "tx-1").unwrap();

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(20));

        // Should succeed because old reservation expired
        assert!(mgr.try_reserve(&coin, 100, "tx-2").is_some());
    }

    #[test]
    fn test_hot_switch_disable() {
        let mgr = CoinReservationManager::default();
        let coin = test_coin_id(1);

        // Reserve
        let h1 = mgr.try_reserve(&coin, 100, "tx-1").unwrap();
        assert_eq!(mgr.reservation_count(), 1);

        // Disable
        mgr.set_enabled(false);
        assert!(!mgr.is_enabled());
        assert_eq!(mgr.reservation_count(), 0); // Cleared on disable

        // Should succeed even for same coin (disabled = pass-through)
        let h2 = mgr.try_reserve(&coin, 100, "tx-2").unwrap();
        assert!(h2.reservation_id.is_nil()); // Nil UUID indicates bypass

        // Release is no-op when disabled
        mgr.release(&h1);
        mgr.release(&h2);
    }

    #[test]
    fn test_cleanup_expired() {
        let mgr = CoinReservationManager::new(Duration::from_millis(10));

        // Create multiple reservations
        for i in 0..5 {
            mgr.try_reserve(&test_coin_id(i), 100, &format!("tx-{}", i));
        }
        assert_eq!(mgr.reservation_count(), 5);

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(20));

        // Cleanup
        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 5);
        assert_eq!(mgr.reservation_count(), 0);
    }

    #[test]
    fn test_wrong_handle_release() {
        let mgr = CoinReservationManager::default();
        let coin = test_coin_id(1);

        // Reserve with tx-1
        let _h1 = mgr.try_reserve(&coin, 100, "tx-1").unwrap();

        // Try to release with a fake handle (different UUID)
        let fake_handle = ReservationHandle {
            reservation_id: Uuid::new_v4(),
            coin_id: coin.clone(),
        };
        mgr.release(&fake_handle);

        // Original reservation should still exist
        assert_eq!(mgr.reservation_count(), 1);
    }

    // ── try_reserve_batch tests ──

    #[test]
    fn test_batch_reserve_success() {
        let mgr = CoinReservationManager::default();
        let coins: Vec<(ObjectId, u64)> = (0..3)
            .map(|i| (test_coin_id(i), 100))
            .collect();

        let refs: Vec<(&ObjectId, u64)> = coins.iter().map(|(id, a)| (id, *a)).collect();
        let handles = mgr.try_reserve_batch(&refs, "tx-batch-1").unwrap();

        assert_eq!(handles.len(), 3);
        assert_eq!(mgr.reservation_count(), 3);

        mgr.release_batch(&handles);
        assert_eq!(mgr.reservation_count(), 0);
    }

    #[test]
    fn test_batch_reserve_rollback_on_conflict() {
        let mgr = CoinReservationManager::default();
        let coin_a = test_coin_id(10);
        let coin_b = test_coin_id(11);
        let coin_c = test_coin_id(12);

        // Pre-reserve coin_b by another transfer
        let _h_other = mgr.try_reserve(&coin_b, 50, "tx-other").unwrap();
        assert_eq!(mgr.reservation_count(), 1);

        // Batch tries to reserve [a, b, c] — should fail on b → rollback a
        let batch: Vec<(&ObjectId, u64)> = vec![(&coin_a, 100), (&coin_b, 100), (&coin_c, 100)];
        let result = mgr.try_reserve_batch(&batch, "tx-batch-2");
        assert!(result.is_none(), "batch should fail because coin_b is taken");

        // Only the pre-existing reservation for coin_b should remain
        assert_eq!(mgr.reservation_count(), 1);

        // coin_a should be free again (rolled back)
        assert!(mgr.try_reserve(&coin_a, 100, "tx-after").is_some());
    }

    #[test]
    fn test_batch_reserve_empty() {
        let mgr = CoinReservationManager::default();
        let handles = mgr.try_reserve_batch(&[], "tx-empty").unwrap();
        assert!(handles.is_empty());
        assert_eq!(mgr.reservation_count(), 0);
    }
}
