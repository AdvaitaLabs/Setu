// PWOO (Public-Writable Owned Objects) — Phase 7 shell-test contract.
//
// Exercises three PWOO paths end-to-end via HTTP API:
//   1. create_shared: one-shot create + share in same TX
//   2. increment:     &mut on a Shared counter (any sender)
//   3. create/share:  two-step owned → shared (for negative-path tests)
module examples::pwoo_counter {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// Publicly writable counter.
    struct Counter has key, store {
        id: UID,
        value: u64,
        creator: address,
    }

    /// Create a new counter with value=0 and immediately share it.
    /// After this TX lands, the object's Ownership is `Shared { initial_shared_version }`.
    /// Per §4.5 self-reference rule: share in same TX as create is allowed.
    public entry fun create_shared(ctx: &mut TxContext) {
        let counter = Counter {
            id: object::new(ctx),
            value: 0,
            creator: tx_context::sender(ctx),
        };
        transfer::share_object(counter);
    }

    /// Create an owned counter (not yet shared). Two-step flow.
    public entry fun create_owned(ctx: &mut TxContext) {
        let counter = Counter {
            id: object::new(ctx),
            value: 0,
            creator: tx_context::sender(ctx),
        };
        transfer::transfer(counter, tx_context::sender(ctx));
    }

    /// Share an already-owned counter (owner only; ownership enforced by TaskPreparer).
    public entry fun share(counter: Counter) {
        transfer::share_object(counter);
    }

    /// Increment a shared counter. Anyone can call this because the counter's
    /// Ownership::Shared allows non-owner references via `shared_object_ids`.
    public entry fun increment(counter: &mut Counter) {
        counter.value = counter.value + 1;
    }
}
