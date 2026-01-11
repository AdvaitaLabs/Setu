//! Core entities and traits shared across the Setu stack.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Unique identifier for a transfer.
pub type TransferId = String;
/// Logical clock key (e.g., node id).
pub type ClockKey = String;
/// Object or resource key used for routing and conflict detection.
pub type ResourceKey = String;
/// Object identifier (e.g., `alice_flux_obj`).
pub type ObjectId = String;

/// Logical type of the underlying object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Flux,
    Power,
    Task,
}

/// High-level transfer kind (semantic label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferType {
    FluxTransfer,
    PowerConsume,
    TaskSubmit,
}

/// Verifiable logical clock (vector clock / DOBC-style).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vlc {
    // BTreeMap for stable ordering and deterministic serialization.
    pub entries: BTreeMap<ClockKey, u64>,
}

impl Vlc {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

/// Minimal transfer representation (aligned with `transfers` table at a high level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub id: TransferId,
    /// Logical sender (owner) of the transfer.
    pub from: String,
    /// Logical recipient; may be empty for some system transfers.
    pub to: String,
    /// Transfer amount in smallest unit (e.g., 1 Flux = 10^18).
    pub amount: i128,
    /// Application-level classification of this transfer.
    pub transfer_type: TransferType,
    pub resources: Vec<ResourceKey>,
    pub vlc: Vlc,
    /// Power/work score used for tie-breaks.
    pub power: u64,
    
    /// Optional: Preferred solver ID for manual routing
    pub preferred_solver: Option<String>,
    /// Optional: Shard ID for shard-based routing
    pub shard_id: Option<String>,
    /// Optional: Subnet ID for subnet-based routing
    pub subnet_id: Option<String>,
    
    /// VLC assigned by Validator when receiving the transfer
    /// Solver should use this VLC when creating Event, NOT generate its own
    pub assigned_vlc: Option<AssignedVlc>,
}

/// VLC assigned by Validator to a Transfer
/// This is passed to Solver and should be used in the resulting Event
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignedVlc {
    /// The logical time assigned by Validator
    pub logical_time: u64,
    /// The physical time when assigned
    pub physical_time: u64,
    /// The Validator ID that assigned this VLC
    pub validator_id: String,
}

/// Snapshot of an object's version, roughly mirroring `object_versions` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectVersion {
    pub object_id: ObjectId,
    pub object_type: ObjectType,
    /// Monotonic version counter.
    pub current_version: i32,
    /// Transfer that produced this version.
    pub produced_by: TransferId,
    /// Owner address / principal.
    pub owner: String,
    /// JSON-like payload; keep generic at core-types level.
    pub data_json: String,
}

/// Compare causal relationships.
pub trait CausalComparable {
    fn happens_before(&self, other: &Self) -> bool;
    fn concurrent_with(&self, other: &Self) -> bool {
        !self.happens_before(other) && !other.happens_before(self)
    }
}

impl CausalComparable for Vlc {
    fn happens_before(&self, other: &Self) -> bool {
        // Standard vector clock partial order.
        let mut le = false;
        for (k, v) in &self.entries {
            let ov = other.entries.get(k).copied().unwrap_or_default();
            if v > &ov {
                return false;
            }
            if v < &ov {
                le = true;
            }
        }
        // If self has keys other lacks, other has implicit 0.
        for (k, v) in &other.entries {
            if !self.entries.contains_key(k) && *v > 0 {
                le = true;
            }
        }
        le
    }
}

