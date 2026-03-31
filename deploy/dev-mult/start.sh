#!/bin/bash
# ============================================================================
# 启动所有 Validator 节点
# 用法: ./start.sh [1|2|3|all]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"

start_validator() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local vid="${VALIDATOR_IDS[$idx]}"
    local peers
    peers=$(get_peer_validators "$idx")

    echo "  启动 ${vid} (${host})..."

    # 检查二进制是否存在
    if ! remote_exec "$host" "test -f ${REMOTE_BIN}/setu-validator" 2>/dev/null; then
        print_err "${vid}: setu-validator 二进制未找到，请先运行 ./build.sh"
        return 1
    fi

    # 检查是否已在运行
    if remote_exec "$host" "pidof setu-validator" &>/dev/null; then
        print_warn "${vid}: 已在运行，先停止..."
        remote_exec "$host" "kill \$(pidof setu-validator) 2>/dev/null || true; sleep 2"
    fi

    # 启动 validator
    local output
    output=$(remote_exec "$host" "
        cd ${REMOTE_BASE}
        nohup env \
            NODE_ID=${vid} \
            VALIDATOR_HTTP_PORT=${HTTP_PORT} \
            VALIDATOR_P2P_PORT=${P2P_PORT} \
            VALIDATOR_LISTEN_ADDR=0.0.0.0 \
            PEER_VALIDATORS='${peers}' \
            GENESIS_FILE=${REMOTE_CONFIG}/genesis-remote.json \
            VALIDATOR_KEY_FILE=${REMOTE_KEYS}/${vid}.key \
            VALIDATOR_DB_PATH=${REMOTE_DATA}/db \
            RUST_LOG='${RUST_LOG}' \
            ${REMOTE_BIN}/setu-validator \
            >> ${REMOTE_LOGS}/validator.log 2>&1 &
        
        sleep 1
        if pidof setu-validator > /dev/null 2>&1; then
            echo 'STARTED'
        else
            echo 'FAILED'
        fi
    " 2>&1) || {
        print_err "${vid}: SSH 连接失败 (${host})"
        return 1
    }
    
    if echo "$output" | grep -q 'STARTED'; then
        print_ok "${vid} 已启动 (HTTP=${host}:${HTTP_PORT}, P2P=${host}:${P2P_PORT})"
    else
        print_err "${vid} 启动失败! 查看日志: ./logs.sh $((idx+1))"
    fi
}

# ── 主逻辑 ──────────────────────────────────────────────────────────────────
print_header "启动 Setu Validator 集群"

case "$TARGET" in
    1) start_validator 0 ;;
    2) start_validator 1 ;;
    3) start_validator 2 ;;
    all)
        for i in "${!SERVERS[@]}"; do
            start_validator "$i"
            # 节点间间隔启动，让 seed peer 先就绪
            if [ "$i" -lt $((${#SERVERS[@]} - 1)) ]; then
                echo "  等待 3 秒..."
                sleep 3
            fi
        done
        ;;
    *)
        echo "用法: $0 [1|2|3|all]"
        exit 1
        ;;
esac

# 健康检查
echo ""
echo "  等待节点就绪..."
sleep 5

echo ""
echo "━━━ 节点状态 ━━━"
for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    if wait_for_health "$host" "$HTTP_PORT" 10; then
        print_ok "${vid} (${host}:${HTTP_PORT}) — 健康"
    else
        print_warn "${vid} (${host}:${HTTP_PORT}) — 未响应 (可能仍在启动)"
    fi
done

echo ""
echo "  查看日志: ./logs.sh [1|2|3]"
echo "  检查状态: ./status.sh"
