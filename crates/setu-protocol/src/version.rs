// Copyright (c) Setu Contributors
// SPDX-License-Identifier: Apache-2.0

//! Protocol version management
//!
//! This module provides versioning support for the Setu protocol, enabling
//! backward-compatible protocol evolution.

use serde::{Deserialize, Serialize};

/// Protocol version for Setu network messages
///
/// Versioning allows for protocol evolution while maintaining backward
/// compatibility with older nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Major version (breaking changes)
    pub major: u16,
    /// Minor version (backward-compatible additions)
    pub minor: u16,
    /// Patch version (bug fixes)
    pub patch: u16,
}

impl ProtocolVersion {
    /// Current protocol version
    pub const CURRENT: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    /// Minimum supported protocol version
    pub const MIN_SUPPORTED: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    /// Create a new protocol version
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version is compatible with another version
    ///
    /// Versions are compatible if they have the same major version and
    /// the minor version is >= the other's minor version.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major && self.minor >= other.minor
    }

    /// Check if this version is at least the minimum supported version
    pub fn is_supported(&self) -> bool {
        *self >= Self::MIN_SUPPORTED
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// Protocol negotiation result
#[derive(Debug, Clone)]
pub struct NegotiatedProtocol {
    /// The agreed-upon version
    pub version: ProtocolVersion,
    /// Whether this peer initiated the negotiation
    pub is_initiator: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_compatibility() {
        let v1_0 = ProtocolVersion::new(1, 0, 0);
        let v1_1 = ProtocolVersion::new(1, 1, 0);
        let v2_0 = ProtocolVersion::new(2, 0, 0);

        // Same major version, higher minor is compatible
        assert!(v1_1.is_compatible_with(&v1_0));
        // Same version is compatible
        assert!(v1_0.is_compatible_with(&v1_0));
        // Lower minor is not compatible
        assert!(!v1_0.is_compatible_with(&v1_1));
        // Different major is not compatible
        assert!(!v2_0.is_compatible_with(&v1_0));
    }

    #[test]
    fn test_version_ordering() {
        let v1_0 = ProtocolVersion::new(1, 0, 0);
        let v1_1 = ProtocolVersion::new(1, 1, 0);
        let v1_0_1 = ProtocolVersion::new(1, 0, 1);

        assert!(v1_1 > v1_0);
        assert!(v1_0_1 > v1_0);
        assert!(v1_1 > v1_0_1);
    }

    #[test]
    fn test_current_is_supported() {
        assert!(ProtocolVersion::CURRENT.is_supported());
    }
}
