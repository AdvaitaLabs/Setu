#!/usr/bin/env bash
# tests/move_overlay/run_all.sh — run every MoveCall-overlay integration test.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

TESTS=(
    "mo01_read_your_writes.sh"
    "mo02_sequential_increments.sh"
    "mo03_cross_cf_handoff.sh"
    "mo04_restart_persistence.sh"
    "mo05_batch_mixed_sender.sh"
    "mo06_immutable_mutate_rejected.sh"
    "mo07_objectowner_raw_input_rejected.sh"
)

TOTAL_SUITES=${#TESTS[@]}
PASSED_SUITES=0
FAILED_SUITES=()

echo "════════════════════════════════════════════════════════"
echo "  Setu MoveCall Speculative Overlay — Integration Suite"
echo "  Running ${TOTAL_SUITES} suites"
echo "════════════════════════════════════════════════════════"

for t in "${TESTS[@]}"; do
    echo ""
    echo "▶▶▶  ${t}"
    echo "────────────────────────────────────────────────────────"
    if bash "${SCRIPT_DIR}/${t}"; then
        PASSED_SUITES=$((PASSED_SUITES + 1))
        echo "✅  ${t} PASSED"
    else
        FAILED_SUITES+=("${t}")
        echo "❌  ${t} FAILED (rc=$?)"
    fi
done

echo ""
echo "════════════════════════════════════════════════════════"
echo "  Summary: ${PASSED_SUITES}/${TOTAL_SUITES} suites passed"
if [ ${#FAILED_SUITES[@]} -gt 0 ]; then
    echo "  FAILED:"
    for f in "${FAILED_SUITES[@]}"; do echo "    - $f"; done
    echo "════════════════════════════════════════════════════════"
    exit 1
fi
echo "════════════════════════════════════════════════════════"
