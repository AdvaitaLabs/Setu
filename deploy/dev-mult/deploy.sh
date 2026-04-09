#!/bin/bash
# ============================================================================
# 全量部署: 同步源码 → 编译 → 分发二进制 → 分发配置/密钥 → 启动
# 用法: ./deploy.sh [--skip-build] [--no-start]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

SKIP_BUILD=false
NO_START=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build) SKIP_BUILD=true; shift ;;
        --no-start)   NO_START=true; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

print_header "Setu Multi-Validator 全量部署"
echo "  服务器: ${SERVERS[*]}"
echo "  构建服务器: ${BUILD_SERVER}"
echo ""

# ── Step 1: 编译二进制 ──────────────────────────────────────────────────────
if [ "$SKIP_BUILD" = false ]; then
    echo "━━━ [1/4] 编译 & 分发二进制 ━━━"
    bash "${SCRIPT_DIR}/build.sh"
else
    echo "━━━ [1/4] 跳过编译 ━━━"
fi

# ── Step 2: 分发配置文件 ────────────────────────────────────────────────────
echo ""
echo "━━━ [2/4] 分发配置文件 ━━━"
for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    echo "  → ${vid} (${host})"
    
    # 确保目录存在
    remote_exec "$host" "mkdir -p ${REMOTE_CONFIG} ${REMOTE_KEYS}"
    
    # 复制 genesis-remote.json
    remote_copy "${SCRIPT_DIR}/genesis-remote.json" "$host" "${REMOTE_CONFIG}/genesis-remote.json"
done
print_ok "genesis-remote.json 已分发到所有节点"

# ── Step 3: 分发/检查密钥文件 ───────────────────────────────────────────────
echo ""
echo "━━━ [3/4] 检查密钥文件 ━━━"
KEYS_READY=true
for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    local_key="${PROJECT_DIR}/keys/${vid}.key"
    
    # 尝试从本地复制密钥
    if [ -f "$local_key" ]; then
        remote_copy "$local_key" "$host" "${REMOTE_KEYS}/${vid}.key"
        print_ok "${vid}: 密钥已从本地复制"
    elif remote_exec "$host" "test -f ${REMOTE_KEYS}/${vid}.key" 2>/dev/null; then
        print_ok "${vid}: 远程密钥已存在"
    else
        print_warn "${vid}: 密钥文件不存在!"
        echo "    本地路径: ${local_key}"
        echo "    远程路径: ${host}:${REMOTE_KEYS}/${vid}.key"
        KEYS_READY=false
    fi
done

if [ "$KEYS_READY" = false ]; then
    echo ""
    print_warn "部分密钥缺失。可选操作:"
    echo "    1) 本地生成: cd ${PROJECT_DIR} && cargo run -p setu-cli -- gen-key generate --scheme ed25519 --output keys/validator-N.key"
    echo "    2) 远程生成: ./keygen.sh"
    echo "    3) 无密钥继续 (签名验证将被跳过)"
    echo ""
    read -p "  是否继续部署? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# ── Step 4: 验证部署就绪 ────────────────────────────────────────────────────
echo ""
echo "━━━ [4/4] 验证部署就绪 ━━━"
for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    
    # 检查二进制
    if remote_exec "$host" "test -f ${REMOTE_BIN}/setu-validator" 2>/dev/null; then
        print_ok "${vid}: 二进制就绪"
    else
        print_err "${vid}: setu-validator 二进制缺失!"
    fi
    
    # 检查 genesis
    if remote_exec "$host" "test -f ${REMOTE_CONFIG}/genesis-remote.json" 2>/dev/null; then
        print_ok "${vid}: genesis 配置就绪"
    else
        print_err "${vid}: genesis-remote.json 缺失!"
    fi
done

# ── 启动 ────────────────────────────────────────────────────────────────────
if [ "$NO_START" = false ]; then
    echo ""
    bash "${SCRIPT_DIR}/start.sh" all
else
    echo ""
    print_ok "部署完成 (未启动)"
    echo "  手动启动: ./start.sh all"
fi
