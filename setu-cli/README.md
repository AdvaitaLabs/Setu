# Setu CLI

Command-line interface for operator-oriented Setu node management.

The current CLI documentation is not a public wallet/client stable V1 contract.
Validator and solver registration, unregister, and status commands are
operator/internal surfaces unless a separate public admission contract promotes
them. Public scoped V1 clients should use the documented HTTP surfaces and treat
event finality or committed-state reads as durable receipts.

## Installation

```bash
cargo install --path setu-cli
```

Or build from source:

```bash
cargo build -p setu-cli --release
```

## Usage

```bash
# Show help
setu-cli --help

# Register a validator (operator/internal)
setu-cli validator register --id validator-1 --address 127.0.0.1:8001

# Register a solver (operator/internal)
setu-cli solver register --id solver-1 --address 127.0.0.1:9001 --capacity 100

# List registered validators
setu-cli validator list

# List registered solvers
setu-cli solver list
```

## Commands

### Validator

Manage validator nodes. These commands are operator/internal for the current V1
scope.

```bash
# Register a validator
setu-cli validator register \
  --id <validator_id> \
  --address <address> \
  [--stake <stake_amount>]

# List all validators
setu-cli validator list

# Unregister a validator
setu-cli validator unregister --id <validator_id>

# Check validator status
setu-cli validator status --id <validator_id>
```

### Solver

Manage solver nodes. These commands are operator/internal for the current V1
scope. Replayed solver entries are query/status information only until live
re-registration restores routability.

```bash
# Register a solver
setu-cli solver register \
  --id <solver_id> \
  --address <address> \
  --capacity <capacity> \
  [--shard <shard_id>] \
  [--resources <resource1,resource2,...>]

# List all solvers
setu-cli solver list

# Unregister a solver
setu-cli solver unregister --id <solver_id>

# Check solver status
setu-cli solver status --id <solver_id>

# Update solver capacity
setu-cli solver update --id <solver_id> --capacity <new_capacity>
```

### Status

Check operator-facing system status. Status output is not a durable receipt for
public writes; use event finality or committed-state reads for that.

```bash
# Check overall system status
setu-cli status

# Check specific node
setu-cli status --node <node_id>
```

## Configuration

The CLI can be configured via:

1. Command-line arguments
2. Environment variables
3. Config file (`~/.setu/config.toml`)

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| SETU_RPC_URL | RPC endpoint URL | http://localhost:8000 |
| SETU_TIMEOUT | Request timeout (seconds) | 30 |

### Config File

```toml
# ~/.setu/config.toml
rpc_url = "http://localhost:8000"
timeout = 30
```

## Examples

### Register Validators (operator/internal, 3 nodes for BFT)

```bash
setu-cli validator register --id validator-1 --address 127.0.0.1:8001 --stake 1000
setu-cli validator register --id validator-2 --address 127.0.0.1:8002 --stake 1000
setu-cli validator register --id validator-3 --address 127.0.0.1:8003 --stake 1000
```

### Register Solvers (operator/internal, 6 nodes)

```bash
setu-cli solver register --id solver-1 --address 127.0.0.1:9001 --capacity 100
setu-cli solver register --id solver-2 --address 127.0.0.1:9002 --capacity 100
setu-cli solver register --id solver-3 --address 127.0.0.1:9003 --capacity 100
setu-cli solver register --id solver-4 --address 127.0.0.1:9004 --capacity 100
setu-cli solver register --id solver-5 --address 127.0.0.1:9005 --capacity 100
setu-cli solver register --id solver-6 --address 127.0.0.1:9006 --capacity 100
```

### Check System Status

```bash
# List all nodes
setu-cli validator list
setu-cli solver list

# Check overall status
setu-cli status
```
