#!/bin/bash
# Setu Node Stop Script
# Usage: ./scripts/stop_nodes.sh [--local]

set -e

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${YELLOW}Stopping Setu nodes...${NC}"

# Get the project root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Check for --local flag
LOCAL_MODE=false
for arg in "$@"; do
    if [ "$arg" = "--local" ]; then
        LOCAL_MODE=true
    fi
done

# Configuration - use local directory for development
if [ "$LOCAL_MODE" = true ] || [ ! -d "/data" ]; then
    DATA_DIR=${DATA_DIR:-${PROJECT_ROOT}/.setu-data}
else
    DATA_DIR=${DATA_DIR:-/data}
fi

PIDS_DIR=${DATA_DIR}/pids

# Stop Solver
if [ -f "${PIDS_DIR}/solver.pid" ]; then
    SOLVER_PID=$(cat ${PIDS_DIR}/solver.pid)
    if ps -p $SOLVER_PID > /dev/null 2>&1; then
        echo -e "${YELLOW}Stopping Solver (PID: $SOLVER_PID)...${NC}"
        kill $SOLVER_PID
        sleep 2
        if ps -p $SOLVER_PID > /dev/null 2>&1; then
            echo -e "${RED}Force stopping Solver...${NC}"
            kill -9 $SOLVER_PID
        fi
        echo -e "${GREEN}✓ Solver stopped${NC}"
    else
        echo -e "${YELLOW}Solver not running${NC}"
    fi
    rm -f ${PIDS_DIR}/solver.pid
else
    echo -e "${YELLOW}Solver PID file not found${NC}"
fi

# Stop Validator
if [ -f "${PIDS_DIR}/validator.pid" ]; then
    VALIDATOR_PID=$(cat ${PIDS_DIR}/validator.pid)
    if ps -p $VALIDATOR_PID > /dev/null 2>&1; then
        echo -e "${YELLOW}Stopping Validator (PID: $VALIDATOR_PID)...${NC}"
        kill $VALIDATOR_PID
        sleep 2
        if ps -p $VALIDATOR_PID > /dev/null 2>&1; then
            echo -e "${RED}Force stopping Validator...${NC}"
            kill -9 $VALIDATOR_PID
        fi
        echo -e "${GREEN}✓ Validator stopped${NC}"
    else
        echo -e "${YELLOW}Validator not running${NC}"
    fi
    rm -f ${PIDS_DIR}/validator.pid
else
    echo -e "${YELLOW}Validator PID file not found${NC}"
fi

echo -e "${GREEN}All nodes stopped${NC}"

