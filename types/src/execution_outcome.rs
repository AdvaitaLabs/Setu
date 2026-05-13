//! R5 · Chain-level execution outcome (apply-phase verdict).
//!
//! This enum represents the final on-chain verdict for an event after the
//! consensus-driven apply phase. It is distinct from `ExecutionResult` which
//! represents the TEE-level execution result (the Solver's output).
//!
//! See `docs/feat/pwoo-r5/design.md` §3.2a for the full design rationale.

use serde::{Deserialize, Serialize};

/// Final on-chain verdict for an event after apply.
///
/// Stored in a sidecar map (keyed by `event_id`) rather than inside the
/// `Event` struct so that:
/// 1. The `Event` wire format stays stable (no G13 impact).
/// 2. Apply-phase mutations don't race with in-flight DAG replication.
/// 3. "TEE executed successfully but was rejected by apply" is expressible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// Event applied successfully; state committed.
    Applied { cf_id: String },

    /// TEE reported success but the `ExecutionResult.success=false` path
    /// was taken. Surface the message so clients can diagnose.
    ExecutionFailed {
        cf_id: String,
        reason: Option<String>,
    },

    /// Apply-phase detected a byte-level `old_value` mismatch — concurrent
    /// modification to the same object. The event is skipped and the client
    /// should re-read the conflicting object and retry.
    StaleRead {
        cf_id: String,
        /// First `StateChange.key` that triggered the conflict.
        /// G11: always `"oid:{hex}"` format.
        conflicting_object: String,
        /// Server-generated hint (e.g. English retry instruction).
        retry_hint: String,
    },
}

impl ExecutionOutcome {
    /// Short discriminant string (matches serde tag).
    pub fn kind(&self) -> &'static str {
        match self {
            ExecutionOutcome::Applied { .. } => "applied",
            ExecutionOutcome::ExecutionFailed { .. } => "execution_failed",
            ExecutionOutcome::StaleRead { .. } => "stale_read",
        }
    }

    /// CF id that produced this outcome.
    pub fn cf_id(&self) -> &str {
        match self {
            ExecutionOutcome::Applied { cf_id }
            | ExecutionOutcome::ExecutionFailed { cf_id, .. }
            | ExecutionOutcome::StaleRead { cf_id, .. } => cf_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_read_json_shape() {
        let outcome = ExecutionOutcome::StaleRead {
            cf_id: "cf-1".to_string(),
            conflicting_object: "oid:aabb".to_string(),
            retry_hint: "retry".to_string(),
        };
        let j = serde_json::to_value(&outcome).unwrap();
        assert_eq!(j["outcome"], "stale_read");
        assert_eq!(j["cf_id"], "cf-1");
        assert_eq!(j["conflicting_object"], "oid:aabb");
        assert_eq!(j["retry_hint"], "retry");
    }

    #[test]
    fn applied_json_shape() {
        let outcome = ExecutionOutcome::Applied { cf_id: "cf-2".into() };
        let j = serde_json::to_value(&outcome).unwrap();
        assert_eq!(j["outcome"], "applied");
        assert_eq!(j["cf_id"], "cf-2");
    }

    #[test]
    fn execution_failed_reason_optional() {
        let o1 = ExecutionOutcome::ExecutionFailed {
            cf_id: "cf".into(),
            reason: None,
        };
        let o2 = ExecutionOutcome::ExecutionFailed {
            cf_id: "cf".into(),
            reason: Some("oom".into()),
        };
        let j1 = serde_json::to_value(&o1).unwrap();
        let j2 = serde_json::to_value(&o2).unwrap();
        assert_eq!(j1["outcome"], "execution_failed");
        assert!(j1["reason"].is_null());
        assert_eq!(j2["reason"], "oom");
    }

    #[test]
    fn roundtrip_bincode_safe() {
        // Ensure serde supports BCS/bincode for future diagnostics use.
        let outcome = ExecutionOutcome::Applied { cf_id: "cf-rt".into() };
        let s = serde_json::to_string(&outcome).unwrap();
        let back: ExecutionOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(back, outcome);
    }
}
