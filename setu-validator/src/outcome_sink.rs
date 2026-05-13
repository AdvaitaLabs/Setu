// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! R5 · DashMap-backed implementation of [`setu_consensus::OutcomeSink`].
//!
//! The validator constructs one `DashMapOutcomeSink`, wires it into the
//! consensus layer via `ConsensusEngine::set_outcomes_sink`, and shares the
//! inner map with `ValidatorNetworkService` so HTTP handlers can serve
//! `GET /api/v1/event/:id`.
//!
//! No persistence (first cut). See `docs/feat/pwoo-r5/design.md` §7 R-1
//! for the follow-up plan to cap memory growth.

use dashmap::DashMap;
use consensus::OutcomeSink;
use setu_types::ExecutionOutcome;
use std::sync::Arc;

/// Lock-free concurrent map from `event_id` to its final on-chain outcome.
///
/// The inner `Arc<DashMap<..>>` is shared with [`ValidatorNetworkService`] so
/// consensus writes and RPC reads observe the same map without additional
/// synchronization.
#[derive(Debug, Default)]
pub struct DashMapOutcomeSink {
    map: Arc<DashMap<String, ExecutionOutcome>>,
}

impl DashMapOutcomeSink {
    /// Build a new empty sink.
    pub fn new() -> Self {
        Self {
            map: Arc::new(DashMap::new()),
        }
    }

    /// Clone the inner shared map so RPC handlers can read outcomes.
    pub fn map(&self) -> Arc<DashMap<String, ExecutionOutcome>> {
        self.map.clone()
    }
}

impl OutcomeSink for DashMapOutcomeSink {
    fn record(&self, event_id: String, outcome: ExecutionOutcome) {
        // Idempotent: later record overrides earlier (e.g. Genesis re-apply).
        self.map.insert(event_id, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_read_same_arc() {
        let sink = DashMapOutcomeSink::new();
        let map_a = sink.map();
        sink.record(
            "ev-1".into(),
            ExecutionOutcome::Applied { cf_id: "cf-1".into() },
        );
        let map_b = sink.map();

        // Both clones observe the same underlying DashMap.
        assert_eq!(map_a.len(), 1);
        assert_eq!(map_b.len(), 1);
        let entry = map_b.get("ev-1").expect("outcome recorded");
        assert_eq!(entry.kind(), "applied");
    }

    #[test]
    fn record_is_idempotent_on_overwrite() {
        let sink = DashMapOutcomeSink::new();
        sink.record(
            "ev-1".into(),
            ExecutionOutcome::Applied { cf_id: "cf-1".into() },
        );
        sink.record(
            "ev-1".into(),
            ExecutionOutcome::StaleRead {
                cf_id: "cf-2".into(),
                conflicting_object: "oid:abcd".into(),
                retry_hint: "retry".into(),
            },
        );
        let map = sink.map();
        let entry = map.get("ev-1").unwrap();
        assert_eq!(entry.kind(), "stale_read");
    }
}
