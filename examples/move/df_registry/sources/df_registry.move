// Dynamic Fields — M5 shell-test contract.
//
// Exercises the five `setu::dynamic_field` natives end-to-end through the
// HTTP `/api/v1/move/call` surface. Paired with `tests/dynamic_fields/*.sh`.
//
// Design:  docs/feat/dynamic-fields/design.md §5.1 (M5)
// Sibling: examples/move/pwoo_counter (same pattern, different feature).
//
// Why entry funs use `assert!` instead of returning values:
//   Move entry functions cannot return values to the caller. To verify a
//   read (borrow / exists_) result from shell, we assert inside Move and
//   let the TX succeed-or-abort drive the test assertion.
module examples::df_registry {
    use setu::object::{Self, UID};
    use setu::dynamic_field as df;
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    // ── Assertion codes (shell-test visible via abort code) ──
    const E_VALUE_MISMATCH: u64 = 1000;
    const E_EXISTS_MISMATCH: u64 = 1001;

    /// The parent object under which dynamic fields are attached.
    /// `has key, store` so it can be AddressOwner / Shared / Immutable.
    struct Registry has key, store {
        id: UID,
        owner: address,
    }

    // ── Constructors ─────────────────────────────────────────────────────

    /// Create a Registry and immediately share it (Shared ownership).
    public entry fun create_shared(ctx: &mut TxContext) {
        let r = Registry { id: object::new(ctx), owner: tx_context::sender(ctx) };
        transfer::share_object(r);
    }

    /// Create a Registry owned by the sender (AddressOwner ownership).
    public entry fun create_owned(ctx: &mut TxContext) {
        let r = Registry { id: object::new(ctx), owner: tx_context::sender(ctx) };
        transfer::transfer(r, tx_context::sender(ctx));
    }

    /// Freeze a previously-owned Registry into Immutable.
    /// Used by shell tests to exercise the Immutable-parent rejection path.
    public entry fun freeze_reg(r: Registry) {
        transfer::freeze_object(r);
    }

    // ── DF mutators ──────────────────────────────────────────────────────

    /// `df::add<u64, u64>`. Aborts `E_FIELD_ALREADY_EXISTS` if key exists.
    public entry fun put_u64(r: &mut Registry, key: u64, value: u64) {
        df::add<u64, u64>(&mut r.id, key, value);
    }

    /// Replace an existing value under `key`.
    /// Aborts `E_FIELD_DOES_NOT_EXIST` if key absent.
    public entry fun set_u64(r: &mut Registry, key: u64, value: u64) {
        // M5 test helper: express "mutate" as remove+add in one tx.
        // This keeps parent bytes stable while updating the DF slot.
        // NOTE: true in-place `borrow_mut` writeback is tracked separately
        // in VM/runtime milestones.
        let _old = df::remove<u64, u64>(&mut r.id, key);
        df::add<u64, u64>(&mut r.id, key, value);
    }

    /// Borrow-mut path helper used by concurrency tests.
    ///
    /// This exercises `df::borrow_mut` call flow without issuing a DF create,
    /// so PWOO conflict tests can focus on slot-level contention behavior.
    public entry fun touch_u64(r: &mut Registry, key: u64, value: u64) {
        let slot = df::borrow_mut<u64, u64>(&mut r.id, key);
        *slot = value;
    }

    /// `df::remove<u64, u64>` + drop. Aborts `E_FIELD_DOES_NOT_EXIST`
    /// if key absent. Value type `u64` has `copy + drop` so it can be
    /// silently dropped here.
    public entry fun take_u64(r: &mut Registry, key: u64) {
        let _v = df::remove<u64, u64>(&mut r.id, key);
    }

    // ── DF assertions (drive shell "read" tests) ─────────────────────────

    /// `df::borrow<u64, u64>` and assert the loaded value equals `expected`.
    /// Aborts `E_VALUE_MISMATCH` otherwise — shell sees `success=false`.
    public entry fun assert_u64(r: &Registry, key: u64, expected: u64) {
        let v = df::borrow<u64, u64>(&r.id, key);
        assert!(*v == expected, E_VALUE_MISMATCH);
    }

    /// `df::exists_<u64>` and assert the result equals `expected`.
    /// Used to cover both true and false branches of `exists_internal`.
    public entry fun assert_exists(r: &Registry, key: u64, expected: bool) {
        let found = df::exists_<u64>(&r.id, key);
        assert!(found == expected, E_EXISTS_MISMATCH);
    }
}
