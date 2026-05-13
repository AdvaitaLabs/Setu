// ===== examples/move/counter/sources/counter.move =====
// A simple counter — the "Hello World" of Setu Move contracts.
//
// Demonstrates:
//   - Creating owned objects (Counter)
//   - Mutating objects via entry functions
//   - Reading object state
//   - Transferring ownership
//
// Deploy:
//   POST /api/v1/move/publish { "sender": "alice", "modules": ["<hex bytecode>"] }
//
// Usage:
//   # Create a counter (needs TxContext for UID generation)
//   POST /api/v1/move/call {
//     "sender": "alice", "package": "0xcafe", "module": "counter",
//     "function": "create", "needs_tx_context": true
//   }
//
//   # Increment (pass counter as &mut, no TxContext needed)
//   POST /api/v1/move/call {
//     "sender": "alice", "package": "0xcafe", "module": "counter",
//     "function": "increment",
//     "input_object_ids": ["<counter_id>"], "mutable_indices": [0],
//     "needs_tx_context": false
//   }
//
//   # Increment by amount (pure arg = BCS little-endian u64)
//   POST /api/v1/move/call {
//     "sender": "alice", "package": "0xcafe", "module": "counter",
//     "function": "increment_by",
//     "input_object_ids": ["<counter_id>"], "mutable_indices": [0],
//     "args": ["0a00000000000000"],
//     "needs_tx_context": false
//   }
module examples::counter {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// A counter object owned by an address
    struct Counter has key, store {
        id: UID,
        value: u64,
        owner: address,
    }

    /// Create a new counter with value 0, transferred to the sender
    public entry fun create(ctx: &mut TxContext) {
        let counter = Counter {
            id: object::new(ctx),
            value: 0,
            owner: tx_context::sender(ctx),
        };
        transfer::transfer(counter, tx_context::sender(ctx));
    }

    /// Increment the counter by 1
    public entry fun increment(counter: &mut Counter) {
        counter.value = counter.value + 1;
    }

    /// Increment the counter by a specific amount
    public entry fun increment_by(counter: &mut Counter, amount: u64) {
        counter.value = counter.value + amount;
    }

    /// Reset the counter to 0
    public entry fun reset(counter: &mut Counter) {
        counter.value = 0;
    }

    /// Get current value (read-only)
    public fun value(counter: &Counter): u64 {
        counter.value
    }

    /// Transfer the counter to a new owner
    public entry fun transfer_to(counter: Counter, recipient: address) {
        transfer::transfer(counter, recipient);
    }
}
