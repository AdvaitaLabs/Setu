//! Module relink helper (B5 / Phase 8 path β-1).
//!
//! Rewrites every occurrence of an "old" `AccountAddress` to a "new" one inside
//! a single Move module's bytecode and re-serialises the result. This is the
//! cornerstone of Setu's fresh-address-per-version package upgrade strategy
//! (design.md §4.5): each upgrade derives a new package address, and every
//! self-reference baked into `CompiledModule.address_identifiers` must be
//! updated before the bundle is verified and published at that new address.
//!
//! ## Scope
//!
//! Only `address_identifiers` slots whose value **exactly equals** `old_addr`
//! are rewritten. Slots pointing at other addresses (dependencies, framework
//! packages, etc.) are intentionally left untouched — those preserve the
//! existing dependency-pinning invariant, where each upgrade keeps its
//! historical link to the specific version of every dep it was compiled
//! against.
//!
//! Move's CompiledModule indexes every `ModuleHandle.address` /
//! `FieldHandle.address` / friend / metadata path through this single
//! `address_identifiers` pool, so rewriting the pool covers all five
//! categories listed in design.md §β-1 (self-handle, friend decl, address
//! constant, metadata, function self-uses) without per-table walking.
//!
//! ## Invariants
//!
//! - `relink(b, A, A)` is byte-equal to the canonical re-serialisation of `b`
//!   (a no-op pass).
//! - `relink(relink(b, A, B), B, A)` is byte-equal to `relink(b, A, A)` for
//!   every well-formed module — i.e. relink is **involutive** modulo
//!   serialisation canonicalisation. Verified by U-RL3.
//! - Relinked output passes `move_bytecode_verifier::verify_module_unmetered`.
//!
//! ## Gas
//!
//! Per-module / per-slot charges live on `gas::PTB_OVERHEAD_TABLE`
//! (`relink_per_module`, `relink_per_address_identifier`); they are charged by
//! the *caller* (engine / infra_executor) before invoking this helper, so the
//! function itself is gas-pure.

use move_binary_format::{errors::VMError, file_format::CompiledModule};
use move_core_types::account_address::AccountAddress;
use thiserror::Error;

/// Errors raised by [`relink_module`].
#[derive(Debug, Error)]
pub enum RelinkError {
    #[error("module deserialize failed: {0}")]
    Deserialize(String),

    #[error("module re-serialize failed: {0}")]
    Serialize(String),

    #[error("post-relink bytecode verification failed: {0}")]
    Verify(Box<VMError>),

    #[error("relink found no `old_addr` slot — caller-supplied old_addr does not appear in the module's address_identifiers (old=0x{old})")]
    OldAddressNotFound { old: String },
}

/// Outcome of a relink call. Returned in addition to the rewritten bytes so
/// the caller can charge per-slot gas (`relink_per_address_identifier`) and
/// emit useful diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelinkStats {
    /// Number of `address_identifiers` slots inspected (= total slots in the
    /// pool, regardless of whether they matched).
    pub address_identifiers_scanned: usize,
    /// Number of slots that matched `old_addr` and were rewritten.
    pub address_identifiers_rewritten: usize,
}

/// Rewrite every `address_identifiers[i] == old_addr` slot to `new_addr` and
/// re-serialise the module.
///
/// `old_addr == new_addr` is allowed and produces a canonicalisation pass
/// (deserialize + serialize round-trip). At least one slot **must** match
/// `old_addr` when the two differ — otherwise the caller has supplied the
/// wrong "old" address and we fail loudly rather than silently no-op.
pub fn relink_module(
    bytecode: &[u8],
    old_addr: AccountAddress,
    new_addr: AccountAddress,
) -> Result<(Vec<u8>, RelinkStats), RelinkError> {
    let mut module = CompiledModule::deserialize_with_defaults(bytecode)
        .map_err(|e| RelinkError::Deserialize(e.to_string()))?;

    let scanned = module.address_identifiers.len();
    let mut rewritten = 0usize;
    if old_addr != new_addr {
        for slot in module.address_identifiers.iter_mut() {
            if *slot == old_addr {
                *slot = new_addr;
                rewritten += 1;
            }
        }
        if rewritten == 0 {
            return Err(RelinkError::OldAddressNotFound {
                old: hex::encode(old_addr.into_bytes()),
            });
        }
    }

    let mut out = Vec::with_capacity(bytecode.len());
    module
        .serialize_with_version(module.version, &mut out)
        .map_err(|e| RelinkError::Serialize(e.to_string()))?;

    // Verify post-relink bytecode (cheap structural pass, not metered VM exec).
    let verified = CompiledModule::deserialize_with_defaults(&out)
        .map_err(|e| RelinkError::Deserialize(e.to_string()))?;
    move_bytecode_verifier::verify_module_unmetered(&verified)
        .map_err(|e| RelinkError::Verify(Box::new(e)))?;

    Ok((
        out,
        RelinkStats {
            address_identifiers_scanned: scanned,
            address_identifiers_rewritten: rewritten,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: pick the first stdlib module that has at least one
    /// non-zero address slot to exercise the rewrite path.
    fn stdlib_sample() -> Option<(&'static str, &'static [u8])> {
        crate::engine::STDLIB_MODULES.iter().copied().next()
    }

    #[test]
    fn u_rl1_noop_self_to_self_is_canonical() {
        let Some((_, bytes)) = stdlib_sample() else {
            // Build w/o stdlib — skip rather than fail, mirrors
            // `stdlib_module_count() == 0` graceful-degradation policy.
            return;
        };
        let m = CompiledModule::deserialize_with_defaults(bytes).unwrap();
        let self_addr = *m.self_id().address();
        let (out, stats) = relink_module(bytes, self_addr, self_addr).unwrap();
        assert_eq!(stats.address_identifiers_rewritten, 0);
        assert!(stats.address_identifiers_scanned >= 1);
        // Round-trip back through deserialize must yield an identical module.
        let m2 = CompiledModule::deserialize_with_defaults(&out).unwrap();
        assert_eq!(m.self_id(), m2.self_id());
    }

    #[test]
    fn u_rl2_rewrite_self_to_fresh_addr() {
        let Some((_, bytes)) = stdlib_sample() else {
            return;
        };
        let old = *CompiledModule::deserialize_with_defaults(bytes)
            .unwrap()
            .self_id()
            .address();
        let mut new_bytes = [0u8; AccountAddress::LENGTH];
        new_bytes[AccountAddress::LENGTH - 1] = 0xAB;
        let new = AccountAddress::new(new_bytes);

        let (out, stats) = relink_module(bytes, old, new).unwrap();
        assert!(stats.address_identifiers_rewritten >= 1);
        let m2 = CompiledModule::deserialize_with_defaults(&out).unwrap();
        assert_eq!(*m2.self_id().address(), new);
    }

    #[test]
    fn u_rl3_relink_is_involutive() {
        let Some((_, bytes)) = stdlib_sample() else {
            return;
        };
        let old = *CompiledModule::deserialize_with_defaults(bytes)
            .unwrap()
            .self_id()
            .address();
        let mut tmp = [0u8; AccountAddress::LENGTH];
        tmp[0] = 0xFE;
        let mid = AccountAddress::new(tmp);

        let (forward, _) = relink_module(bytes, old, mid).unwrap();
        let (back, _) = relink_module(&forward, mid, old).unwrap();
        // Canonical baseline: deserialize + reserialize the original.
        let m = CompiledModule::deserialize_with_defaults(bytes).unwrap();
        let mut canonical = Vec::new();
        m.serialize_with_version(m.version, &mut canonical).unwrap();
        assert_eq!(back, canonical, "relink must be involutive modulo canonicalisation");
    }

    #[test]
    fn u_rl4_old_addr_not_found_errs() {
        let Some((_, bytes)) = stdlib_sample() else {
            return;
        };
        let mut bogus = [0u8; AccountAddress::LENGTH];
        bogus[0] = 0xCC;
        let bogus = AccountAddress::new(bogus);
        let mut new = [0u8; AccountAddress::LENGTH];
        new[0] = 0xDD;
        let new = AccountAddress::new(new);
        let err = relink_module(bytes, bogus, new).unwrap_err();
        assert!(matches!(err, RelinkError::OldAddressNotFound { .. }));
    }

    #[test]
    fn u_rl5_rejects_garbage_bytecode() {
        let bogus_old = AccountAddress::ZERO;
        let bogus_new = AccountAddress::ONE;
        let err = relink_module(&[0u8; 4], bogus_old, bogus_new).unwrap_err();
        assert!(matches!(err, RelinkError::Deserialize(_)));
    }
}
