#!/bin/bash
# Setu Quick Start Script
# 
# This script provides a simple way to start a local Setu network for development

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
LOG_DIR="/data/log"

cd "$PROJECT_DIR"

# Create log directory if it doesn't exist
mkdir -p "$LOG_DIR"

echo "╔════════════════════════════════════════════════════════════╗"
echo "║              Setu Quick Start                              ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""

# Start validator
echo "[1/2] Starting Validator..."
export VALIDATOR_ID=validator-1
export VALIDATOR_HTTP_PORT=8080
export VALIDATOR_LISTEN_ADDR=127.0.0.1

cargo run --release -p setu-validator > "$LOG_DIR/validator.log" 2>&1 &
VALIDATOR_PID=$!
echo "  └─ Validator PID: $VALIDATOR_PID"
echo "  └─ Waiting for validator to start..."
sleep 5

# Check if validator is running
if curl -s http://127.0.0.1:8080/api/v1/health > /dev/null 2>&1; then
    echo "  └─ ✓ Validator is running"
else
    echo "  └─ ✗ Failed to start validator"
    kill $VALIDATOR_PID 2>/dev/null || true
    exit 1
fi

echo ""

# Start solver
echo "[2/2] Starting Solver..."
export SOLVER_ID=solver-1
export SOLVER_PORT=9001
export SOLVER_LISTEN_ADDR=127.0.0.1
export SOLVER_CAPACITY=100
export VALIDATOR_ADDRESS=127.0.0.1
export VALIDATOR_HTTP_PORT=8080
export AUTO_REGISTER=true

cargo run --release -p setu-solver > "$LOG_DIR/solver.log" 2>&1 &
SOLVER_PID=$!
echo "  └─ Solver PID: $SOLVER_PID"
echo "  └─ Waiting for solver to register..."
sleep 5

# Check if solver is registered
if curl -s http://127.0.0.1:8080/api/v1/solvers | grep -q "solver-1"; then
    echo "  └─ ✓ Solver registered successfully"
else
    echo "  └─ ⚠ Solver registration status unknown"
fi

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║              Setu Network Started                          ║"
echo "╠════════════════════════════════════════════════════════════╣"
echo "║  Validator API: http://127.0.0.1:8080/api/v1              ║"
echo "║  Health Check:  http://127.0.0.1:8080/api/v1/health       ║"
echo "║                                                            ║"
echo "║  Validator PID: $VALIDATOR_PID                                        ║"
echo "║  Solver PID:    $SOLVER_PID                                        ║"
echo "║                                                            ║"
echo "║  Logs:          /data/log/validator.log, /data/log/solver.log ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""
echo "To stop the network:"
echo "  kill $VALIDATOR_PID $SOLVER_PID"
echo ""
echo "To test the API:"
echo "  curl http://127.0.0.1:8080/api/v1/health"
echo "  curl http://127.0.0.1:8080/api/v1/solvers"
echo ""

# Save PIDs for later
mkdir -p pids
echo $VALIDATOR_PID > pids/validator.pid
echo $SOLVER_PID > pids/solver.pid

echo "PIDs saved to pids/ directory"

