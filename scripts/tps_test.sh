#!/bin/bash
#
# Setu TPS Benchmark Test Script
# ===============================
# 
# 功能:
#   1. 关闭代理
#   2. 创建日志目录
#   3. 启动 Validator 和 Solver (数量可配置)
#   4. 运行 Benchmark 测试
#   5. 收集日志和结果
#
# 用法:
#   ./scripts/tps_test.sh [OPTIONS]
#
# 示例:
#   ./scripts/tps_test.sh                           # 默认配置
#   ./scripts/tps_test.sh -s 3 -t 1000 -c 100       # 3个Solver, 1000交易, 100并发
#   ./scripts/tps_test.sh --solvers 5 --sustained  # 5个Solver, 持续模式
#

set -e

# ============================================================================
# 默认配置
# ============================================================================
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_BASE_DIR="${PROJECT_ROOT}/logs"

# 服务配置
NUM_VALIDATORS=1          # Validator 数量 (当前仅支持1个)
NUM_SOLVERS=1             # Solver 数量
VALIDATOR_PORT=8080       # Validator 端口
SOLVER_BASE_PORT=9001     # Solver 起始端口

# Benchmark 配置
TOTAL_REQUESTS=500        # 总请求数
CONCURRENCY=50            # 并发数
WARMUP_REQUESTS=50        # 预热请求数
USE_TEST_ACCOUNTS=true    # 使用预初始化账户
NUM_TEST_ACCOUNTS=20      # 测试账户数量 (需与代码中一致)
BENCHMARK_MODE="burst"    # 模式: burst, sustained, ramp
SUSTAINED_DURATION=30     # sustained 模式持续时间(秒)
SUSTAINED_TPS=100         # sustained 模式目标 TPS

# 其他配置
MOCK_TEE=true             # 使用 Mock TEE
RUST_LOG_LEVEL="warn"     # 日志级别: error, warn, info, debug, trace
WAIT_STARTUP=5            # 服务启动等待时间(秒)
HEALTH_CHECK_RETRIES=10   # 健康检查重试次数

# ============================================================================
# 颜色输出
# ============================================================================
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_step()  { echo -e "${CYAN}==>${NC} $1"; }

# ============================================================================
# 帮助信息
# ============================================================================
show_help() {
    cat << EOF
Setu TPS Benchmark Test Script

用法: $0 [OPTIONS]

服务配置:
  -s, --solvers NUM       Solver 数量 (默认: $NUM_SOLVERS)
  -p, --port PORT         Validator 端口 (默认: $VALIDATOR_PORT)
  --mock-tee              使用 Mock TEE (默认: $MOCK_TEE)
  --real-tee              使用真实 TEE

Benchmark 配置:
  -t, --requests NUM      总请求数 (默认: $TOTAL_REQUESTS)
  -c, --concurrency NUM   并发数 (默认: $CONCURRENCY)
  -w, --warmup NUM        预热请求数 (默认: $WARMUP_REQUESTS)
  --no-test-accounts      不使用预初始化账户

Benchmark 模式:
  --burst                 突发模式 (默认)
  --sustained             持续模式
  --sustained-duration S  持续模式时长 (默认: $SUSTAINED_DURATION 秒)
  --sustained-tps TPS     持续模式目标 TPS (默认: $SUSTAINED_TPS)
  --ramp                  渐进模式

日志配置:
  -l, --log-level LEVEL   日志级别: error,warn,info,debug,trace (默认: $RUST_LOG_LEVEL)
  --log-dir DIR           日志目录 (默认: $LOG_BASE_DIR)

其他:
  -h, --help              显示帮助信息
  --dry-run               仅显示配置，不执行

示例:
  $0                                    # 默认配置测试
  $0 -s 3 -t 1000 -c 100               # 3个Solver, 1000请求, 100并发
  $0 --solvers 5 --sustained           # 5个Solver, 持续模式
  $0 -s 2 -t 5000 -c 200 -l info       # 高负载测试，info日志

EOF
    exit 0
}

# ============================================================================
# 解析参数
# ============================================================================
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -s|--solvers)
            NUM_SOLVERS="$2"
            shift 2
            ;;
        -p|--port)
            VALIDATOR_PORT="$2"
            shift 2
            ;;
        --mock-tee)
            MOCK_TEE=true
            shift
            ;;
        --real-tee)
            MOCK_TEE=false
            shift
            ;;
        -t|--requests)
            TOTAL_REQUESTS="$2"
            shift 2
            ;;
        -c|--concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        -w|--warmup)
            WARMUP_REQUESTS="$2"
            shift 2
            ;;
        --no-test-accounts)
            USE_TEST_ACCOUNTS=false
            shift
            ;;
        --burst)
            BENCHMARK_MODE="burst"
            shift
            ;;
        --sustained)
            BENCHMARK_MODE="sustained"
            shift
            ;;
        --sustained-duration)
            SUSTAINED_DURATION="$2"
            shift 2
            ;;
        --sustained-tps)
            SUSTAINED_TPS="$2"
            shift 2
            ;;
        --ramp)
            BENCHMARK_MODE="ramp"
            shift
            ;;
        -l|--log-level)
            RUST_LOG_LEVEL="$2"
            shift 2
            ;;
        --log-dir)
            LOG_BASE_DIR="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            show_help
            ;;
        *)
            log_error "未知参数: $1"
            show_help
            ;;
    esac
done

# ============================================================================
# 创建日志目录
# ============================================================================
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
TEST_LOG_DIR="${LOG_BASE_DIR}/${TIMESTAMP}"
VALIDATOR_LOG="${TEST_LOG_DIR}/validator.log"
BENCHMARK_LOG="${TEST_LOG_DIR}/benchmark.log"
RESULT_FILE="${TEST_LOG_DIR}/result.txt"
CONFIG_FILE="${TEST_LOG_DIR}/config.json"

create_log_dir() {
    log_step "创建日志目录: ${TEST_LOG_DIR}"
    mkdir -p "${TEST_LOG_DIR}"
    
    # 收集系统信息
    local cpu_info=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || cat /proc/cpuinfo 2>/dev/null | grep "model name" | head -1 | cut -d: -f2 || echo "Unknown")
    local cpu_cores=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo "Unknown")
    local memory_gb=$(echo "scale=1; $(sysctl -n hw.memsize 2>/dev/null || grep MemTotal /proc/meminfo 2>/dev/null | awk '{print $2*1024}' || echo 0) / 1073741824" | bc 2>/dev/null || echo "Unknown")
    
    # 保存配置
    cat > "${CONFIG_FILE}" << EOF
{
    "timestamp": "${TIMESTAMP}",
    "system": {
        "cpu": "${cpu_info}",
        "cpu_cores": ${cpu_cores},
        "memory_gb": ${memory_gb},
        "os": "$(uname -s) $(uname -r)"
    },
    "services": {
        "num_validators": ${NUM_VALIDATORS},
        "num_solvers": ${NUM_SOLVERS},
        "validator_port": ${VALIDATOR_PORT},
        "solver_base_port": ${SOLVER_BASE_PORT},
        "mock_tee": ${MOCK_TEE}
    },
    "benchmark": {
        "mode": "${BENCHMARK_MODE}",
        "total_requests": ${TOTAL_REQUESTS},
        "concurrency": ${CONCURRENCY},
        "warmup_requests": ${WARMUP_REQUESTS},
        "use_test_accounts": ${USE_TEST_ACCOUNTS},
        "num_test_accounts": ${NUM_TEST_ACCOUNTS},
        "sustained_duration": ${SUSTAINED_DURATION},
        "sustained_tps": ${SUSTAINED_TPS}
    },
    "logging": {
        "rust_log_level": "${RUST_LOG_LEVEL}",
        "log_dir": "${TEST_LOG_DIR}"
    }
}
EOF
    log_ok "配置已保存到 ${CONFIG_FILE}"
}

# ============================================================================
# 关闭代理
# ============================================================================
disable_proxy() {
    log_step "关闭代理设置"
    unset http_proxy
    unset https_proxy
    unset HTTP_PROXY
    unset HTTPS_PROXY
    export NO_PROXY="127.0.0.1,localhost,*"
    log_ok "代理已关闭"
}

# ============================================================================
# 清理旧进程
# ============================================================================
cleanup_processes() {
    log_step "清理旧进程"
    pkill -f "setu-validator" 2>/dev/null || true
    pkill -f "setu-solver" 2>/dev/null || true
    sleep 1
    
    # 确认端口已释放
    for port in $(seq $VALIDATOR_PORT $VALIDATOR_PORT) $(seq $SOLVER_BASE_PORT $((SOLVER_BASE_PORT + NUM_SOLVERS - 1))); do
        if lsof -i :$port >/dev/null 2>&1; then
            log_warn "端口 $port 仍被占用，强制释放"
            lsof -i :$port | awk 'NR>1 {print $2}' | xargs -r kill -9 2>/dev/null || true
        fi
    done
    sleep 1
    log_ok "旧进程已清理"
}

# ============================================================================
# 清理数据库
# ============================================================================
cleanup_database() {
    log_step "清理数据库"
    rm -rf "${PROJECT_ROOT}/example_db"
    log_ok "数据库已清理"
}

# ============================================================================
# 启动 Validator
# ============================================================================
start_validator() {
    log_step "启动 Validator"
    
    local tee_flag=""
    if [ "$MOCK_TEE" = true ]; then
        tee_flag="--mock-tee"
    fi
    
    # 明确设置环境变量禁用代理，并通过环境变量配置 Validator
    env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
    NO_PROXY="127.0.0.1,localhost,*" \
    RUST_LOG="${RUST_LOG_LEVEL}" \
    VALIDATOR_HTTP_PORT="${VALIDATOR_PORT}" \
    VALIDATOR_LISTEN_ADDR="127.0.0.1" \
    "${PROJECT_ROOT}/target/release/setu-validator" ${tee_flag} \
        >> "${VALIDATOR_LOG}" 2>&1 &
    
    VALIDATOR_PID=$!
    echo $VALIDATOR_PID > "${TEST_LOG_DIR}/validator.pid"
    
    log_info "Validator PID: ${VALIDATOR_PID}"
    log_info "等待 Validator 启动..."
    sleep ${WAIT_STARTUP}
    
    # 检查健康状态 (带重试)
    local retries=0
    while [ $retries -lt $HEALTH_CHECK_RETRIES ]; do
        if curl -s "http://127.0.0.1:${VALIDATOR_PORT}/api/v1/health" 2>/dev/null | grep -q "healthy"; then
            log_ok "Validator 启动成功"
            return 0
        fi
        retries=$((retries + 1))
        log_info "等待 Validator 就绪... ($retries/$HEALTH_CHECK_RETRIES)"
        sleep 1
    done
    
    log_error "Validator 启动失败"
    cat "${VALIDATOR_LOG}"
    exit 1
}

# ============================================================================
# 启动 Solvers
# ============================================================================
start_solvers() {
    log_step "启动 ${NUM_SOLVERS} 个 Solver"
    
    local tee_flag=""
    if [ "$MOCK_TEE" = true ]; then
        tee_flag="--mock-tee"
    fi
    
    for i in $(seq 1 $NUM_SOLVERS); do
        local solver_port=$((SOLVER_BASE_PORT + i - 1))
        local solver_log="${TEST_LOG_DIR}/solver_${i}.log"
        
        # 使用环境变量配置 Solver（而不是命令行参数）
        # 明确设置禁用代理的环境变量
        env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
        NO_PROXY="127.0.0.1,localhost,*" \
        SOLVER_ID="solver_${i}" \
        SOLVER_PORT="${solver_port}" \
        VALIDATOR_ADDRESS="127.0.0.1" \
        VALIDATOR_HTTP_PORT="${VALIDATOR_PORT}" \
        RUST_LOG="${RUST_LOG_LEVEL}" \
        "${PROJECT_ROOT}/target/release/setu-solver" \
            ${tee_flag} \
            >> "${solver_log}" 2>&1 &
        
        local solver_pid=$!
        echo $solver_pid >> "${TEST_LOG_DIR}/solver.pids"
        log_info "Solver ${i} PID: ${solver_pid}, Port: ${solver_port}"
    done
    
    log_info "等待 Solvers 启动..."
    
    # 等待所有 Solver 注册 (带重试)
    local retries=0
    local solver_count=0
    while [ $retries -lt $HEALTH_CHECK_RETRIES ]; do
        solver_count=$(curl -s "http://127.0.0.1:${VALIDATOR_PORT}/api/v1/health" 2>/dev/null | grep -o '"solver_count":[0-9]*' | grep -o '[0-9]*' || echo "0")
        if [ "$solver_count" -ge "$NUM_SOLVERS" ]; then
            log_ok "所有 ${NUM_SOLVERS} 个 Solver 启动成功 (注册数: ${solver_count})"
            return 0
        fi
        retries=$((retries + 1))
        log_info "等待 Solver 注册... ($solver_count/$NUM_SOLVERS) [$retries/$HEALTH_CHECK_RETRIES]"
        sleep 1
    done
    
    log_warn "Solver 启动可能不完整 (期望: ${NUM_SOLVERS}, 注册: ${solver_count})"
}

# ============================================================================
# 运行 Benchmark
# ============================================================================
run_benchmark() {
    log_step "运行 Benchmark 测试"
    
    local benchmark_args="-t ${TOTAL_REQUESTS} -c ${CONCURRENCY}"
    
    if [ "$USE_TEST_ACCOUNTS" = true ]; then
        benchmark_args="${benchmark_args} --use-test-accounts"
    fi
    
    if [ "$WARMUP_REQUESTS" -gt 0 ]; then
        benchmark_args="${benchmark_args} --warmup ${WARMUP_REQUESTS}"
    fi
    
    # 根据模式添加参数
    case $BENCHMARK_MODE in
        sustained)
            benchmark_args="${benchmark_args} -m sustained --duration ${SUSTAINED_DURATION} --target-tps ${SUSTAINED_TPS}"
            ;;
        ramp)
            benchmark_args="${benchmark_args} -m ramp"
            ;;
        *)
            # burst 模式是默认
            benchmark_args="${benchmark_args} -m burst"
            ;;
    esac
    
    log_info "Benchmark 参数: ${benchmark_args}"
    echo "Benchmark 参数: ${benchmark_args}" >> "${RESULT_FILE}"
    echo "======================================" >> "${RESULT_FILE}"
    
    # 运行 Benchmark（明确禁用代理）
    env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
    NO_PROXY="127.0.0.1,localhost,*" \
    "${PROJECT_ROOT}/target/release/setu-benchmark" \
        -u "http://127.0.0.1:${VALIDATOR_PORT}" \
        ${benchmark_args} \
        2>&1 | tee -a "${BENCHMARK_LOG}" "${RESULT_FILE}"
    
    log_ok "Benchmark 完成"
}

# ============================================================================
# 收集结果
# ============================================================================
collect_results() {
    log_step "收集测试结果"
    
    # 提取关键指标 (使用更精确的正则)
    local tps=$(grep "Final TPS" "${RESULT_FILE}" | grep -o "TPS: [0-9.]*" | grep -o "[0-9.]*" || echo "N/A")
    local success_rate=$(grep "Final TPS" "${RESULT_FILE}" | grep -o "Success Rate: [0-9.]*%" | grep -o "[0-9.]*%" || echo "N/A")
    local p99=$(grep "Final TPS" "${RESULT_FILE}" | grep -o "P99 Latency: [0-9.]*ms" | grep -o "[0-9.]*" || echo "N/A")
    [ "$p99" != "N/A" ] && p99="${p99} ms"
    
    # 生成摘要
    cat >> "${RESULT_FILE}" << EOF

======================================
测试摘要
======================================
时间戳:       ${TIMESTAMP}
Solver 数量:  ${NUM_SOLVERS}
总请求数:     ${TOTAL_REQUESTS}
并发数:       ${CONCURRENCY}
模式:         ${BENCHMARK_MODE}

结果:
  TPS:          ${tps}
  成功率:       ${success_rate}
  P99 延迟:     ${p99}
======================================
EOF

    log_ok "结果已保存到: ${RESULT_FILE}"
    
    # 显示摘要
    echo ""
    echo "=============================================="
    echo -e "${GREEN}测试完成!${NC}"
    echo "=============================================="
    echo "  日志目录:   ${TEST_LOG_DIR}"
    echo "  Solver数量: ${NUM_SOLVERS}"
    echo "  总请求:     ${TOTAL_REQUESTS}"
    echo "  并发:       ${CONCURRENCY}"
    echo ""
    echo -e "  ${CYAN}TPS:${NC}        ${tps}"
    echo -e "  ${CYAN}成功率:${NC}     ${success_rate}"
    echo -e "  ${CYAN}P99延迟:${NC}    ${p99}"
    echo "=============================================="
}

# ============================================================================
# 清理函数
# ============================================================================
cleanup() {
    log_step "清理进程"
    pkill -f "setu-validator" 2>/dev/null || true
    pkill -f "setu-solver" 2>/dev/null || true
    log_ok "进程已清理"
}

# ============================================================================
# 显示配置 (dry-run)
# ============================================================================
show_config() {
    echo ""
    echo "=============================================="
    echo "测试配置 (Dry Run)"
    echo "=============================================="
    echo "服务配置:"
    echo "  Validator 数量: ${NUM_VALIDATORS}"
    echo "  Solver 数量:    ${NUM_SOLVERS}"
    echo "  Validator 端口: ${VALIDATOR_PORT}"
    echo "  Solver 起始端口: ${SOLVER_BASE_PORT}"
    echo "  Mock TEE:       ${MOCK_TEE}"
    echo ""
    echo "Benchmark 配置:"
    echo "  模式:           ${BENCHMARK_MODE}"
    echo "  总请求数:       ${TOTAL_REQUESTS}"
    echo "  并发数:         ${CONCURRENCY}"
    echo "  预热请求:       ${WARMUP_REQUESTS}"
    echo "  使用测试账户:   ${USE_TEST_ACCOUNTS}"
    if [ "$BENCHMARK_MODE" = "sustained" ]; then
        echo "  持续时间:       ${SUSTAINED_DURATION}s"
        echo "  目标 TPS:       ${SUSTAINED_TPS}"
    fi
    echo ""
    echo "日志配置:"
    echo "  日志级别:       ${RUST_LOG_LEVEL}"
    echo "  日志目录:       ${TEST_LOG_DIR}"
    echo "=============================================="
}

# ============================================================================
# 主流程
# ============================================================================
main() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════╗"
    echo "║           Setu TPS Benchmark Test                        ║"
    echo "╚══════════════════════════════════════════════════════════╝"
    echo ""
    
    if [ "$DRY_RUN" = true ]; then
        show_config
        exit 0
    fi
    
    # 检查二进制文件
    if [ ! -f "${PROJECT_ROOT}/target/release/setu-validator" ] || \
       [ ! -f "${PROJECT_ROOT}/target/release/setu-solver" ] || \
       [ ! -f "${PROJECT_ROOT}/target/release/setu-benchmark" ]; then
        log_error "请先编译: cargo build --release"
        exit 1
    fi
    
    # 设置清理钩子
    trap cleanup EXIT
    
    # 执行测试流程
    disable_proxy
    create_log_dir
    cleanup_processes
    cleanup_database
    start_validator
    start_solvers
    run_benchmark
    collect_results
    
    log_ok "测试完成!"
}

# 执行主流程
main
