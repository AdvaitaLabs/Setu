#!/bin/bash
# ============================================================================
# 查看远程日志
# 用法: ./logs.sh [1|2|3|all] [--tail N] [--follow] [--grep PATTERN]
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"
TAIL_N=100
FOLLOW=false
GREP_PATTERN=""

# 检测第一个参数是 flag 而非节点编号 — 默认 TARGET=all
if [[ "$TARGET" == -* ]]; then
    TARGET="all"
    # 不 shift，让 while 循环处理所有参数
else
    shift 2>/dev/null || true
fi
while [[ $# -gt 0 ]]; do
    case $1 in
        --tail|-n)   TAIL_N="$2"; shift 2 ;;
        --follow|-f) FOLLOW=true; shift ;;
        --grep|-g)   GREP_PATTERN="$2"; shift 2 ;;
        *) shift ;;
    esac
done

show_logs() {
    local idx="$1"
    local host="${SERVERS[$idx]}"
    local vid="${VALIDATOR_IDS[$idx]}"

    echo "━━━ ${vid} (${host}) ━━━"

    local cmd="tail -n ${TAIL_N} ${REMOTE_LOGS}/validator.log"
    if [ -n "$GREP_PATTERN" ]; then
        cmd="${cmd} | grep --color=always '${GREP_PATTERN}'"
    fi

    if [ "$FOLLOW" = true ]; then
        cmd="tail -f ${REMOTE_LOGS}/validator.log"
        if [ -n "$GREP_PATTERN" ]; then
            cmd="${cmd} | grep --line-buffered --color=always '${GREP_PATTERN}'"
        fi
        echo "  (Ctrl+C 退出)"
        remote_exec "$host" "$cmd" || true
    else
        remote_exec "$host" "$cmd" 2>/dev/null || echo "  (无日志)"
        echo ""
    fi
}

case "$TARGET" in
    1) show_logs 0 ;;
    2) show_logs 1 ;;
    3) show_logs 2 ;;
    all)
        if [ "$FOLLOW" = true ]; then
            echo "follow 模式仅支持单节点，请指定节点: ./logs.sh 1 -f"
            exit 1
        fi
        for i in "${!SERVERS[@]}"; do
            show_logs "$i"
        done
        ;;
    *)
        echo "用法: $0 [1|2|3|all] [--tail N] [--follow] [--grep PATTERN]"
        echo ""
        echo "示例:"
        echo "  $0 1 -f              # 实时跟踪 validator-1 日志"
        echo "  $0 all -n 50         # 所有节点最近 50 行"
        echo "  $0 2 --grep ERROR    # validator-2 的错误日志"
        echo "  $0 all --grep 'CF.*finalize'  # 所有节点的 CF finalize 日志"
        exit 1
        ;;
esac
