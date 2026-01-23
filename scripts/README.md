# Setu Scripts

This directory contains deployment and management scripts for the Setu network.

## Script List

### 1. Deployment Scripts

#### `deploy_with_keys.sh` - Complete Deployment Script (Recommended)

Complete deployment script including key generation, node startup, and health checks.

**Features:**
- Automatically creates directory structure (/data/keys, /data/logs, /data/pids)
- Generates Validator and Solver keypairs (BIP39 mnemonic)
- Displays mnemonic for backup
- Starts Validator and Solver nodes
- Performs health checks

**Usage:**
```bash
./scripts/deploy_with_keys.sh
```

**On first deployment:**
- Generates new key files
- Displays mnemonic (please backup!)
- Starts all nodes

**On subsequent runs:**
- Detects existing key files
- Asks whether to regenerate
- Asks whether to restart nodes

---

### 2. Node Management Scripts

#### `start_nodes.sh` - Start Nodes

Starts Validator and Solver nodes (assumes keys are already generated).

**Usage:**
```bash
./scripts/start_nodes.sh
```

**Prerequisites:**
- Key files exist (/data/keys/validator-key.json and /data/keys/solver-key.json)
- Project is compiled (./target/release/setu-validator and ./target/release/setu-solver)

#### `stop_nodes.sh` - Stop Nodes

Stops all running Setu nodes.

**Usage:**
```bash
./scripts/stop_nodes.sh
```

**Features:**
- Gracefully stops Solver and Validator
- Force terminates if graceful stop fails
- Cleans up PID files

---

### 3. Testing Scripts

#### `test_e2e.sh` - End-to-End Testing

Executes complete end-to-end test workflow.

**Usage:**
```bash
./scripts/test_e2e.sh
```

#### `verify_solver_tee3.sh` - TEE Architecture Verification

Verifies Solver TEE3 architecture implementation.

**Usage:**
```bash
./scripts/verify_solver_tee3.sh
```

---

## Quick Start

### First Deployment

```bash
# 1. Compile project
cargo build --release

# 2. Run complete deployment script
./scripts/deploy_with_keys.sh

# 3. View logs
tail -f /data/logs/validator.log
tail -f /data/logs/solver.log
```

### Daily Usage

```bash
# Start nodes
./scripts/start_nodes.sh

# Stop nodes
./scripts/stop_nodes.sh

# Check status
curl http://localhost:8080/api/v1/health
```

---

## Environment Variables

All scripts support customization via environment variables:

### Validator Configuration

```bash
export VALIDATOR_ID=validator-1              # Validator ID
export VALIDATOR_HTTP_PORT=8080              # HTTP API port
export VALIDATOR_P2P_PORT=8081               # P2P port
export VALIDATOR_LISTEN_ADDR=0.0.0.0         # Listen address
export VALIDATOR_KEY_FILE=/data/keys/validator-key.json  # Key file
export RUST_LOG=info,setu_validator=debug    # Log level
```

### Solver Configuration

```bash
export SOLVER_ID=solver-1                    # Solver ID
export SOLVER_PORT=9001                      # Listen port
export SOLVER_LISTEN_ADDR=0.0.0.0            # Listen address
export SOLVER_CAPACITY=100                   # Maximum capacity
export VALIDATOR_ADDRESS=127.0.0.1           # Validator address
export VALIDATOR_HTTP_PORT=8080              # Validator port
export SOLVER_KEY_FILE=/data/keys/solver-key.json  # Key file
export AUTO_REGISTER=true                    # Auto-register
export RUST_LOG=info,setu_solver=debug       # Log level
```

### Directory Configuration

```bash
export DATA_DIR=/data                        # Data root directory
```

---

## Directory Structure

Scripts will create the following directory structure:

```
/data/
├── keys/                    # Key files (permission 700)
│   ├── validator-key.json   # Validator key
│   └── solver-key.json      # Solver key
├── logs/                    # Log files
│   ├── validator.log        # Validator log
│   └── solver.log           # Solver log
└── pids/                    # PID files
    ├── validator.pid        # Validator PID
    └── solver.pid           # Solver PID
```

---

## FAQ

### Q: How to view node logs?

```bash
# Real-time Validator log
tail -f /data/logs/validator.log

# Real-time Solver log
tail -f /data/logs/solver.log

# Search for errors
grep ERROR /data/logs/validator.log
```

### Q: How to check if nodes are running?

```bash
# Check processes
ps aux | grep setu-validator
ps aux | grep setu-solver

# Health check
curl http://localhost:8080/api/v1/health
```

### Q: How to restart nodes?

```bash
# Stop nodes
./scripts/stop_nodes.sh

# Start nodes
./scripts/start_nodes.sh
```

### Q: What if key files are lost?

If you have mnemonic backup:
```bash
./target/release/setu-cli keygen recover \
  --mnemonic "your twelve or twenty four word mnemonic" \
  --output /data/keys/recovered-key.json
```

If you don't have backup, keys cannot be recovered. **Please backup your mnemonic!**

### Q: How to change ports?

Set environment variables and restart:
```bash
export VALIDATOR_HTTP_PORT=9090
./scripts/start_nodes.sh
```

---

## Security Reminders

⚠️ **Important Security Tips:**

1. **Backup Mnemonic**: After key generation, mnemonic will be displayed. Please backup to a safe place
2. **Protect Key Files**: Key files contain private keys. Do not leak or commit to Git
3. **Set File Permissions**: Key directory should be 700, key files should be 600
4. **Production Environment**: Use Hardware Security Module (HSM) in production

---

## Troubleshooting

### Node Won't Start

1. Check if port is in use: `lsof -i :8080`
2. Check if key files exist: `ls -l /data/keys/`
3. View logs for detailed errors: `tail -100 /data/logs/validator.log`

### Registration Failed

1. Confirm Validator is running: `curl http://localhost:8080/api/v1/health`
2. Check if key file path is correct
3. View Solver log: `tail -100 /data/logs/solver.log`

### Network Connection Issues

1. Check firewall settings
2. Confirm port configuration is correct
3. Verify network connectivity: `telnet localhost 8080`

---

## More Documentation

- [Complete Deployment Guide](../docs/DEPLOYMENT_WITH_KEYS.md)
- [API Documentation](~/setu-md/API_DOCUMENTATION.md)
- [Key Management Guide](../docs/DEPLOYMENT_WITH_KEYS.md#key-management)

---

**Last Updated**: 2025-01-23
