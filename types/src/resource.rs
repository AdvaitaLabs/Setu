//! Resource types for the three-resource economic model: SETU / Flux / Power
//!
//! - SETU: transferable token (existing Coin type, BCS serialized)
//! - Flux: non-transferable credit score (JSON serialized)
//! - Power: non-transferable life counter (JSON serialized)

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use crate::ObjectId;
use crate::hash_utils::setu_hash_with_domain;

// ========== Constants ==========

/// Initial Power for new accounts (21 million events lifetime)
pub const INITIAL_POWER: u64 = 21_000_000;

/// Initial Flux for new accounts
pub const INITIAL_FLUX: u64 = 0;

// ========== FluxState ==========

/// Flux (credit score) stored as independent object in Merkle tree (JSON serialized).
///
/// State key: `"oid:{hex}"` where hex = BLAKE3("SETU_FLUX:" || address)
///
/// Per-address (global), NOT per-subnet. One FluxState per user regardless of
/// how many subnets they're registered in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FluxState {
    /// User address (hex string)
    pub address: String,
    /// Flux score — increases via successful solver-executed events
    pub flux: u64,
    /// Last activity timestamp (deterministic, from ExecutionContext.timestamp)
    pub last_active_at: u64,
    /// Version (incremented on each write)
    pub version: u64,
}

impl FluxState {
    /// Create a new FluxState for a newly registered user.
    pub fn new(address: &str, timestamp: u64) -> Self {
        Self {
            address: address.to_string(),
            flux: INITIAL_FLUX,
            last_active_at: timestamp,
            version: 0,
        }
    }
}

// ========== PowerState ==========

/// Power (life counter) stored as independent object in Merkle tree (JSON serialized).
///
/// State key: `"oid:{hex}"` where hex = BLAKE3("SETU_POWER:" || address)
///
/// Per-address (global), NOT per-subnet. One PowerState per user regardless of
/// how many subnets they're registered in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PowerState {
    /// User address (hex string)
    pub address: String,
    /// Remaining Power (starts at INITIAL_POWER, decremented by 1 per solver-executed event)
    pub power_remaining: u64,
    /// Version (incremented on each write)
    pub version: u64,
}

impl PowerState {
    /// Create a new PowerState for a newly registered user.
    pub fn new(address: &str) -> Self {
        Self {
            address: address.to_string(),
            power_remaining: INITIAL_POWER,
            version: 0,
        }
    }
}

// ========== Governance ==========

/// Resource governance mode (pre-submission only, never affects deterministic executor).
///
/// - `Enabled`: enforce admission checks (freeze check for Power, score check for Flux)
/// - `Disabled`: skip admission checks (default — allows gradual rollout)
/// - `DryRun`: compute and log checks but don't enforce
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum ResourceGovernanceMode {
    Enabled = 0,
    #[default]
    Disabled = 1,
    DryRun = 2,
}

/// Atomic wrapper for ResourceGovernanceMode (thread-safe, lock-free).
///
/// Node-local configuration — different validators can have different settings.
/// This is safe because governance only affects pre-submission admission checks,
/// never the deterministic executor.
pub struct AtomicGovernanceMode(AtomicU8);

impl AtomicGovernanceMode {
    pub fn new(mode: ResourceGovernanceMode) -> Self {
        Self(AtomicU8::new(mode as u8))
    }

    pub fn load(&self) -> ResourceGovernanceMode {
        match self.0.load(Ordering::Relaxed) {
            0 => ResourceGovernanceMode::Enabled,
            2 => ResourceGovernanceMode::DryRun,
            _ => ResourceGovernanceMode::Disabled,
        }
    }

    pub fn store(&self, mode: ResourceGovernanceMode) {
        self.0.store(mode as u8, Ordering::Relaxed);
    }
}

impl Default for AtomicGovernanceMode {
    fn default() -> Self {
        Self::new(ResourceGovernanceMode::default())
    }
}

// ========== ResourceParams — On-chain governance parameters ==========

/// On-chain resource governance parameters.
/// Stored as a SINGLE JSON object in GOVERNANCE SMT.
/// State key: `"oid:{hex}"` where hex = BLAKE3("SETU_RESOURCE_PARAMS")
///
/// All fields have defaults matching current hardcoded behavior.
/// Changes via GovernanceProposal(UpdateResourceParam { .. }).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceParams {
    // ===== Power =====
    pub power_mode: ResourceGovernanceMode,
    pub power_cost_per_event: u64,
    // ===== Flux =====
    pub flux_mode: ResourceGovernanceMode,
    pub flux_reward_on_success: u64,
    pub flux_penalty_on_failure: u64,
    // ===== SETU Gas (Phase 5 — deferred) =====
    pub gas_mode: ResourceGovernanceMode,
    pub gas_base_price: u64,
    // ===== SETU Token Limits (Phase 6 — deferred) =====
    pub min_transfer_amount: u64,
    pub max_transfer_amount: u64,
    pub max_merge_sources: u32,
    pub max_split_outputs: u32,
    pub power_revival_price: u64,
    pub transfer_fee_rate_bps: u32,
    // ===== Metadata =====
    pub version: u64,
    pub last_updated_at: u64,
}

impl Default for ResourceParams {
    fn default() -> Self {
        Self {
            power_mode: ResourceGovernanceMode::Disabled,
            power_cost_per_event: 1,
            flux_mode: ResourceGovernanceMode::Disabled,
            flux_reward_on_success: 1,
            flux_penalty_on_failure: 0,
            gas_mode: ResourceGovernanceMode::Disabled,
            gas_base_price: 0,
            min_transfer_amount: 1,
            max_transfer_amount: u64::MAX,
            max_merge_sources: 50,
            max_split_outputs: 50,
            power_revival_price: 0,
            transfer_fee_rate_bps: 0,
            version: 0,
            last_updated_at: 0,
        }
    }
}

/// Typed resource parameter change — each variant maps to one field in ResourceParams.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceParamChange {
    // Power
    SetPowerMode(ResourceGovernanceMode),
    SetPowerCostPerEvent(u64),
    // Flux
    SetFluxMode(ResourceGovernanceMode),
    SetFluxRewardOnSuccess(u64),
    SetFluxPenaltyOnFailure(u64),
    // Gas (Phase 5 — deferred)
    SetGasMode(ResourceGovernanceMode),
    SetGasBasePrice(u64),
    // SETU Token Limits (Phase 6 — deferred)
    SetMinTransferAmount(u64),
    SetMaxTransferAmount(u64),
    SetMaxMergeSources(u32),
    SetMaxSplitOutputs(u32),
    SetPowerRevivalPrice(u64),
    SetTransferFeeRateBps(u32),
}

/// Apply a single ResourceParamChange to a ResourceParams, incrementing version.
pub fn apply_resource_param_change(
    params: &mut ResourceParams,
    change: &ResourceParamChange,
    timestamp: u64,
) {
    match change {
        ResourceParamChange::SetPowerMode(v) => params.power_mode = *v,
        ResourceParamChange::SetPowerCostPerEvent(v) => params.power_cost_per_event = *v,
        ResourceParamChange::SetFluxMode(v) => params.flux_mode = *v,
        ResourceParamChange::SetFluxRewardOnSuccess(v) => params.flux_reward_on_success = *v,
        ResourceParamChange::SetFluxPenaltyOnFailure(v) => params.flux_penalty_on_failure = *v,
        ResourceParamChange::SetGasMode(v) => params.gas_mode = *v,
        ResourceParamChange::SetGasBasePrice(v) => params.gas_base_price = *v,
        ResourceParamChange::SetMinTransferAmount(v) => params.min_transfer_amount = *v,
        ResourceParamChange::SetMaxTransferAmount(v) => params.max_transfer_amount = *v,
        ResourceParamChange::SetMaxMergeSources(v) => params.max_merge_sources = *v,
        ResourceParamChange::SetMaxSplitOutputs(v) => params.max_split_outputs = *v,
        ResourceParamChange::SetPowerRevivalPrice(v) => params.power_revival_price = *v,
        ResourceParamChange::SetTransferFeeRateBps(v) => params.transfer_fee_rate_bps = *v,
    }
    params.version += 1;
    params.last_updated_at = timestamp;
}

// ========== Deterministic ObjectId Helpers ==========

/// Compute deterministic ObjectId for a user's FluxState.
///
/// Key: BLAKE3("SETU_FLUX:" || canonical_address)
/// Address is lowercased to ensure canonical form (R8-ISSUE-1).
/// Stored as: `"oid:{hex}"` (G11 compliant)
pub fn flux_state_object_id(address: &str) -> ObjectId {
    let canonical = address.to_ascii_lowercase();
    ObjectId::new(setu_hash_with_domain(b"SETU_FLUX:", canonical.as_bytes()))
}

/// Compute deterministic ObjectId for a user's PowerState.
///
/// Key: BLAKE3("SETU_POWER:" || canonical_address)
/// Address is lowercased to ensure canonical form (R8-ISSUE-1).
/// Stored as: `"oid:{hex}"` (G11 compliant)
pub fn power_state_object_id(address: &str) -> ObjectId {
    let canonical = address.to_ascii_lowercase();
    ObjectId::new(setu_hash_with_domain(b"SETU_POWER:", canonical.as_bytes()))
}

/// Deterministic ObjectId for the global ResourceParams singleton.
/// Key: BLAKE3("SETU_RESOURCE_PARAMS:" || "") — no address suffix (global singleton).
pub fn resource_params_object_id() -> ObjectId {
    ObjectId::new(setu_hash_with_domain(b"SETU_RESOURCE_PARAMS:", b""))
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flux_state_new() {
        let fs = FluxState::new("0xabc", 1000);
        assert_eq!(fs.flux, 0);
        assert_eq!(fs.last_active_at, 1000);
        assert_eq!(fs.version, 0);
    }

    #[test]
    fn test_power_state_new() {
        let ps = PowerState::new("0xabc");
        assert_eq!(ps.power_remaining, INITIAL_POWER);
        assert_eq!(ps.version, 0);
    }

    #[test]
    fn test_flux_state_json_roundtrip() {
        let fs = FluxState::new("0xabc", 1000);
        let json = serde_json::to_vec(&fs).unwrap();
        let fs2: FluxState = serde_json::from_slice(&json).unwrap();
        assert_eq!(fs, fs2);
    }

    #[test]
    fn test_power_state_json_roundtrip() {
        let ps = PowerState::new("0xdef");
        let json = serde_json::to_vec(&ps).unwrap();
        let ps2: PowerState = serde_json::from_slice(&json).unwrap();
        assert_eq!(ps, ps2);
    }

    #[test]
    fn test_object_ids_deterministic() {
        let id1 = flux_state_object_id("0xabc");
        let id2 = flux_state_object_id("0xabc");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_object_ids_differ_by_type() {
        let flux_id = flux_state_object_id("0xabc");
        let power_id = power_state_object_id("0xabc");
        assert_ne!(flux_id, power_id);
    }

    #[test]
    fn test_governance_default_disabled() {
        let mode = ResourceGovernanceMode::default();
        assert_eq!(mode, ResourceGovernanceMode::Disabled);
    }

    #[test]
    fn test_atomic_governance_mode() {
        let agm = AtomicGovernanceMode::default();
        assert_eq!(agm.load(), ResourceGovernanceMode::Disabled);
        agm.store(ResourceGovernanceMode::Enabled);
        assert_eq!(agm.load(), ResourceGovernanceMode::Enabled);
        agm.store(ResourceGovernanceMode::DryRun);
        assert_eq!(agm.load(), ResourceGovernanceMode::DryRun);
    }

    #[test]
    fn test_resource_params_default() {
        let p = ResourceParams::default();
        assert_eq!(p.power_mode, ResourceGovernanceMode::Disabled);
        assert_eq!(p.power_cost_per_event, 1);
        assert_eq!(p.flux_mode, ResourceGovernanceMode::Disabled);
        assert_eq!(p.flux_reward_on_success, 1);
        assert_eq!(p.flux_penalty_on_failure, 0);
        assert_eq!(p.gas_mode, ResourceGovernanceMode::Disabled);
        assert_eq!(p.gas_base_price, 0);
        assert_eq!(p.min_transfer_amount, 1);
        assert_eq!(p.max_transfer_amount, u64::MAX);
        assert_eq!(p.max_merge_sources, 50);
        assert_eq!(p.max_split_outputs, 50);
        assert_eq!(p.power_revival_price, 0);
        assert_eq!(p.transfer_fee_rate_bps, 0);
        assert_eq!(p.version, 0);
        assert_eq!(p.last_updated_at, 0);
    }

    #[test]
    fn test_resource_params_object_id_deterministic() {
        let id1 = resource_params_object_id();
        let id2 = resource_params_object_id();
        assert_eq!(id1, id2);
        // Must differ from per-user ObjectIds
        assert_ne!(id1, flux_state_object_id("0xabc"));
    }

    #[test]
    fn test_resource_params_json_roundtrip() {
        let p = ResourceParams::default();
        let json = serde_json::to_vec(&p).unwrap();
        let p2: ResourceParams = serde_json::from_slice(&json).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn test_resource_param_change_serde() {
        let change = ResourceParamChange::SetPowerCostPerEvent(2);
        let json = serde_json::to_string(&change).unwrap();
        assert_eq!(json, r#"{"SetPowerCostPerEvent":2}"#);
        let decoded: ResourceParamChange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, change);
    }

    #[test]
    fn test_apply_resource_param_change_all_variants() {
        let changes: Vec<(ResourceParamChange, Box<dyn Fn(&ResourceParams) -> bool>)> = vec![
            (ResourceParamChange::SetPowerMode(ResourceGovernanceMode::Enabled),
             Box::new(|p: &ResourceParams| p.power_mode == ResourceGovernanceMode::Enabled)),
            (ResourceParamChange::SetPowerCostPerEvent(5),
             Box::new(|p: &ResourceParams| p.power_cost_per_event == 5)),
            (ResourceParamChange::SetFluxMode(ResourceGovernanceMode::DryRun),
             Box::new(|p: &ResourceParams| p.flux_mode == ResourceGovernanceMode::DryRun)),
            (ResourceParamChange::SetFluxRewardOnSuccess(10),
             Box::new(|p: &ResourceParams| p.flux_reward_on_success == 10)),
            (ResourceParamChange::SetFluxPenaltyOnFailure(3),
             Box::new(|p: &ResourceParams| p.flux_penalty_on_failure == 3)),
            (ResourceParamChange::SetGasMode(ResourceGovernanceMode::Enabled),
             Box::new(|p: &ResourceParams| p.gas_mode == ResourceGovernanceMode::Enabled)),
            (ResourceParamChange::SetGasBasePrice(100),
             Box::new(|p: &ResourceParams| p.gas_base_price == 100)),
            (ResourceParamChange::SetMinTransferAmount(10),
             Box::new(|p: &ResourceParams| p.min_transfer_amount == 10)),
            (ResourceParamChange::SetMaxTransferAmount(1_000_000),
             Box::new(|p: &ResourceParams| p.max_transfer_amount == 1_000_000)),
            (ResourceParamChange::SetMaxMergeSources(100),
             Box::new(|p: &ResourceParams| p.max_merge_sources == 100)),
            (ResourceParamChange::SetMaxSplitOutputs(25),
             Box::new(|p: &ResourceParams| p.max_split_outputs == 25)),
            (ResourceParamChange::SetPowerRevivalPrice(500),
             Box::new(|p: &ResourceParams| p.power_revival_price == 500)),
            (ResourceParamChange::SetTransferFeeRateBps(50),
             Box::new(|p: &ResourceParams| p.transfer_fee_rate_bps == 50)),
        ];
        for (i, (change, check)) in changes.into_iter().enumerate() {
            let mut p = ResourceParams::default();
            apply_resource_param_change(&mut p, &change, 1000);
            assert!(check(&p), "variant {} failed", i);
            assert_eq!(p.version, 1);
            assert_eq!(p.last_updated_at, 1000);
        }
    }
}
