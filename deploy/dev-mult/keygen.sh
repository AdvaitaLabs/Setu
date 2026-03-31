#!/bin/bash
# ============================================================================
# 生成 Validator 密钥对并更新 genesis-remote.json
# 在构建服务器上使用 setu-cli 生成 ed25519 密钥
# 用法: ./keygen.sh
# ============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/config.sh"

print_header "生成 Validator 密钥对"

# 检查本地 jq (更新 genesis-remote.json 需要)
if ! command -v jq &>/dev/null; then
    print_warn "jq 未安装，无法自动更新 genesis-remote.json 的公钥"
    echo "  macOS 安装: brew install jq"
    echo "  密钥仍会生成，但需手动更新公钥"
    echo ""
fi

# 检查构建服务器上是否有 setu-cli
echo "  检查 setu-cli..."
if ! remote_exec "$BUILD_SERVER" "test -f ${REMOTE_BIN}/setu-cli" 2>/dev/null; then
    print_err "setu-cli 未找到，请先运行 ./build.sh"
    exit 1
fi

# 为每个 validator 生成密钥
PUB_KEYS=()
for i in "${!VALIDATOR_IDS[@]}"; do
    vid="${VALIDATOR_IDS[$i]}"
    host="${SERVERS[$i]}"
    key_file="${REMOTE_KEYS}/${vid}.key"
    
    print_step $((i+1)) ${#VALIDATOR_IDS[@]} "生成 ${vid} 密钥..."
    
    # 在构建服务器上生成密钥
    output=$(remote_exec "$BUILD_SERVER" "
        ${REMOTE_BIN}/setu-cli gen-key generate \
            --scheme ed25519 \
            --output ${REMOTE_KEYS}/${vid}.key \
            --json 2>/dev/null || echo 'KEYGEN_FAILED'
    ")
    
    if echo "$output" | grep -q 'KEYGEN_FAILED'; then
        print_err "密钥生成失败: ${vid}"
        exit 1
    fi

    # 提取公钥
    pub_key=$(remote_exec "$BUILD_SERVER" "
        ${REMOTE_BIN}/setu-cli gen-key inspect ${REMOTE_KEYS}/${vid}.key 2>/dev/null \
            | grep -i 'public.*key' | head -1 | awk '{print \$NF}' \
            || echo ''
    ")
    
    # 如果 inspect 无法提取，尝试 JSON 输出
    if [ -z "$pub_key" ]; then
        pub_key=$(echo "$output" | jq -r '.public_key // empty' 2>/dev/null || echo "")
    fi

    PUB_KEYS+=("$pub_key")
    echo "    公钥: ${pub_key:0:16}..."
    
    # 分发密钥到对应服务器
    if [ "$i" -ne 0 ]; then
        echo "    → 分发到 ${host}"
        remote_exec "$host" "mkdir -p ${REMOTE_KEYS}"
        remote_to_remote_copy "$BUILD_SERVER" "${key_file}" "$host" "${key_file}"
    fi
done

echo ""
print_ok "所有密钥已生成并分发"

# 更新本地 genesis-remote.json 中的 public_key
if ! command -v jq &>/dev/null; then
    print_warn "jq 未安装，跳过 genesis-remote.json 更新"
    echo "  请手动将以下公钥填入 genesis-remote.json:"
    for i in "${!VALIDATOR_IDS[@]}"; do
        echo "    ${VALIDATOR_IDS[$i]}: ${PUB_KEYS[$i]}"
    done
elif [ ${#PUB_KEYS[@]} -eq ${#VALIDATOR_IDS[@]} ] && [ -n "${PUB_KEYS[0]}" ]; then
    echo ""
    echo "  更新 genesis-remote.json 中的公钥..."
    
    local_genesis="${SCRIPT_DIR}/genesis-remote.json"
    
    for i in "${!VALIDATOR_IDS[@]}"; do
        vid="${VALIDATOR_IDS[$i]}"
        pk="${PUB_KEYS[$i]}"
        if [ -n "$pk" ]; then
            # 使用 jq 更新对应 validator 的 public_key
            tmp=$(mktemp)
            jq --arg vid "$vid" --arg pk "$pk" \
                '(.validators[] | select(.id == $vid)).public_key = $pk' \
                "$local_genesis" > "$tmp" && mv "$tmp" "$local_genesis"
        fi
    done
    
    print_ok "genesis-remote.json 已更新"
    echo "  请重新运行 ./deploy.sh 以分发更新后的配置"
else
    print_warn "部分公钥获取失败，请手动更新 genesis-remote.json"
fi
