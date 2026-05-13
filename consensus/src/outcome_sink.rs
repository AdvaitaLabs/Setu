// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! R5 · `OutcomeSink` trait — event-level apply outcome hook.
//!
//! The consensus layer calls `OutcomeSink::record` after every successful
//! state apply to let downstream subscribers observe per-event outcomes
//! (Applied / ExecutionFailed / StaleRead).
//!
//! ## Design
//!
//! The trait's abstraction level is deliberately aligned with
//! [`crate::broadcaster::ConsensusBroadcaster`]: both expose a single
//! generic hook to any consensus implementation without leaking
//! consensus-algorithm details. A future consensus upgrade (e.g. Mysticeti)
//! does not need to change this trait — downstream observers keep working
//! as long as "events get applied and produce outcomes" stays conceptually
//! valid.
//!
//! No business mapping lives here. The `Applied/StaleRead/retry_hint`
//! logic is in the caller (`AnchorBuilder::ingest_outcomes`), and the
//! DashMap-backed implementation lives in `setu-validator` so that
//! `consensus/` needs no `dashmap` dependency.
//!
//! See `docs/feat/pwoo-r5/design.md` §3.2b.

use setu_types::ExecutionOutcome;

/// Hook invoked by the consensus layer after each event's apply verdict
/// is determined.
///
/// Implementations must be `Send + Sync` because the sink is shared across
/// the consensus tokio runtime and stored behind `Arc<dyn OutcomeSink>`.
pub trait OutcomeSink: Send + Sync {
    /// Record the final verdict for `event_id`.
    ///
    /// Called **once** per applied event per CF finalization. Re-apply of
    /// the same `event_id` (e.g. Genesis rebuild) overwrites prior records
    /// — implementations should make `record` idempotent on repeated keys.
    fn record(&self, event_id: String, outcome: ExecutionOutcome);
}
