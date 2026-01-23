#!/bin/bash
# Setu Complete Deployment Script (with Key Generation)
# Usage: ./scripts/deploy_with_keys.sh

set -e

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║      Setu Node Deployment Script (Key System Version)     ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════╝${NC}"

# Configuration directories
DATA_DIR=${DATA_DIR:-/data}
KEYS_DIR=${DATA_DIR}/keys
LOGS_DIR=${DATA_DIR}/logs
PIDS_DIR=${DATA_DIR}/pids

# Create necessary directories
echo -e "${YELLOW}[1/6] Creating directory structure...${NC}"
mkdir -p ${KEYS_DIR}
mkdir -p ${LOGS_DIR}
mkdir -p ${PIDS_DIR}
chmod 700 ${KEYS_DIR}  # Set strict permissions for keys directory

# Check if already compiled
if [ ! -f "${VALIDATOR_BIN}" ] || [ ! -f "${SOLVER_BIN}" ] || [ ! -f "${CLI_BIN}" ]; then
    echo -e "${YELLOW}[2/6] Compiling project...${NC}"
    cargo build --release
else
    echo -e "${GREEN}[2/6] ✓ Already compiled${NC}"
fi

# Get the project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CLI_BIN="${PROJECT_ROOT${CLI_BIN}"
VALIDATOR_BIN="${PROJECT_ROOT${VALIDATOR_BIN}"
SOLVER_BIN="${PROJECT_ROOT${SOLVER_BIN}"

# ============================================
# Generate Validator Key
# ============================================
echo -e "${YELLOW}[3/6] Generating Validator key...${NC}"

if [ -f "${KEYS_DIR}/validator-key.json" ]; then
    echo -e "${GREEN}  ✓ Validator key already exists: ${KEYS_DIR}/validator-key.json${NC}"
    read -p "  Regenerate? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}  Backing up old key...${NC}"
        mv ${KEYS_DIR}/validator-key.json ${KEYS_DIR}/validator-key.json.backup.$(date +%s)
        
        echo -e "${YELLOW}  Generating new key...${NC}"
        ${CLI_BIN} keygen generate \
            --output ${KEYS_DIR}/validator-key.json \
            --words 24
        
        echo -e "${GREEN}  ✓ New key generated${NC}"
        echo -e "${RED}  ⚠️  Please backup your mnemonic phrase!${NC}"
        ${CLI_BIN} keygen export \
            --key-file ${KEYS_DIR}/validator-key.json \
            --format mnemonic
    fi
else
    echo -e "${YELLOW}  Generating new key...${NC}"
    ${CLI_BIN} keygen generate \
        --output ${KEYS_DIR}/validator-key.json \
        --words 24
    
    echo -e "${GREEN}  ✓ Key generated${NC}"
    echo -e "${RED}  ⚠️  Please backup your mnemonic phrase!${NC}"
    ${CLI_BIN} keygen export \
        --key-file ${KEYS_DIR}/validator-key.json \
        --format mnemonic
fi

# Display Validator address info
echo -e "${YELLOW}  Validator address info:${NC}"
${CLI_BIN} keygen export \
    --key-file ${KEYS_DIR}/validator-key.json \
    --format json | grep -E "address|public_key"

# ============================================
# Generate Solver Key
# ============================================
echo -e "${YELLOW}[4/6] Generating Solver key...${NC}"

if [ -f "${KEYS_DIR}/solver-key.json" ]; then
    echo -e "${GREEN}  ✓ Solver key already exists: ${KEYS_DIR}/solver-key.json${NC}"
    read -p "  Regenerate? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}  Backing up old key...${NC}"
        mv ${KEYS_DIR}/solver-key.json ${KEYS_DIR}/solver-key.json.backup.$(date +%s)
        
        echo -e "${YELLOW}  Generating new key...${NC}"
        ${CLI_BIN} keygen generate \
            --output ${KEYS_DIR}/solver-key.json \
            --words 24
        
        echo -e "${GREEN}  ✓ New key generated${NC}"
        echo -e "${RED}  ⚠️  Please backup your mnemonic phrase!${NC}"
        ${CLI_BIN} keygen export \
            --key-file ${KEYS_DIR}/solver-key.json \
            --format mnemonic
    fi
else
    echo -e "${YELLOW}  Generating new key...${NC}"
    ${CLI_BIN} keygen generate \
        --output ${KEYS_DIR}/solver-key.json \
        --words 24
    
    echo -e "${GREEN}  ✓ Key generated${NC}"
    echo -e "${RED}  ⚠️  Please backup your mnemonic phrase!${NC}"
    ${CLI_BIN} keygen export \
        --key-file ${KEYS_DIR}/solver-key.json \
        --format mnemonic
fi

# Display Solver address info
echo -e "${YELLOW}  Solver address info:${NC}"
${CLI_BIN} keygen export \
    --key-file ${KEYS_DIR}/solver-key.json \
    --format json | grep -E "address|public_key"

# ============================================
# Start Validator
# ============================================
echo -e "${YELLOW}[5/6] Starting Validator...${NC}"

# Check if already running
if [ -f "${PIDS_DIR}/validator.pid" ]; then
    OLD_PID=$(cat ${PIDS_DIR}/validator.pid)
    if ps -p $OLD_PID > /dev/null 2>&1; then
        echo -e "${YELLOW}  Validator already running (PID: $OLD_PID)${NC}"
        read -p "  Restart? (y/N): " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            echo -e "${YELLOW}  Stopping old process...${NC}"
            kill $OLD_PID
            sleep 2
        else
            echo -e "${GREEN}  Skipping Validator startup${NC}"
            SKIP_VALIDATOR=1
        fi
    fi
fi

if [ -z "$SKIP_VALIDATOR" ]; then
    export VALIDATOR_ID=${VALIDATOR_ID:-validator-1}
    export VALIDATOR_HTTP_PORT=${VALIDATOR_HTTP_PORT:-8080}
    export VALIDATOR_P2P_PORT=${VALIDATOR_P2P_PORT:-8081}
    export VALIDATOR_LISTEN_ADDR=${VALIDATOR_LISTEN_ADDR:-0.0.0.0}
    export VALIDATOR_KEY_FILE=${KEYS_DIR}/validator-key.json
    export RUST_LOG=${RUST_LOG:-info,setu_validator=debug,consensus=debug}

    nohup ${VALIDATOR_BIN} >> ${LOGS_DIR}/validator.log 2>&1 &
    echo $! > ${PIDS_DIR}/validator.pid
    
    echo -e "${GREEN}  ✓ Validator started (PID: $(cat ${PIDS_DIR}/validator.pid))${NC}"
    echo -e "${GREEN}    Log: ${LOGS_DIR}/validator.log${NC}"
    
    # Wait for startup
    sleep 3
    
    # Health check
    if curl -s http://localhost:${VALIDATOR_HTTP_PORT}/api/v1/health > /dev/null; then
        echo -e "${GREEN}  ✓ Validator health check passed${NC}"
        curl -s http://localhost:${VALIDATOR_HTTP_PORT}/api/v1/health | jq .
    else
        echo -e "${RED}  ✗ Validator health check failed${NC}"
    fi
fi

# ============================================
# Start Solver
# ============================================
echo -e "${YELLOW}[6/6] Starting Solver...${NC}"

# Check if already running
if [ -f "${PIDS_DIR}/solver.pid" ]; then
    OLD_PID=$(cat ${PIDS_DIR}/solver.pid)
    if ps -p $OLD_PID > /dev/null 2>&1; then
        echo -e "${YELLOW}  Solver already running (PID: $OLD_PID)${NC}"
        read -p "  Restart? (y/N): " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            echo -e "${YELLOW}  Stopping old process...${NC}"
            kill $OLD_PID
            sleep 2
        else
            echo -e "${GREEN}  Skipping Solver startup${NC}"
            SKIP_SOLVER=1
        fi
    fi
fi

if [ -z "$SKIP_SOLVER" ]; then
    export SOLVER_ID=${SOLVER_ID:-solver-1}
    export SOLVER_PORT=${SOLVER_PORT:-9001}
    export SOLVER_LISTEN_ADDR=${SOLVER_LISTEN_ADDR:-0.0.0.0}
    export SOLVER_CAPACITY=${SOLVER_CAPACITY:-100}
    export VALIDATOR_ADDRESS=${VALIDATOR_ADDRESS:-127.0.0.1}
    export VALIDATOR_HTTP_PORT=${VALIDATOR_HTTP_PORT:-8080}
    export SOLVER_KEY_FILE=${KEYS_DIR}/solver-key.json
    export AUTO_REGISTER=${AUTO_REGISTER:-true}
    export RUST_LOG=${RUST_LOG:-info,setu_solver=debug}

    nohup ${SOLVER_BIN} >> ${LOGS_DIR}/solver.log 2>&1 &
    echo $! > ${PIDS_DIR}/solver.pid
    
    echo -e "${GREEN}  ✓ Solver started (PID: $(cat ${PIDS_DIR}/solver.pid))${NC}"
    echo -e "${GREEN}    Log: ${LOGS_DIR}/solver.log${NC}"
fi

# ============================================
# Deployment Complete
# ============================================
echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║                  Deployment Complete!                      ║${NC}"
echo -e "${GREEN}╠════════════════════════════════════════════════════════════╣${NC}"
echo -e "${GREEN}║  Validator:                                                ║${NC}"
echo -e "${GREEN}║    PID:      $(cat ${PIDS_DIR}/validator.pid 2>/dev/null || echo 'N/A')                                              ║${NC}"
echo -e "${GREEN}║    HTTP:     http://localhost:${VALIDATOR_HTTP_PORT}                       ║${NC}"
echo -e "${GREEN}║    Key:      ${KEYS_DIR}/validator-key.json        ║${NC}"
echo -e "${GREEN}║    Log:      ${LOGS_DIR}/validator.log             ║${NC}"
echo -e "${GREEN}║                                                            ║${NC}"
echo -e "${GREEN}║  Solver:                                                   ║${NC}"
echo -e "${GREEN}║    PID:      $(cat ${PIDS_DIR}/solver.pid 2>/dev/null || echo 'N/A')                                              ║${NC}"
echo -e "${GREEN}║    Port:     ${SOLVER_PORT}                                           ║${NC}"
echo -e "${GREEN}║    Key:      ${KEYS_DIR}/solver-key.json           ║${NC}"
echo -e "${GREEN}║    Log:      ${LOGS_DIR}/solver.log                ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${YELLOW}Common commands:${NC}"
echo -e "  View Validator log: tail -f ${LOGS_DIR}/validator.log"
echo -e "  View Solver log:    tail -f ${LOGS_DIR}/solver.log"
echo -e "  Stop Validator:     kill \$(cat ${PIDS_DIR}/validator.pid)"
echo -e "  Stop Solver:        kill \$(cat ${PIDS_DIR}/solver.pid)"
echo -e "  Health check:       curl http://localhost:${VALIDATOR_HTTP_PORT}/api/v1/health"
echo ""
echo -e "${RED}⚠️  Important reminders:${NC}"
echo -e "  1. Please backup your key files and mnemonic phrases!"
echo -e "  2. Key files location: ${KEYS_DIR}/"
echo -e "  3. Do not commit key files to Git repository!"
echo ""

