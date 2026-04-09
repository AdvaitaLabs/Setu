#!/bin/bash
#
# Setu End-to-End Test Script
#
# This script tests the complete registration and transfer flow:
# 1. Start Validator
# 2. Register a Solver (via CLI)
# 3. Submit a Transfer (via CLI)
# 4. Check Transfer Status
#
# Usage:
#   ./scripts/test_e2e.sh
#
# Prerequisites:
#   - Rust toolchain installed
#   - Project compiled (cargo build)

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color
BOLD='\033[1m'

# Configuration
VALIDATOR_PORT=8080
SOLVER_PORT=9001
VALIDATOR_ADDR="127.0.0.1:${VALIDATOR_PORT}"

echo -e "${CYAN}${BOLD}"
echo "╔════════════════════════════════════════════════════════════╗"
echo "║              Setu End-to-End Test                          ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# Function to cleanup background processes
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"
    if [ ! -z "$VALIDATOR_PID" ]; then
        kill $VALIDATOR_PID 2>/dev/null || true
    fi
    if [ ! -z "$SOLVER_PID" ]; then
        kill $SOLVER_PID 2>/dev/null || true
    fi
    echo -e "${GREEN}Done${NC}"
}

trap cleanup EXIT

# Build the project
echo -e "${CYAN}[1/6] Building project...${NC}"
cargo build --release -p setu-validator -p setu-solver -p setu-cli 2>&1 | tail -5

# Start Validator
echo -e "\n${CYAN}[2/6] Starting Validator...${NC}"
VALIDATOR_HTTP_PORT=$VALIDATOR_PORT \
NODE_ID="validator-1" \
cargo run --release -p setu-validator &
VALIDATOR_PID=$!

# Wait for Validator to start
echo -e "${YELLOW}Waiting for Validator to start...${NC}"
sleep 3

# Check if Validator is running
if ! curl -s "http://${VALIDATOR_ADDR}/api/v1/health" > /dev/null; then
    echo -e "${RED}Error: Validator failed to start${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Validator is running${NC}"

# Register a Solver via CLI
echo -e "\n${CYAN}[3/6] Registering Solver via CLI...${NC}"
cargo run --release -p setu-cli -- solver register \
    --id solver-1 \
    --address 127.0.0.1 \
    --port $SOLVER_PORT \
    --capacity 100 \
    --shard shard-0 \
    --resources ETH,BTC \
    --router $VALIDATOR_ADDR

# List registered solvers
echo -e "\n${CYAN}[4/6] Listing registered Solvers...${NC}"
cargo run --release -p setu-cli -- solver list --router $VALIDATOR_ADDR

# Submit a Transfer
echo -e "\n${CYAN}[5/6] Submitting Transfer...${NC}"
TRANSFER_OUTPUT=$(cargo run --release -p setu-cli -- transfer submit \
    --from alice \
    --to bob \
    --amount 1000 \
    --transfer-type setu \
    --router $VALIDATOR_ADDR 2>&1)

echo "$TRANSFER_OUTPUT"

# Extract transfer ID from output (if available)
TRANSFER_ID=$(echo "$TRANSFER_OUTPUT" | grep -o 'tx-[0-9]*-[0-9]*' | head -1 || echo "")

# Check Transfer Status (if we got a transfer ID)
if [ ! -z "$TRANSFER_ID" ]; then
    echo -e "\n${CYAN}[6/6] Checking Transfer Status...${NC}"
    cargo run --release -p setu-cli -- transfer status \
        --id "$TRANSFER_ID" \
        --router $VALIDATOR_ADDR
else
    echo -e "\n${YELLOW}[6/6] Skipping status check (no transfer ID found)${NC}"
fi

# Health check
echo -e "\n${CYAN}Health Check:${NC}"
curl -s "http://${VALIDATOR_ADDR}/api/v1/health" | python3 -m json.tool 2>/dev/null || \
curl -s "http://${VALIDATOR_ADDR}/api/v1/health"

echo -e "\n${GREEN}${BOLD}"
echo "╔════════════════════════════════════════════════════════════╗"
echo "║              ✓ End-to-End Test Complete                    ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

echo -e "${YELLOW}Press Ctrl+C to stop the Validator...${NC}"
wait $VALIDATOR_PID

