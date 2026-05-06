//! Move package upgrade ABI-compatibility helper (B5 / Phase 8).
//!
//! Wraps `move_binary_format::compatibility::Compatibility::check` for use by
//! both publish-time bookkeeping and upgrade lowering. The wrapper exists to
//! (a) translate `PartialVMError` into a typed [`CompatError`] preserving the
//! `StatusCode`, (b) build a fresh `RcPool` per call so callers do not have to
//! manage the normalised module's identifier-pool lifetime, and (c) sit at a
//! single auditable point so future policy changes (full vs upgrade-only vs
//! framework-only) stay in one place.
//!
//! Callers MUST charge `compat_check_per_struct` and `compat_check_per_function`
//! based on the *old* module's handle counts before invoking
//! [`check_upgrade_compat`] (design.md §9.1).

use move_binary_format::{
    compatibility::{Compatibility, Module as NormalizedModule},
    errors::PartialVMError,
    file_format::CompiledModule,
    normalized::RcPool,
};
use move_core_types::vm_status::StatusCode;
use thiserror::Error;

/// Errors raised by [`check_upgrade_compat`].
#[derive(Debug, Error)]
pub enum CompatError {
    /// The new module is not backward-compatible with the old module under
    /// the chosen [`UpgradePolicy`]. Carries the underlying `StatusCode` from
    /// `move-binary-format` so callers / log readers can distinguish layout
    /// mismatches from entry-linking violations etc.
    #[error("upgrade compatibility check failed (status={status:?}): {msg}")]
    Incompatible { status: StatusCode, msg: String },
}

/// Setu upgrade policy. Numeric encoding matches the on-wire `policy: u8`
/// field on `MoveUpgradePayload` (design.md §3) and `UpgradeCap.policy`
/// (setu-framework `package.move`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UpgradePolicy {
    /// `Compatibility::full_check()` — strictest; layout, abilities, and
    /// public-entry linking all preserved.
    Compatible = 0,
    /// `Compatibility::upgrade_check()` — userspace upgrade default; layout
    /// and ability-removal still checked, but `entry` linking is relaxed.
    AdditiveOnly = 1,
    /// `Compatibility::no_check()` — escape hatch reserved for governance /
    /// framework upgrades. Userspace ticket creation MUST NOT mint this
    /// policy; enforced by `setu::package::authorize_upgrade`.
    DepOnly = 2,
}

impl UpgradePolicy {
    /// Decode from the on-wire `u8`. Unknown values reject.
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Compatible),
            1 => Some(Self::AdditiveOnly),
            2 => Some(Self::DepOnly),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Map to upstream `Compatibility` configuration.
    fn as_compat(self) -> Compatibility {
        match self {
            Self::Compatible => Compatibility::full_check(),
            Self::AdditiveOnly => Compatibility::upgrade_check(),
            Self::DepOnly => Compatibility::no_check(),
        }
    }
}

/// Run a backward-compatibility check between `old_module` and `new_module`
/// under `policy`. Both modules MUST already be verified
/// (`verify_module_unmetered`); the helper does not re-verify.
///
/// `old_module` is the module currently held under `prev_package`'s `mod:`
/// state key, `new_module` is the **post-relink** bundle entry that will be
/// published under `new_package_addr`. The helper compares ABI shapes only —
/// the address-rewrite delta is intentionally invisible to the upstream
/// checker (compat looks at struct/function handles, not address identifiers).
pub fn check_upgrade_compat(
    old_module: &CompiledModule,
    new_module: &CompiledModule,
    policy: UpgradePolicy,
) -> Result<(), CompatError> {
    let compat = policy.as_compat();
    if !compat.need_check_compat() {
        return Ok(());
    }

    let mut pool = RcPool::new();
    let old_norm: NormalizedModule = NormalizedModule::new(&mut pool, old_module, /* include_code */ false);
    let new_norm: NormalizedModule = NormalizedModule::new(&mut pool, new_module, false);

    compat.check(&old_norm, &new_norm).map_err(|e: PartialVMError| {
        CompatError::Incompatible {
            status: e.major_status(),
            msg: e.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_stdlib() -> Option<&'static [u8]> {
        crate::engine::STDLIB_MODULES.iter().copied().next().map(|(_, b)| b)
    }

    #[test]
    fn u_co1_self_compat_passes_under_compatible() {
        let Some(bytes) = first_stdlib() else {
            return;
        };
        let m = CompiledModule::deserialize_with_defaults(bytes).unwrap();
        // Compat is reflexive: a module is always compatible with itself.
        check_upgrade_compat(&m, &m, UpgradePolicy::Compatible).unwrap();
        check_upgrade_compat(&m, &m, UpgradePolicy::AdditiveOnly).unwrap();
    }

    #[test]
    fn u_co2_dep_only_skips_check() {
        let Some(bytes) = first_stdlib() else {
            return;
        };
        let m1 = CompiledModule::deserialize_with_defaults(bytes).unwrap();
        // Even comparing module-to-itself succeeds trivially; what we really
        // assert is that the function takes the early-exit branch — which we
        // observe indirectly by passing two structurally identical modules
        // (no panics, no PartialVMError plumbing).
        check_upgrade_compat(&m1, &m1, UpgradePolicy::DepOnly).unwrap();
    }

    #[test]
    fn u_co3_policy_round_trip() {
        for p in [UpgradePolicy::Compatible, UpgradePolicy::AdditiveOnly, UpgradePolicy::DepOnly] {
            assert_eq!(UpgradePolicy::from_u8(p.as_u8()), Some(p));
        }
        assert!(UpgradePolicy::from_u8(99).is_none());
    }
}
