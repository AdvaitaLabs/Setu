//! PTB execution engine — runs N `Command`s of a `ProgrammableTransaction`
//! inside ONE Move VM `Session`.
//!
//! Design source: `docs/feat/move-vm-phase9-ptb-exec/design.md`.
//!
//! # Phase 3a scope
//!
//! This file currently contains only the *leaf* primitives:
//!
//! - [`ArgumentSlot`] — canonical identifier for a consumable PTB slot
//! - [`PtbContext`] — cross-command result / borrow-stack tracking
//! - [`coin_inner_type_from_tag`] — `Coin<T>` → `T` extractor (§4.8)
//!
//! The actual `execute_ptb(...)` driver and per-command lowerings land in
//! Phase 3b–3f. Until then, the public surface of this module is empty;
//! everything is `pub(crate)` for the sibling modules (`engine`, `hybrid`)
//! that will wire the driver in.
//!
//! # Invariants enforced here
//!
//! - **§4.2**: each slot carries `(bytes, runtime_layout, Option<TypeTag>)`.
//!   `None` for `TypeTag` means "untracked" (e.g. result of a generic `MoveCall`)
//!   and any Coin command receiving such a slot aborts deterministically.
//! - **§4.6**: an `Argument` slot can be **consumed** at most once across the
//!   entire PTB. `consume()` is a one-shot take that errors on the second call.
//! - **§4.8**: `coin_inner_type_from_tag` matches the canonical setu-framework
//!   `0x1::coin::Coin<T>` shape and never fabricates a fallback `T`.

use std::collections::BTreeSet;
use std::fmt;

use move_binary_format::file_format::AbilitySet;
use move_core_types::language_storage::TypeTag;
use move_core_types::runtime_value::MoveTypeLayout;

use setu_runtime::error::RuntimeError;
use setu_types::ptb::Argument;

// ─────────────────────────────────────────────────────────────────────────────
// setu-framework constants — all Coin commands lower to functions in this
// address::module namespace. Keep in sync with `setu-framework/Move.toml`
// (named address `setu = "0x1"`).
// ─────────────────────────────────────────────────────────────────────────────

/// `0x1` — setu-framework named-address.
pub(crate) const SETU_FRAMEWORK_ADDR: move_core_types::account_address::AccountAddress =
    move_core_types::account_address::AccountAddress::ONE;
/// Move module name for `Coin`/`TreasuryCap`/`CoinMetadata`.
pub(crate) const COIN_MODULE: &str = "coin";
/// Move module name for `UpgradeCap`/`UpgradeTicket`/`UpgradeReceipt`.
/// Engine PTB lowering for `Command::Publish` / `Command::Upgrade` issues
/// MoveCalls into `0x1::package::{make_upgrade_cap, consume_ticket, make_receipt}`.
pub(crate) const PACKAGE_MODULE: &str = "package";
/// Type-tag suffix that identifies an UpgradeCap state-change in the
/// engine's per-PTB `state_changes` output. Used by the validator handler
/// (`network::move_handler::submit_move_ptb`) to surface fresh cap UIDs
/// on `MovePtbResponse.cap_ids` — see design.md §15.3.
pub const UPGRADE_CAP_TYPE_TAG_SUFFIX: &str = "::package::UpgradeCap";

// ─────────────────────────────────────────────────────────────────────────────
// ArgumentSlot — canonical key into the consumed-set
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical, ordered identifier for a consumable PTB slot.
///
/// `Argument::Result(c)` and `Argument::NestedResult(c, 0)` BOTH map to
/// `CmdResult { cmd: c, idx: 0 }` — they refer to the same physical slot
/// (Move semantics: a single-tuple result is also accessible as element 0
/// of a tuple). Canonicalization happens in [`ArgumentSlot::from_argument`].
///
/// `Argument::GasCoin` is intentionally NOT representable here. B6b does not
/// implement gas-coin semantics (those land in B6c), so any `Argument::GasCoin`
/// reaching `consume()` aborts via `from_argument(...) -> None`. See §4.6 +
/// design.md `Risk register: per-command rollback granularity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ArgumentSlot {
    Input(u16),
    CmdResult { cmd: u16, idx: u16 },
}

impl ArgumentSlot {
    /// Map an `Argument` to its canonical slot. Returns `None` for `GasCoin`
    /// (not yet supported in B6b).
    pub(crate) fn from_argument(arg: &Argument) -> Option<Self> {
        match arg {
            Argument::GasCoin => None,
            Argument::Input(i) => Some(ArgumentSlot::Input(*i)),
            Argument::Result(c) => Some(ArgumentSlot::CmdResult { cmd: *c, idx: 0 }),
            Argument::NestedResult(c, j) => Some(ArgumentSlot::CmdResult { cmd: *c, idx: *j }),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PtbContext — per-PTB cross-command state
// ─────────────────────────────────────────────────────────────────────────────

/// One entry in the result/input table — a serialized Move value with its
/// runtime layout, optionally tracked TypeTag, and optionally tracked abilities.
///
/// The TypeTag is `None` for slots whose type cannot be represented cheaply as
/// a concrete `TypeTag` at the PTB layer (e.g. generic `MoveCall` results).
/// Generic `MoveCall` results still carry `abilities` from the instantiated VM
/// return `Type`, so the hot-potato sweep can enforce no-drop/no-key values
/// even when no `TypeTag` is available. Coin-command lowerings (`SplitCoins`,
/// `MergeCoins`, `TransferObjects`) still require the `TypeTag` and abort with
/// `PtbInvalidCoinLayout` when it is `None`.
///
/// The `layout` field is also `Option`-al: PTB inputs (`CallArg::Pure(bytes)`)
/// are passed verbatim to the Move VM which deserializes them against the
/// callee's parameter type — we have no layout for them at PtbContext
/// construction time. Layouts are populated for `MoveCall` return values
/// (the VM gives us `MoveTypeLayout` per return slot) and for serialized
/// object inputs (where the layout matches the on-chain envelope's struct).
#[derive(Clone)]
pub(crate) struct Slot {
    pub bytes: Vec<u8>,
    /// Move VM type layout. Populated for MoveCall return slots and for
    /// SplitCoins outputs (where the VM gives us the canonical layout).
    /// Currently only used to establish provenance — will become readable
    /// when B6c gas accounting needs layout-aware byte-cost calculation
    /// and when later phases add precise return-type tracking. See design
    /// F13 (Slot triple).
    #[allow(dead_code)]
    pub layout: Option<MoveTypeLayout>,
    pub type_tag: Option<TypeTag>,
    pub abilities: Option<AbilitySet>,
}

impl fmt::Debug for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slot")
            .field("bytes", &self.bytes)
            .field("layout", &self.layout)
            .field("type_tag", &self.type_tag)
            .field(
                "abilities",
                &self.abilities.map(|abilities| {
                    (
                        abilities.has_copy(),
                        abilities.has_drop(),
                        abilities.has_store(),
                        abilities.has_key(),
                    )
                }),
            )
            .finish()
    }
}

/// PTB-scoped state carried across commands.
///
/// **Lifetime**: created at the start of `execute_ptb`, dropped at session
/// finalize. The `consumed` set is purely advisory — Move VM's own borrow
/// checker is the authoritative enforcer for `&mut` aliasing inside a single
/// function call. `consumed` adds the cross-command linear-type rule that
/// Move's static checker cannot see across PTB-level Argument indirections.
#[derive(Debug)]
pub(crate) struct PtbContext {
    /// `results[cmd_idx][result_idx]`. Outer index = command index, inner =
    /// position in that command's tuple-result.
    results: Vec<Vec<Slot>>,
    /// Pre-resolved PTB inputs (Pure + Object refs). Indexed by the wire-level
    /// `Argument::Input(i)`.
    inputs: Vec<Slot>,
    /// Linear-type tracking. See §4.6.
    consumed: BTreeSet<ArgumentSlot>,
}

impl PtbContext {
    /// Build with the pre-resolved inputs and an empty result table sized
    /// for `n_commands` (each entry initially an empty `Vec`, populated by
    /// [`Self::record_result`] in command-execution order).
    pub(crate) fn new(inputs: Vec<Slot>, n_commands: usize) -> Self {
        Self {
            results: vec![Vec::new(); n_commands],
            inputs,
            consumed: BTreeSet::new(),
        }
    }

    /// Resolve an `Argument` to its underlying slot **without** marking it
    /// consumed. Used for borrow (`&` / `&mut`) reads at lowering time.
    ///
    /// Errors:
    /// - [`RuntimeError::PtbArgumentOutOfBounds`] for any index past the
    ///   resolved input vec or beyond the current command's recorded results.
    /// - [`RuntimeError::PtbArgumentOutOfBounds`] for `GasCoin` (not yet
    ///   supported in B6b — see [`ArgumentSlot::from_argument`] doc).
    pub(crate) fn resolve(&self, arg: &Argument) -> Result<&Slot, RuntimeError> {
        if let Some(slot_id) = ArgumentSlot::from_argument(arg) {
            if self.consumed.contains(&slot_id) {
                return Err(RuntimeError::PtbArgumentAlreadyConsumed(format!(
                    "resolve on consumed slot: {:?}",
                    slot_id
                )));
            }
        }
        match arg {
            Argument::GasCoin => Err(RuntimeError::PtbArgumentOutOfBounds(
                "GasCoin not supported in B6b".to_string(),
            )),
            Argument::Input(i) => self.inputs.get(*i as usize).ok_or_else(|| {
                RuntimeError::PtbArgumentOutOfBounds(format!(
                    "Input({}) of {}",
                    i,
                    self.inputs.len()
                ))
            }),
            Argument::Result(c) => self.lookup_cmd(*c, 0),
            Argument::NestedResult(c, j) => self.lookup_cmd(*c, *j),
        }
    }

    fn lookup_cmd(&self, cmd: u16, idx: u16) -> Result<&Slot, RuntimeError> {
        let row = self.results.get(cmd as usize).ok_or_else(|| {
            RuntimeError::PtbArgumentOutOfBounds(format!(
                "Result references cmd {} but only {} commands recorded",
                cmd,
                self.results.len()
            ))
        })?;
        row.get(idx as usize).ok_or_else(|| {
            RuntimeError::PtbArgumentOutOfBounds(format!(
                "NestedResult({},{}) but cmd {} produced {} values",
                cmd,
                idx,
                cmd,
                row.len()
            ))
        })
    }

    /// Mark the slot consumed (linear-type) and return a clone of the slot
    /// payload. Errors if already consumed (§4.6).
    ///
    /// Cloning keeps the implementation simple — consumed slots could in
    /// principle be moved out of the inputs/results vectors, but doing so
    /// would invalidate later `&self.inputs[i]` borrows used by adjacent
    /// `resolve()` calls in the same command. The clone cost is negligible
    /// for typical PTBs (≤1024 commands × small payloads).
    pub(crate) fn consume(&mut self, arg: &Argument) -> Result<Slot, RuntimeError> {
        let slot_id = ArgumentSlot::from_argument(arg).ok_or_else(|| {
            RuntimeError::PtbArgumentOutOfBounds("GasCoin not supported in B6b".to_string())
        })?;
        if self.consumed.contains(&slot_id) {
            return Err(RuntimeError::PtbArgumentAlreadyConsumed(format!(
                "{:?}",
                slot_id
            )));
        }
        // Resolve first (validates index) THEN mark consumed. Order matters:
        // a resolve-error MUST NOT poison the consumed set.
        let payload = self.resolve(arg)?.clone();
        self.consumed.insert(slot_id);
        Ok(payload)
    }

    /// Record cmd[`cmd_idx`]'s tuple-result. Must be called exactly once per
    /// command, in command order (idx == current results.len()).
    pub(crate) fn record_result(&mut self, cmd_idx: usize, slots: Vec<Slot>) {
        debug_assert!(
            cmd_idx < self.results.len() && self.results[cmd_idx].is_empty(),
            "record_result called out-of-order or twice for cmd {}",
            cmd_idx
        );
        self.results[cmd_idx] = slots;
    }

    /// Read-only access to the result table. Used by the engine's end-of-PTB
    /// hot-potato sweep (engine.rs::execute_ptb_body) to walk every result
    /// slot and check its type's `drop` ability against the `consumed` set.
    pub(crate) fn results_for_sweep(&self) -> &[Vec<Slot>] {
        &self.results
    }

    /// Whether the given canonical slot has been consumed via `consume()`.
    /// Used by the engine's end-of-PTB hot-potato sweep to skip slots that
    /// were properly moved out (e.g. an `UpgradeReceipt` consumed by a
    /// trailing `commit_upgrade` MoveCall).
    pub(crate) fn is_consumed(&self, slot: ArgumentSlot) -> bool {
        self.consumed.contains(&slot)
    }

    /// Phase 3d — write-back path for `&mut` argument mutations.
    ///
    /// SplitCoins / MergeCoins lower to Move calls that take `&mut Coin<T>`;
    /// after each call we must overwrite the slot's payload with the bytes
    /// returned by the VM in `mutable_reference_outputs`. `mutate_slot`
    /// preserves `layout` and `type_tag` since `&mut` mutations don't change
    /// the type, only the contents.
    ///
    /// Errors on `GasCoin` (not yet routed through PtbContext) and on any
    /// out-of-range / unknown-cmd argument.
    pub(crate) fn mutate_slot(
        &mut self,
        arg: &Argument,
        new_bytes: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let key = ArgumentSlot::from_argument(arg).ok_or_else(|| {
            RuntimeError::InvalidTransaction(
                "mutate_slot on GasCoin not supported in B6b".to_string(),
            )
        })?;
        // Linear-type symmetry: `consume()` rejects a re-take of a consumed
        // slot; `mutate_slot()` must reject a write-back to one too.
        // Without this, a buggy lowering could silently overwrite the bytes
        // of a slot that has already been moved out (e.g. a source Coin
        // both consumed by MergeCoins and re-mutated). See
        // `docs/feat/move-vm-phase9-ptb-exec/review-log.md` R2-ISSUE-3.
        if self.consumed.contains(&key) {
            return Err(RuntimeError::PtbArgumentAlreadyConsumed(format!(
                "mutate_slot on consumed slot: {:?}",
                key
            )));
        }
        match key {
            ArgumentSlot::Input(i) => {
                let inputs_len = self.inputs.len();
                let slot = self.inputs.get_mut(i as usize).ok_or_else(|| {
                    RuntimeError::PtbArgumentOutOfBounds(format!(
                        "Input({i}) for mutate_slot (inputs.len()={inputs_len})"
                    ))
                })?;
                slot.bytes = new_bytes;
                Ok(())
            }
            ArgumentSlot::CmdResult { cmd, idx } => {
                let row = self.results.get_mut(cmd as usize).ok_or_else(|| {
                    RuntimeError::PtbArgumentOutOfBounds(format!(
                        "Result({cmd}) cmd row missing for mutate_slot"
                    ))
                })?;
                let slot = row.get_mut(idx as usize).ok_or_else(|| {
                    RuntimeError::PtbArgumentOutOfBounds(format!(
                        "NestedResult({cmd},{idx}) missing for mutate_slot"
                    ))
                })?;
                slot.bytes = new_bytes;
                Ok(())
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// §4.8 — TypeTag-based Coin<T> recognition
// ─────────────────────────────────────────────────────────────────────────────

/// Extract `T` from a `0x1::coin::Coin<T>` `TypeTag`. Returns `None` for any
/// other shape (including `Vector<u8>`, `u64`, malformed `Coin` with wrong
/// arity, or `Coin` from a non-setu address).
///
/// Why not panic on malformed Coin: the wrapping lowering (e.g.
/// `lower_split_coins`) needs to produce a typed `RuntimeError::PtbInvalidCoinLayout`
/// with context, not a generic `unwrap()` panic. So this fn uses `Option`
/// and the caller decides the abort code.
pub(crate) fn coin_inner_type_from_tag(tag: &TypeTag) -> Option<TypeTag> {
    match tag {
        TypeTag::Struct(st)
            if st.address == SETU_FRAMEWORK_ADDR
                && st.module.as_str() == COIN_MODULE
                && st.name.as_str() == "Coin"
                && st.type_params.len() == 1 =>
        {
            Some(st.type_params[0].clone())
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (Phase 3a — leaf primitives only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::StructTag;

    // ── Test fixtures ────────────────────────────────────────────────────────

    /// Build a minimal `Slot` with the given bytes and an `U64` runtime
    /// layout. Default TypeTag: `None` unless the helper variant is used.
    fn slot(bytes: &[u8]) -> Slot {
        Slot {
            bytes: bytes.to_vec(),
            layout: Some(MoveTypeLayout::U64),
            type_tag: None,
            abilities: None,
        }
    }

    fn slot_with_tag(bytes: &[u8], tag: TypeTag) -> Slot {
        Slot {
            bytes: bytes.to_vec(),
            layout: Some(MoveTypeLayout::U64),
            type_tag: Some(tag),
            abilities: None,
        }
    }

    fn slot_with_abilities(bytes: &[u8], abilities: AbilitySet) -> Slot {
        Slot {
            bytes: bytes.to_vec(),
            layout: Some(MoveTypeLayout::U64),
            type_tag: None,
            abilities: Some(abilities),
        }
    }

    /// Build `0x1::coin::Coin<inner>` TypeTag.
    fn coin_tag(inner: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: SETU_FRAMEWORK_ADDR,
            module: Identifier::new(COIN_MODULE).unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![inner],
        }))
    }

    // ── U1 ── PtbContext::resolve(Argument::Input(i)) round-trip ───────────

    #[test]
    fn u1_resolve_input_ok() {
        let ctx = PtbContext::new(vec![slot(&[1, 2, 3]), slot(&[4, 5])], 0);
        let s = ctx.resolve(&Argument::Input(0)).expect("Input(0) resolves");
        assert_eq!(s.bytes, vec![1, 2, 3]);
        let s = ctx.resolve(&Argument::Input(1)).expect("Input(1) resolves");
        assert_eq!(s.bytes, vec![4, 5]);
    }

    // ── U2 ── out-of-bounds Input ──────────────────────────────────────────

    #[test]
    fn u2_resolve_out_of_bounds() {
        let ctx = PtbContext::new(vec![slot(&[1, 2, 3]), slot(&[4, 5])], 0);
        let err = ctx
            .resolve(&Argument::Input(99))
            .expect_err("Input(99) on 2-input PTB rejects");
        assert!(matches!(err, RuntimeError::PtbArgumentOutOfBounds(_)));
        match err {
            RuntimeError::PtbArgumentOutOfBounds(msg) => {
                assert!(msg.contains("Input(99)"), "msg lacks index: {msg}");
                assert!(msg.contains("of 2"), "msg lacks bound: {msg}");
            }
            _ => unreachable!(),
        }
    }

    // ── U3 ── consume marks slot; second consume rejects ───────────────────

    #[test]
    fn u3_consume_marks_slot() {
        let mut ctx = PtbContext::new(vec![slot(&[7, 8])], 0);
        let payload = ctx.consume(&Argument::Input(0)).expect("first consume ok");
        assert_eq!(payload.bytes, vec![7, 8]);
        let err = ctx
            .consume(&Argument::Input(0))
            .expect_err("second consume rejects");
        assert!(matches!(err, RuntimeError::PtbArgumentAlreadyConsumed(_)));
    }

    // ── U4 ── Result and Input slots tracked independently ─────────────────

    #[test]
    fn u4_consume_result_slot_independent_from_input() {
        let mut ctx = PtbContext::new(vec![slot(&[1])], 1);
        ctx.record_result(0, vec![slot(&[42])]);

        // Both consumable in any order; neither blocks the other.
        let _ = ctx.consume(&Argument::Input(0)).expect("Input consume ok");
        let _ = ctx
            .consume(&Argument::Result(0))
            .expect("Result consume ok");

        // Each individually now blocked.
        assert!(matches!(
            ctx.consume(&Argument::Input(0)),
            Err(RuntimeError::PtbArgumentAlreadyConsumed(_))
        ));
        assert!(matches!(
            ctx.consume(&Argument::Result(0)),
            Err(RuntimeError::PtbArgumentAlreadyConsumed(_))
        ));
    }

    // ── U4b ── Result(c) and NestedResult(c, 0) canonicalize to same slot ──

    #[test]
    fn u4b_result_and_nested_zero_canonicalize() {
        let mut ctx = PtbContext::new(vec![], 1);
        ctx.record_result(0, vec![slot(&[9])]);

        // Consume via Result(0); NestedResult(0, 0) MUST then be blocked.
        ctx.consume(&Argument::Result(0)).expect("first ok");
        let err = ctx
            .consume(&Argument::NestedResult(0, 0))
            .expect_err("NestedResult(0,0) is the same slot");
        assert!(matches!(err, RuntimeError::PtbArgumentAlreadyConsumed(_)));
    }

    // ── U5 ── forward-ref Result rejected at resolve time ──────────────────

    #[test]
    fn u5_resolve_result_forward_ref_rejected() {
        // 3-command PTB; only cmd[0] has recorded results so far.
        let mut ctx = PtbContext::new(vec![], 3);
        ctx.record_result(0, vec![slot(&[1])]);

        // cmd[1] hasn't run yet → Result(2) referencing the future cmd[2] errors.
        // (Wire-level validate_wire already catches this; PtbContext is the
        // last-line defense.)
        let err = ctx
            .resolve(&Argument::Result(2))
            .expect_err("forward ref to empty cmd row rejects");
        assert!(matches!(err, RuntimeError::PtbArgumentOutOfBounds(_)));
        match err {
            RuntimeError::PtbArgumentOutOfBounds(msg) => {
                // The message says "produced 0 values" because cmd 2's result
                // row IS allocated (we sized 3) but empty.
                assert!(msg.contains("produced 0 values"), "msg={msg}");
            }
            _ => unreachable!(),
        }
    }

    // ── U5b ── Result referencing entirely-undeclared cmd index ────────────

    #[test]
    fn u5b_resolve_result_unknown_cmd() {
        let ctx = PtbContext::new(vec![], 1);
        // Only 1 command declared; Result(5) is past the end.
        let err = ctx.resolve(&Argument::Result(5)).expect_err("unknown cmd");
        match err {
            RuntimeError::PtbArgumentOutOfBounds(msg) => {
                assert!(msg.contains("only 1 commands"), "msg={msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    // ── U6 ── record_result preserves index alignment ──────────────────────

    #[test]
    fn u6_record_result_ordering() {
        let mut ctx = PtbContext::new(vec![], 3);
        ctx.record_result(0, vec![slot(&[1]), slot(&[2])]);
        ctx.record_result(1, vec![]);
        ctx.record_result(2, vec![slot(&[3])]);

        assert_eq!(
            ctx.resolve(&Argument::NestedResult(0, 0)).unwrap().bytes,
            vec![1]
        );
        assert_eq!(
            ctx.resolve(&Argument::NestedResult(0, 1)).unwrap().bytes,
            vec![2]
        );
        assert!(matches!(
            ctx.resolve(&Argument::Result(1)),
            Err(RuntimeError::PtbArgumentOutOfBounds(_))
        ));
        assert_eq!(
            ctx.resolve(&Argument::NestedResult(2, 0)).unwrap().bytes,
            vec![3]
        );
    }

    // ── U7 ── coin_inner_type_from_tag on Coin<u64> ────────────────────────

    #[test]
    fn u7_coin_inner_type_walk_ok() {
        let tag = coin_tag(TypeTag::U64);
        let inner = coin_inner_type_from_tag(&tag).expect("Coin<u64> matches");
        assert!(matches!(inner, TypeTag::U64));
    }

    // ── U8 ── non-Coin TypeTag returns None ───────────────────────────────

    #[test]
    fn u8_coin_inner_type_walk_non_coin() {
        // Vector<u8> is not Coin.
        let tag = TypeTag::Vector(Box::new(TypeTag::U8));
        assert!(coin_inner_type_from_tag(&tag).is_none());
        // Plain u64 is not Coin.
        assert!(coin_inner_type_from_tag(&TypeTag::U64).is_none());
        // address is not Coin.
        assert!(coin_inner_type_from_tag(&TypeTag::Address).is_none());
    }

    // ── U9 ── Coin-shaped from wrong address / wrong arity rejected ───────

    #[test]
    fn u9_coin_inner_type_walk_malformed_coin() {
        // Right module name, wrong address.
        let wrong_addr = TypeTag::Struct(Box::new(StructTag {
            address: move_core_types::account_address::AccountAddress::TWO,
            module: Identifier::new(COIN_MODULE).unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![TypeTag::U64],
        }));
        assert!(coin_inner_type_from_tag(&wrong_addr).is_none());

        // Right address+module, no type params.
        let no_params = TypeTag::Struct(Box::new(StructTag {
            address: SETU_FRAMEWORK_ADDR,
            module: Identifier::new(COIN_MODULE).unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![],
        }));
        assert!(coin_inner_type_from_tag(&no_params).is_none());

        // Right address+module, two type params (wrong arity).
        let two_params = TypeTag::Struct(Box::new(StructTag {
            address: SETU_FRAMEWORK_ADDR,
            module: Identifier::new(COIN_MODULE).unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![TypeTag::U64, TypeTag::U64],
        }));
        assert!(coin_inner_type_from_tag(&two_params).is_none());

        // Right shape but module is "treasury_cap", not "coin".
        let wrong_module = TypeTag::Struct(Box::new(StructTag {
            address: SETU_FRAMEWORK_ADDR,
            module: Identifier::new("treasury_cap").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![TypeTag::U64],
        }));
        assert!(coin_inner_type_from_tag(&wrong_module).is_none());
    }

    // ── U10 ── target/source TypeTag mismatch detection ───────────────────
    //
    // Note: this test exercises the *helper-level* check that callers
    // (lower_merge_coins, lower_transfer_objects) must perform with
    // PartialEq. Doing the check here keeps the eventual Phase 3d/3e
    // implementation honest — if those lowerings forget to compare tags,
    // the I6/I7 integration tests will catch it; this is the leaf-level
    // anchor.

    #[test]
    fn u10_typetag_mismatch_is_strict_partialeq() {
        let a = coin_tag(TypeTag::U64);
        let b = coin_tag(TypeTag::U128);
        assert_ne!(a, b, "Coin<u64> != Coin<u128>");
        let c = coin_tag(TypeTag::U64);
        assert_eq!(a, c, "same shape ⇒ equal");

        // And the inner-extractor agrees.
        let inner_a = coin_inner_type_from_tag(&a).unwrap();
        let inner_b = coin_inner_type_from_tag(&b).unwrap();
        assert_ne!(inner_a, inner_b);
    }

    // ── U11 ── all 5 new RuntimeError variants exist + carry context ──────

    #[test]
    fn u11_runtime_error_variants_carry_context() {
        let cases: Vec<(RuntimeError, &str)> = vec![
            (
                RuntimeError::PtbArgumentOutOfBounds("ctx-1".into()),
                "out of bounds",
            ),
            (
                RuntimeError::PtbArgumentAlreadyConsumed("ctx-2".into()),
                "already consumed",
            ),
            (
                RuntimeError::PtbInvalidCoinLayout("ctx-3".into()),
                "invalid coin layout",
            ),
            (
                RuntimeError::PtbUnsupportedTransferType("ctx-4".into()),
                "unsupported transfer type",
            ),
            (
                RuntimeError::PtbInvalidTypeTag("ctx-5".into()),
                "invalid type tag",
            ),
        ];
        for (err, fragment) in cases {
            let s = format!("{err}").to_lowercase();
            assert!(
                s.contains(fragment),
                "Display for {err:?} missing '{fragment}': '{s}'"
            );
            // Each carries its `ctx-N` payload through `Display`.
            assert!(s.contains("ctx-"), "context lost in '{s}'");
        }
    }

    // ── Bonus: from_argument canonicalization edge cases ───────────────────

    #[test]
    fn argument_slot_canonicalization() {
        assert_eq!(
            ArgumentSlot::from_argument(&Argument::Result(7)),
            Some(ArgumentSlot::CmdResult { cmd: 7, idx: 0 })
        );
        assert_eq!(
            ArgumentSlot::from_argument(&Argument::NestedResult(7, 0)),
            Some(ArgumentSlot::CmdResult { cmd: 7, idx: 0 })
        );
        assert_eq!(
            ArgumentSlot::from_argument(&Argument::NestedResult(7, 3)),
            Some(ArgumentSlot::CmdResult { cmd: 7, idx: 3 })
        );
        assert_eq!(ArgumentSlot::from_argument(&Argument::GasCoin), None);
        assert_eq!(
            ArgumentSlot::from_argument(&Argument::Input(42)),
            Some(ArgumentSlot::Input(42))
        );
    }

    #[test]
    fn slot_with_tag_round_trip() {
        let s = slot_with_tag(&[1, 2], coin_tag(TypeTag::U64));
        assert!(s.type_tag.is_some());
        assert!(coin_inner_type_from_tag(s.type_tag.as_ref().unwrap()).is_some());
    }

    // ── U12 ── mutate_slot updates Input slot bytes in place ───────────────
    #[test]
    fn u12_mutate_slot_updates_input() {
        let mut pctx = PtbContext::new(vec![slot(b"old")], 0);
        pctx.mutate_slot(&Argument::Input(0), b"new".to_vec())
            .unwrap();
        assert_eq!(pctx.resolve(&Argument::Input(0)).unwrap().bytes, b"new");
    }

    // ── U13 ── mutate_slot updates a recorded result slot ───────────────────
    #[test]
    fn u13_mutate_slot_updates_result() {
        let mut pctx = PtbContext::new(vec![], 1);
        pctx.record_result(0, vec![slot(b"a"), slot(b"b")]);
        pctx.mutate_slot(&Argument::NestedResult(0, 1), b"BB".to_vec())
            .unwrap();
        assert_eq!(
            pctx.resolve(&Argument::NestedResult(0, 1)).unwrap().bytes,
            b"BB"
        );
        // Sibling untouched.
        assert_eq!(pctx.resolve(&Argument::Result(0)).unwrap().bytes, b"a");
    }

    // ── U14 ── mutate_slot rejects out-of-range Input ──────────────────────
    #[test]
    fn u14_mutate_slot_out_of_bounds() {
        let mut pctx = PtbContext::new(vec![slot(b"x")], 0);
        let err = pctx.mutate_slot(&Argument::Input(5), vec![]).unwrap_err();
        assert!(matches!(err, RuntimeError::PtbArgumentOutOfBounds(_)));
    }

    // ── U15 ── mutate_slot rejects GasCoin ─────────────────────────────────
    #[test]
    fn u15_mutate_slot_gas_coin_rejected() {
        let mut pctx = PtbContext::new(vec![], 0);
        let err = pctx.mutate_slot(&Argument::GasCoin, vec![]).unwrap_err();
        assert!(matches!(err, RuntimeError::InvalidTransaction(_)));
    }

    // ── U16 ── mutate_slot preserves type_tag (only payload changes) ───────
    #[test]
    fn u16_mutate_slot_preserves_type_tag() {
        let coin = coin_tag(TypeTag::U64);
        let mut pctx = PtbContext::new(vec![slot_with_tag(&[1], coin.clone())], 0);
        pctx.mutate_slot(&Argument::Input(0), vec![9, 9]).unwrap();
        let s = pctx.resolve(&Argument::Input(0)).unwrap();
        assert_eq!(s.bytes, vec![9, 9]);
        assert_eq!(s.type_tag.as_ref(), Some(&coin));
    }

    // ── U16b ── mutate_slot preserves abilities (only payload changes) ─────
    #[test]
    fn u16b_mutate_slot_preserves_abilities() {
        let mut pctx = PtbContext::new(vec![slot_with_abilities(&[1], AbilitySet::PRIMITIVES)], 0);
        pctx.mutate_slot(&Argument::Input(0), vec![9, 9]).unwrap();
        let s = pctx.resolve(&Argument::Input(0)).unwrap();
        assert_eq!(s.bytes, vec![9, 9]);
        assert!(s.abilities == Some(AbilitySet::PRIMITIVES));
    }

    // ── U17 ── mutate_slot rejects writes to consumed slots ────────────────
    //
    // Linear-type symmetry: `consume()` rejects re-take, `mutate_slot()`
    // must reject re-write. Without this guard, a buggy lowering could
    // silently overwrite the bytes of an already-moved-out source coin.
    // See `docs/feat/move-vm-phase9-ptb-exec/review-log.md` R2-ISSUE-3.
    #[test]
    fn u17_mutate_slot_after_consume_rejected() {
        let mut pctx = PtbContext::new(vec![slot(b"x")], 0);
        let _ = pctx.consume(&Argument::Input(0)).unwrap();
        let err = pctx
            .mutate_slot(&Argument::Input(0), b"new".to_vec())
            .unwrap_err();
        assert!(
            matches!(err, RuntimeError::PtbArgumentAlreadyConsumed(_)),
            "expected PtbArgumentAlreadyConsumed, got {err:?}"
        );
    }

    // ── U18 ── resolve rejects reads after move ────────────────────────────
    #[test]
    fn u18_resolve_after_consume_rejected() {
        let mut pctx = PtbContext::new(vec![slot(b"x")], 0);
        let _ = pctx.consume(&Argument::Input(0)).unwrap();
        let err = pctx.resolve(&Argument::Input(0)).unwrap_err();
        assert!(
            matches!(err, RuntimeError::PtbArgumentAlreadyConsumed(_)),
            "expected PtbArgumentAlreadyConsumed, got {err:?}"
        );
    }
}
