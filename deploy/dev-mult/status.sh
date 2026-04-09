#!/bin/bash
# ============================================================================
# 检查所有节点状态: 进程、HTTP 健康、P2P 端口、磁盘空间
# 用法: ./status.sh [--verbose]
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

VERBOSE="${1:-}"

print_header "Setu 集群状态"

printf "  %-14s %-18s %-10s %-10s %-10s %s\n" \
    "节点" "IP" "进程" "HTTP" "P2P" "备注"
echo "  ────────────── ────────────────── ────────── ────────── ────────── ──────"

for i in "${!SERVERS[@]}"; do
    host="${SERVERS[$i]}"
    vid="${VALIDATOR_IDS[$i]}"
    
    # 检查进程
    proc_status="✗"
    if remote_exec "$host" "pgrep -f setu-validator" &>/dev/null; then
        proc_status="✓ 运行"
    else
        proc_status="✗ 停止"
    fi
    
    # 检查 HTTP 健康
    http_status="✗"
    health_resp=$(curl -sf --connect-timeout 3 "http://${host}:${HTTP_PORT}/api/v1/health" 2>/dev/null || echo "")
    if [ -n "$health_resp" ]; then
        http_status="✓ 健康"
    else
        http_status="✗ 无响应"
    fi
    
    # 检查 P2P 端口 (UDP/QUIC)
    p2p_status="?"
    if remote_exec "$host" "ss -ulnp | grep -q ':${P2P_PORT}'" 2>/dev/null; then
        p2p_status="✓ 监听"
    else
        p2p_status="✗ 未监"
    fi
    
    # 备注
    note=""
    if [ "$i" -eq 0 ]; then
        note="(构建服务器)"
    fi
    
    printf "  %-14s %-18s %-10s %-10s %-10s %s\n" \
        "$vid" "$host" "$proc_status" "$http_status" "$p2p_status" "$note"
done

if [ "$VERBOSE" = "--verbose" ] || [ "$VERBOSE" = "-v" ]; then
    echo ""
    echo "━━━ 详细信息 ━━━"
    for i in "${!SERVERS[@]}"; do
        host="${SERVERS[$i]}"
        vid="${VALIDATOR_IDS[$i]}"
        
        echo ""
        echo "  【${vid}】${host}"
        
        # 进程信息
        echo "  进程:"
        remote_exec "$host" "ps aux | grep -E 'setu-(validator|solver)' | grep -v grep || echo '    (无运行进程)'" 2>/dev/null
        
        # 磁盘空间
        echo "  磁盘:"
        remote_exec "$host" "df -h ${REMOTE_BASE} 2>/dev/null | tail -1 || echo '    (未知)'" 2>/dev/null
        
        # RocksDB 大小
        echo "  数据:"
        remote_exec "$host" "du -sh ${REMOTE_DATA}/db 2>/dev/null || echo '    (无数据)'" 2>/dev/null
        
        # 日志最后几行
        echo "  最近日志:"
        remote_exec "$host" "tail -3 ${REMOTE_LOGS}/validator.log 2>/dev/null || echo '    (无日志)'" 2>/dev/null
        
        # 健康详情
        health=$(curl -sf --connect-timeout 3 "http://${host}:${HTTP_PORT}/api/v1/health" 2>/dev/null || echo "")
        if [ -n "$health" ]; then
            echo "  健康响应: ${health}"
        fi
    done
fi

echo ""
