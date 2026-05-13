#!/bin/bash
# ============================================================================
# 编译 + 分发: 同步源码到构建服务器, cargo build, 分发二进制到所有节点
# 用法: ./build.sh [--skip-sync] [--skip-distribute]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

SKIP_SYNC=false
SKIP_DIST=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-sync)       SKIP_SYNC=true; shift ;;
        --skip-distribute) SKIP_DIST=true; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

# Expand CARGO_FEATURES LOCALLY (not inside the remote heredoc). ssh does not
# forward env vars, so ${CARGO_FEATURES} must be interpolated before the
# command is sent over the wire.
LOCAL_FEATURE_FLAGS=""
if [ -n "${CARGO_FEATURES:-}" ]; then
    LOCAL_FEATURE_FLAGS="--features ${CARGO_FEATURES}"
fi

SOURCE_FINGERPRINT_TOOL="${SCRIPT_DIR}/source_fingerprint.py"
LOCAL_SOURCE_FINGERPRINT="$(python3 "${SOURCE_FINGERPRINT_TOOL}" "${PROJECT_DIR}")"
LOCAL_GIT_COMMIT="$(git -C "${PROJECT_DIR}" rev-parse --short HEAD 2>/dev/null || echo unknown)"
BUILD_TIME_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

copy_remote_artifact_atomically() {
    local host="$1"
    local artifact="$2"
    local executable="${3:-0}"
    local tmp_path="${REMOTE_BIN}/.${artifact}.new.$$"
    local attempt

    for attempt in 1 2 3 4 5; do
        if remote_to_remote_copy "$BUILD_SERVER" "${REMOTE_BIN}/${artifact}" "$host" "$tmp_path"; then
            if [ "$executable" = "1" ]; then
                if remote_exec "$host" "chmod +x '${tmp_path}' && mv -f '${tmp_path}' '${REMOTE_BIN}/${artifact}'"; then
                    return 0
                fi
            elif remote_exec "$host" "mv -f '${tmp_path}' '${REMOTE_BIN}/${artifact}'"; then
                return 0
            fi
        fi

        print_warn "${host}: 分发 ${artifact} 失败，重试 ${attempt}/5"
        sleep 5
    done

    print_err "${host}: 分发 ${artifact} 失败"
    return 1
}

print_header "Setu 构建 & 分发"
echo "  源码指纹: ${LOCAL_SOURCE_FINGERPRINT}"
echo "  Git commit: ${LOCAL_GIT_COMMIT}"
if [ -n "$LOCAL_FEATURE_FLAGS" ]; then
    echo "  Feature flags: ${LOCAL_FEATURE_FLAGS}"
fi

# ── Step 1: 同步源码到构建服务器 ────────────────────────────────────────────
if [ "$SKIP_SYNC" = false ]; then
    print_step 1 5 "同步源码到构建服务器 (${BUILD_SERVER})..."
    
    # 确保远程源码目录存在
    remote_exec "$BUILD_SERVER" "mkdir -p ${REMOTE_SRC}"
    
    remote_sync "${PROJECT_DIR}/" "$BUILD_SERVER" "${REMOTE_SRC}/"
    print_ok "源码同步完成"
else
    print_step 1 5 "跳过源码同步"
fi

# ── Step 2: 编译 Move stdlib 字节码 ─────────────────────────────────────────
# The validator binary embeds setu-framework/compiled/*.mv at compile time.
# The build server regenerates these files from setu-framework/sources/ before
# cargo build so the remote binary always carries the current stdlib bytecode.
print_step 2 5 "编译 Move stdlib (.mv 字节码)..."
remote_exec "$BUILD_SERVER" "
    set -eo pipefail
    source \"\$HOME/.cargo/env\" 2>/dev/null || true
    cd ${REMOTE_SRC}

    # Build move-compile if not present
    if [ ! -x tools/move-compile/target/release/move-compile ]; then
        echo '  building tools/move-compile...'
        (cd tools/move-compile && cargo build --release --quiet)
    fi

    # Compile setu-framework sources → compiled/*.mv
    mkdir -p setu-framework/compiled
    ./tools/move-compile/target/release/move-compile \\
        setu-framework/sources \\
        --out setu-framework/compiled \\
        --addr setu=0x1 \\
        --addr std=0x1

    # Sanity: must have at least 15 .mv files now (matches build.rs modules[] list)
    count=\$(ls setu-framework/compiled/*.mv 2>/dev/null | wc -l)
    if [ \"\$count\" -lt 15 ]; then
        echo \"FATAL: only \$count .mv files compiled, expected >=15\"
        exit 1
    fi
    echo \"  ✓ \$count stdlib modules compiled\"
"
print_ok "stdlib 编译完成"

# ── Step 3: 远程编译 ────────────────────────────────────────────────────────
print_step 3 5 "在构建服务器上编译 (release)..."
echo "  构建目标: setu-validator, setu-solver, setu-cli, setu-benchmark"
echo "  (首次编译可能需要 20-40 分钟，请耐心等待...)"

if [ -n "$LOCAL_FEATURE_FLAGS" ]; then
    echo "  [build] forwarding feature flags to remote: ${LOCAL_FEATURE_FLAGS}"
fi

remote_exec "$BUILD_SERVER" "
    set -eo pipefail
    source \"\$HOME/.cargo/env\" 2>/dev/null || true
    cd ${REMOTE_SRC}

    # Optional diag feature for validator only. Solver/cli/benchmark don't
    # declare this feature, so we must split into two cargo invocations.
    FEATURE_FLAGS='${LOCAL_FEATURE_FLAGS}'
    if [ -n \"\$FEATURE_FLAGS\" ]; then
        echo \"  [build] validator uses \$FEATURE_FLAGS\"
    fi

    # validator — carries optional feature
    cargo build --release \\
        -p setu-validator \\
        \$FEATURE_FLAGS \\
        2>&1

    # solver / cli / benchmark — never carry the feature
    cargo build --release \\
        -p setu-solver \\
        -p setu-cli \\
        -p setu-benchmark \\
        2>&1
"
print_ok "编译完成"

# ── Step 4: 复制二进制到 bin 目录 ───────────────────────────────────────────
print_step 4 5 "安装二进制到构建服务器..."
remote_exec "$BUILD_SERVER" "
    set -eo pipefail
    install_bin() {
        name=\"\$1\"
        src=\"${REMOTE_SRC}/target/release/\${name}\"
        tmp=\"${REMOTE_BIN}/.\${name}.new.\$\$\"
        cp \"\$src\" \"\$tmp\"
        chmod +x \"\$tmp\"
        mv -f \"\$tmp\" \"${REMOTE_BIN}/\${name}\"
    }

    install_bin setu-validator
    install_bin setu-solver
    if [ -f ${REMOTE_SRC}/target/release/setu-cli ]; then install_bin setu-cli; fi
    if [ -f ${REMOTE_SRC}/target/release/setu-benchmark ]; then install_bin setu-benchmark; fi

    cat > ${REMOTE_BIN}/setu-build-info.env <<'EOF'
SETU_SOURCE_FINGERPRINT=${LOCAL_SOURCE_FINGERPRINT}
SETU_GIT_COMMIT=${LOCAL_GIT_COMMIT}
SETU_FEATURE_FLAGS=${LOCAL_FEATURE_FLAGS}
SETU_BUILD_TIME_UTC=${BUILD_TIME_UTC}
EOF

    ls -lh ${REMOTE_BIN}/
    cat ${REMOTE_BIN}/setu-build-info.env
"
print_ok "构建服务器 (${BUILD_SERVER}) 二进制就绪"

# ── Step 5: 分发到其他服务器 ────────────────────────────────────────────────
if [ "$SKIP_DIST" = false ]; then
    print_step 5 5 "分发二进制到其他服务器..."
    for i in "${!SERVERS[@]}"; do
        if [ "$i" -eq 0 ]; then
            continue  # 跳过构建服务器自身
        fi
        local_host="${SERVERS[$i]}"
        echo "    → ${VALIDATOR_IDS[$i]} (${local_host})"
        
        # 确保远程目录存在
        remote_exec "$local_host" "mkdir -p ${REMOTE_BIN}"
        
        # 通过构建服务器中转复制二进制
        # 关键二进制: 失败则报错
        for bin_name in setu-validator setu-solver; do
            copy_remote_artifact_atomically "$local_host" "$bin_name" 1
        done
        # 可选二进制: 失败时静默跳过
        for bin_name in setu-cli setu-benchmark; do
            copy_remote_artifact_atomically "$local_host" "$bin_name" 1 2>/dev/null || true
        done

        copy_remote_artifact_atomically "$local_host" "setu-build-info.env" 0
        remote_exec "$local_host" "chmod +x ${REMOTE_BIN}/* 2>/dev/null || true"
    done
    print_ok "二进制分发完成"
else
    print_step 5 5 "跳过二进制分发"
fi

echo ""
print_ok "构建完成!"
echo ""
echo "  二进制位置: ${REMOTE_BIN}/"
echo "  下一步: ./deploy.sh   # 分发配置并启动"
