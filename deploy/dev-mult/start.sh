#!/bin/bash
# ============================================================================
# 启动 Validator + Solver 节点
# 用法: ./start.sh [1|2|3|all] [--no-solver]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"
NO_SOLVER=false
for arg in "$@"; do
    case "$arg" in
        --no-solver) NO_SOLVER=true ;;
    esac
done

retry_remote_exec() {
    local host="$1"
    shift
    local attempt
    for attempt in 1 2 3 4 5; do
        if remote_exec "$host" "$@"; then
            return 0
        fi
        print_warn "${host}: SSH 操作失败，重试 ${attempt}/5"
        sleep 3
    done
    return 1
}

start_validator() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local vid="${VALIDATOR_IDS[$idx]}"
    local peers
    peers=$(get_peer_validators "$idx")

    echo "  启动 ${vid} (${host})..."

    # 检查二进制是否存在
    if ! retry_remote_exec "$host" "test -f ${REMOTE_BIN}/setu-validator" >/dev/null 2>&1; then
        print_err "${vid}: setu-validator 二进制未找到，请先运行 ./build.sh"
        return 1
    fi

    # 检查是否已在运行
    if remote_exec "$host" "pidof setu-validator" &>/dev/null; then
        print_warn "${vid}: 已在运行，先停止..."
        retry_remote_exec "$host" "kill \$(pidof setu-validator) 2>/dev/null || true; sleep 2" >/dev/null 2>&1 || true
    fi

    # 启动 validator
    local output
    output=$(retry_remote_exec "$host" "
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
            SETU_RAW_TRANSFER_API_TOKEN='${SETU_RAW_TRANSFER_API_TOKEN:-}' \
            RUST_LOG='${RUST_LOG}' \
            ${REMOTE_BIN}/setu-validator \
                < /dev/null \
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

start_solver() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local sid="solver-$((idx + 1))"

    echo "  启动 ${sid} (${host})..."

    # 检查二进制是否存在
    if ! retry_remote_exec "$host" "test -f ${REMOTE_BIN}/setu-solver" >/dev/null 2>&1; then
        print_warn "${sid}: setu-solver 二进制未找到，跳过"
        return 1
    fi

    # 检查是否已在运行
    if remote_exec "$host" "pidof setu-solver" &>/dev/null; then
        print_warn "${sid}: 已在运行，先停止..."
        retry_remote_exec "$host" "kill \$(pidof setu-solver) 2>/dev/null || true; sleep 2" >/dev/null 2>&1 || true
    fi

    # 启动 solver — 连接到本机 validator
    local output
    output=$(retry_remote_exec "$host" "
        cd ${REMOTE_BASE}
        nohup env \\
            SOLVER_ID=${sid} \\
            SOLVER_LISTEN_ADDR=0.0.0.0 \\
            SOLVER_PORT=${SOLVER_PORT} \\
            SOLVER_CAPACITY=100 \\
            VALIDATOR_ADDRESS=${host} \\
            VALIDATOR_HTTP_PORT=${HTTP_PORT} \\
            AUTO_REGISTER=true \\
            HEARTBEAT_INTERVAL=30 \\
            RUST_LOG='info,setu_solver=debug' \\
            ${REMOTE_BIN}/setu-solver \\
                < /dev/null \
            >> ${REMOTE_LOGS}/solver.log 2>&1 &

        sleep 2
        if pidof setu-solver > /dev/null 2>&1; then
            echo 'STARTED'
        else
            echo 'FAILED'
        fi
    " 2>&1) || {
        print_err "${sid}: SSH 连接失败 (${host})"
        return 1
    }

    if echo "$output" | grep -q 'STARTED'; then
        print_ok "${sid} 已启动 (${host}:${SOLVER_PORT}, 连接 validator ${host}:${HTTP_PORT})"
    else
        print_err "${sid} 启动失败! 查看日志: ./logs.sh $((idx+1)) solver"
    fi
}

# ── 主逻辑 ──────────────────────────────────────────────────────────────────
print_header "启动 Setu Validator + Solver 集群"

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

# 健康检查 (validator)
echo ""
echo "  等待 Validator 节点就绪..."
sleep 5

echo ""
echo "━━━ Validator 状态 ━━━"
for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    if wait_for_health "$host" "$HTTP_PORT" 10; then
        print_ok "${vid} (${host}:${HTTP_PORT}) — 健康"
    else
        print_warn "${vid} (${host}:${HTTP_PORT}) — 未响应 (可能仍在启动)"
    fi
done

# 启动 Solver
if [ "$NO_SOLVER" = false ]; then
    echo ""
    echo "━━━ 启动 Solver ━━━"
    case "$TARGET" in
        1) start_solver 0 ;;
        2) start_solver 1 ;;
        3) start_solver 2 ;;
        all)
            for i in "${!SERVERS[@]}"; do
                start_solver "$i"
            done
            ;;
    esac

    # 等待 Solver 注册
    echo ""
    echo "  等待 Solver 注册到 Validator..."
    sleep 5

    echo ""
    echo "━━━ Solver 注册状态 ━━━"
    for i in "${!SERVERS[@]}"; do
        host="${SERVERS[$i]}"
        vid="${VALIDATOR_IDS[$i]}"
        health=$(curl -sf --connect-timeout 3 "http://${host}:${HTTP_PORT}/api/v1/health" 2>/dev/null || echo "")
        solver_count=$(echo "$health" | python3 -c "import sys,json; print(json.load(sys.stdin).get('solver_count',0))" 2>/dev/null || echo "0")
        if [ "$solver_count" -gt 0 ] 2>/dev/null; then
            print_ok "${vid}: solver_count=${solver_count}"
        else
            print_warn "${vid}: solver_count=0 (Solver 可能仍在注册)"
        fi
    done
fi

echo ""
echo "  查看日志: ./logs.sh [1|2|3] [validator|solver]"
echo "  检查状态: ./status.sh"
