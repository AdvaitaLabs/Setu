//! `ptb-encode` subcommand — encode a Programmable Transaction Block (PTB)
//! from a JSON spec file to BCS-hex bytes consumable by `/api/v1/move/ptb`.
//!
//! **Internal-only**: the spec format is consumed by the
//! `tests/move_overlay/mo_pkg_upgrade_*.sh` shell tests and by the
//! `setu-cli` test harness. It is not a stable public CLI surface — when
//! the `Command` enum in [`setu_types::ptb`] grows, this spec format may
//! change without notice.
//!
//! ## Spec shape
//!
//! ```json
//! {
//!   "inputs":   [ <CallArgSpec>, ... ],
//!   "commands": [ <CommandSpec>, ... ],
//!   "dynamic_field_accesses": [ <DfAccessSpec>, ... ]   // optional, default []
//! }
//! ```
//!
//! See the `#[derive(Deserialize)]` types below for the exact schemas of each
//! variant. `serde(deny_unknown_fields)` is set on every type, so typos in
//! fixtures are rejected at parse time (test U5).
//!
//! ## Output
//!
//! Lowercase-hex BCS bytes of `ProgrammableTransaction`, no `0x` prefix, no
//! trailing newline (suitable for direct embedding into a JSON request body
//! via `$(cat ptb.hex)`). When `--out` is omitted, prints to stdout.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

use setu_types::dynamic_field::DfAccessMode;
use setu_types::object::ObjectId;
use setu_types::ptb::{
    Argument, CallArg, Command, MoveCall, ObjectArg, ProgrammableTransaction, PtbDfAccess,
};

// ─── Spec types (deserialised from JSON) ─────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PtbSpec {
    #[serde(default)]
    inputs: Vec<CallArgSpec>,
    commands: Vec<CommandSpec>,
    #[serde(default)]
    dynamic_field_accesses: Vec<DfAccessSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CallArgSpec {
    /// `Pure(BCS-of-u8)`
    PureU8 { value: u8 },
    /// `Pure(BCS-of-u64)`
    PureU64 { value: u64 },
    /// `Pure(BCS-of-bool)`
    PureBool { value: bool },
    /// `Pure(BCS-of-vec<u8>)` — value is hex-encoded byte string (with or without `0x`).
    PureVecU8Hex { value: String },
    /// `Pure(BCS-of-AccountAddress)` — value is canonical 0x-prefixed hex (short or padded).
    PureAddress { value: String },
    /// Raw `Pure` bytes — caller supplies the BCS payload directly as hex.
    /// Used by negative tests (e.g. `ticket_forge`) to construct fake values.
    PureRawHex { value: String },
    ImmOrOwnedObject {
        id: String,
        version: u64,
        digest: String,
    },
    SharedObject {
        id: String,
        initial_shared_version: u64,
        mutable: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CommandSpec {
    MoveCall {
        package: String,
        module: String,
        function: String,
        #[serde(default)]
        type_arguments: Vec<String>,
        #[serde(default)]
        arguments: Vec<ArgumentSpec>,
    },
    TransferObjects {
        objects: Vec<ArgumentSpec>,
        recipient: ArgumentSpec,
    },
    SplitCoins {
        coin: ArgumentSpec,
        amounts: Vec<ArgumentSpec>,
    },
    MergeCoins {
        target: ArgumentSpec,
        sources: Vec<ArgumentSpec>,
    },
    MakeMoveVec {
        #[serde(default)]
        type_tag: Option<String>,
        args: Vec<ArgumentSpec>,
    },
    Publish {
        /// Hex-encoded compiled module bytes — one entry per module.
        modules_hex: Vec<String>,
        #[serde(default)]
        deps: Vec<String>,
    },
    Upgrade {
        modules_hex: Vec<String>,
        #[serde(default)]
        deps: Vec<String>,
        current_package: String,
        upgrade_ticket: ArgumentSpec,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ArgumentSpec {
    GasCoin,
    Input { index: u16 },
    Result { index: u16 },
    NestedResult { index: u16, sub_index: u16 },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DfAccessSpec {
    parent: String,
    key_type: String,
    /// Hex-encoded BCS bytes of the key.
    key_bcs_hex: String,
    /// One of: `read`, `mutate`, `create`, `delete` (lowercase).
    mode: String,
    #[serde(default)]
    value_type: Option<String>,
}

// ─── Spec → wire types ────────────────────────────────────────────────────────

/// Parse hex (with or without `0x` prefix) and **left-pad to 32 bytes**, matching
/// `setu-validator/src/network/move_handler.rs::decode_object_id_hex` semantics
/// so that short forms like `0xcafe` round-trip.
fn parse_object_id(s: &str) -> Result<ObjectId> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.is_empty() {
        return Err(anyhow!("empty object id"));
    }
    // Pad to even length so `0x1` decodes as `[0x01]` then left-pads to 32 bytes.
    let even = if stripped.len() % 2 == 1 {
        format!("0{}", stripped)
    } else {
        stripped.to_string()
    };
    let raw = hex::decode(&even)
        .with_context(|| format!("invalid hex object id: {}", s))?;
    if raw.len() > 32 {
        return Err(anyhow!("object id too long: {} > 32 bytes", raw.len()));
    }
    let mut padded = [0u8; 32];
    padded[32 - raw.len()..].copy_from_slice(&raw);
    Ok(ObjectId::from_bytes(&padded).map_err(|e| anyhow!("ObjectId::from_bytes: {}", e))?)
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(stripped).with_context(|| format!("invalid hex: {}", s))
}

fn parse_digest(s: &str) -> Result<[u8; 32]> {
    let raw = parse_hex_bytes(s)?;
    if raw.len() != 32 {
        return Err(anyhow!("digest must be 32 bytes, got {}", raw.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&raw);
    Ok(arr)
}

fn arg_from_spec(spec: &ArgumentSpec) -> Argument {
    match *spec {
        ArgumentSpec::GasCoin => Argument::GasCoin,
        ArgumentSpec::Input { index } => Argument::Input(index),
        ArgumentSpec::Result { index } => Argument::Result(index),
        ArgumentSpec::NestedResult { index, sub_index } => Argument::NestedResult(index, sub_index),
    }
}

fn input_from_spec(spec: &CallArgSpec) -> Result<CallArg> {
    Ok(match spec {
        CallArgSpec::PureU8 { value } => {
            CallArg::Pure(bcs::to_bytes(value).context("encode PureU8")?)
        }
        CallArgSpec::PureU64 { value } => {
            CallArg::Pure(bcs::to_bytes(value).context("encode PureU64")?)
        }
        CallArgSpec::PureBool { value } => {
            CallArg::Pure(bcs::to_bytes(value).context("encode PureBool")?)
        }
        CallArgSpec::PureVecU8Hex { value } => {
            let bytes = parse_hex_bytes(value)?;
            CallArg::Pure(bcs::to_bytes(&bytes).context("encode PureVecU8Hex")?)
        }
        CallArgSpec::PureAddress { value } => {
            let oid = parse_object_id(value)?;
            // Move addresses are 32 bytes — same wire as ObjectId. BCS of [u8; 32]
            // is the raw bytes (no length prefix), matching Sui upstream.
            CallArg::Pure(oid.as_bytes().to_vec())
        }
        CallArgSpec::PureRawHex { value } => CallArg::Pure(parse_hex_bytes(value)?),
        CallArgSpec::ImmOrOwnedObject {
            id,
            version,
            digest,
        } => CallArg::Object(ObjectArg::ImmOrOwnedObject(
            parse_object_id(id)?,
            *version,
            parse_digest(digest)?,
        )),
        CallArgSpec::SharedObject {
            id,
            initial_shared_version,
            mutable,
        } => CallArg::Object(ObjectArg::SharedObject {
            id: parse_object_id(id)?,
            initial_shared_version: *initial_shared_version,
            mutable: *mutable,
        }),
    })
}

fn command_from_spec(spec: &CommandSpec) -> Result<Command> {
    Ok(match spec {
        CommandSpec::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => Command::MoveCall(MoveCall {
            package: parse_object_id(package)?,
            module: module.clone(),
            function: function.clone(),
            type_arguments: type_arguments.clone(),
            arguments: arguments.iter().map(arg_from_spec).collect(),
        }),
        CommandSpec::TransferObjects { objects, recipient } => Command::TransferObjects(
            objects.iter().map(arg_from_spec).collect(),
            arg_from_spec(recipient),
        ),
        CommandSpec::SplitCoins { coin, amounts } => Command::SplitCoins(
            arg_from_spec(coin),
            amounts.iter().map(arg_from_spec).collect(),
        ),
        CommandSpec::MergeCoins { target, sources } => Command::MergeCoins(
            arg_from_spec(target),
            sources.iter().map(arg_from_spec).collect(),
        ),
        CommandSpec::MakeMoveVec { type_tag, args } => Command::MakeMoveVec {
            type_tag: type_tag.clone(),
            args: args.iter().map(arg_from_spec).collect(),
        },
        CommandSpec::Publish { modules_hex, deps } => {
            let modules = modules_hex
                .iter()
                .map(|h| parse_hex_bytes(h))
                .collect::<Result<Vec<_>>>()?;
            let deps = deps
                .iter()
                .map(|d| parse_object_id(d))
                .collect::<Result<Vec<_>>>()?;
            Command::Publish { modules, deps }
        }
        CommandSpec::Upgrade {
            modules_hex,
            deps,
            current_package,
            upgrade_ticket,
        } => {
            let modules = modules_hex
                .iter()
                .map(|h| parse_hex_bytes(h))
                .collect::<Result<Vec<_>>>()?;
            let deps = deps
                .iter()
                .map(|d| parse_object_id(d))
                .collect::<Result<Vec<_>>>()?;
            Command::Upgrade {
                modules,
                deps,
                current_package: parse_object_id(current_package)?,
                upgrade_ticket: arg_from_spec(upgrade_ticket),
            }
        }
    })
}

fn df_access_from_spec(spec: &DfAccessSpec) -> Result<PtbDfAccess> {
    let mode = match spec.mode.as_str() {
        "read" => DfAccessMode::Read,
        "mutate" => DfAccessMode::Mutate,
        "create" => DfAccessMode::Create,
        "delete" => DfAccessMode::Delete,
        other => return Err(anyhow!("unknown df mode: {}", other)),
    };
    Ok(PtbDfAccess {
        parent: parse_object_id(&spec.parent)?,
        key_type: spec.key_type.clone(),
        key_bcs: parse_hex_bytes(&spec.key_bcs_hex)?,
        mode,
        value_type: spec.value_type.clone(),
    })
}

fn build_ptb(spec: &PtbSpec) -> Result<ProgrammableTransaction> {
    let inputs = spec
        .inputs
        .iter()
        .map(input_from_spec)
        .collect::<Result<Vec<_>>>()?;
    let commands = spec
        .commands
        .iter()
        .map(command_from_spec)
        .collect::<Result<Vec<_>>>()?;
    let dynamic_field_accesses = spec
        .dynamic_field_accesses
        .iter()
        .map(df_access_from_spec)
        .collect::<Result<Vec<_>>>()?;
    Ok(ProgrammableTransaction {
        inputs,
        commands,
        dynamic_field_accesses,
    })
}

// ─── Public entry point ──────────────────────────────────────────────────────

pub fn handle(spec_path: &str, out: Option<&str>) -> Result<()> {
    let raw = fs::read_to_string(spec_path)
        .with_context(|| format!("read spec file: {}", spec_path))?;
    let spec: PtbSpec = serde_json::from_str(&raw)
        .with_context(|| format!("parse spec JSON: {}", spec_path))?;
    let ptb = build_ptb(&spec)?;

    // Wire-level validation up front — surface errors to the test author
    // before they reach the validator. This catches Result-index OOB and the
    // other parser-level rules in `validate_wire`.
    ptb.validate_wire()
        .map_err(|e| anyhow!("validate_wire: {}", e))?;

    let bytes = bcs::to_bytes(&ptb).context("BCS encode ProgrammableTransaction")?;
    let hex_out = hex::encode(&bytes);

    match out {
        Some(path) => {
            let parent = Path::new(path).parent();
            if let Some(p) = parent {
                if !p.as_os_str().is_empty() {
                    fs::create_dir_all(p).ok();
                }
            }
            fs::write(path, &hex_out).with_context(|| format!("write {}", path))?;
        }
        None => {
            print!("{}", hex_out);
        }
    }
    Ok(())
}

/// Compute `blake3(bcs::to_bytes(modules: Vec<Vec<u8>>))` — the digest a
/// caller must pass into `package::authorize_upgrade(cap, policy, digest)`
/// for the validator-side gate at
/// `crates/setu-move-vm/src/engine.rs` to accept.
///
/// Inputs are hex strings (with or without `0x` prefix). Output is
/// lowercase hex of the 32-byte BLAKE3 digest, printed to stdout with no
/// trailing newline.
pub fn handle_bundle_digest(module_hex: &[String]) -> Result<()> {
    let mut modules: Vec<Vec<u8>> = Vec::with_capacity(module_hex.len());
    for (i, h) in module_hex.iter().enumerate() {
        let stripped = h.strip_prefix("0x").unwrap_or(h.as_str()).trim();
        let bytes = hex::decode(stripped)
            .with_context(|| format!("module #{} is not valid hex", i))?;
        modules.push(bytes);
    }
    let bcs_bytes = bcs::to_bytes(&modules)
        .context("BCS-encode Vec<Vec<u8>> for digest computation")?;
    let digest = blake3::hash(&bcs_bytes);
    print!("{}", hex::encode(digest.as_bytes()));
    Ok(())
}



#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(spec_json: serde_json::Value) -> ProgrammableTransaction {
        let spec: PtbSpec = serde_json::from_value(spec_json).expect("parse spec");
        let ptb = build_ptb(&spec).expect("build ptb");
        let bytes = bcs::to_bytes(&ptb).expect("encode");
        let decoded: ProgrammableTransaction = bcs::from_bytes(&bytes).expect("decode");
        assert_eq!(ptb, decoded, "round-trip mismatch");
        ptb
    }

    /// U1 — canonical 4-cmd upgrade flow round-trips.
    #[test]
    fn u1_canonical_upgrade_round_trips() {
        let cap = "0x".to_string() + &"a".repeat(64);
        let pkg = "0x".to_string() + &"b".repeat(64);
        let digest = "0x".to_string() + &"c".repeat(64);
        let modules_hex = "deadbeef".to_string();

        let spec = json!({
            "inputs": [
                { "kind": "imm_or_owned_object",
                  "id": cap, "version": 1, "digest": digest },
                { "kind": "pure_u8",  "value": 0 },
                { "kind": "pure_vec_u8_hex", "value": "abcd" },
                { "kind": "imm_or_owned_object",
                  "id": cap, "version": 1, "digest": digest }
            ],
            "commands": [
                { "kind": "move_call",
                  "package": "0x1", "module": "package", "function": "authorize_upgrade",
                  "type_arguments": [],
                  "arguments": [
                    { "kind": "input", "index": 0 },
                    { "kind": "input", "index": 1 },
                    { "kind": "input", "index": 2 }
                  ] },
                { "kind": "upgrade",
                  "modules_hex": [modules_hex],
                  "deps": [],
                  "current_package": pkg,
                  "upgrade_ticket": { "kind": "result", "index": 0 } },
                { "kind": "move_call",
                  "package": "0x1", "module": "package", "function": "commit_upgrade",
                  "type_arguments": [],
                  "arguments": [
                    { "kind": "input", "index": 3 },
                    { "kind": "result", "index": 1 }
                  ] }
            ]
        });
        let ptb = round_trip(spec);
        assert_eq!(ptb.inputs.len(), 4);
        assert_eq!(ptb.commands.len(), 3);
        assert!(matches!(ptb.commands[1], Command::Upgrade { .. }));
    }

    /// U2 — every Argument kind round-trips.
    #[test]
    fn u2_each_argument_kind_round_trips() {
        let spec = json!({
            "inputs": [{ "kind": "pure_u64", "value": 7 }],
            "commands": [
                { "kind": "make_move_vec", "type_tag": "u64",
                  "args": [
                    { "kind": "gas_coin" },
                    { "kind": "input", "index": 0 },
                    { "kind": "result", "index": 0 },
                    { "kind": "nested_result", "index": 0, "sub_index": 1 }
                  ] },
                { "kind": "make_move_vec", "args": [] }
            ]
        });
        let ptb = round_trip(spec);
        if let Command::MakeMoveVec { args, .. } = &ptb.commands[0] {
            assert_eq!(args[0], Argument::GasCoin);
            assert_eq!(args[1], Argument::Input(0));
            assert_eq!(args[2], Argument::Result(0));
            assert_eq!(args[3], Argument::NestedResult(0, 1));
        } else {
            panic!("expected MakeMoveVec");
        }
    }

    /// U3 — every CallArg kind round-trips.
    #[test]
    fn u3_each_input_kind_round_trips() {
        let id = "0x".to_string() + &"d".repeat(64);
        let digest = "0x".to_string() + &"e".repeat(64);
        let spec = json!({
            "inputs": [
                { "kind": "pure_u8",         "value": 1 },
                { "kind": "pure_u64",        "value": 42 },
                { "kind": "pure_bool",       "value": true },
                { "kind": "pure_vec_u8_hex", "value": "0xff00" },
                { "kind": "pure_address",    "value": "0xcafe" },
                { "kind": "pure_raw_hex",    "value": "0102" },
                { "kind": "imm_or_owned_object",
                  "id": id, "version": 5, "digest": digest },
                { "kind": "shared_object",
                  "id": id, "initial_shared_version": 9, "mutable": false }
            ],
            "commands": [
                { "kind": "make_move_vec", "args": [] }
            ]
        });
        let ptb = round_trip(spec);
        // PureU8 BCS is 1 byte = 0x01.
        assert!(matches!(&ptb.inputs[0], CallArg::Pure(b) if b == &vec![1u8]));
        // PureBool BCS true = 0x01.
        assert!(matches!(&ptb.inputs[2], CallArg::Pure(b) if b == &vec![1u8]));
        // PureAddress: short form 0xcafe → left-padded 32 bytes ending cafe.
        if let CallArg::Pure(addr_bytes) = &ptb.inputs[4] {
            assert_eq!(addr_bytes.len(), 32);
            assert_eq!(&addr_bytes[30..], &[0xca, 0xfe]);
            assert_eq!(&addr_bytes[..30], &[0u8; 30][..]);
        } else {
            panic!("expected Pure for address");
        }
        // PureRawHex: bytes pass through unchanged.
        assert!(matches!(&ptb.inputs[5], CallArg::Pure(b) if b == &vec![0x01, 0x02]));
        assert!(matches!(&ptb.inputs[6], CallArg::Object(ObjectArg::ImmOrOwnedObject(_, 5, _))));
        assert!(matches!(
            &ptb.inputs[7],
            CallArg::Object(ObjectArg::SharedObject { initial_shared_version: 9, mutable: false, .. })
        ));
    }

    /// U4 — every Command kind round-trips.
    #[test]
    fn u4_each_command_kind_round_trips() {
        let id = "0x".to_string() + &"f".repeat(64);
        let digest = "0x".to_string() + &"1".repeat(64);
        let spec = json!({
            "inputs": [
                { "kind": "imm_or_owned_object", "id": id, "version": 1, "digest": digest }
            ],
            "commands": [
                { "kind": "move_call",
                  "package": "0x1", "module": "m", "function": "f",
                  "type_arguments": ["u64"],
                  "arguments": [{ "kind": "input", "index": 0 }] },
                { "kind": "transfer_objects",
                  "objects": [{ "kind": "result", "index": 0 }],
                  "recipient": { "kind": "input", "index": 0 } },
                { "kind": "split_coins",
                  "coin": { "kind": "gas_coin" },
                  "amounts": [{ "kind": "input", "index": 0 }] },
                { "kind": "merge_coins",
                  "target": { "kind": "gas_coin" },
                  "sources": [{ "kind": "result", "index": 2 }] },
                { "kind": "make_move_vec",
                  "type_tag": "0x1::package::UpgradeCap",
                  "args": [] },
                { "kind": "publish", "modules_hex": ["0011"], "deps": [] },
                { "kind": "upgrade",
                  "modules_hex": ["2233"], "deps": [],
                  "current_package": "0x1",
                  "upgrade_ticket": { "kind": "result", "index": 0 } }
            ]
        });
        let ptb = round_trip(spec);
        assert_eq!(ptb.commands.len(), 7);
        assert!(matches!(&ptb.commands[0], Command::MoveCall(_)));
        assert!(matches!(&ptb.commands[1], Command::TransferObjects(_, _)));
        assert!(matches!(&ptb.commands[2], Command::SplitCoins(_, _)));
        assert!(matches!(&ptb.commands[3], Command::MergeCoins(_, _)));
        assert!(matches!(&ptb.commands[4], Command::MakeMoveVec { .. }));
        assert!(matches!(&ptb.commands[5], Command::Publish { .. }));
        assert!(matches!(&ptb.commands[6], Command::Upgrade { .. }));
    }

    /// U5 — typo / unknown field is rejected at JSON-parse time.
    #[test]
    fn u5_unknown_field_rejected() {
        let spec = json!({
            "inputs": [],
            "commandz": []   // typo
        });
        let err = serde_json::from_value::<PtbSpec>(spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("commandz") || msg.contains("unknown field"),
            "expected 'unknown field' rejection, got: {}",
            msg,
        );
    }

    /// U6 — Result index out of bounds is caught by validate_wire.
    #[test]
    fn u6_argument_index_oob_rejected() {
        // Single command referring to Result(99).
        let spec = json!({
            "inputs": [],
            "commands": [
                { "kind": "make_move_vec",
                  "args": [{ "kind": "result", "index": 99 }] }
            ]
        });
        let parsed: PtbSpec = serde_json::from_value(spec).expect("parse");
        let ptb = build_ptb(&parsed).expect("build");
        let err = ptb
            .validate_wire()
            .expect_err("validate_wire should fail for OOB Result");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("result") || msg.contains("index") || msg.contains("bound"),
            "expected OOB-related error, got: {}",
            msg,
        );
    }

    /// U7 — output is lowercase hex, no whitespace, no `0x`.
    #[test]
    fn u7_output_is_lowercase_hex() {
        let spec = json!({
            "inputs": [],
            "commands": [{ "kind": "make_move_vec", "args": [] }]
        });
        let parsed: PtbSpec = serde_json::from_value(spec).expect("parse");
        let ptb = build_ptb(&parsed).expect("build");
        let bytes = bcs::to_bytes(&ptb).expect("encode");
        let hex_out = hex::encode(&bytes);
        assert!(
            hex_out.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "expected lowercase hex only, got: {}",
            hex_out,
        );
        assert!(!hex_out.starts_with("0x"));
        assert!(!hex_out.contains(char::is_whitespace));
    }
}
