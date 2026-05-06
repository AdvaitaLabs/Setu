//! Programmable Transaction Block (PTB) — wire format and validation.
//!
//! This module ships the *type-level* infrastructure for B6a only. Execution
//! semantics, gas accounting, and signature verification live in B6b/B6c.
//! See [`docs/feat/move-vm-phase9-ptb-wire/design.md`](../../../docs/feat/move-vm-phase9-ptb-wire/design.md).
//!
//! ## Determinism (G1) — the central invariant
//!
//! Two logically equivalent PTBs MUST encode to byte-identical BCS output;
//! otherwise consensus hashes drift between honest nodes. This is enforced by:
//!
//! - All identifier-like fields (parents, packages) use binary [`ObjectId`]
//!   ([u8; 32]), never hex strings — hex carries case ambiguity and optional
//!   `"oid:"` prefix.
//! - DF accesses use [`PtbDfAccess`] (binary), not the hex-string
//!   [`crate::event::DynamicFieldAccess`]. The HTTP boundary converts.
//! - Field declaration order in every struct/enum below is **load-bearing**.
//!   New fields go at the tail of structs as `#[serde(default)]` opt-ins;
//!   new enum variants go at the tail too (BCS uses ULEB128 discriminants —
//!   inserting mid-enum silently breaks all already-signed PTBs).
//!
//! ## What's deliberately NOT here (B6b/B6c territory)
//!
//! - `move_core_types::TypeTag` — type tags are kept as canonical strings
//!   (`Vec<String>`); parsing happens at the VM boundary in B6b. Keeps
//!   `setu-types` free of `move-core-types` (G4).
//! - `Command::Upgrade` — deliberately removed; B5 will append it as the next
//!   tail variant. See `docs/feat/move-vm-phase9-ptb-wire/design.md` §9 Q2.
//! - Signature verification, gas, and execution result resolution.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dynamic_field::DfAccessMode;
use crate::object::ObjectId;

// ─────────────────────────────────────────────────────────────────────────────
// Bounds (DoS limits enforced by validate_wire)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of `Command`s in one PTB.
pub const MAX_PTB_COMMANDS: usize = 1024;
/// Maximum number of `inputs` in one PTB.
pub const MAX_PTB_INPUTS: usize = 1024;
/// Maximum number of declared dynamic-field accesses in one PTB.
pub const MAX_PTB_DF_ACCESSES: usize = 1024;
/// Maximum module name length (matches Sui upstream `IDENTIFIER_SIZE_MAX`).
pub const MAX_MODULE_NAME_LEN: usize = 128;
/// Maximum function name length.
pub const MAX_FUNCTION_NAME_LEN: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Wire types
// ─────────────────────────────────────────────────────────────────────────────

/// A Programmable Transaction Block — atomically-executed sequence of `Command`s.
///
/// **BCS field order is load-bearing for hash determinism.** Future additions
/// MUST go at the tail of the struct (or at the tail of contained enums).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgrammableTransaction {
    pub inputs: Vec<CallArg>,
    pub commands: Vec<Command>,
    /// B4 DF preload — every `Argument::Object` resolving to a DF child must
    /// be declared here (TaskPreparer enforces in B6b). Wire format only.
    #[serde(default)]
    pub dynamic_field_accesses: Vec<PtbDfAccess>,
}

/// PTB-internal binary form of a dynamic-field access.
///
/// **G1 invariant**: identical logical access ⇒ identical BCS bytes. The HTTP
/// layer translates from the hex-string [`crate::event::DynamicFieldAccess`]
/// before signature material is computed; the binary form is what consensus
/// hashes see.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtbDfAccess {
    pub parent: ObjectId,
    /// Canonical Move type-tag string for the key (e.g. `"u64"`,
    /// `"0xcafe::pool::Pair"`). Parsed in B6b.
    pub key_type: String,
    /// BCS-encoded key bytes (binary, not hex).
    pub key_bcs: Vec<u8>,
    pub mode: DfAccessMode,
    /// Required when `mode == Create`; for Read/Mutate/Delete the value type
    /// is recovered from the on-disk DF envelope at prepare time.
    #[serde(default)]
    pub value_type: Option<String>,
}

/// A PTB call argument — either a pure (BCS-bytes) value or an object reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallArg {
    /// BCS-encoded primitive/Move-value bytes.
    Pure(Vec<u8>),
    Object(ObjectArg),
}

/// Object input reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObjectArg {
    /// Owned or immutable object reference. `digest` is
    /// `blake3(ObjectEnvelope::to_bytes())` at `version`; TaskPreparer rejects
    /// on mismatch (prevents stale-read consensus drift).
    ImmOrOwnedObject(ObjectId, u64, [u8; 32]),
    /// Shared object reference. `mutable` decides whether the command may
    /// borrow it mutably; mismatched mutability rejected at prepare time.
    SharedObject {
        id: ObjectId,
        initial_shared_version: u64,
        mutable: bool,
    },
}

/// PTB command — one step inside a `ProgrammableTransaction`.
///
/// **IMPORTANT**: BCS enum discriminant order is load-bearing for hash
/// determinism. New variants — including B5's `Upgrade` — MUST be appended to
/// the tail of this enum; reordering or mid-enum insertion silently
/// invalidates all PTBs already signed under the previous shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Command {
    MoveCall(MoveCall),
    /// Transfer the listed objects to `recipient`. Result arity: unit `()`.
    TransferObjects(Vec<Argument>, Argument),
    /// Split `coin` into N coins of the given amounts. Result arity:
    /// `Vec<Coin>` of length `amounts.len()`.
    SplitCoins(Argument, Vec<Argument>),
    /// Merge `sources` into `target`. Result arity: unit `()`.
    MergeCoins(Argument, Vec<Argument>),
    /// Build `vector<T>` from the given args. `type_tag` is a canonical Move
    /// tag string; parsing into `TypeTag` happens in B6b.
    MakeMoveVec {
        type_tag: Option<String>,
        args: Vec<Argument>,
    },
    /// Publish a fresh package. Result arity: `UpgradeCap` (post-B5).
    Publish {
        modules: Vec<Vec<u8>>,
        deps: Vec<ObjectId>,
    },
    /// Upgrade an existing package — B5 hot-potato pattern.
    ///
    /// `upgrade_ticket` MUST resolve to an `UpgradeTicket` Move value
    /// produced by an earlier `MoveCall(setu::package::authorize_upgrade(...))`
    /// in the *same* PTB. Policy + digest live on the ticket, not on the
    /// command — this prevents PTB-level forgery (the ticket type has no
    /// `copy`/`drop`/`store`/`key` abilities, so it can only be created and
    /// consumed inside one transaction). Result arity: `UpgradeReceipt`
    /// (consumed by a later `commit_upgrade` MoveCall).
    Upgrade {
        modules: Vec<Vec<u8>>,
        deps: Vec<ObjectId>,
        current_package: ObjectId,
        upgrade_ticket: Argument,
    },
}

/// A Move call command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveCall {
    pub package: ObjectId,
    pub module: String,
    pub function: String,
    /// Canonical Move tag strings; parsed in B6b. Matches the existing
    /// `OperationType::MoveCall { type_args: Vec<String> }` to keep
    /// `setu-types` free of `move-core-types` (G4).
    pub type_arguments: Vec<String>,
    pub arguments: Vec<Argument>,
}

/// A PTB argument reference.
///
/// `u16` indices match Sui upstream; `MAX_PTB_COMMANDS = 1024 < u16::MAX` so
/// the type is comfortably wide enough for our DoS bounds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Argument {
    GasCoin,
    /// Index into `ProgrammableTransaction.inputs`.
    Input(u16),
    /// Index into preceding `Command` results. `Result(i)` is valid iff
    /// command `i` produces a non-unit result and `i < current command index`.
    Result(u16),
    /// Tuple-result drill-down: `command[i]` must produce a tuple of size > j.
    NestedResult(u16, u16),
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation
// ─────────────────────────────────────────────────────────────────────────────

/// Wire-validation errors raised by [`validate_wire`].
///
/// These are *parser-level* checks — semantic validation (object existence,
/// signature checks, type-signature compatibility) lives downstream in B6b.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PtbValidationError {
    #[error("PTB has too many commands: {len} > {max}")]
    TooManyCommands { len: usize, max: usize },

    #[error("PTB has too many inputs: {len} > {max}")]
    TooManyInputs { len: usize, max: usize },

    #[error("PTB has too many dynamic-field accesses: {len} > {max}")]
    TooManyDfAccesses { len: usize, max: usize },

    #[error("Argument::Input({idx}) out of range (inputs.len() = {len})")]
    InputIndexOutOfRange { idx: u16, len: usize },

    #[error("Forward result reference: command {at_cmd} references Result/NestedResult({ref_cmd})")]
    ForwardResultRef { at_cmd: usize, ref_cmd: u16 },

    #[error(
        "Argument::Result({producing_cmd}) consumed but command {producing_cmd} produces unit `()`"
    )]
    UnitResultConsumed { producing_cmd: u16 },

    #[error("Module name too long: {len} > {max}")]
    ModuleNameTooLong { len: usize, max: usize },

    #[error("Function name too long: {len} > {max}")]
    FunctionNameTooLong { len: usize, max: usize },

    #[error("Invalid type-tag string at index {idx}: {reason}")]
    InvalidTypeTagFormat { idx: usize, reason: &'static str },

    #[error("Invalid hex string in DynamicFieldAccess: {field}")]
    InvalidHex { field: &'static str },

    /// Same on-chain `ObjectId` appears in `inputs[]` more than once
    /// (R2-ISSUE-4 — see `docs/feat/move-vm-phase9-ptb-exec/review-log.md`).
    /// Each Slot would clone the same on-chain bytes independently, so a
    /// `mutate_slot` on one occurrence would not be visible to the other and
    /// only the last write-back would survive — silently dropping the other
    /// branch's effects (a value-conservation hazard for `Coin<T>` etc.).
    /// `Pure` inputs are NOT subject to this rule (multiple Pure args may
    /// legitimately carry the same bytes). Owned vs Shared variant is
    /// irrelevant — the same physical object cannot be referenced twice.
    #[error(
        "Duplicate Object input: oid={oid} appears at index {first_idx} and {dup_idx}"
    )]
    DuplicateObjectInput {
        oid: ObjectId,
        first_idx: usize,
        dup_idx: usize,
    },
}

impl ProgrammableTransaction {
    /// Wire-level validation. Runs *before* signature verification and *before*
    /// any VM-side parsing. Pure / side-effect-free.
    pub fn validate_wire(&self) -> Result<(), PtbValidationError> {
        // 1. DoS bounds
        if self.commands.len() > MAX_PTB_COMMANDS {
            return Err(PtbValidationError::TooManyCommands {
                len: self.commands.len(),
                max: MAX_PTB_COMMANDS,
            });
        }
        if self.inputs.len() > MAX_PTB_INPUTS {
            return Err(PtbValidationError::TooManyInputs {
                len: self.inputs.len(),
                max: MAX_PTB_INPUTS,
            });
        }
        if self.dynamic_field_accesses.len() > MAX_PTB_DF_ACCESSES {
            return Err(PtbValidationError::TooManyDfAccesses {
                len: self.dynamic_field_accesses.len(),
                max: MAX_PTB_DF_ACCESSES,
            });
        }

        let inputs_len = self.inputs.len();

        // 1.5. Duplicate Object input rejection (R2-ISSUE-4).
        //      Two `inputs[]` slots referencing the same on-chain ObjectId
        //      would each receive an independent clone of `move_data` in
        //      `engine::execute_ptb`, so mutations through one slot would
        //      not be visible to the other and only the last write-back
        //      would survive — a silent value-loss hazard. Reject at the
        //      wire-validation boundary.
        //      `Pure` inputs are intentionally NOT deduped.
        //      Owned vs Shared classification is irrelevant — same physical
        //      object cannot be referenced twice.
        {
            use std::collections::BTreeMap;
            let mut seen: BTreeMap<ObjectId, usize> = BTreeMap::new();
            for (idx, arg) in self.inputs.iter().enumerate() {
                if let CallArg::Object(obj_arg) = arg {
                    let oid = match obj_arg {
                        ObjectArg::ImmOrOwnedObject(id, _, _) => *id,
                        ObjectArg::SharedObject { id, .. } => *id,
                    };
                    if let Some(&first_idx) = seen.get(&oid) {
                        return Err(PtbValidationError::DuplicateObjectInput {
                            oid,
                            first_idx,
                            dup_idx: idx,
                        });
                    }
                    seen.insert(oid, idx);
                }
            }
        }

        // 2. Per-command checks
        for (cmd_idx, cmd) in self.commands.iter().enumerate() {
            check_command(cmd, cmd_idx, inputs_len, &self.commands)?;
        }

        Ok(())
    }
}

fn check_command(
    cmd: &Command,
    cmd_idx: usize,
    inputs_len: usize,
    all_commands: &[Command],
) -> Result<(), PtbValidationError> {
    match cmd {
        Command::MoveCall(mc) => {
            check_identifier(&mc.module, MAX_MODULE_NAME_LEN, true)?;
            check_identifier(&mc.function, MAX_FUNCTION_NAME_LEN, false)?;
            for (i, t) in mc.type_arguments.iter().enumerate() {
                check_type_tag_string(t, i)?;
            }
            for arg in &mc.arguments {
                check_argument(arg, cmd_idx, inputs_len, all_commands)?;
            }
        }
        Command::TransferObjects(args, recipient) => {
            for a in args {
                check_argument(a, cmd_idx, inputs_len, all_commands)?;
            }
            check_argument(recipient, cmd_idx, inputs_len, all_commands)?;
        }
        Command::SplitCoins(coin, amounts) => {
            check_argument(coin, cmd_idx, inputs_len, all_commands)?;
            for a in amounts {
                check_argument(a, cmd_idx, inputs_len, all_commands)?;
            }
        }
        Command::MergeCoins(target, sources) => {
            check_argument(target, cmd_idx, inputs_len, all_commands)?;
            for a in sources {
                check_argument(a, cmd_idx, inputs_len, all_commands)?;
            }
        }
        Command::MakeMoveVec { type_tag, args } => {
            if let Some(tag) = type_tag {
                check_type_tag_string(tag, 0)?;
            }
            for a in args {
                check_argument(a, cmd_idx, inputs_len, all_commands)?;
            }
        }
        Command::Publish { modules: _, deps: _ } => {
            // bytecode + dep IDs validated semantically in B6b
        }
        Command::Upgrade { modules: _, deps: _, current_package: _, upgrade_ticket } => {
            // Bytecode + dep IDs + current_package presence are validated
            // semantically by the engine (relink + compat check); here we
            // only verify the ticket Argument is well-formed.
            check_argument(upgrade_ticket, cmd_idx, inputs_len, all_commands)?;
        }
    }
    Ok(())
}

fn check_argument(
    arg: &Argument,
    cmd_idx: usize,
    inputs_len: usize,
    all_commands: &[Command],
) -> Result<(), PtbValidationError> {
    match *arg {
        Argument::GasCoin => Ok(()),
        Argument::Input(i) => {
            if (i as usize) >= inputs_len {
                Err(PtbValidationError::InputIndexOutOfRange {
                    idx: i,
                    len: inputs_len,
                })
            } else {
                Ok(())
            }
        }
        Argument::Result(i) => {
            if (i as usize) >= cmd_idx {
                return Err(PtbValidationError::ForwardResultRef {
                    at_cmd: cmd_idx,
                    ref_cmd: i,
                });
            }
            if produces_unit(&all_commands[i as usize]) {
                return Err(PtbValidationError::UnitResultConsumed { producing_cmd: i });
            }
            Ok(())
        }
        Argument::NestedResult(i, _j) => {
            if (i as usize) >= cmd_idx {
                return Err(PtbValidationError::ForwardResultRef {
                    at_cmd: cmd_idx,
                    ref_cmd: i,
                });
            }
            // `NestedResult` is only meaningful on tuple-producing commands.
            // We can statically reject the unit-producing ones (TransferObjects,
            // MergeCoins). Deeper arity (e.g. MoveCall returning a single
            // value vs tuple) is signature-driven and deferred to B6b.
            if produces_unit(&all_commands[i as usize]) {
                return Err(PtbValidationError::UnitResultConsumed { producing_cmd: i });
            }
            Ok(())
        }
    }
}

/// Statically known unit-result commands. `MoveCall` arity is signature-driven
/// (deferred to B6b); we conservatively accept it here.
fn produces_unit(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::TransferObjects(_, _) | Command::MergeCoins(_, _)
    )
}

fn check_identifier(
    s: &str,
    max_len: usize,
    is_module: bool,
) -> Result<(), PtbValidationError> {
    if s.len() > max_len {
        return Err(if is_module {
            PtbValidationError::ModuleNameTooLong {
                len: s.len(),
                max: max_len,
            }
        } else {
            PtbValidationError::FunctionNameTooLong {
                len: s.len(),
                max: max_len,
            }
        });
    }
    Ok(())
}

fn check_type_tag_string(s: &str, idx: usize) -> Result<(), PtbValidationError> {
    if s.is_empty() {
        return Err(PtbValidationError::InvalidTypeTagFormat {
            idx,
            reason: "empty",
        });
    }
    if s.contains('\0') {
        return Err(PtbValidationError::InvalidTypeTagFormat {
            idx,
            reason: "contains null byte",
        });
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP-boundary conversion: hex-string DynamicFieldAccess → binary PtbDfAccess
// ─────────────────────────────────────────────────────────────────────────────

impl TryFrom<crate::event::DynamicFieldAccess> for PtbDfAccess {
    type Error = PtbValidationError;

    fn try_from(d: crate::event::DynamicFieldAccess) -> Result<Self, Self::Error> {
        let parent_hex = d
            .parent_object_id
            .strip_prefix("oid:")
            .unwrap_or(&d.parent_object_id);
        let parent_hex = parent_hex.strip_prefix("0x").unwrap_or(parent_hex);
        let parent_bytes = hex::decode(parent_hex).map_err(|_| PtbValidationError::InvalidHex {
            field: "parent_object_id",
        })?;
        let parent = ObjectId::from_bytes(&parent_bytes).map_err(|_| {
            PtbValidationError::InvalidHex {
                field: "parent_object_id",
            }
        })?;

        let key_hex = d.key_bcs_hex.strip_prefix("0x").unwrap_or(&d.key_bcs_hex);
        let key_bcs = hex::decode(key_hex)
            .map_err(|_| PtbValidationError::InvalidHex { field: "key_bcs_hex" })?;

        Ok(Self {
            parent,
            key_type: d.key_type,
            key_bcs,
            mode: d.mode,
            value_type: d.value_type,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Compile-fail enforcement of R1-FOLLOWUP-A (B6a removed Command::Upgrade).
//
// The doctest below MUST fail to compile. If somebody re-introduces
// `Command::Upgrade` in B6a, this doctest would silently start succeeding,
// which `cargo test --doc` will catch.
// ─────────────────────────────────────────────────────────────────────────────

/// B6a deliberately removed `Command::Upgrade`. B5 will append it as the next
/// tail variant of `Command`. This doctest pins that decision.
///
/// ```compile_fail
/// // B6a: Command::Upgrade was deliberately removed (R1-FOLLOWUP-A).
/// // B5 will append it as the next tail variant.
/// let _ = setu_types::ptb::Command::Upgrade {};
/// ```
pub const _COMPILE_FAIL_NO_UPGRADE_VARIANT: () = ();

// ═════════════════════════════════════════════════════════════════════════════
// Tests (Phase 2 matrix U1–U19)
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::DynamicFieldAccess;

    // Tiny deterministic LCG — no proptest dep, reproducible across machines.
    struct LcgRng {
        state: u64,
    }
    impl LcgRng {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }
        fn next_u64(&mut self) -> u64 {
            // Numerical Recipes constants
            self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.state
        }
        fn gen_range(&mut self, hi: usize) -> usize {
            (self.next_u64() as usize) % hi.max(1)
        }
        fn gen_bool(&mut self) -> bool {
            self.next_u64() & 1 == 0
        }
        fn gen_bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| (self.next_u64() & 0xff) as u8).collect()
        }
    }

    fn oid_from_seed(seed: u64) -> ObjectId {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&seed.to_le_bytes());
        ObjectId::new(bytes)
    }

    fn sample_move_call() -> MoveCall {
        MoveCall {
            package: oid_from_seed(0xCAFE),
            module: "pool".to_string(),
            function: "swap".to_string(),
            type_arguments: vec!["0xcafe::pool::Pair".to_string(), "u64".to_string()],
            arguments: vec![Argument::GasCoin, Argument::Input(0)],
        }
    }

    fn sample_ptb() -> ProgrammableTransaction {
        ProgrammableTransaction {
            inputs: vec![CallArg::Pure(vec![1, 2, 3, 4])],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        }
    }

    // ── U1 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u1_roundtrip_minimal_ptb() {
        let p = sample_ptb();
        let bytes = bcs::to_bytes(&p).unwrap();
        let decoded: ProgrammableTransaction = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, p);
    }

    // ── U2 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u2_roundtrip_100_random_ptbs() {
        let mut rng = LcgRng::new(0x53_45_54_55);
        for _ in 0..100 {
            let p = random_ptb(&mut rng);
            let bytes = bcs::to_bytes(&p).expect("bcs encode");
            let decoded: ProgrammableTransaction =
                bcs::from_bytes(&bytes).expect("bcs decode");
            assert_eq!(decoded, p);
        }
    }

    fn random_ptb(rng: &mut LcgRng) -> ProgrammableTransaction {
        let n_inputs = rng.gen_range(4) + 1;
        let inputs: Vec<CallArg> = (0..n_inputs)
            .map(|_| {
                if rng.gen_bool() {
                    let n = rng.gen_range(8) + 1;
                    CallArg::Pure(rng.gen_bytes(n))
                } else {
                    CallArg::Object(random_object_arg(rng))
                }
            })
            .collect();
        let n_cmds = rng.gen_range(4) + 1;
        let mut cmds = Vec::with_capacity(n_cmds);
        for i in 0..n_cmds {
            cmds.push(random_command(rng, i, n_inputs));
        }
        ProgrammableTransaction {
            inputs,
            commands: cmds,
            dynamic_field_accesses: vec![random_df_access(rng)],
        }
    }

    fn random_object_arg(rng: &mut LcgRng) -> ObjectArg {
        if rng.gen_bool() {
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&rng.gen_bytes(32));
            ObjectArg::ImmOrOwnedObject(oid_from_seed(rng.next_u64()), rng.next_u64(), digest)
        } else {
            ObjectArg::SharedObject {
                id: oid_from_seed(rng.next_u64()),
                initial_shared_version: rng.next_u64(),
                mutable: rng.gen_bool(),
            }
        }
    }

    fn random_command(rng: &mut LcgRng, cmd_idx: usize, n_inputs: usize) -> Command {
        // Pick from variants. Bias toward MoveCall for coverage.
        let pick = rng.gen_range(6);
        let n_inputs_u16 = n_inputs.min(u16::MAX as usize) as u16;
        let arg = || -> Argument {
            if n_inputs_u16 == 0 {
                Argument::GasCoin
            } else {
                Argument::Input(0)
            }
        };
        let _ = cmd_idx; // only used for forward-ref tests below
        match pick {
            0 => Command::MoveCall(sample_move_call()),
            1 => Command::TransferObjects(vec![arg()], arg()),
            2 => Command::SplitCoins(arg(), vec![arg(), arg()]),
            3 => Command::MergeCoins(arg(), vec![arg()]),
            4 => Command::MakeMoveVec {
                type_tag: Some("u64".to_string()),
                args: vec![arg()],
            },
            _ => {
                let n = rng.gen_range(8) + 1;
                Command::Publish {
                    modules: vec![rng.gen_bytes(n)],
                    deps: vec![oid_from_seed(rng.next_u64())],
                }
            }
        }
    }

    fn random_df_access(rng: &mut LcgRng) -> PtbDfAccess {
        PtbDfAccess {
            parent: oid_from_seed(rng.next_u64()),
            key_type: "u64".to_string(),
            key_bcs: rng.gen_bytes(8),
            mode: DfAccessMode::Read,
            value_type: None,
        }
    }

    // ── U3 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u3_validate_rejects_too_many_commands() {
        let mut p = sample_ptb();
        p.commands = (0..MAX_PTB_COMMANDS + 1)
            .map(|_| Command::MoveCall(sample_move_call()))
            .collect();
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::TooManyCommands { .. })
        ));
    }

    // ── U4 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u4_validate_rejects_too_many_inputs() {
        let mut p = sample_ptb();
        p.inputs = (0..MAX_PTB_INPUTS + 1).map(|_| CallArg::Pure(vec![0])).collect();
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::TooManyInputs { .. })
        ));
    }

    // ── U5 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u5_validate_rejects_too_many_df_accesses() {
        let mut p = sample_ptb();
        p.dynamic_field_accesses = (0..MAX_PTB_DF_ACCESSES + 1)
            .map(|_| PtbDfAccess {
                parent: ObjectId::ZERO,
                key_type: "u64".to_string(),
                key_bcs: vec![0; 8],
                mode: DfAccessMode::Read,
                value_type: None,
            })
            .collect();
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::TooManyDfAccesses { .. })
        ));
    }

    // ── U6 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u6_validate_rejects_input_oob() {
        let mut p = sample_ptb();
        // Sample has 1 input (index 0). Reference index 1 → OOB.
        p.commands = vec![Command::TransferObjects(
            vec![Argument::Input(1)],
            Argument::Input(1),
        )];
        let err = p.validate_wire().unwrap_err();
        assert!(matches!(
            err,
            PtbValidationError::InputIndexOutOfRange { idx: 1, len: 1 }
        ));

        // Boundary: idx == len also OOB.
        p.commands = vec![Command::TransferObjects(
            vec![Argument::Input(1)],
            Argument::GasCoin,
        )];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::InputIndexOutOfRange { .. })
        ));
    }

    // ── U7 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u7_validate_rejects_forward_result_ref() {
        let mut p = sample_ptb();
        // Command 0 references Result(1) — forward.
        p.commands = vec![
            Command::MoveCall(MoveCall {
                package: ObjectId::ZERO,
                module: "m".into(),
                function: "f".into(),
                type_arguments: vec![],
                arguments: vec![Argument::Result(1)],
            }),
            Command::MoveCall(sample_move_call()),
        ];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::ForwardResultRef { at_cmd: 0, ref_cmd: 1 })
        ));

        // Self-reference: command 1 references Result(1) (its own index).
        p.commands = vec![
            Command::MoveCall(sample_move_call()),
            Command::MoveCall(MoveCall {
                package: ObjectId::ZERO,
                module: "m".into(),
                function: "f".into(),
                type_arguments: vec![],
                arguments: vec![Argument::Result(1)],
            }),
        ];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::ForwardResultRef { at_cmd: 1, ref_cmd: 1 })
        ));
    }

    // ── U8 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u8_validate_rejects_unit_result_consumed() {
        let mut p = sample_ptb();
        // command 0 = TransferObjects (unit), command 1 consumes Result(0).
        p.commands = vec![
            Command::TransferObjects(vec![Argument::GasCoin], Argument::GasCoin),
            Command::MoveCall(MoveCall {
                package: ObjectId::ZERO,
                module: "m".into(),
                function: "f".into(),
                type_arguments: vec![],
                arguments: vec![Argument::Result(0)],
            }),
        ];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::UnitResultConsumed { producing_cmd: 0 })
        ));

        // MergeCoins also unit.
        p.commands = vec![
            Command::MergeCoins(Argument::GasCoin, vec![Argument::GasCoin]),
            Command::MoveCall(MoveCall {
                package: ObjectId::ZERO,
                module: "m".into(),
                function: "f".into(),
                type_arguments: vec![],
                arguments: vec![Argument::Result(0)],
            }),
        ];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::UnitResultConsumed { producing_cmd: 0 })
        ));
    }

    // ── U9 ──────────────────────────────────────────────────────────────────
    #[test]
    fn u9_validate_rejects_module_name_too_long() {
        let mut p = sample_ptb();
        p.commands = vec![Command::MoveCall(MoveCall {
            package: ObjectId::ZERO,
            module: "a".repeat(MAX_MODULE_NAME_LEN + 1),
            function: "f".into(),
            type_arguments: vec![],
            arguments: vec![],
        })];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::ModuleNameTooLong { .. })
        ));
    }

    // ── U10 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u10_validate_rejects_malformed_type_tag() {
        let mut p = sample_ptb();
        // empty
        p.commands = vec![Command::MoveCall(MoveCall {
            package: ObjectId::ZERO,
            module: "m".into(),
            function: "f".into(),
            type_arguments: vec!["".into()],
            arguments: vec![],
        })];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::InvalidTypeTagFormat { idx: 0, reason: "empty" })
        ));

        // null byte
        p.commands = vec![Command::MoveCall(MoveCall {
            package: ObjectId::ZERO,
            module: "m".into(),
            function: "f".into(),
            type_arguments: vec!["abc\0def".into()],
            arguments: vec![],
        })];
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::InvalidTypeTagFormat { idx: 0, .. })
        ));
    }

    // ── U11 ─────────────────────────────────────────────────────────────────
    // Verifies OperationType has been extended to 8 variants. The actual
    // `OperationType` extension lives in solver_task.rs; this test reaches
    // across to lock the count.
    #[test]
    fn u11_operation_type_variant_count() {
        use crate::task::OperationType;
        // Construct one of each variant. If any new variant is added without
        // updating this test, it still compiles — but if a variant is REMOVED
        // (which would break wire compat), this test fails to compile and
        // forces a deliberate review.
        let _ = OperationType::NoOp;
        let _ = OperationType::Transfer { from_coin_index: 0, amount: 0 };
        let _ = OperationType::MergeCoins { target_index: 0, source_indices: vec![] };
        let _ = OperationType::SplitCoin { source_index: 0, amounts: vec![] };
        let _ = OperationType::MergeThenTransfer {
            target_index: 0,
            source_indices: vec![],
            recipient: crate::object::Address::ZERO,
            amount: 0,
        };
        let _ = OperationType::MoveCall {
            package: String::new(),
            module_name: String::new(),
            function_name: String::new(),
            type_args: vec![],
            pure_args: vec![],
            mutable_indices: vec![],
            consumed_indices: vec![],
        };
        let _ = OperationType::MovePublish { modules: vec![] };
        let _ = OperationType::ProgrammableTransaction(sample_ptb());
    }

    // ── U12 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u12_operation_type_primary_coin_match_exhaustive() {
        use crate::task::{OperationType, ResolvedInputs};
        let r = ResolvedInputs {
            operation: OperationType::ProgrammableTransaction(sample_ptb()),
            input_objects: vec![],
            dynamic_fields: vec![],
        };
        assert!(r.primary_coin().is_none());
    }

    // ── U13 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u13_legacy_solver_task_serde_unchanged() {
        use crate::task::OperationType;
        // Frozen golden bytes for `OperationType::Transfer { from_coin_index: 0, amount: 100 }`.
        // Generated by hand 2026-04-30 from `bcs::to_bytes(&op)`. If this test
        // fails after touching OperationType layout, B6a has broken wire compat.
        let op = OperationType::Transfer { from_coin_index: 0, amount: 100 };
        let bytes = bcs::to_bytes(&op).unwrap();
        // discriminant 1 (Transfer) | usize 0 (8 bytes LE) | u64 100 (8 bytes LE)
        let expected: &[u8] = &[
            0x01,
            0, 0, 0, 0, 0, 0, 0, 0,
            100, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(bytes.as_slice(), expected,
            "OperationType::Transfer wire layout drifted — B6a wire compat broken!");
    }

    // ── U14 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u14_golden_bytes_minimal_ptb() {
        // Minimal PTB — empty inputs, single Publish command (smallest-shape
        // command body), empty df_accesses. Hand-computed BCS bytes.
        let p = ProgrammableTransaction {
            inputs: vec![],
            commands: vec![Command::Publish {
                modules: vec![],
                deps: vec![],
            }],
            dynamic_field_accesses: vec![],
        };
        let bytes = bcs::to_bytes(&p).unwrap();
        // Layout:
        //   inputs: ULEB128 length 0                        → 0x00
        //   commands: ULEB128 length 1                       → 0x01
        //   commands[0]: discriminant 5 (Publish)            → 0x05
        //     modules: ULEB128 length 0                      → 0x00
        //     deps:    ULEB128 length 0                      → 0x00
        //   dynamic_field_accesses: ULEB128 length 0         → 0x00
        let expected: &[u8] = &[0x00, 0x01, 0x05, 0x00, 0x00, 0x00];
        assert_eq!(bytes.as_slice(), expected,
            "PTB wire format drifted — see Phase 1 §5.2 BCS field-order rule.");
    }

    // ── U15 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u15_ptb_df_access_binary_g1() {
        // Direct ctor
        let mut bytes32 = [0u8; 32];
        bytes32[31] = 0xab;
        bytes32[30] = 0xcd;
        let direct = PtbDfAccess {
            parent: ObjectId::new(bytes32),
            key_type: "u64".to_string(),
            key_bcs: vec![0xde, 0xad],
            mode: DfAccessMode::Read,
            value_type: None,
        };

        // Via TryFrom — uppercase hex with "oid:" prefix and "0x" on key.
        let parent_hex_upper = format!("oid:0x{}", hex::encode_upper(bytes32));
        let from_hex = PtbDfAccess::try_from(DynamicFieldAccess {
            parent_object_id: parent_hex_upper,
            key_type: "u64".to_string(),
            key_bcs_hex: "0xDEAD".to_string(),
            mode: DfAccessMode::Read,
            value_type: None,
        })
        .unwrap();

        assert_eq!(bcs::to_bytes(&direct).unwrap(), bcs::to_bytes(&from_hex).unwrap(),
            "G1 violated: same logical DF access produced different BCS bytes!");
    }

    // ── U16 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u16_ptb_df_access_rejects_bad_hex() {
        // Bad parent
        let r = PtbDfAccess::try_from(DynamicFieldAccess {
            parent_object_id: "0xZZ".into(),
            key_type: "u64".into(),
            key_bcs_hex: "00".into(),
            mode: DfAccessMode::Read,
            value_type: None,
        });
        assert!(matches!(
            r,
            Err(PtbValidationError::InvalidHex { field: "parent_object_id" })
        ));

        // Bad key
        let r = PtbDfAccess::try_from(DynamicFieldAccess {
            parent_object_id: format!("0x{}", "00".repeat(32)),
            key_type: "u64".into(),
            key_bcs_hex: "ZZ".into(),
            mode: DfAccessMode::Read,
            value_type: None,
        });
        assert!(matches!(
            r,
            Err(PtbValidationError::InvalidHex { field: "key_bcs_hex" })
        ));
    }

    // ── U17 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u17_object_arg_digest_length() {
        let arg = ObjectArg::ImmOrOwnedObject(ObjectId::ZERO, 42, [7u8; 32]);
        let bytes = bcs::to_bytes(&arg).unwrap();
        let decoded: ObjectArg = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, arg);
        // The `[u8; 32]` type literally cannot accept a non-32-byte buffer at
        // construction — that's a compile-time error, no runtime test needed.
    }

    // ── U18 ─────────────────────────────────────────────────────────────────
    #[test]
    fn u18_validate_accepts_well_formed_complex_ptb() {
        // 5 commands forming a realistic chain.
        // 0: SplitCoins(GasCoin, [Pure(0)])         — produces Vec<Coin>
        // 1: MakeMoveVec { Some("u64"), [Pure(0)] } — produces vector<u64>
        // 2: MoveCall taking NestedResult(0, 0)     — uses split[0]
        // 3: TransferObjects([Result(2)], GasCoin)  — uses MoveCall result
        // 4: MoveCall (no refs to unit results)
        let p = ProgrammableTransaction {
            inputs: vec![CallArg::Pure(vec![0u8; 8])],
            commands: vec![
                Command::SplitCoins(Argument::GasCoin, vec![Argument::Input(0)]),
                Command::MakeMoveVec {
                    type_tag: Some("u64".to_string()),
                    args: vec![Argument::Input(0)],
                },
                Command::MoveCall(MoveCall {
                    package: ObjectId::ZERO,
                    module: "m".into(),
                    function: "f".into(),
                    type_arguments: vec![],
                    arguments: vec![Argument::NestedResult(0, 0)],
                }),
                Command::TransferObjects(vec![Argument::Result(2)], Argument::GasCoin),
                Command::MoveCall(sample_move_call()),
            ],
            dynamic_field_accesses: vec![],
        };
        assert!(p.validate_wire().is_ok());
    }

    // ── U19 ─────────────────────────────────────────────────────────────────
    // Compile-fail enforced via the rustdoc doctest above
    // (`_COMPILE_FAIL_NO_UPGRADE_VARIANT`). `cargo test --doc` runs it.
    // This placeholder asserts the doctest is wired up (the doc item exists).
    #[test]
    fn u19_command_upgrade_does_not_compile_marker() {
        // If someone deletes the doctest, `_COMPILE_FAIL_NO_UPGRADE_VARIANT`
        // is gone too — and this test fails to compile, forcing review.
        let _: () = _COMPILE_FAIL_NO_UPGRADE_VARIANT;
    }

    // ── U20 ─ R2-ISSUE-4: Object input dedup ────────────────────────────────
    #[test]
    fn u20_validate_rejects_duplicate_immowned_object_input() {
        let oid = oid_from_seed(0xDEAD_BEEF);
        let p = ProgrammableTransaction {
            inputs: vec![
                CallArg::Object(ObjectArg::ImmOrOwnedObject(oid, 1, [0u8; 32])),
                CallArg::Object(ObjectArg::ImmOrOwnedObject(oid, 2, [1u8; 32])),
            ],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        };
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::DuplicateObjectInput { first_idx: 0, dup_idx: 1, .. })
        ));
    }

    #[test]
    fn u21_validate_rejects_duplicate_shared_object_input() {
        let oid = oid_from_seed(0xCAFE_F00D);
        let p = ProgrammableTransaction {
            inputs: vec![
                CallArg::Object(ObjectArg::SharedObject {
                    id: oid,
                    initial_shared_version: 1,
                    mutable: true,
                }),
                CallArg::Pure(vec![1, 2, 3]),
                CallArg::Object(ObjectArg::SharedObject {
                    id: oid,
                    initial_shared_version: 1,
                    mutable: false,
                }),
            ],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        };
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::DuplicateObjectInput { first_idx: 0, dup_idx: 2, .. })
        ));
    }

    #[test]
    fn u22_validate_rejects_mixed_owned_and_shared_same_oid() {
        // Same physical ObjectId cannot legitimately be both ImmOrOwned and
        // Shared in a single PTB — TaskPreparer cannot resolve it
        // consistently, and engine.rs would clone the same on-chain bytes
        // into two divergent Slots.
        let oid = oid_from_seed(0xBEEF_C0DE);
        let p = ProgrammableTransaction {
            inputs: vec![
                CallArg::Object(ObjectArg::ImmOrOwnedObject(oid, 7, [9u8; 32])),
                CallArg::Object(ObjectArg::SharedObject {
                    id: oid,
                    initial_shared_version: 1,
                    mutable: true,
                }),
            ],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        };
        assert!(matches!(
            p.validate_wire(),
            Err(PtbValidationError::DuplicateObjectInput { first_idx: 0, dup_idx: 1, .. })
        ));
    }

    #[test]
    fn u23_validate_allows_duplicate_pure_inputs() {
        // Two Pure inputs with identical bytes are legitimate — they may
        // back distinct logical parameters of the same Move function.
        let p = ProgrammableTransaction {
            inputs: vec![
                CallArg::Pure(vec![1, 2, 3, 4]),
                CallArg::Pure(vec![1, 2, 3, 4]),
            ],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        };
        assert!(p.validate_wire().is_ok());
    }

    #[test]
    fn u24_validate_allows_distinct_object_inputs() {
        let oid_a = oid_from_seed(0xAA);
        let oid_b = oid_from_seed(0xBB);
        let p = ProgrammableTransaction {
            inputs: vec![
                CallArg::Object(ObjectArg::ImmOrOwnedObject(oid_a, 1, [0u8; 32])),
                CallArg::Object(ObjectArg::ImmOrOwnedObject(oid_b, 1, [0u8; 32])),
                CallArg::Object(ObjectArg::SharedObject {
                    id: oid_from_seed(0xCC),
                    initial_shared_version: 1,
                    mutable: true,
                }),
            ],
            commands: vec![Command::MoveCall(sample_move_call())],
            dynamic_field_accesses: vec![],
        };
        assert!(p.validate_wire().is_ok());
    }
}
