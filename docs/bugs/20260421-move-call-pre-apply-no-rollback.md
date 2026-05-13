# MoveCall pre-apply has no rollback → multi-validator state-root divergence

**Status**: OPEN
**Impact**: major (blocks safe multi-validator MoveCall deployment)
**Discovered during**: DF FDP M5 root-cause review (session 2026-04-21), while
analysing the idempotency escape added to `storage/src/state/manager.rs` for
OBS-020.

## Symptom

In a multi-validator deployment, two concurrent MoveCall events `E1` and `E2`
that touch the same key, routed to different validators `V_R1` and `V_R2`
respectively, can cause the per-subnet SMT to diverge across nodes after CF
finalization. Consequently the CF `state_root` disagrees, breaking BFT safety.

The divergence exists independently of the OBS-020 idempotency escape — it is
a pre-existing property of the MoveCall hot path. The escape makes the
outcome (`applied` / `stale_read`) inconsistency louder but does not change
the underlying state divergence.

## Suspected location

`setu-validator/src/network/move_handler.rs` around L183-L192:

```rust
// Apply state changes to validator state (so created objects are
// available for subsequent MoveCall references)
if success {
    if let Some(r) = result_event.execution_result.as_ref() {
        let shared = state_provider.shared_state_manager();
        let mut manager = shared.lock_write();
        for sc in &r.state_changes {
            manager.apply_state_change(SubnetId::ROOT, sc);
        }
        shared.publish_snapshot(&manager);
    }
}
```

This writes the TEE output to the local SMT immediately after TEE returns,
*before* consensus has ordered the event into a CF. There is no corresponding
rollback path — `grep -n "rollback" setu-validator` confirms the only
rollback machinery lives in `consensus/` (CF-level) and
`task_preparer/single.rs` (reservation rollback), not in move_handler.

## Suspected root cause

MoveCall's "read-your-writes" contract requires that a follow-up HTTP call
from the same client can see the objects created/mutated by the previous
call. The current implementation buys that visibility by mutating the local
SMT synchronously. Because only the *receiving* validator pre-applies, other
validators see the pre-tx bytes until CF apply runs, producing a window in
which the SMT physically differs across nodes.

If in that window a competing `E2` hits a different validator touching the
same key:

| step                  | V_R1 SMT[oid] | V_R2 SMT[oid] | V_N SMT[oid] |
|-----------------------|---------------|---------------|--------------|
| pre-apply E1 @ V_R1   | `E1.new`      | `pre-tx`      | `pre-tx`     |
| pre-apply E2 @ V_R2   | `E1.new`      | `E2.new`      | `pre-tx`     |
| CF apply E1           | escape→accept | stale_read    | accept       |
| CF apply E2           | stale_read    | escape→accept | stale_read   |
| **final SMT**         | `E1.new`      | `E2.new`      | `E1.new`     |

`V_R2` diverges from the 2/3 majority → CF `state_root` disagreement.

Before the OBS-020 escape the same divergence existed with a different
symptom: V_R1 and V_R2 both rejected the CF's event as stale_read (because
pre-apply had overwritten `expected_old`), but the SMT bytes on each side
were still the pre-applied values (no rollback) — identical state-root
divergence, different outcome log.

## Reproduction (proposed)

No existing shell test exercises this because `tests/dynamic_fields/` and
`tests/pwoo/` both run a single validator. A proper repro needs:

1. `deploy/dev-mult/start.sh all` to bring up 3 validators.
2. Two clients submitting same-key MoveCall to different validators
   concurrently (`curl` with `&` wait, or k6 with per-target URLs).
3. Poll each node's `GET /api/v1/state_root?subnet=ROOT&cf_id=...` after
   finalization. Expect different roots on the `V_R2` side.

Alternative: a cross-validator unit test that instantiates two
`GlobalStateManager` instances and replays the same CF events after
simulating pre-apply on different "hosts".

## Impact

- **Single-validator deployments** (current dev/CI): unaffected. All DF
  tests, PWOO tests, and the M5 milestone are safe.
- **Multi-validator deployments** (`deploy/dev-mult/`, any production path):
  any MoveCall hot key routed to ≥ 2 distinct validators in the same CF
  window triggers state-root divergence → BFT safety gap.
- The OBS-020 idempotency escape does NOT regress this (divergence existed
  pre-escape) but it makes `on_chain.outcome` inconsistent across nodes too,
  which may complicate client-side retry logic that relies on outcome
  agreement.

## Proposed fix directions

1. **Speculative overlay** (preferred, architectural): move pre-apply into
   a per-tx overlay (`HashMap<(subnet, object_id), Vec<u8>>`) that read
   paths merge on top of SMT. On CF finalize, either promote the overlay
   into the SMT (if applied) or discard it (if stale_read). This gives
   read-your-writes without touching the canonical SMT before consensus.
2. **Pre-apply with journaled undo**: keep current behaviour but record
   `(key, prev_bytes)` per pre-applied event; on CF finalize, if the event
   landed as stale_read or got dropped, restore from the journal before
   CF apply runs. Simpler than (1) but fights against the idempotency
   escape — the escape becomes unnecessary if undo is in place.
3. **Remove pre-apply, hide latency**: drop the write and return the
   TEE-produced bytes directly to the HTTP response (client sees what they
   need immediately); any subsequent MoveCall from the same client rides
   on DAG-ordered visibility. Breaks existing "created_objects are queryable
   via `GET /object/:id` right after MoveCall" expectations.

Direction (1) is the cleanest and removes the need for the current OBS-020
idempotency escape as a side effect.

## Related

- `docs/feat/dynamic-fields/design.md` §5.3 — bincode 1.3 non-optional fields
  (separate multi-node compatibility concern, orthogonal to this bug).
- `ai/LESSONS_LEARNED.md` OBS-020 — the DF-02 "both stale_read" symptom
  whose surface fix (idempotency escape) surfaced this deeper issue.
- `storage/src/state/manager.rs::apply_committed_events` — the CF-apply
  conflict detector that sees the divergence from the other side.

## Priority / next steps

- **Do NOT** block M5 commit on this; single-validator tests pass.
- **Do** gate `deploy/dev-mult/` MoveCall smoke tests on a fix before the
  next multi-node milestone.
- Owner: TBD. Est: 2-3 days for direction (1) + integration tests.
