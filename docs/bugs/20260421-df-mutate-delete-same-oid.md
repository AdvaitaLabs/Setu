# DF remove → add → remove same-oid produces ambiguous StateChanges

**Status**: OPEN
**Impact**: minor (unreachable from current Move front-end, but VM invariant gap)
**Discovered during**: post-M5 review of the dead `replaced_df_oids` branch in
`crates/setu-move-vm/src/engine.rs` (session 2026-04-21).

## Symptom

For a single MoveCall transaction that performs

```move
df::remove<K, V>(&mut parent.id, k);   // ①
df::add<K, V>(&mut parent.id, k, v);   // ② mutate_replace path
df::remove<K, V>(&mut parent.id, k);   // ③
```

the `SetuObjectRuntime` effect maps end up with

- `df_mutated[oid] = DfMutateEffect { new = v, old = pre-tx }`  (from ②)
- `df_deleted[oid] = DfDeleteEffect { old = v }`  (from ③)

`convert_results_to_state_changes` then emits **two** `MoveStateChange` for
the same `oid`: an `Update` followed by a `Delete`. Downstream
`apply_committed_events` stores these in `pending_writes:
HashMap<(subnet, oid), ..>`, so the second entry silently overwrites the
first at the pending-writes layer, but the per-event conflict detection
runs against the *original SMT slot* — both events compare their
`old_value` against the same pre-tx bytes, so the inter-event ordering
(Update must happen before Delete) is only enforced by the iteration order
inside `convert_results_to_state_changes`, not by any check.

## Suspected location

- `crates/setu-move-vm/src/natives.rs` around L595-L615
  (`native_df_add_internal` mutate-replace branch that calls
  `take_df_delete` and writes to `df_mutated`).
- `crates/setu-move-vm/src/natives.rs` around L685-L700
  (`native_df_remove_internal` writing to `df_deleted`).
- `crates/setu-move-vm/src/engine.rs::convert_results_to_state_changes`
  L620-L720 (emits Update and Delete independently).

## Suspected root cause

The `df_cache` key is `(parent, key_tag, key_bcs)` — no `mode` component,
no per-access counter. Combined with `DfAccessMode` being a single value per
preloaded entry (not a sequence), the natives lack a way to distinguish
"first remove" from "second remove after intervening add". The mutate_replace
fold in add_internal handles `remove → add` correctly by consuming the
`df_deleted` entry via `take_df_delete`; but after the add completes, the
cache slot is `value_bytes = Some(v)` and `mode = Mutate` again, which
satisfies `native_df_remove_internal`'s guard `matches!(entry.mode, Delete |
Mutate)` → a third call successfully re-inserts into `df_deleted` without
clearing `df_mutated`.

## Reproduction

No existing test triggers this. A unit test in
`crates/setu-move-vm/src/engine.rs` can manually populate
`results.df_mutated` and `results.df_deleted` with the same oid and assert
that `convert_results_to_state_changes` either:

1. bails with `RuntimeError::InvalidTransaction`, or
2. collapses the pair into a single `Delete` using the original `old_value`.

Currently it does neither.

## Impact

- **Move front-end reachability**: the only way to trigger this is a contract
  that calls `df::remove`, `df::add`, `df::remove` on the same key within
  one entry function. Not exercised by any current `examples/move/*`
  contract or shell test.
- **Silent state corruption potential**: the two-StateChange emission
  appears consistent at the envelope layer (Update→Delete reduces to
  Delete) but both events carry the *same* `expected_old = pre-tx bytes`.
  In a multi-validator setting with the existing
  `move_handler.rs` pre-apply hot path (see
  `docs/bugs/20260421-move-call-pre-apply-no-rollback.md`), the combination
  of pre-apply + double StateChange could produce hard-to-debug ordering
  artefacts.
- No known production or test regression today.

## Proposed fix

Add an explicit invariant check in
`SetuObjectRuntime::record_df_delete`:

```rust
pub(crate) fn record_df_delete(&mut self, df_oid: ObjectId, effect: DfDeleteEffect) {
    if self.df_mutated.swap_remove(&df_oid).is_some() {
        // Mutate+Delete pair in one tx collapses to Delete with the
        // ORIGINAL pre-tx old_value (the mutate never persisted).
        // Keep `effect` as-is — its old_value_bcs already holds the
        // last observed bytes, which equal the post-mutate value.
        // Overwrite old_value with the pre-mutate bytes carried in
        // the consumed DfMutateEffect.
        //   (implementation detail: DfMutateEffect stores
        //    both old_value_bcs and on_disk_envelope; reuse those
        //    when emitting the final Delete.)
    }
    self.df_deleted.insert(df_oid, effect);
}
```

Plus a `debug_assert!` in `convert_results_to_state_changes`:

```rust
debug_assert!(
    results.df_created.keys().all(|k| !results.df_deleted.contains_key(k)),
    "df_created ∩ df_deleted must be empty (native mode-check invariant)"
);
debug_assert!(
    results.df_mutated.keys().all(|k| !results.df_deleted.contains_key(k)),
    "df_mutated ∩ df_deleted must be empty after record_df_delete fold"
);
```

Alternative (simpler, slightly weaker): make `native_df_remove_internal`
bail with a new abort code `E_DF_REMOVE_AFTER_REPLACE` when the cache slot
was produced by a prior `mutate_replace` in the same tx. Needs a
per-entry "touched by add_internal in this tx" flag.

## Priority

Low — not reachable from any shipped contract. Should be fixed before
DEX-like hot-path contracts that legitimately call `remove + add` in loops
(e.g. pool rebalance). Bundle with the `replaced_df_oids` dead-code
cleanup commit since both reshape the VM→Engine DF effect contract.

## Related

- `docs/bugs/20260421-move-call-pre-apply-no-rollback.md` — amplifier.
- `docs/feat/dynamic-fields/design.md` §3.5 + §3.7 — effect-to-StateChange
  mapping contract.
- Session notes (2026-04-21): "replaced_df_oids 死代码复核" — established
  that `df_created ∩ df_deleted ≡ ∅` but `df_mutated ∩ df_deleted` is a
  real gap.
