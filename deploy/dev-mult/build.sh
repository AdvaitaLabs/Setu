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

print_header "Setu 构建 & 分发"

# ── Step 1: 同步源码到构建服务器 ────────────────────────────────────────────
if [ "$SKIP_SYNC" = false ]; then
    print_step 1 4 "同步源码到构建服务器 (${BUILD_SERVER})..."
    
    # 确保远程源码目录存在
    remote_exec "$BUILD_SERVER" "mkdir -p ${REMOTE_SRC}"
    
    remote_sync "${PROJECT_DIR}/" "$BUILD_SERVER" "${REMOTE_SRC}/"
    print_ok "源码同步完成"
else
    print_step 1 4 "跳过源码同步"
fi

# ── Step 2: 远程编译 ────────────────────────────────────────────────────────
print_step 2 4 "在构建服务器上编译 (release)..."
echo "  构建目标: setu-validator, setu-solver, setu-cli, setu-benchmark"
echo "  (首次编译可能需要 20-40 分钟，请耐心等待...)"

remote_exec "$BUILD_SERVER" "
    set -eo pipefail
    source \"\$HOME/.cargo/env\" 2>/dev/null || true
    cd ${REMOTE_SRC}
    cargo build --release \\
        -p setu-validator \\
        -p setu-solver \\
        -p setu-cli \\
        -p setu-benchmark \\
        2>&1
"
print_ok "编译完成"

# ── Step 3: 复制二进制到 bin 目录 ───────────────────────────────────────────
print_step 3 4 "安装二进制到构建服务器..."
remote_exec "$BUILD_SERVER" "
    cp ${REMOTE_SRC}/target/release/setu-validator ${REMOTE_BIN}/
    cp ${REMOTE_SRC}/target/release/setu-solver    ${REMOTE_BIN}/
    cp ${REMOTE_SRC}/target/release/setu-cli       ${REMOTE_BIN}/ 2>/dev/null || true
    cp ${REMOTE_SRC}/target/release/setu-benchmark ${REMOTE_BIN}/ 2>/dev/null || true
    chmod +x ${REMOTE_BIN}/*
    ls -lh ${REMOTE_BIN}/
"
print_ok "构建服务器 (${BUILD_SERVER}) 二进制就绪"

# ── Step 4: 分发到其他服务器 ────────────────────────────────────────────────
if [ "$SKIP_DIST" = false ]; then
    print_step 4 4 "分发二进制到其他服务器..."
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
            remote_to_remote_copy "$BUILD_SERVER" "${REMOTE_BIN}/${bin_name}" \
                                  "$local_host" "${REMOTE_BIN}/${bin_name}"
        done
        # 可选二进制: 失败时静默跳过
        for bin_name in setu-cli setu-benchmark; do
            remote_to_remote_copy "$BUILD_SERVER" "${REMOTE_BIN}/${bin_name}" \
                                  "$local_host" "${REMOTE_BIN}/${bin_name}" 2>/dev/null || true
        done
        
        remote_exec "$local_host" "chmod +x ${REMOTE_BIN}/* 2>/dev/null || true"
    done
    print_ok "二进制分发完成"
else
    print_step 4 4 "跳过二进制分发"
fi

echo ""
print_ok "构建完成!"
echo ""
echo "  二进制位置: ${REMOTE_BIN}/"
echo "  下一步: ./deploy.sh   # 分发配置并启动"
