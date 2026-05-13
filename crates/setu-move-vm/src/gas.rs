//! InstructionCountGasMeter — Phase 1 minimal gas metering.
//!
//! Counts each instruction as 1 gas unit. Prevents infinite loops
//! and excessive recursion without economic precision (Phase 3+).
//!
//! v3.8 R8-2: Based on POC-6 audit against Sui mainnet-v1.66.2.
//! All 27 GasMeter trait methods verified to compile.

use move_binary_format::errors::{PartialVMError, PartialVMResult};
use move_core_types::{
    gas_algebra::{InternalGas, NumArgs, NumBytes},
    language_storage::ModuleId,
    vm_status::StatusCode,
};
use move_vm_types::{
    gas::{GasMeter, SimpleInstruction},
    views::{TypeView, ValueView},
};

/// Instruction-counting gas meter for Phase 1.
///
/// Each bytecode instruction costs 1 gas unit (except calls = 10).
/// No `gas_used()` in GasMeter trait — use `instructions_executed()`.
pub struct InstructionCountGasMeter {
    max_instructions: u64,
    instructions_executed: u64,
}

impl InstructionCountGasMeter {
    pub fn new(max_instructions: u64) -> Self {
        Self {
            max_instructions,
            instructions_executed: 0,
        }
    }

    pub fn instructions_executed(&self) -> u64 {
        self.instructions_executed
    }

    fn charge(&mut self, cost: u64) -> PartialVMResult<()> {
        // B6c · atomic charge: reject BEFORE mutating the counter so that
        // `instructions_executed()` after an OOM still reflects only work
        // that was successfully committed. Required for the `gas_used <=
        // gas_budget` invariant on the abort path (design §4.3).
        let new_total = self.instructions_executed.saturating_add(cost);
        if new_total > self.max_instructions {
            Err(PartialVMError::new(StatusCode::OUT_OF_GAS))
        } else {
            self.instructions_executed = new_total;
            Ok(())
        }
    }

    /// B6c · public outer-tier surcharge entry point.
    ///
    /// PTB executor calls this BEFORE invoking each command (overhead for
    /// argument resolution, lowering, result-table push) and AFTER
    /// finalize for storage-key writes / reads. Reuses the same counter as
    /// the inner per-instruction `charge` so a single `OUT_OF_GAS` boundary
    /// covers both tiers.
    pub fn charge_outer(&mut self, cost: u64) -> PartialVMResult<()> {
        self.charge(cost)
    }

    /// B6c · raw remaining-budget probe; lets the executor capture
    /// `instructions_executed()` BEFORE returning the abort path so that
    /// `MoveExecutionOutput.gas_used` reflects work done up to the failure
    /// point (G1 cross-solver determinism).
    pub fn instructions_remaining(&self) -> u64 {
        self.max_instructions
            .saturating_sub(self.instructions_executed)
    }
}

// ════════════════════════════════════════════════════════════════════════
// B6c · PTB outer-tier overhead table
// ════════════════════════════════════════════════════════════════════════
//
// These constants are charged on top of the existing per-instruction inner
// tier (see `InstructionCountGasMeter::charge_*` impls above) so a malicious
// or oversized PTB cannot pin a validator. Values are ~10× the inner
// `charge_call` cost to reflect per-Command argument resolution + result
// table maintenance overhead.
//
// Native-function rows (e.g. `bcs::to_bytes`) are intentionally **omitted**
// in v1; they will be added when B3 (natives) lands and finalizes the list.

/// PTB-only minimum gas budget (charged at PTB start, NOT applied to the
/// legacy `OperationType::MoveCall` path — see B6c design §4.5).
pub const MIN_GAS_PTB: u64 = 1_000;

/// PTB-only maximum gas budget; submissions above this are rejected at the
/// validator entry (`network/move_handler.rs`).
pub const MAX_GAS_BUDGET: u64 = 50_000_000;

/// Per-command outer-tier surcharge table. Values represent the cost of
/// the executor-side bookkeeping for one command, NOT the cost of any
/// bytecode it triggers (that is accounted for separately by the inner
/// per-instruction meter).
#[derive(Debug, Clone, Copy)]
pub struct PtbOverhead {
    pub move_call: u64,
    pub split_coins: u64,
    pub merge_coins: u64,
    pub transfer_objects: u64,
    pub publish: u64,
    pub make_move_vec: u64,
    /// Charged once per state-key written by the PTB (covers all G11
    /// prefixes: `oid:`, `mod:`, `pkg:`, `user:`, `solver:`, `event:`,
    /// `linkage:latest:`, `linkage:hist:`).
    pub storage_write_per_key: u64,
    /// Charged once per state-key read into the executor (input objects
    /// + dynamic-field preload entries).
    pub storage_read_per_key: u64,
    // ── B5 package upgrade gas (design.md §9.1, R1g baseline) ─────────
    /// Per module in an upgrade bundle. Charged at the start of
    /// `lower_upgrade_inline` / `execute_move_upgrade` after deserialization.
    pub relink_per_module: u64,
    /// Per `address_identifiers` slot scanned inside `relink_module`.
    pub relink_per_address_identifier: u64,
    /// Per `StructHandle` compared inside `check_upgrade_compat`.
    pub compat_check_per_struct: u64,
    /// Per `FunctionHandle` signature compared inside `check_upgrade_compat`.
    pub compat_check_per_function: u64,
    /// Per byte of relinked bytecode handed to `publish_module_bundle`.
    pub publish_module_bundle_per_module_byte: u64,
    /// Charged once per `Command::Publish` for the implicit
    /// `make_upgrade_cap` + `transfer::public_transfer<UpgradeCap>` calls
    /// (design.md §4.5 v0 sequence).
    pub publish_cap_creation: u64,
}

/// Baseline used by upgrade-related charges. Equal to Sui's
/// `MOVE_BYTECODE_VERIFY_BASE` order of magnitude; precise values calibrated
/// in R2 against the existing publish charge.
pub const MOVE_BYTECODE_VERIFY_BASE: u64 = 5_000;

pub const PTB_OVERHEAD_TABLE: PtbOverhead = PtbOverhead {
    move_call: 100,
    split_coins: 50,
    merge_coins: 50,
    transfer_objects: 20,
    publish: 5_000,
    make_move_vec: 10,
    storage_write_per_key: 100,
    storage_read_per_key: 50,
    relink_per_module: MOVE_BYTECODE_VERIFY_BASE,
    relink_per_address_identifier: 10,
    compat_check_per_struct: MOVE_BYTECODE_VERIFY_BASE / 4,
    compat_check_per_function: MOVE_BYTECODE_VERIFY_BASE / 4,
    publish_module_bundle_per_module_byte: 1,
    publish_cap_creation: MOVE_BYTECODE_VERIFY_BASE / 8,
};

/// Compute the outer-tier overhead cost for a single PTB command. The
/// executor charges this on the shared `InstructionCountGasMeter` BEFORE
/// dispatching the command (B6c design §5 step 3a).
pub fn ptb_overhead_cost(cmd: &setu_types::ptb::Command) -> u64 {
    use setu_types::ptb::Command;
    match cmd {
        Command::MoveCall(_) => PTB_OVERHEAD_TABLE.move_call,
        Command::SplitCoins(_, amounts) => {
            // One outer charge per resulting coin so a 1×1024 SplitCoins
            // cannot dodge per-element overhead.
            PTB_OVERHEAD_TABLE
                .split_coins
                .saturating_mul(amounts.len() as u64)
        }
        Command::MergeCoins(_, sources) => PTB_OVERHEAD_TABLE
            .merge_coins
            .saturating_mul(sources.len() as u64),
        Command::TransferObjects(objs, _) => PTB_OVERHEAD_TABLE
            .transfer_objects
            .saturating_mul(objs.len() as u64),
        Command::Publish { modules, .. } => {
            // Charge the per-bundle baseline plus 5 000 per module
            // bytecode-verify; clamping `len().max(1)` so that an empty
            // module list still pays the bundle baseline (anti-DoS).
            PTB_OVERHEAD_TABLE
                .publish
                .saturating_mul((modules.len() as u64).max(1))
        }
        Command::MakeMoveVec { args, .. } => PTB_OVERHEAD_TABLE
            .make_move_vec
            .saturating_mul(args.len() as u64),
        Command::Upgrade { modules, .. } => {
            // Upgrade pays the publish baseline plus per-module relink and
            // per-byte bundle charges. Compat-check tier charges happen
            // inside the helper itself.
            let base = PTB_OVERHEAD_TABLE
                .publish
                .saturating_mul((modules.len() as u64).max(1));
            let relink = PTB_OVERHEAD_TABLE
                .relink_per_module
                .saturating_mul(modules.len() as u64);
            let bytes: u64 = modules.iter().map(|m| m.len() as u64).sum();
            let publish = PTB_OVERHEAD_TABLE
                .publish_module_bundle_per_module_byte
                .saturating_mul(bytes);
            base.saturating_add(relink).saturating_add(publish)
        }
    }
}

impl GasMeter for InstructionCountGasMeter {
    // (1) Simple instruction: 1 unit
    fn charge_simple_instr(&mut self, _instr: SimpleInstruction) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (2) Pop: free
    fn charge_pop(&mut self, _popped_val: impl ValueView) -> PartialVMResult<()> {
        Ok(())
    }

    // (3) Function call: 10 units
    fn charge_call(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        self.charge(10)
    }

    // (4) Generic function call: 10 units (replaces old charge_call_native)
    fn charge_call_generic(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        self.charge(10)
    }

    // (5) Load constant: 1 unit
    fn charge_ld_const(&mut self, _size: NumBytes) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (6) Constant after deserialization: free (already charged in ld_const)
    fn charge_ld_const_after_deserialization(
        &mut self,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        Ok(())
    }

    // (7) Copy local: 1 unit (Phase 1 flat cost; legacy_abstract_memory_size not available)
    fn charge_copy_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (8) Move local: 1 unit
    fn charge_move_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (9) Store local: 1 unit
    fn charge_store_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (10) Pack struct: max(1, fields)
    fn charge_pack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(std::cmp::max(1, args.len() as u64))
    }

    // (11) Unpack struct: max(1, fields)
    fn charge_unpack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(std::cmp::max(1, args.len() as u64))
    }

    // (12) Enum variant switch: 1 unit
    fn charge_variant_switch(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (13) Read reference: 1 unit
    fn charge_read_ref(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (14) Write reference: 1 unit
    fn charge_write_ref(
        &mut self,
        _new_val: impl ValueView,
        _old_val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (15) Equality: 1 unit (Phase 1 flat; legacy_abstract_memory_size not available)
    fn charge_eq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (16) Inequality: 1 unit
    fn charge_neq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (17) Vec pack: max(1, elems)
    fn charge_vec_pack<'a>(
        &mut self,
        _ty: impl TypeView + 'a,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(std::cmp::max(1, args.len() as u64))
    }

    // (18) Vec len: 1 unit
    fn charge_vec_len(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (19) Vec borrow: 1 unit
    fn charge_vec_borrow(
        &mut self,
        _is_mut: bool,
        _ty: impl TypeView,
        _is_success: bool,
    ) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (20) Vec push back: 2 units
    fn charge_vec_push_back(
        &mut self,
        _ty: impl TypeView,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(2)
    }

    // (21) Vec pop back: 1 unit
    fn charge_vec_pop_back(
        &mut self,
        _ty: impl TypeView,
        _val: Option<impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (22) Vec unpack: max(1, num_elements) — 3-param signature (v3.8 R8-2)
    fn charge_vec_unpack(
        &mut self,
        _ty: impl TypeView,
        expect_num_elements: NumArgs,
        _elems: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(std::cmp::max(1, u64::from(expect_num_elements)))
    }

    // (23) Vec swap: 2 units
    fn charge_vec_swap(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        self.charge(2)
    }

    // (24) Native function gas: amount reported by native
    // v3.8: u64::from(amount) — not into_inner()
    fn charge_native_function(
        &mut self,
        amount: InternalGas,
        _ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        self.charge(u64::from(amount))
    }

    // (25) Pre-native check: free
    fn charge_native_function_before_execution(
        &mut self,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        Ok(())
    }

    // (26) Drop frame: 1 unit
    fn charge_drop_frame(
        &mut self,
        _locals: impl Iterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(1)
    }

    // (27) Remaining gas
    fn remaining_gas(&self) -> InternalGas {
        InternalGas::new(
            self.max_instructions
                .saturating_sub(self.instructions_executed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_meter_new() {
        let meter = InstructionCountGasMeter::new(1000);
        assert_eq!(meter.instructions_executed(), 0);
        assert_eq!(u64::from(meter.remaining_gas()), 1000);
    }

    #[test]
    fn test_gas_meter_charge() {
        let mut meter = InstructionCountGasMeter::new(100);
        assert!(meter.charge(50).is_ok());
        assert_eq!(meter.instructions_executed(), 50);
        assert_eq!(u64::from(meter.remaining_gas()), 50);
    }

    #[test]
    fn test_gas_meter_out_of_gas() {
        let mut meter = InstructionCountGasMeter::new(10);
        assert!(meter.charge(11).is_err());
    }

    #[test]
    fn test_gas_meter_exact_limit() {
        let mut meter = InstructionCountGasMeter::new(10);
        assert!(meter.charge(10).is_ok());
        assert_eq!(u64::from(meter.remaining_gas()), 0);
        assert!(meter.charge(1).is_err());
    }

    #[test]
    fn test_remaining_gas_arithmetic() {
        let meter = InstructionCountGasMeter::new(0);
        assert_eq!(u64::from(meter.remaining_gas()), 0);
    }
}
