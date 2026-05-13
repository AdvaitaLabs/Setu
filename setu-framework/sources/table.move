// ===== setu-framework/sources/table.move =====
// `Table<K, V>` — a typed, size-tracking key/value map backed by dynamic fields.
//
// All entries live in the SMT under derived DF object IDs (parent = Table.id).
// Adding/removing an entry costs the same as a raw `dynamic_field` op plus one
// extra envelope write to bump `size` on the Table parent.
//
// Caller contract — preload mandatory:
//   Every `(table.id, type_tag(K), bcs(k))` touched by a tx MUST appear in the
//   SolverTask `dynamic_fields` array with the appropriate access mode
//   (Create / Mutate / Delete). Otherwise the underlying native aborts with
//   `E_DF_NOT_PRELOADED = 100`.
//
// Debugging tip: abort `100` does NOT distinguish "not preloaded at all" from
// "preloaded with the wrong mode" (e.g. calling `update<K, V>` after a `Create`
// or `Delete` preload instead of `Mutate`). When you hit `100`, first verify
// the preload mode matches the call:
//   - `add`     → `Create`
//   - `remove`  → `Delete` or `Mutate`
//   - `update`  → `Mutate` (only)
//   - `borrow`  → any mode that resulted in a populated cache slot
//
// `borrow_mut` is intentionally NOT exposed: the underlying native does not
// persist mutations through `&mut V` (see crates/setu-move-vm/src/natives.rs
// `native_df_borrow_mut_internal` doc-comment). Use `update<K, V>(t, k, v): V`
// for in-place value replacement; the call requires `Mutate` preload mode.
//
// Design: docs/feat/move-vm-phase7-table-bag/design.md
module setu::table {
    use setu::object::{Self, UID};
    use setu::tx_context::TxContext;
    use setu::dynamic_field as df;

    // ── Errors ──

    /// Tried to `destroy_empty` a non-empty Table.
    const E_TABLE_NOT_EMPTY: u64 = 0;

    // Other aborts (`E_FIELD_ALREADY_EXISTS`, `E_FIELD_DOES_NOT_EXIST`,
    // `E_FIELD_TYPE_MISMATCH`, `E_DF_NOT_PRELOADED`) propagate from the
    // underlying `dynamic_field` natives — see dynamic_field.move for codes.

    // ── Type ──

    /// A typed map from `K` to `V`. `K` and `V` are phantom on the struct:
    /// the struct only carries the parent UID and the size counter; values
    /// are stored as dynamic fields under that UID.
    struct Table<phantom K: copy + drop + store, phantom V: store> has key, store {
        id: UID,
        size: u64,
    }

    // ── Constructors / destructors ──

    /// Create a new empty `Table<K, V>`.
    public fun new<K: copy + drop + store, V: store>(ctx: &mut TxContext): Table<K, V> {
        Table { id: object::new(ctx), size: 0 }
    }

    /// Destroy an empty Table. Aborts `E_TABLE_NOT_EMPTY` if any entries remain.
    public fun destroy_empty<K: copy + drop + store, V: store>(t: Table<K, V>) {
        let Table { id, size } = t;
        assert!(size == 0, E_TABLE_NOT_EMPTY);
        object::delete(id);
    }

    // ── Mutators ──

    /// Insert a new entry. Aborts `E_FIELD_ALREADY_EXISTS` if `k` already maps.
    public fun add<K: copy + drop + store, V: store>(
        t: &mut Table<K, V>,
        k: K,
        v: V,
    ) {
        df::add<K, V>(&mut t.id, k, v);
        t.size = t.size + 1;
    }

    /// Remove and return the value mapped to `k`.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if `k` is absent.
    public fun remove<K: copy + drop + store, V: store>(
        t: &mut Table<K, V>,
        k: K,
    ): V {
        let v = df::remove<K, V>(&mut t.id, k);
        t.size = t.size - 1;
        v
    }

    /// Replace the value under `k` and return the old one.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if `k` is absent.
    /// Pre-condition: caller declared `Mutate` preload mode for `k`
    /// (the underlying native walks the `mutate_replace` branch).
    public fun update<K: copy + drop + store, V: store>(
        t: &mut Table<K, V>,
        k: K,
        v: V,
    ): V {
        // size is unchanged: one removal + one insertion → net 0
        let old = df::remove<K, V>(&mut t.id, k);
        df::add<K, V>(&mut t.id, k, v);
        old
    }

    // ── Read accessors ──

    /// Immutable borrow of the value under `k`. Aborts if absent.
    public fun borrow<K: copy + drop + store, V: store>(
        t: &Table<K, V>,
        k: K,
    ): &V {
        df::borrow<K, V>(&t.id, k)
    }

    /// Returns true iff `k` is mapped (V-erased; uses df::exists_).
    public fun contains<K: copy + drop + store, V: store>(
        t: &Table<K, V>,
        k: K,
    ): bool {
        df::exists_<K>(&t.id, k)
    }

    /// Number of entries.
    public fun size<K: copy + drop + store, V: store>(t: &Table<K, V>): u64 {
        t.size
    }

    /// True iff the table holds no entries.
    public fun is_empty<K: copy + drop + store, V: store>(t: &Table<K, V>): bool {
        t.size == 0
    }
}
