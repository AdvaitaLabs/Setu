// ===== setu-framework/sources/dynamic_field.move =====
// Per-object key/value fields that live in their own SMT slots.
// See docs/feat/dynamic-fields/design.md (Setu DF FDP v1.5).
//
// Each DF entry is stored as an independent ObjectEnvelope whose
// `ownership = ObjectOwner(parent_oid)`; the entry's ObjectId is
// derived deterministically from `(parent, type_tag(K), bcs(name))`.
//
// Ability constraints:
//   K: copy + drop + store  — key must be BCS-serializable for OID derivation
//                             and must not hold resources.
//   V: store                — value follows Move's store discipline.
//                             NOTE: Move signatures cannot express "V: store
//                             and NOT key"; the native entry in
//                             setu-move-vm/natives.rs checks at runtime and
//                             aborts with E_DF_VALUE_HAS_KEY if V implements
//                             `key` (i.e. is a first-class object). This
//                             Phase deliberately excludes object-typed DFs.
//
// M1 delivers the Move module + bytecode only; the five native functions
// are wired in M2. Calling any DF function before M2 aborts on the native.
module setu::dynamic_field {
    use setu::object::UID;

    // ── Error codes (stable wire contract; see §3.1 of design) ──

    /// Attempted to `add` a name that already exists under `parent`.
    const E_FIELD_ALREADY_EXISTS: u64 = 0;
    /// Attempted to `borrow` / `borrow_mut` / `remove` a name that does not exist.
    const E_FIELD_DOES_NOT_EXIST: u64 = 1;
    /// Persisted value's declared type tag does not match `V`.
    const E_FIELD_TYPE_MISMATCH: u64 = 2;
    /// `V` implements `key` — object-typed dynamic fields are not supported in this Phase.
    const E_DF_VALUE_HAS_KEY: u64 = 3;

    // Defensive runtime guards that pair with TaskPreparer pre-checks.
    // TaskPreparer should reject these before execution; aborts here mean a
    // bug in the validator pipeline or a hand-crafted task.
    /// df_oid is not present in the preloaded DF cache (client did not declare this access).
    const E_DF_NOT_PRELOADED: u64 = 100;
    /// df_oid collides with an already-declared input object ID.
    const E_DF_DUPLICATE_DECLARATION: u64 = 101;

    // ── Public surface ──

    /// Attach `value` to `parent` under `name`.
    /// Aborts `E_FIELD_ALREADY_EXISTS` if `name` is already used.
    public fun add<K: copy + drop + store, V: store>(
        parent: &mut UID,
        name: K,
        value: V,
    ) {
        add_internal<K, V>(parent, name, value);
    }

    /// Remove and return the value stored under `name`.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if not present.
    public fun remove<K: copy + drop + store, V: store>(
        parent: &mut UID,
        name: K,
    ): V {
        remove_internal<K, V>(parent, name)
    }

    /// Immutable borrow of the value stored under `name`.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if not present.
    public fun borrow<K: copy + drop + store, V: store>(
        parent: &UID,
        name: K,
    ): &V {
        borrow_internal<K, V>(parent, name)
    }

    /// Mutable borrow of the value stored under `name`.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if not present.
    public fun borrow_mut<K: copy + drop + store, V: store>(
        parent: &mut UID,
        name: K,
    ): &mut V {
        borrow_mut_internal<K, V>(parent, name)
    }

    /// Returns true when a dynamic field named `name` is attached to `parent`.
    public fun exists_<K: copy + drop + store>(
        parent: &UID,
        name: K,
    ): bool {
        exists_internal<K>(parent, name)
    }

    // ── Native functions (implemented in setu-move-vm/src/natives.rs, M2) ──
    native fun add_internal<K: copy + drop + store, V: store>(parent: &mut UID, name: K, value: V);
    native fun remove_internal<K: copy + drop + store, V: store>(parent: &mut UID, name: K): V;
    native fun borrow_internal<K: copy + drop + store, V: store>(parent: &UID, name: K): &V;
    native fun borrow_mut_internal<K: copy + drop + store, V: store>(parent: &mut UID, name: K): &mut V;
    native fun exists_internal<K: copy + drop + store>(parent: &UID, name: K): bool;
}
