// ===== examples/move/freeze_then_mutate/sources/freeze_then_mutate.move =====
// Test fixture for `docs/feat/fix-immutable-mutable-ref-not-blocked`.
//
// Exposes:
//   - `mint_then_freeze`: create a `Frozen` object and freeze it (sender is
//     the original creator; once frozen, no one can mutate or consume it).
//   - `try_mutate(&mut Frozen)`: should be REJECTED at TaskPreparer when the
//     caller marks the input as `mutable_indices=[0]`.
//   - `try_consume(Frozen)`: should be REJECTED at TaskPreparer when the
//     caller marks the input as `consumed_indices=[0]`.
//   - `try_read(&Frozen)`: should SUCCEED — read-only access to a frozen
//     object is the long-standing happy path.
module examples::freeze_then_mutate {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::TxContext;

    struct Frozen has key, store {
        id: UID,
        value: u64,
    }

    /// Create a `Frozen` and freeze it. The resulting object has
    /// `Ownership::Immutable` and the API returns its id so the test
    /// harness can probe it post-freeze.
    public entry fun mint_then_freeze(initial: u64, ctx: &mut TxContext) {
        let f = Frozen {
            id: object::new(ctx),
            value: initial,
        };
        transfer::freeze_object(f);
    }

    /// MUST FAIL when called with mutable_indices=[0] on a frozen Frozen.
    public entry fun try_mutate(f: &mut Frozen, delta: u64) {
        f.value = f.value + delta;
    }

    /// MUST FAIL when called with consumed_indices=[0] on a frozen Frozen.
    public entry fun try_consume(f: Frozen) {
        let Frozen { id, value: _ } = f;
        object::delete(id);
    }
}
