//! Setu Move VM — Sui fork integration for Move smart contract execution.
//!
//! This crate is the home for all Move VM logic per ADR-6.
//! setu-runtime stays pure (G5: depends on types/ only).
//!
//! ## Module Layout (Phase 1)
//!
//! - `address_compat` — Setu ↔ Move address conversions
//! - `gas`            — InstructionCountGasMeter (27 GasMeter methods)
//! - `resolver`       — SetuModuleResolver (module loading from storage)
//! - `object_runtime` — SetuObjectRuntime (NativeContextExtensions)
//! - `natives`        — Native function implementations + registration table
//! - `engine`         — SetuMoveEngine (VM lifecycle, session, execute)

pub mod address_compat;
pub mod compat;
pub mod engine;
pub mod gas;
pub mod hybrid;
pub mod natives;
pub mod natives_b3;
pub mod object_runtime;
pub mod ptb_executor;
pub mod relink;
pub mod resolver;

// Re-export move-core-types so downstream crates (e.g. setu-enclave)
// can access Move types without a direct dependency.
pub use move_core_types;

/// Number of Move stdlib modules baked into this binary at build time.
///
/// Returns 0 if the binary was compiled without `setu-framework/compiled/*.mv`
/// (i.e. `STDLIB_MODULES` fell back to the empty array in `build.rs`).
/// Used by binary entry points (e.g. `setu-solver`) for startup sanity checks
/// to fail fast on misbuilt binaries — see
/// `docs/bugs/20260421-deploy-stdlib-missing.md`.
pub fn stdlib_module_count() -> usize {
    crate::engine::STDLIB_MODULES.len()
}
