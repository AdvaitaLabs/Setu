# Setu Node Deployment Guide (Key System Version)

## Overview

This guide explains how to deploy Setu Validator and Solver nodes using the new key system.

## New Features

- ✅ BIP39 mnemonic-based key generation
- ✅ Secp256k1 + Keccak256 address derivation
- ✅ Registration message signature verification
- ✅ Account address for receiving rewards
- ✅ Economic model fields (stake amount, commission rate)

## Quick Start

### 1. Complete Deployment (First Time)

```bash
# Compile the project
cargo build --release

# Run complete deployment script (includes key generation)
./scripts/deploy_with_keys.sh
```

This script will:
1. Create necessary directory structure
2. Generate Validator and Solver keypairs
3. Display mnemonic phrases (please backup!)
4. Start Validator and Solver nodes

### 2. Start Nodes (With Existing Keys)

```bash
# Start all nodes
./scripts/start_nodes.sh
```

### 3. Stop Nodes

```bash
# Stop all nodes
./scripts/stop_nodes.sh
```

## Manual Deployment Steps

### Step 1: Generate Keys

#### Generate Validator Key

```bash
# Generate key (24-word mnemonic)
./target/release/setu-cli keygen generate \
  --output /data/keys/validator-key.json \
  --words 24

# Export mnemonic (please backup!)
./target/release/setu-cli keygen export \
  --key-file /data/keys/validator-key.json \
  --format mnemonic

# View address info
./target/release/setu-cli keygen export \
  --key-file /data/keys/validator-key.json \
  --format json
```

#### Generate Solver Key

```bash
# Generate key
./target/release/setu-cli keygen generate \
  --output /data/keys/solver-key.json \
  --words 24

# Export mnemonic
./target/release/setu-cli keygen export \
  --key-file /data/keys/solver-key.json \
  --format mnemonic
```

### Step 2: Start Validator

```bash
export VALIDATOR_ID=validator-1
export VALIDATOR_HTTP_PORT=8080
export VALIDATOR_P2P_PORT=8081
export VALIDATOR_LISTEN_ADDR=0.0.0.0
export VALIDATOR_KEY_FILE=/data/keys/validator-key.json
export RUST_LOG=info,setu_validator=debug,consensus=debug

nohup ./target/release/setu-validator >> /data/logs/validator.log 2>&1 &
echo $! > /data/pids/validator.pid

# Health check
curl http://localhost:8080/api/v1/health
```

### Step 3: Start Solver

```bash
export SOLVER_ID=solver-1
export SOLVER_PORT=9001
export SOLVER_LISTEN_ADDR=0.0.0.0
export SOLVER_CAPACITY=100
export VALIDATOR_ADDRESS=127.0.0.1
export VALIDATOR_HTTP_PORT=8080
export SOLVER_KEY_FILE=/data/keys/solver-key.json
export AUTO_REGISTER=true
export RUST_LOG=info,setu_solver=debug

nohup ./target/release/setu-solver >> /data/logs/solver.log 2>&1 &
echo $! > /data/pids/solver.pid
```

## Environment Variables

### Validator Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `VALIDATOR_ID` | Validator node ID | `validator-1` |
| `VALIDATOR_HTTP_PORT` | HTTP API port | `8080` |
| `VALIDATOR_P2P_PORT` | P2P communication port | `8081` |
| `VALIDATOR_LISTEN_ADDR` | Listen address | `0.0.0.0` |
| `VALIDATOR_KEY_FILE` | Key file path | None (optional) |
| `RUST_LOG` | Log level | `info` |

### Solver Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SOLVER_ID` | Solver node ID | `solver-1` |
| `SOLVER_PORT` | Listen port | `9001` |
| `SOLVER_LISTEN_ADDR` | Listen address | `0.0.0.0` |
| `SOLVER_CAPACITY` | Maximum capacity | `100` |
| `VALIDATOR_ADDRESS` | Validator address | `127.0.0.1` |
| `VALIDATOR_HTTP_PORT` | Validator HTTP port | `8080` |
| `SOLVER_KEY_FILE` | Key file path | None (optional) |
| `AUTO_REGISTER` | Auto-register | `true` |
| `RUST_LOG` | Log level | `info` |

## Key Management

### Key File Format

Key files are Base64-encoded JSON format:

```json
{
  "scheme": "Secp256k1",
  "private_key": "base64_encoded_private_key",
  "public_key": "base64_encoded_public_key",
  "address": "0x..."
}
```

### Recover Key from Mnemonic

```bash
./target/release/setu-cli keygen recover \
  --mnemonic "your twelve or twenty four word mnemonic phrase here" \
  --output /data/keys/recovered-key.json
```

### Export Private Key

```bash
# Export as JSON
./target/release/setu-cli keygen export \
  --key-file /data/keys/validator-key.json \
  --format json

# Export mnemonic
./target/release/setu-cli keygen export \
  --key-file /data/keys/validator-key.json \
  --format mnemonic

# Export private key (hex)
./target/release/setu-cli keygen export \
  --key-file /data/keys/validator-key.json \
  --format private-key
```

## Security Recommendations

### 1. Key File Permissions

```bash
# Set strict file permissions
chmod 600 /data/keys/*.json
chmod 700 /data/keys
```

### 2. Backup Mnemonic

⚠️ **Important**: Please backup your mnemonic phrase!

- Write the mnemonic on paper and store it in a safe place
- Do not store the mnemonic on internet-connected devices
- Do not transmit the mnemonic over the network
- Do not commit key files to Git repository

### 3. Production Environment Recommendations

- Use Hardware Security Module (HSM) to store keys
- Rotate keys regularly
- Use multi-signature schemes
- Monitor abnormal signing activities

## Directory Structure

```
/data/
├── keys/                    # Key files directory (permission 700)
│   ├── validator-key.json   # Validator key
│   └── solver-key.json      # Solver key
├── logs/                    # Log directory
│   ├── validator.log        # Validator log
│   └── solver.log           # Solver log
└── pids/                    # PID files directory
    ├── validator.pid        # Validator PID
    └── solver.pid           # Solver PID
```

## FAQ

### Q: What happens if no key file is provided?

A: The node can still start but will use placeholder values for registration. It's recommended to always use key files in production.

### Q: Can I use the same key on multiple nodes?

A: Not recommended. Each node should have its own independent keypair.

### Q: How to change keys?

A: 
1. Generate new key
2. Stop the node
3. Update `VALIDATOR_KEY_FILE` or `SOLVER_KEY_FILE` environment variable
4. Restart the node

### Q: What if I forget the mnemonic?

A: If you forget the mnemonic and lose the key file, it cannot be recovered. Please backup your mnemonic!

## Monitoring and Maintenance

### View Logs

```bash
# Real-time Validator log
tail -f /data/logs/validator.log

# Real-time Solver log
tail -f /data/logs/solver.log

# Search for errors
grep ERROR /data/logs/validator.log
```

### Check Node Status

```bash
# Check processes
ps aux | grep setu-validator
ps aux | grep setu-solver

# Health check
curl http://localhost:8080/api/v1/health

# View Solver list
curl http://localhost:8080/api/v1/solvers | jq .

# View Validator list
curl http://localhost:8080/api/v1/validators | jq .
```

## Upgrade Guide

### Upgrading from Old Version

If you previously deployed a version without the key system:

1. Generate key files
2. Update environment variables to add `*_KEY_FILE`
3. Restart nodes

Old registration data will be preserved, new registrations will use key signatures.

## Troubleshooting

### Node Won't Start

1. Check if key file exists and is readable
2. Check if port is already in use
3. View log files for detailed error messages

### Registration Failed

1. Confirm key file path is correct
2. Check if key file format is correct
3. Verify network connection to Validator

### Signature Verification Failed

1. Confirm using the correct key file
2. Check if key file is corrupted
3. Try recovering key from mnemonic

## Contact Support

If you have issues:
1. Check log files
2. Check GitHub Issues
3. Contact development team

---

**Last Updated**: 2025-01-23
