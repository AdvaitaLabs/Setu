//! API-specific types
//!
//! Types used by the HTTP API layer that are not part of the core RPC protocol.

use serde::{Deserialize, Serialize};
use setu_types::event::{DynamicFieldAccess, Event};

// ============================================
// Event Submission
// ============================================

/// Request to submit an event to the validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventRequest {
    /// The event to submit
    pub event: Event,
}

/// Response to event submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventResponse {
    /// Whether submission was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Event ID
    pub event_id: Option<String>,
    /// VLC time assigned
    pub vlc_time: Option<u64>,
}

// ============================================
// State Query Types (Scheme B)
// ============================================

/// Response for balance query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceResponse {
    /// Account address
    pub account: String,
    /// Balance amount
    pub balance: u128,
    /// Whether the account exists
    pub exists: bool,
}

/// Response for object query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetObjectResponse {
    /// Object key
    pub key: String,
    /// Object value (if exists)
    pub value: Option<Vec<u8>>,
    /// Whether the object exists
    pub exists: bool,
}

// ============================================
// Move VM Types (Phase 4)
// ============================================

/// Request to call a Move function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCallRequest {
    /// Transaction sender address (hex)
    pub sender: String,
    /// Package address (hex) where the module lives
    pub package: String,
    /// Module name
    pub module: String,
    /// Function name
    pub function: String,
    /// Type arguments (string representation)
    #[serde(default)]
    pub type_args: Vec<String>,
    /// Pure arguments (hex-encoded BCS bytes)
    #[serde(default)]
    pub args: Vec<String>,
    /// Input object IDs (hex-encoded ObjectIds) — must be owned by sender (or Immutable)
    #[serde(default)]
    pub input_object_ids: Vec<String>,
    /// Shared object IDs (hex-encoded ObjectIds) — must have `Ownership::Shared` (PWOO)
    #[serde(default)]
    pub shared_object_ids: Vec<String>,
    /// Indices into the concatenated `[input_object_ids..., shared_object_ids...]`
    /// list that are mutably borrowed.
    #[serde(default)]
    pub mutable_indices: Vec<usize>,
    /// Indices into `input_object_ids` that are consumed (transferred/deleted).
    /// Must be < `input_object_ids.len()` — shared objects cannot be consumed in PWOO Phase 1.
    #[serde(default)]
    pub consumed_indices: Vec<usize>,
    /// Whether the function needs TxContext injection
    #[serde(default = "default_true")]
    pub needs_tx_context: bool,
    /// Target subnet (defaults to ROOT)
    #[serde(default)]
    pub subnet_id: Option<String>,
    /// Dynamic-field access declarations (DF FDP M5).
    /// Each entry declares a (parent, key, mode) triple so the validator
    /// can preload the DF envelope + Merkle proof before TEE execution.
    /// Empty by default — pre-M5 JSON clients keep working unchanged.
    #[serde(default)]
    pub dynamic_field_accesses: Vec<DynamicFieldAccess>,
}

/// Response to Move function call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCallResponse {
    /// Event ID of the submitted event
    pub event_id: String,
    /// Whether execution succeeded
    pub success: bool,
    /// Number of state changes produced
    pub state_changes: usize,
    /// Created object IDs (hex-encoded, "oid:{hex}" keys from state changes)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_objects: Vec<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request to publish Move modules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePublishRequest {
    /// Publisher address (hex)
    pub sender: String,
    /// Compiled module bytecodes (hex-encoded)
    pub modules: Vec<String>,
}

/// Response to Move module publish
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePublishResponse {
    /// Event ID of the submitted event
    pub event_id: String,
    /// Number of modules published
    pub module_count: usize,
    /// Whether publish succeeded
    pub success: bool,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================
// Programmable Transaction Block (PTB) — executable
// ============================================
//
// Submission endpoint for Programmable Transaction Blocks. After FDP
// `move-vm-phase9-ptb-event-wire`, the validator parses + validates the PTB,
// then dispatches it through TaskPreparer → solver → engine.execute_ptb.
// Malformed PTBs are rejected with HTTP 4xx; successful executions return
// HTTP 200.

/// Stable error code historically returned (HTTP 501) by the B6a stub branch
/// of `submit_move_ptb` for well-formed PTBs. After FDP
/// `move-vm-phase9-ptb-event-wire` removed the stub, the live system no
/// longer emits this code; the constant is retained only for backward
/// compatibility with B6a-era clients still pattern-matching on it.
pub const PTB_NOT_YET_EXECUTABLE: &str = "PTB_NOT_YET_EXECUTABLE";

/// Request to submit a Programmable Transaction Block.
///
/// `ptb` is BCS-encoded and hex-wrapped over the wire to keep the JSON
/// envelope simple — the binary form is what the validator hashes/verifies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePtbRequest {
    /// Sender address (hex)
    pub sender: String,
    /// BCS-encoded `setu_types::ptb::ProgrammableTransaction`, hex-encoded.
    pub ptb: String,
    /// Target subnet (defaults to ROOT)
    #[serde(default)]
    pub subnet_id: Option<String>,
    /// B6c · Optional max gas units for this PTB. When omitted, the
    /// validator applies a default within `[MIN_GAS_PTB..MAX_GAS_BUDGET]`.
    /// Submissions outside that range are rejected at the validator entry
    /// (`network/move_handler.rs`); see B6c design §4.5.
    #[serde(default)]
    pub gas_budget: Option<u64>,
}

/// Response to a PTB submission.
///
/// HTTP mapping (post `move-vm-phase9-ptb-event-wire`):
///   * `success = true`  → HTTP 200, `event_id` populated, `code = None`.
///   * `success = false` → HTTP 400, `error` describes the wire/domain
///                          rejection, `code = None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePtbResponse {
    /// Event ID of the submitted event (empty when wire validation failed).
    pub event_id: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// Error message (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Reserved for stable error codes. Currently always `None`; retained as
    /// an extension point for future error taxonomies and for backward
    /// compatibility with B6a-era clients that pattern-matched on
    /// `PTB_NOT_YET_EXECUTABLE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

fn default_true() -> bool {
    true
}

// ============================================
// Move Object/Module Query Types (Phase 5b)
// ============================================

/// Response for Move object query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMoveObjectResponse {
    /// Object key ("oid:{hex}")
    pub key: String,
    /// Object ID (hex)
    pub object_id: String,
    /// Owner address (hex)
    pub owner: String,
    /// Ownership type
    pub ownership: String,
    /// Move type tag (e.g. "0x1::coin::Coin<0x1::setu::SETU>")
    pub type_tag: String,
    /// Object version
    pub version: u64,
    /// BCS-serialized data (hex)
    pub data_hex: String,
    /// Whether the object exists
    pub exists: bool,
    /// Error message (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response for module ABI query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetModuleAbiResponse {
    /// Module address
    pub address: String,
    /// Module name
    pub name: String,
    /// Public/entry functions
    pub functions: Vec<FunctionAbi>,
    /// Whether the module exists
    pub exists: bool,
    /// Error message (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Function ABI info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionAbi {
    /// Function name
    pub name: String,
    /// Number of type parameters
    pub type_param_count: usize,
    /// Parameter types (string representation)
    pub parameters: Vec<String>,
    /// Whether this is an entry function
    pub is_entry: bool,
}

/// Response for listing modules at an address
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListModulesResponse {
    /// Module address
    pub address: String,
    /// Module names
    pub modules: Vec<String>,
    /// Error message (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================
// R5 · Event lookup (GET /api/v1/event/:id)
// ============================================

/// Chain-level verdict projection exposed to HTTP clients.
///
/// Distinct from `setu_types::ExecutionOutcome` so the API can evolve its
/// wire shape (e.g. add fields, rename) without touching consensus/storage
/// layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OnChainOutcome {
    Applied {
        cf_id: String,
    },
    ExecutionFailed {
        cf_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    StaleRead {
        cf_id: String,
        conflicting_object: String,
        retry_hint: String,
    },
}

/// Off-chain (TEE / solver) execution result snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub state_changes_count: usize,
}

/// Minimal event metadata for client display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    pub event_type: String,
    pub creator: String,
    pub timestamp: u64,
    pub vlc_time: u64,
    pub parent_count: usize,
}

/// Response for `GET /api/v1/event/:id`.
///
/// `on_chain` is `None` until the event's CF is finalized by the local node.
/// `execution` is `None` for events that never reached the solver (e.g.
/// Genesis / validator-executed system events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetEventResponse {
    pub event_id: String,
    pub status: String,
    pub execution: Option<ExecutionReport>,
    pub on_chain: Option<OnChainOutcome>,
    pub metadata: EventMetadata,
}

// ============================================
// M5-Pre tests — MoveCallRequest.dynamic_field_accesses
// ============================================

#[cfg(test)]
mod m5_pre_tests {
    use super::*;
    use setu_types::dynamic_field::DfAccessMode;

    /// Pre-M5 clients that omit `dynamic_field_accesses` must still deserialize;
    /// field defaults to an empty Vec. This guarantees wire back-compat.
    #[test]
    fn move_call_request_defaults_df_empty_when_missing() {
        let json = r#"{
            "sender": "alice",
            "package": "0xcafe",
            "module": "m",
            "function": "f"
        }"#;
        let req: MoveCallRequest = serde_json::from_str(json).unwrap();
        assert!(req.dynamic_field_accesses.is_empty());
        // Sanity: other defaulted fields unaffected.
        assert!(req.needs_tx_context);
    }

    /// Serialize → deserialize preserves every DF access entry end-to-end.
    #[test]
    fn move_call_request_df_roundtrip() {
        let orig = MoveCallRequest {
            sender: "alice".to_string(),
            package: "0xcafe".to_string(),
            module: "df_registry".to_string(),
            function: "put_u64".to_string(),
            type_args: vec![],
            args: vec![],
            input_object_ids: vec!["aa".repeat(32)],
            shared_object_ids: vec![],
            mutable_indices: vec![0],
            consumed_indices: vec![],
            needs_tx_context: false,
            subnet_id: None,
            dynamic_field_accesses: vec![DynamicFieldAccess {
                parent_object_id: format!("oid:{}", "aa".repeat(32)),
                key_type: "u64".to_string(),
                key_bcs_hex: "0100000000000000".to_string(),
                mode: DfAccessMode::Create,
                value_type: Some("u64".to_string()),
            }],
        };
        let json = serde_json::to_string(&orig).unwrap();
        let back: MoveCallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dynamic_field_accesses.len(), 1);
        assert_eq!(back.dynamic_field_accesses[0].mode, DfAccessMode::Create);
        assert_eq!(
            back.dynamic_field_accesses[0].value_type.as_deref(),
            Some("u64")
        );
    }
}

// ============================================
// B6a PTB wire-stub tests (Phase 2 matrix I1, I2, I4)
// ============================================

#[cfg(test)]
mod b6a_ptb_tests {
    use super::*;
    use setu_types::dynamic_field::DfAccessMode;
    use setu_types::object::ObjectId;
    use setu_types::ptb::{
        Argument, CallArg, Command, MoveCall, ProgrammableTransaction, MAX_PTB_COMMANDS,
    };

    fn well_formed_ptb() -> ProgrammableTransaction {
        ProgrammableTransaction {
            inputs: vec![CallArg::Pure(vec![1, 2, 3])],
            commands: vec![Command::MoveCall(MoveCall {
                package: ObjectId::ZERO,
                module: "m".to_string(),
                function: "f".to_string(),
                type_arguments: vec![],
                arguments: vec![Argument::GasCoin],
            })],
            dynamic_field_accesses: vec![],
        }
    }

    /// Helper exercising the wire-validation steps that the live
    /// `submit_move_ptb` performs before delegating to the executor:
    /// hex → BCS → `validate_wire`. The terminal branches we test:
    ///   * Ok  — wire validation passed (live system would proceed to
    ///           prepare/route/execute; this helper stops there).
    ///   * Err — wire validation rejected the PTB (4xx in the live system).
    ///
    /// This is intentionally decoupled from the validator service so the
    /// tests stay pure-unit. End-to-end coverage of the live path lives in
    /// `tests/end-to-end/e2e_ptb_wire_smoke.sh`.
    fn drive_wire_validation(req: MovePtbRequest) -> Result<(), MovePtbResponse> {
        let hex_str = req.ptb.trim_start_matches("0x");
        let bcs_bytes = hex::decode(hex_str).map_err(|e| MovePtbResponse {
            event_id: String::new(),
            success: false,
            error: Some(format!("Invalid hex in `ptb` field: {}", e)),
            code: None,
        })?;
        let ptb: ProgrammableTransaction =
            bcs::from_bytes(&bcs_bytes).map_err(|e| MovePtbResponse {
                event_id: String::new(),
                success: false,
                error: Some(format!("BCS deserialize failed: {}", e)),
                code: None,
            })?;
        ptb.validate_wire().map_err(|e| MovePtbResponse {
            event_id: String::new(),
            success: false,
            error: Some(format!("PTB validation failed: {}", e)),
            code: None,
        })?;
        Ok(())
    }

    /// I1: well-formed PTB passes wire validation (live system would proceed
    /// to execute; pre-event-wire this returned a 501 stub, now it does not).
    #[test]
    fn i1_submit_move_ptb_well_formed_passes_wire_validation() {
        let ptb = well_formed_ptb();
        let req = MovePtbRequest {
            sender: "alice".into(),
            ptb: hex::encode(bcs::to_bytes(&ptb).unwrap()),
            subnet_id: None,
        };
        drive_wire_validation(req).expect("well-formed PTB must pass wire validation");
    }

    /// I2: malformed PTB (over MAX_PTB_COMMANDS) is rejected at wire
    /// validation — maps to HTTP 4xx in the live system. The `code` field
    /// must be `None`: there is no longer a `PTB_NOT_YET_EXECUTABLE` marker
    /// to distinguish from (event-wire FDP removed it).
    #[test]
    fn i2_submit_move_ptb_malformed_rejected_with_code_none() {
        let mut ptb = well_formed_ptb();
        ptb.commands = (0..MAX_PTB_COMMANDS + 1)
            .map(|_| {
                Command::MoveCall(MoveCall {
                    package: ObjectId::ZERO,
                    module: "m".into(),
                    function: "f".into(),
                    type_arguments: vec![],
                    arguments: vec![],
                })
            })
            .collect();
        let req = MovePtbRequest {
            sender: "alice".into(),
            ptb: hex::encode(bcs::to_bytes(&ptb).unwrap()),
            subnet_id: None,
        };
        let resp = drive_wire_validation(req).expect_err("oversize PTB must be rejected");
        assert!(!resp.success);
        assert!(resp.code.is_none(),
            "Plain validation error — no PTB_NOT_YET_EXECUTABLE marker after event-wire FDP.");
        assert!(resp.error.unwrap().contains("validation failed"));
    }

    /// I4: HTTP boundary converts hex DynamicFieldAccess → binary PtbDfAccess.
    /// Verifies the conversion works for a typical mixed-case hex with the
    /// "oid:" prefix pattern the existing API uses.
    #[test]
    fn i4_hex_to_binary_df_conversion_at_handler_boundary() {
        use setu_types::event::DynamicFieldAccess;
        use setu_types::ptb::PtbDfAccess;

        let parent_bytes = [0xABu8; 32];
        let hex_form = DynamicFieldAccess {
            parent_object_id: format!("oid:0x{}", hex::encode_upper(parent_bytes)),
            key_type: "u64".into(),
            key_bcs_hex: "0xDEADBEEF".into(),
            mode: DfAccessMode::Read,
            value_type: None,
        };
        let bin: PtbDfAccess = hex_form.try_into().expect("conversion succeeds");
        assert_eq!(bin.parent.as_bytes(), &parent_bytes);
        assert_eq!(bin.key_bcs, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // And: round-tripping the binary form through bcs is byte-stable.
        let bytes_a = bcs::to_bytes(&bin).unwrap();
        let bytes_b = bcs::to_bytes(&bin).unwrap();
        assert_eq!(bytes_a, bytes_b);
    }
}
