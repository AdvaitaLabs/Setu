#!/bin/bash
# ============================================================================
# 重启所有 Validator 节点
# 用法: ./restart.sh [1|2|3|all]
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

TARGET="${1:-all}"

print_header "重启 Setu Validator 集群"

bash "${SCRIPT_DIR}/stop.sh" "$TARGET"
sleep 2
bash "${SCRIPT_DIR}/start.sh" "$TARGET"
