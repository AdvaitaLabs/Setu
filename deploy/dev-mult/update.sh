#!/bin/bash
# ============================================================================
# 快速更新: 仅同步源码 → 增量编译 → 分发二进制 → 重启
# 适用于代码修改后的快速迭代部署
# 用法: ./update.sh [--no-restart]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

NO_RESTART=false
if [ "${1:-}" = "--no-restart" ]; then
    NO_RESTART=true
fi

print_header "快速更新部署"

START_TIME=$(date +%s)

# Step 1: 同步 + 编译 + 分发
bash "${SCRIPT_DIR}/build.sh"

# Step 2: 重启
if [ "$NO_RESTART" = false ]; then
    echo ""
    bash "${SCRIPT_DIR}/restart.sh" all
fi

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

echo ""
print_ok "更新完成 (耗时 ${ELAPSED}s)"
