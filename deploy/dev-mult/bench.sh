#!/bin/bash
# ============================================================================
# 在远程集群上运行 benchmark 测试
# 用法: ./bench.sh [--solvers N] [--txns N] [--concurrency N] [--target 1|2|3]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

# 默认参数
NUM_SOLVERS=3
BENCH_TXNS=1000
CONCURRENCY=200
INIT_ACCOUNTS=100
TARGET_IDX=0  # 默认在 validator-1 上运行 benchmark

while [[ $# -gt 0 ]]; do
    case $1 in
        --solvers)     NUM_SOLVERS="$2"; shift 2 ;;
        --txns)        BENCH_TXNS="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --accounts)    INIT_ACCOUNTS="$2"; shift 2 ;;
        --target)      TARGET_IDX=$(($2 - 1)); shift 2 ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

TARGET_HOST="${SERVERS[$TARGET_IDX]}"
TARGET_VID="${VALIDATOR_IDS[$TARGET_IDX]}"

print_header "远程 Benchmark 测试"
echo "  目标: ${TARGET_VID} (${TARGET_HOST}:${HTTP_PORT})"
echo "  Solvers: ${NUM_SOLVERS}, 交易数: ${BENCH_TXNS}, 并发: ${CONCURRENCY}"
echo ""

# 检查 benchmark 二进制
if ! remote_exec "$TARGET_HOST" "test -f ${REMOTE_BIN}/setu-benchmark" 2>/dev/null; then
    print_err "setu-benchmark 未找到，请先运行 ./build.sh"
    exit 1
fi

# 启动 solver(s) 连接到目标 validator
echo "  启动 ${NUM_SOLVERS} 个 Solver..."
for s in $(seq 1 "$NUM_SOLVERS"); do
    remote_exec "$TARGET_HOST" "
        SOLVER_ID=solver-bench-${s} \
        SOLVER_PORT=$((SOLVER_PORT + s - 1)) \
        SOLVER_LISTEN_ADDR=127.0.0.1 \
        SOLVER_CAPACITY=100 \
        VALIDATOR_ADDRESS=127.0.0.1 \
        VALIDATOR_HTTP_PORT=${HTTP_PORT} \
        AUTO_REGISTER=true \
        RUST_LOG=warn \
        nohup ${REMOTE_BIN}/setu-solver > ${REMOTE_LOGS}/solver-bench-${s}.log 2>&1 &
    "
done
sleep 5
print_ok "${NUM_SOLVERS} 个 Solver 已启动"

# 运行 benchmark
echo ""
echo "  运行 Benchmark..."
remote_exec "$TARGET_HOST" "
    ${REMOTE_BIN}/setu-benchmark \
        --validator-url http://127.0.0.1:${HTTP_PORT} \
        --total ${BENCH_TXNS} \
        --concurrency ${CONCURRENCY} \
        --init-accounts ${INIT_ACCOUNTS} \
        --genesis-file ${REMOTE_CONFIG}/genesis-remote.json \
        --use-test-accounts \
        2>&1
" || true

# 清理 solver
echo ""
echo "  清理 Solver 进程..."
remote_exec "$TARGET_HOST" "SPID=\$(pidof setu-solver 2>/dev/null || true); [ -n \"\$SPID\" ] && kill \$SPID 2>/dev/null || true"
print_ok "Benchmark 完成"
