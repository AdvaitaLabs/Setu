#!/bin/bash
# ============================================================================
# Setu Multi-Validator 远程部署配置
# ============================================================================

# ── .env 配置加载 ────────────────────────────────────────────────────────────
# 所有敏感信息（IP、密码）从 deploy/.env 读取 (格式: KEY = value)
# 模板见 deploy/.env.example
_ENV_FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.env"
if [ ! -f "$_ENV_FILE" ]; then
    echo "ERROR: deploy/.env not found. Copy deploy/.env.example to deploy/.env and fill in values."
    exit 1
fi

_read_env() { grep "^$1" "$_ENV_FILE" | cut -d'=' -f2 | tr -d ' '; }

# ── 服务器列表 (从 .env 读取) ───────────────────────────────────────────────
SERVERS=(
    "$(_read_env 'VALIDATOR-NODE-IP-01')"
    "$(_read_env 'VALIDATOR-NODE-IP-02')"
    "$(_read_env 'VALIDATOR-NODE-IP-03')"
)

VALIDATOR_IDS=(
    "validator-1"
    "validator-2"
    "validator-3"
)

# ── SSH 配置 ─────────────────────────────────────────────────────────────────
SSH_USER=$(_read_env 'VALIDATOR-NODE-USERNAME')
SSH_PWD=$(_read_env 'VALIDATOR-NODE-PWD')
SSH_USER="${SSH_USER:-root}"
SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10 -o ServerAliveInterval=30 -o ServerAliveCountMax=10 -o LogLevel=ERROR"

# 构建服务器 (默认使用第一台)
BUILD_SERVER="${SERVERS[0]}"

# ── 远程路径 ─────────────────────────────────────────────────────────────────
REMOTE_BASE="/opt/setu"
REMOTE_SRC="${REMOTE_BASE}/src"
REMOTE_BIN="${REMOTE_BASE}/bin"
REMOTE_KEYS="${REMOTE_BASE}/keys"
REMOTE_CONFIG="${REMOTE_BASE}/config"
REMOTE_DATA="${REMOTE_BASE}/data"
REMOTE_LOGS="${REMOTE_BASE}/logs"

# ── 端口配置 ─────────────────────────────────────────────────────────────────
HTTP_PORT=8080          # HTTP API 端口 (所有节点统一)
P2P_PORT=9000           # P2P 端口 (所有节点统一)
SOLVER_PORT=9001        # Solver 端口

# ── 日志级别 ─────────────────────────────────────────────────────────────────
RUST_LOG="info,setu_validator=debug,consensus=debug"

# ── 本地项目路径 ─────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ============================================================================
# SSH 辅助函数
# ============================================================================

# 远程执行命令
remote_exec() {
    local host="$1"
    shift
    if command -v sshpass &>/dev/null; then
        sshpass -p "$SSH_PWD" ssh $SSH_OPTS "${SSH_USER}@${host}" "$@"
    else
        ssh $SSH_OPTS "${SSH_USER}@${host}" "$@"
    fi
}

# 远程复制文件 (本地 → 远程)
remote_copy() {
    local src="$1"
    local host="$2"
    local dest="$3"
    if command -v sshpass &>/dev/null; then
        sshpass -p "$SSH_PWD" scp $SSH_OPTS -r "$src" "${SSH_USER}@${host}:${dest}"
    else
        scp $SSH_OPTS -r "$src" "${SSH_USER}@${host}:${dest}"
    fi
}

# 远程同步目录 (增量)
remote_sync() {
    local src="$1"
    local host="$2"
    local dest="$3"
    local rsync_opts="--delete"
    if [ "${4:-}" = "--no-delete" ]; then
        rsync_opts=""
    fi

    local exclude_opts=(
        --exclude 'target/'
        --exclude '.git/'
        --exclude '.github/'
        --exclude 'logs/'
        --exclude 'deploy/'
        --exclude 'docs/'
        --exclude 'ai/'
        --exclude 'keys/'
        --exclude 'tests/'
        --exclude 'docker/'
        --exclude 'scripts/'
        --exclude 'setu-framework/compiled/'
        --exclude '*.md'
        --exclude '*.log'
    )

    if command -v sshpass &>/dev/null; then
        SSHPASS="$SSH_PWD" rsync -az $rsync_opts \
            "${exclude_opts[@]}" \
            -e "sshpass -e ssh $SSH_OPTS" \
            "$src" "${SSH_USER}@${host}:${dest}"
    else
        rsync -az $rsync_opts \
            "${exclude_opts[@]}" \
            -e "ssh $SSH_OPTS" \
            "$src" "${SSH_USER}@${host}:${dest}"
    fi
}

# 服务器间复制文件: 在 from_host 上执行 scp 直接推送到 to_host
# (避免 scp -3 双重认证问题，使用 SSHPASS 环境变量避免密码引号问题)
remote_to_remote_copy() {
    local from_host="$1"
    local from_path="$2"
    local to_host="$3"
    local to_path="$4"
    remote_exec "$from_host" \
        "SSHPASS='${SSH_PWD}' sshpass -e scp -o StrictHostKeyChecking=no -o LogLevel=ERROR '${from_path}' '${SSH_USER}@${to_host}:${to_path}'"
}

# 等待服务健康
wait_for_health() {
    local host="$1"
    local port="$2"
    local max_wait="${3:-30}"
    local elapsed=0
    while [ $elapsed -lt $max_wait ]; do
        if curl -sf --connect-timeout 2 "http://${host}:${port}/api/v1/health" &>/dev/null; then
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done
    return 1
}

# 获取 PEER_VALIDATORS 列表 (排除自身)
get_peer_validators() {
    local self_index="$1"
    local peers=""
    for i in "${!SERVERS[@]}"; do
        if [ "$i" -ne "$self_index" ]; then
            if [ -n "$peers" ]; then
                peers="${peers},"
            fi
            peers="${peers}${SERVERS[$i]}:${P2P_PORT}"
        fi
    done
    echo "$peers"
}

# 打印分隔线
print_header() {
    echo ""
    echo "╔════════════════════════════════════════════════════════════╗"
    printf "║  %-56s ║\n" "$1"
    echo "╚════════════════════════════════════════════════════════════╝"
}

print_step() {
    echo "  [$1/${2}] $3"
}

print_ok() {
    echo "  ✓ $1"
}

print_warn() {
    echo "  ⚠ $1"
}

print_err() {
    echo "  ✗ $1"
}
