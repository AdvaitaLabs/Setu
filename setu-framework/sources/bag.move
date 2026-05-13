// ===== setu-framework/sources/bag.move =====
// `Bag` — a heterogeneous-value key/value map backed by dynamic fields.
//
// Same storage shape as `Table`, but `V` is bound at each call site rather
// than fixed on the struct. Useful when entries store different types under
// different keys (e.g. a feature-flags bag).
//
// CAUTION — DO NOT USE ACROSS PACKAGE UPGRADES UNTIL B5:
//   `Bag::borrow<V>` deserializes stored bytes against the canonical type tag
//   `V`. If the package that defined `V` is upgraded and `V`'s BCS layout
//   changes, the new layout is applied to bytes written by the old layout —
//   this is OWASP A04 (insecure design / out-of-bounds read). Safe usage
//   requires either (1) Setu Package Upgrade (B5) ships layout-version
//   binding for stored DF values, or (2) the dapp guarantees `V` is defined
//   in a non-upgradable package.
//
// Caller contract — preload mandatory: same as `setu::table`. See its top
// doc-comment, including the `E_DF_NOT_PRELOADED = 100` debugging tip about
// preload-mode mismatch.
//
// `borrow_mut` and V-typed existence (`contains_with_type<K, V>`) are
// intentionally NOT exposed for the same reasons documented in
// `setu::table` and docs/feat/move-vm-phase7-table-bag/implementation-log.md.
//
// Design: docs/feat/move-vm-phase7-table-bag/design.md
module setu::bag {
    use setu::object::{Self, UID};
    use setu::tx_context::TxContext;
    use setu::dynamic_field as df;

    // ── Errors ──

    /// Tried to `destroy_empty` a non-empty Bag.
    const E_BAG_NOT_EMPTY: u64 = 0;

    // Other aborts propagate from `dynamic_field` natives.

    // ── Type ──

    /// A heterogeneous-value map. V is bound at each call site, not on the struct.
    struct Bag has key, store {
        id: UID,
        size: u64,
    }

    // ── Constructors / destructors ──

    /// Create a new empty `Bag`.
    public fun new(ctx: &mut TxContext): Bag {
        Bag { id: object::new(ctx), size: 0 }
    }

    /// Destroy an empty Bag. Aborts `E_BAG_NOT_EMPTY` if any entries remain.
    public fun destroy_empty(b: Bag) {
        let Bag { id, size } = b;
        assert!(size == 0, E_BAG_NOT_EMPTY);
        object::delete(id);
    }

    // ── Mutators ──

    /// Insert an entry of value type `V` under key `k`.
    /// Aborts `E_FIELD_ALREADY_EXISTS` if `k` is already used.
    public fun add<K: copy + drop + store, V: store>(
        b: &mut Bag,
        k: K,
        v: V,
    ) {
        df::add<K, V>(&mut b.id, k, v);
        b.size = b.size + 1;
    }

    /// Remove and return the value under `k`. The caller must supply the
    /// correct `V`; mismatch aborts `E_FIELD_TYPE_MISMATCH`.
    public fun remove<K: copy + drop + store, V: store>(
        b: &mut Bag,
        k: K,
    ): V {
        let v = df::remove<K, V>(&mut b.id, k);
        b.size = b.size - 1;
        v
    }

    /// Replace the value under `k` (same V) and return the old one.
    /// Pre-condition: caller declared `Mutate` preload mode for `k`.
    public fun update<K: copy + drop + store, V: store>(
        b: &mut Bag,
        k: K,
        v: V,
    ): V {
        let old = df::remove<K, V>(&mut b.id, k);
        df::add<K, V>(&mut b.id, k, v);
        old
    }

    // ── Read accessors ──

    /// Immutable borrow with explicit V. Aborts on absent / wrong V.
    public fun borrow<K: copy + drop + store, V: store>(
        b: &Bag,
        k: K,
    ): &V {
        df::borrow<K, V>(&b.id, k)
    }

    /// V-erased existence check (does not validate value type).
    public fun contains<K: copy + drop + store>(
        b: &Bag,
        k: K,
    ): bool {
        df::exists_<K>(&b.id, k)
    }

    /// Number of entries.
    public fun size(b: &Bag): u64 {
        b.size
    }

    /// True iff the bag holds no entries.
    public fun is_empty(b: &Bag): bool {
        b.size == 0
    }
}
