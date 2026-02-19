// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! VLC (Vector Logical Clock) Wrapper Module
//!
//! This module provides a wrapper around `setu_vlc::VLCSnapshot` to provide
//! a node-local clock management interface with convenient methods for
//! the consensus engine.

use setu_vlc::{VLCSnapshot, VectorClock};

/// VLC - Local Vector Logical Clock manager for a node
///
/// This wraps `VLCSnapshot` and provides convenient methods for:
/// - Incrementing the local clock (tick)
/// - Merging with received clocks
/// - Taking snapshots for events
#[derive(Debug, Clone)]
pub struct VLC {
    /// The node ID this clock belongs to
    pub node_id: String,

    /// The underlying VLC snapshot
    snapshot: VLCSnapshot,
}

impl VLC {
    /// Create a new VLC for a node
    pub fn new(node_id: String) -> Self {
        Self {
            node_id: node_id.clone(),
            snapshot: VLCSnapshot::for_node(node_id),
        }
    }

    /// Increment the logical time (tick the clock)
    pub fn tick(&mut self) {
        self.snapshot.increment(&self.node_id);
    }

    /// Merge with another VLC snapshot (receive operation)
    pub fn merge(&mut self, other: &VLCSnapshot) {
        self.snapshot.receive(other, &self.node_id);
    }

    /// Get the current logical time
    pub fn logical_time(&self) -> u64 {
        self.snapshot.logical_time
    }

    /// Get the current physical time
    pub fn physical_time(&self) -> u64 {
        self.snapshot.physical_time
    }

    /// Get a reference to the vector clock
    pub fn vector_clock(&self) -> &VectorClock {
        &self.snapshot.vector_clock
    }

    /// Take a snapshot of the current clock state
    pub fn snapshot(&self) -> VLCSnapshot {
        self.snapshot.clone()
    }

    /// Check if this clock happens before another
    pub fn happens_before(&self, other: &VLCSnapshot) -> bool {
        self.snapshot.happens_before(other)
    }

    /// Check if this clock is concurrent with another
    pub fn is_concurrent(&self, other: &VLCSnapshot) -> bool {
        self.snapshot.is_concurrent(other)
    }

    /// Get the clock value for a specific node
    pub fn get_clock(&self, node_id: &str) -> u64 {
        self.snapshot.vector_clock.get(node_id)
    }

    /// Garbage collect inactive nodes
    pub fn gc_inactive_nodes(&mut self, active_nodes: &[String]) -> usize {
        self.snapshot.gc_inactive_nodes(active_nodes)
    }

    /// Restore VLC state from a snapshot (used for node restart recovery)
    ///
    /// This replaces the current snapshot with the provided one, allowing
    /// the node to resume from a previous state after restart.
    pub fn restore_from_snapshot(&mut self, snapshot: &VLCSnapshot) {
        self.snapshot = snapshot.clone();
    }
}

impl Default for VLC {
    fn default() -> Self {
        Self::new("default".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vlc_new() {
        let vlc = VLC::new("node1".to_string());
        assert_eq!(vlc.node_id, "node1");
        assert_eq!(vlc.logical_time(), 0);
    }

    #[test]
    fn test_vlc_tick() {
        let mut vlc = VLC::new("node1".to_string());
        vlc.tick();
        assert_eq!(vlc.logical_time(), 1);
        vlc.tick();
        assert_eq!(vlc.logical_time(), 2);
    }

    #[test]
    fn test_vlc_merge() {
        let mut vlc1 = VLC::new("node1".to_string());
        let mut vlc2 = VLC::new("node2".to_string());

        vlc1.tick();
        vlc1.tick();
        vlc2.tick();

        let snapshot1 = vlc1.snapshot();
        vlc2.merge(&snapshot1);

        // After merge, vlc2's logical time should be max(1, 2) + 1 = 3
        assert_eq!(vlc2.logical_time(), 3);
    }

    #[test]
    fn test_vlc_snapshot() {
        let mut vlc = VLC::new("node1".to_string());
        vlc.tick();

        let snapshot = vlc.snapshot();
        assert_eq!(snapshot.logical_time, 1);
    }

    #[test]
    fn test_vlc_happens_before() {
        let mut vlc1 = VLC::new("node1".to_string());
        let mut vlc2 = VLC::new("node2".to_string());

        vlc1.tick();
        let snapshot1 = vlc1.snapshot();

        vlc2.merge(&snapshot1);
        let snapshot2 = vlc2.snapshot();

        assert!(vlc1.happens_before(&snapshot2));
    }
}
