#!/usr/bin/env bash
# Build the move-compile tool (independent from Setu workspace)
#
# Usage:
#   bash tools/move-compile/build.sh          # release build
#   bash tools/move-compile/build.sh --debug  # debug build
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [[ "${1:-}" == "--debug" ]]; then
    echo "Building move-compile (debug)..."
    cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml"
    echo "Binary: $SCRIPT_DIR/target/debug/move-compile"
else
    echo "Building move-compile (release)..."
    cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"
    echo "Binary: $SCRIPT_DIR/target/release/move-compile"
fi
