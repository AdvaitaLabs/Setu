#!/bin/bash
# scripts/build_stdlib.sh — 编译 setu-framework Move 模块到 .mv 字节码
#
# 使用 tools/move-compile (非 Sui CLI)，避免 Sui CLI 版本兼容问题。
# 添加新 Move 模块后须运行此脚本。
#
# Usage:
#   bash scripts/build_stdlib.sh          # 编译全部 + 一致性检查
#   bash scripts/build_stdlib.sh --check  # 仅检查一致性（不编译）

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPILER="$ROOT/tools/move-compile/target/release/move-compile"
SOURCES="$ROOT/setu-framework/sources"
COMPILED="$ROOT/setu-framework/compiled"
BUILDRS="$ROOT/crates/setu-move-vm/build.rs"

CHECK_ONLY=false
if [[ "${1:-}" == "--check" ]]; then
    CHECK_ONLY=true
fi

# ── Check compiler binary ───────────────────────────────────
if [[ "$CHECK_ONLY" == false ]]; then
    if [[ ! -x "$COMPILER" ]]; then
        echo "⏳ move-compile not found, building..."
        (cd "$ROOT/tools/move-compile" && cargo build --release --quiet)
        echo "✓ move-compile built"
    fi

    # ── Compile ──────────────────────────────────────────────
    echo "🔨 Compiling Move sources..."
    "$COMPILER" \
        "$SOURCES" \
        --out "$COMPILED" \
        --addr setu=0x1 \
        --addr std=0x1
fi

# ── Verify build.rs module list consistency ──────────────────
# Get compiled .mv file names (excluding .gitkeep)
COMPILED_MODULES=$(ls "$COMPILED/"*.mv 2>/dev/null | xargs -I{} basename {} .mv | sort)

# Extract module names from build.rs (lines matching "name" inside the modules array)
BUILDRS_MODULES=$(grep -E '^\s+"[a-z_]+"' "$BUILDRS" | sed 's/.*"\([a-z_]*\)".*/\1/' | sort)

DIFF=$(diff <(echo "$COMPILED_MODULES") <(echo "$BUILDRS_MODULES") 2>/dev/null || true)
if [[ -n "$DIFF" ]]; then
    echo ""
    echo "⚠️  build.rs modules list out of sync with compiled/*.mv"
    EXTRA_COMPILED=$(comm -23 <(echo "$COMPILED_MODULES") <(echo "$BUILDRS_MODULES"))
    EXTRA_BUILDRS=$(comm -13 <(echo "$COMPILED_MODULES") <(echo "$BUILDRS_MODULES"))
    if [[ -n "$EXTRA_COMPILED" ]]; then
        echo "   Compiled but NOT in build.rs:"
        echo "$EXTRA_COMPILED" | sed 's/^/     + /'
    fi
    if [[ -n "$EXTRA_BUILDRS" ]]; then
        echo "   In build.rs but NOT compiled:"
        echo "$EXTRA_BUILDRS" | sed 's/^/     - /'
    fi
    echo ""
    echo "   → Update crates/setu-move-vm/build.rs modules array!"
    exit 1
fi

# ── Check freshness (compiled newer than source?) ────────────
STALE=0
for mv_file in "$COMPILED/"*.mv; do
    mod_name=$(basename "$mv_file" .mv)
    move_file="$SOURCES/${mod_name}.move"
    if [[ -f "$move_file" && "$move_file" -nt "$mv_file" ]]; then
        echo "⚠️  $mod_name.mv is older than $mod_name.move"
        STALE=1
    fi
done
if [[ $STALE -eq 1 && "$CHECK_ONLY" == true ]]; then
    echo "   → Run: bash scripts/build_stdlib.sh"
    exit 1
fi

echo "✓ stdlib: $(echo "$COMPILED_MODULES" | wc -w | tr -d ' ') modules, build.rs in sync"
