// Dynamic Fields — S12 negative test contract.
//
// Purpose: trigger the runtime-only `has key` guard on DF values
// (design.md §3.6, implemented in crates/setu-move-vm/src/natives.rs as
// `E_DF_VALUE_HAS_KEY_ABILITY = 4`, raised via BCS 32-byte prefix
// collision against `input_object_exists`).
//
// Why the guard is needed:
//   The Move signature `V: store` cannot exclude the `key` ability. A
//   malicious contract could otherwise "smuggle" a first-class object into
//   a DF slot, breaking the 1:1 object <-> SMT-slot invariant. The native
//   detects this by observing that a `key` object's BCS serialization
//   starts with its own UID, which must also appear in `input_objects`
//   (since the value is moved in by-value from a caller-provided input).
//
// Why it's tested via a dedicated contract:
//   `df_registry.move` only exposes `K=u64, V=u64`, which cannot trigger
//   the check. We need a struct that has both `key` and `store`.
//
// Expected outcome for `try_add_trojan`:
//   Native aborts with code `4` before any state change; shell sees
//   `success: false` on the HTTP MoveCall response.
module examples::bad_df {
    use setu::object::{Self, UID};
    use setu::dynamic_field as df;
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// Parent object. Pre-created via `create_parent` so its OID can be
    /// declared in `input_object_ids`.
    struct Parent has key, store { id: UID }

    /// A value type with `key + store` — the intended target of the
    /// E_DF_VALUE_HAS_KEY_ABILITY guard.
    struct Trojan has key, store { id: UID }

    public entry fun create_parent(ctx: &mut TxContext) {
        transfer::transfer(Parent { id: object::new(ctx) }, tx_context::sender(ctx));
    }

    public entry fun mint_trojan(ctx: &mut TxContext) {
        transfer::transfer(Trojan { id: object::new(ctx) }, tx_context::sender(ctx));
    }

    /// Attempt to attach a `key` object as a DF value.
    /// Native aborts with code 4 (E_DF_VALUE_HAS_KEY_ABILITY).
    public entry fun try_add_trojan(parent: &mut Parent, trojan: Trojan) {
        df::add<u64, Trojan>(&mut parent.id, 1u64, trojan);
    }
}
