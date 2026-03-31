#!/bin/bash
set -e

echo "========================================="
echo " Setu Multi-Validator Cluster (3 nodes)"
echo "========================================="

GENESIS_FILE="${GENESIS_FILE:-genesis-multi.json}"
PIDS=()

cleanup() {
    echo ""
    echo "Shutting down validators..."
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null
    echo "All validators stopped."
}
trap cleanup EXIT INT TERM

# Validator 1
echo "Starting Validator-1 (HTTP=8080, P2P=9000)..."
NODE_ID=validator-1 \
  VALIDATOR_HTTP_PORT=8080 \
  VALIDATOR_P2P_PORT=9000 \
  VALIDATOR_LISTEN_ADDR=127.0.0.1 \
  PEER_VALIDATORS= \
  GENESIS_FILE="$GENESIS_FILE" \
  VALIDATOR_KEY_FILE=keys/validator-1.key \
  RUST_LOG=info,setu_validator=debug,consensus=debug \
  cargo run --release -p setu-validator &
PIDS+=($!)
sleep 3

# Validator 2
echo "Starting Validator-2 (HTTP=8082, P2P=9002)..."
NODE_ID=validator-2 \
  VALIDATOR_HTTP_PORT=8082 \
  VALIDATOR_P2P_PORT=9002 \
  VALIDATOR_LISTEN_ADDR=127.0.0.1 \
  PEER_VALIDATORS=127.0.0.1:9000 \
  GENESIS_FILE="$GENESIS_FILE" \
  VALIDATOR_KEY_FILE=keys/validator-2.key \
  RUST_LOG=info,setu_validator=debug,consensus=debug \
  cargo run --release -p setu-validator &
PIDS+=($!)
sleep 3

# Validator 3
echo "Starting Validator-3 (HTTP=8084, P2P=9004)..."
NODE_ID=validator-3 \
  VALIDATOR_HTTP_PORT=8084 \
  VALIDATOR_P2P_PORT=9004 \
  VALIDATOR_LISTEN_ADDR=127.0.0.1 \
  PEER_VALIDATORS=127.0.0.1:9000,127.0.0.1:9002 \
  GENESIS_FILE="$GENESIS_FILE" \
  VALIDATOR_KEY_FILE=keys/validator-3.key \
  RUST_LOG=info,setu_validator=debug,consensus=debug \
  cargo run --release -p setu-validator &
PIDS+=($!)
sleep 3

echo ""
echo "========================================="
echo " Cluster Ready"
echo "========================================="
echo "  Validator-1: http://127.0.0.1:8080"
echo "  Validator-2: http://127.0.0.1:8082"
echo "  Validator-3: http://127.0.0.1:8084"
echo ""

# Health checks
for port in 8080 8082 8084; do
    if curl -sf "http://127.0.0.1:$port/api/v1/health" > /dev/null 2>&1; then
        echo "  ✓ Validator on port $port healthy"
    else
        echo "  ✗ Validator on port $port not responding (may still be starting)"
    fi
done

echo ""
echo "Press Ctrl+C to stop all validators"
wait
