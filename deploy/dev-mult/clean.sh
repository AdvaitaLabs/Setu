#!/bin/bash
# ============================================================================
# 清理: 停止进程 + 删除数据 + 删除日志
# 用法: ./clean.sh [1|2|3|all] [--keep-binary] [--keep-keys]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"
KEEP_BIN=false
KEEP_KEYS=false

shift 2>/dev/null || true
while [[ $# -gt 0 ]]; do
    case $1 in
        --keep-binary) KEEP_BIN=true; shift ;;
        --keep-keys)   KEEP_KEYS=true; shift ;;
        *) shift ;;
    esac
done

clean_server() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local vid="${VALIDATOR_IDS[$idx]}"

    echo "  清理 ${vid} (${host})..."

    # 停止进程
    remote_exec "$host" "
        pkill -f setu-validator 2>/dev/null || true
        pkill -f setu-solver 2>/dev/null || true
        sleep 1
        pkill -9 -f setu-validator 2>/dev/null || true
        pkill -9 -f setu-solver 2>/dev/null || true
    "

    # 删除数据
    remote_exec "$host" "rm -rf ${REMOTE_DATA}/db/* ${REMOTE_LOGS}/*.log"
    echo "    ✓ 数据 + 日志已删除"

    if [ "$KEEP_BIN" = false ]; then
        remote_exec "$host" "rm -f ${REMOTE_BIN}/setu-*"
        echo "    ✓ 二进制已删除"
    fi

    if [ "$KEEP_KEYS" = false ]; then
        remote_exec "$host" "rm -f ${REMOTE_KEYS}/*.key"
        echo "    ✓ 密钥文件已删除"
    fi
}

print_header "清理 Setu 集群数据"

echo "  选项: keep_binary=${KEEP_BIN}, keep_keys=${KEEP_KEYS}"
echo ""

case "$TARGET" in
    1) clean_server 0 ;;
    2) clean_server 1 ;;
    3) clean_server 2 ;;
    all)
        for i in "${!SERVERS[@]}"; do
            clean_server "$i"
        done
        ;;
    *)
        echo "用法: $0 [1|2|3|all] [--keep-binary] [--keep-keys]"
        exit 1
        ;;
esac

echo ""
print_ok "清理完成"
