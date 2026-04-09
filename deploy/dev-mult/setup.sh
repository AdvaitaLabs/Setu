#!/bin/bash
# ============================================================================
# 一次性服务器初始化: 安装 Rust 工具链、系统依赖、创建目录、生成密钥
# 用法: ./setup.sh [1|2|3|all]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"

setup_server() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local vid="${VALIDATOR_IDS[$idx]}"

    echo ""
    echo "━━━ 初始化 ${vid} (${host}) ━━━"

    # 1) 创建目录
    print_step 1 4 "创建目录结构..."
    remote_exec "$host" "
        mkdir -p ${REMOTE_BIN} ${REMOTE_KEYS} ${REMOTE_CONFIG} ${REMOTE_DATA}/db ${REMOTE_LOGS}
    "
    print_ok "目录已创建"

    # 2) 安装系统依赖
    print_step 2 4 "安装系统依赖..."
    remote_exec "$host" "
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq \
            build-essential pkg-config libssl-dev libclang-dev cmake \
            protobuf-compiler curl git unzip jq sshpass \
            > /dev/null 2>&1
    "
    print_ok "系统依赖就绪"

    # 3) 仅在构建服务器上安装 Rust (server-1)
    if [ "$idx" -eq 0 ]; then
        print_step 3 4 "安装 Rust 工具链 (构建服务器)..."
        remote_exec "$host" "
            if ! command -v rustup &>/dev/null; then
                curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
            fi
            source \"\$HOME/.cargo/env\"
            rustup update stable 2>/dev/null || true
            rustc --version
            cargo --version
        "
        print_ok "Rust 工具链就绪"
    else
        print_step 3 4 "跳过 Rust 安装 (非构建服务器)"
    fi

    # 4) 网络 & 防火墙配置
    print_step 4 4 "配置网络参数 & 防火墙..."
    remote_exec "$host" "
        # 增大 socket buffer — Anemo P2P 需要 ≥ 2MB
        sysctl -w net.core.wmem_max=8388608 2>/dev/null || true
        sysctl -w net.core.rmem_max=8388608 2>/dev/null || true
        sysctl -w net.core.wmem_default=2097152 2>/dev/null || true
        sysctl -w net.core.rmem_default=2097152 2>/dev/null || true
        grep -q 'wmem_max' /etc/sysctl.conf || {
            echo 'net.core.wmem_max=8388608'    >> /etc/sysctl.conf
            echo 'net.core.rmem_max=8388608'    >> /etc/sysctl.conf
            echo 'net.core.wmem_default=2097152' >> /etc/sysctl.conf
            echo 'net.core.rmem_default=2097152' >> /etc/sysctl.conf
        }
    "
    remote_exec "$host" "
        # 尝试 ufw
        if command -v ufw &>/dev/null; then
            ufw allow ${HTTP_PORT}/tcp 2>/dev/null || true
            ufw allow ${P2P_PORT}/udp 2>/dev/null || true
            ufw allow ${P2P_PORT}/tcp 2>/dev/null || true
            ufw allow ${SOLVER_PORT}/tcp 2>/dev/null || true
        fi
        # 尝试 iptables (如果 ufw 不可用)
        if command -v iptables &>/dev/null; then
            iptables -I INPUT -p tcp --dport ${HTTP_PORT} -j ACCEPT 2>/dev/null || true
            iptables -I INPUT -p udp --dport ${P2P_PORT} -j ACCEPT 2>/dev/null || true
            iptables -I INPUT -p tcp --dport ${P2P_PORT} -j ACCEPT 2>/dev/null || true
            iptables -I INPUT -p tcp --dport ${SOLVER_PORT} -j ACCEPT 2>/dev/null || true
        fi
    "
    print_ok "防火墙规则已添加 (HTTP=${HTTP_PORT}, P2P=${P2P_PORT})"
}

# ── 主逻辑 ──────────────────────────────────────────────────────────────────
print_header "Setu Multi-Validator 服务器初始化"

# 检查本地 sshpass
if ! command -v sshpass &>/dev/null; then
    print_warn "sshpass 未安装, 将使用 SSH 密钥认证"
    echo "  macOS 安装: brew install esolitos/ipa/sshpass"
    echo ""
fi

case "$TARGET" in
    1) setup_server 0 ;;
    2) setup_server 1 ;;
    3) setup_server 2 ;;
    all)
        for i in "${!SERVERS[@]}"; do
            setup_server "$i"
        done
        ;;
    *)
        echo "用法: $0 [1|2|3|all]"
        exit 1
        ;;
esac

echo ""
print_ok "初始化完成!"
echo ""
echo "下一步: ./build.sh   # 编译并分发二进制"
