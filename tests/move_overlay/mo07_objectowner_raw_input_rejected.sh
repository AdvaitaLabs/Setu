#!/usr/bin/env bash
# ============================================================================
# MO-07: ObjectOwner-owned object cannot be smuggled into input_object_ids.
#
# Bug ticket : docs/bugs/20260427-objectowner-mutable-ref-not-blocked.md
# Fix design : docs/feat/fix-objectowner-mutable-ref-not-blocked/design.md
#
# Property:
#   Dynamic-field entries (Ownership::ObjectOwner(parent)) must be addressed
#   exclusively through `dynamic_field_accesses[]`. Submitting their `df_oid`
#   directly in `input_object_ids` MUST be rejected by both
#   TaskPreparer (primary gate) and the TEE mock (defense-in-depth gate),
#   regardless of whether the index is also in mutable/consumed.
#   The on-disk DF envelope MUST remain byte-identical across all rejection
#   attempts.
#
# Sourcing strategy (FDP design.md §6.3, R1-ISSUE-3/4):
#   We source `tests/dynamic_fields/common.sh` because it provides the full
#   toolkit needed for this test (ensure_df_modules, derive_df_oid,
#   wait_object_exists) plus all the helpers (log_*, assert_*, move_*).
#
# Steps:
#   1. publish examples::df_registry (alice)
#   2. alice create_owned() → REG_ID
#   3. alice put_u64(k=1, v=42) via dynamic_field_accesses[Create]
#   4. compute DF_OID = derive_df_oid(REG_ID, "u64", BCS(1))
#   5. probe DF_OID, record sha256(data) and version V0
#   6. negative A: assert_u64 with input_object_ids=[DF_OID], no DF accesses
#                  → expect success=false, error mentions ObjectOwner
#   7. negative B: same + mutable_indices=[0]
#   8. negative C: same + consumed_indices=[0]
#   9. probe DF_OID again, sha256+version unchanged
#  10. positive control: assert_u64 via legitimate
#                        dynamic_field_accesses[mode=Read] succeeds
# ============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# Use the DF common.sh (provides ensure_df_modules + derive_df_oid + all
# generic helpers). It self-locates PROJECT_ROOT via its own SCRIPT_DIR.
source "${PROJECT_ROOT}/tests/dynamic_fields/common.sh"

echo "========================================================"
echo "  MO-07: ObjectOwner cannot be raw input_object_ids"
echo "========================================================"

ensure_df_modules
start_services

MOD="df_registry"
KEY_BCS=$(printf '%016x' 1 | tac -rs ..)  # u64=1 LE; fallback below
# Fallback: portable u64-LE hex via python
u64_le_hex() { python3 -c "import sys; print(int(sys.argv[1]).to_bytes(8,'little').hex())" "$1"; }
KEY_BCS=$(u64_le_hex 1)
VAL42_BCS=$(u64_le_hex 42)
EXP42_BCS=$(u64_le_hex 42)

# ── Step 1: publish ────────────────────────────────────────────────────────
log_step "alice publishes examples::df_registry"
RESP=$(move_publish alice "$DF_REGISTRY_HEX")
assert_eq "publish success" "true" "$(json_val "$RESP" success)"
sleep 1

# ── Step 2: create_owned ───────────────────────────────────────────────────
log_step "alice create_owned()"
BODY=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"create_owned","type_args":[],"args":[],"input_object_ids":[],"shared_object_ids":[],"needs_tx_context":true}
JSON
)
RESP=$(move_call_raw "$BODY")
assert_eq "create_owned success" "true" "$(json_val "$RESP" success)"
REG_ID=$(extract_first_object "$RESP")
if [ -z "$REG_ID" ]; then
    log_fail "no registry_id"; print_summary || exit 1
fi
log_info "registry_id=${REG_ID}"

# ── Step 3: put_u64(Create) ────────────────────────────────────────────────
log_step "alice put_u64(k=1, v=42) via dynamic_field_accesses[Create]"
BODY=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"put_u64","type_args":[],"args":["${KEY_BCS}","${VAL42_BCS}"],"input_object_ids":["${REG_ID}"],"shared_object_ids":[],"mutable_indices":[0],"needs_tx_context":false,"dynamic_field_accesses":[{"parent_object_id":"${REG_ID}","key_type":"u64","key_bcs_hex":"${KEY_BCS}","mode":"Create","value_type":"u64"}]}
JSON
)
RESP=$(move_call_raw "$BODY")
assert_eq "put_u64(Create) success" "true" "$(json_val "$RESP" success)"
sleep 1

# ── Step 4: compute & locate the DF oid ────────────────────────────────────
DF_OID=$(derive_df_oid "$REG_ID" "u64" "$KEY_BCS")
log_info "df_oid=${DF_OID}"
DF_OBJ=$(wait_object_exists "$DF_OID" 15 || true)
assert_eq "df_oid object exists" "true" "$(json_val "$DF_OBJ" exists)"
assert_contains "df_oid is ObjectOwner" "ObjectOwner" "$(json_val "$DF_OBJ" ownership)"

# ── Step 5: pre-rejection probe ────────────────────────────────────────────
PRE=$(curl -s "${VALIDATOR_URL}/api/v1/move/objects/${DF_OID}" 2>/dev/null)
V0=$(json_val "$PRE" version)
PRE_HASH=$(echo -n "$PRE" | shasum -a 256 | awk '{print $1}')
log_info "pre-rejection: version=${V0}, sha256=${PRE_HASH}"

# ── Step 6: negative A — raw read via input_object_ids ─────────────────────
log_step "Negative A: assert_u64 with input_object_ids=[DF_OID], no DF accesses"
NEG_A=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"assert_u64","type_args":[],"args":["${KEY_BCS}","${EXP42_BCS}"],"input_object_ids":["${DF_OID}"],"shared_object_ids":[],"needs_tx_context":false}
JSON
)
RESP=$(move_call_raw "$NEG_A")
SUCCESS=$(json_val "$RESP" success)
ERR=$(json_val "$RESP" error)
assert_eq "negative A success=false" "false" "$SUCCESS"
assert_contains "negative A error mentions ObjectOwner" "ObjectOwner" "$ERR"

# ── Step 7: negative B — raw + mutable_indices ─────────────────────────────
log_step "Negative B: same + mutable_indices=[0]"
NEG_B=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"assert_u64","type_args":[],"args":["${KEY_BCS}","${EXP42_BCS}"],"input_object_ids":["${DF_OID}"],"shared_object_ids":[],"mutable_indices":[0],"needs_tx_context":false}
JSON
)
RESP=$(move_call_raw "$NEG_B")
SUCCESS=$(json_val "$RESP" success)
ERR=$(json_val "$RESP" error)
assert_eq "negative B success=false" "false" "$SUCCESS"
assert_contains "negative B error mentions ObjectOwner" "ObjectOwner" "$ERR"

# ── Step 8: negative C — raw + consumed_indices ────────────────────────────
log_step "Negative C: same + consumed_indices=[0]"
NEG_C=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"assert_u64","type_args":[],"args":["${KEY_BCS}","${EXP42_BCS}"],"input_object_ids":["${DF_OID}"],"shared_object_ids":[],"consumed_indices":[0],"needs_tx_context":false}
JSON
)
RESP=$(move_call_raw "$NEG_C")
SUCCESS=$(json_val "$RESP" success)
ERR=$(json_val "$RESP" error)
assert_eq "negative C success=false" "false" "$SUCCESS"
assert_contains "negative C error mentions ObjectOwner" "ObjectOwner" "$ERR"

# ── Step 9: post-rejection probe ───────────────────────────────────────────
log_step "Re-probe DF_OID — version & data must be unchanged"
POST=$(curl -s "${VALIDATOR_URL}/api/v1/move/objects/${DF_OID}" 2>/dev/null)
V1=$(json_val "$POST" version)
POST_HASH=$(echo -n "$POST" | shasum -a 256 | awk '{print $1}')
assert_eq "version unchanged after 3 rejections" "$V0" "$V1"
assert_eq "GET payload byte-identical pre/post" "$PRE_HASH" "$POST_HASH"

# ── Step 10: positive control ──────────────────────────────────────────────
log_step "Positive control: legitimate dynamic_field_accesses[Read] still works"
POS=$(cat <<JSON
{"sender":"${ALICE}","package":"${PKG}","module":"${MOD}","function":"assert_u64","type_args":[],"args":["${KEY_BCS}","${EXP42_BCS}"],"input_object_ids":["${REG_ID}"],"shared_object_ids":[],"needs_tx_context":false,"dynamic_field_accesses":[{"parent_object_id":"${REG_ID}","key_type":"u64","key_bcs_hex":"${KEY_BCS}","mode":"Read"}]}
JSON
)
RESP=$(move_call_raw "$POS")
assert_eq "positive control success=true" "true" "$(json_val "$RESP" success)"

print_summary || exit 1
