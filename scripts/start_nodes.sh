#!/bin/bash
# Setu Node Startup Script (assumes keys are already generated)
# Usage: ./scripts/start_nodes.sh

set -e

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║              Setu Node Startup Script                      ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════╝${NC}"

# Configuration
DATA_DIR=${DATA_DIR:-/data}
KEYS_DIR=${DATA_DIR}/keys
LOGS_DIR=${DATA_DIR}/logs
PIDS_DIR=${DATA_DIR}/pids

# Check key files
if [ ! -f "${KEYS_DIR}/validator-key.json" ]; then
    echo -e "${RED}Error: Validator key file not found: ${KEYS_DIR}/validator-key.json${NC}"
    echo -e "${YELLOW}Please run first: ./scripts/deploy_with_keys.sh${NC}"
    exit 1
fi

if [ ! -f "${KEYS_DIR}/solver-key.json" ]; then
    echo -e "${RED}Error: Solver key file not found: ${KEYS_DIR}/solver-key.json${NC}"
    echo -e "${YELLOW}Please run first: ./scripts/deploy_with_keys.sh${NC}"
    exit 1
fi

# ============================================
# Start Validator
# ============================================
echo -e "${YELLOW}[1/2] Starting Validator...${NC}"

export VALIDATOR_ID=${VALIDATOR_ID:-validator-1}
export VALIDATOR_HTTP_PORT=${VALIDATOR_HTTP_PORT:-8080}
export VALIDATOR_P2P_PORT=${VALIDATOR_P2P_PORT:-8081}
export VALIDATOR_LISTEN_ADDR=${VALIDATOR_LISTEN_ADDR:-0.0.0.0}
export VALIDATOR_KEY_FILE=${KEYS_DIR}/validator-key.json
export RUST_LOG=${RUST_LOG:-info,setu_validator=debug,consensus=debug}

nohup ./target/release/setu-validator >> ${LOGS_DIR}/validator.log 2>&1 &
echo $! > ${PIDS_DIR}/validator.pid

echo -e "${GREEN}  ✓ Validator started (PID: $(cat ${PIDS_DIR}/validator.pid))${NC}"

# Wait for Validator to start
sleep 3

# Health check
if curl -s http://localhost:${VALIDATOR_HTTP_PORT}/api/v1/health > /dev/null; then
    echo -e "${GREEN}  ✓ Validator health check passed${NC}"
else
    echo -e "${RED}  ✗ Validator health check failed${NC}"
fi

# ============================================
# Start Solver
# ============================================
echo -e "${YELLOW}[2/2] Starting Solver...${NC}"

export SOLVER_ID=${SOLVER_ID:-solver-1}
export SOLVER_PORT=${SOLVER_PORT:-9001}
export SOLVER_LISTEN_ADDR=${SOLVER_LISTEN_ADDR:-0.0.0.0}
export SOLVER_CAPACITY=${SOLVER_CAPACITY:-100}
export VALIDATOR_ADDRESS=${VALIDATOR_ADDRESS:-127.0.0.1}
export VALIDATOR_HTTP_PORT=${VALIDATOR_HTTP_PORT:-8080}
export SOLVER_KEY_FILE=${KEYS_DIR}/solver-key.json
export AUTO_REGISTER=${AUTO_REGISTER:-true}
export RUST_LOG=${RUST_LOG:-info,setu_solver=debug}

nohup ./target/release/setu-solver >> ${LOGS_DIR}/solver.log 2>&1 &
echo $! > ${PIDS_DIR}/solver.pid

echo -e "${GREEN}  ✓ Solver started (PID: $(cat ${PIDS_DIR}/solver.pid))${NC}"

echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║                    Startup Complete!                       ║${NC}"
echo -e "${GREEN}╠════════════════════════════════════════════════════════════╣${NC}"
echo -e "${GREEN}║  Validator PID: $(cat ${PIDS_DIR}/validator.pid)                                      ║${NC}"
echo -e "${GREEN}║  Solver PID:    $(cat ${PIDS_DIR}/solver.pid)                                      ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════╝${NC}"

