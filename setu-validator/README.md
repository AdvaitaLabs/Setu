# Setu Validator

Validator node for the Setu network. Responsible for verifying events, maintaining the DAG, and participating in consensus.

## Responsibilities

- Receive events from Solvers
- Verify event validity (signature, format, TEE proof)
- Maintain the global DAG (Foldgraph)
- Participate in periodic folding consensus
- Route transfers to appropriate Solvers (using setu-router-core)

## Architecture

```
┌─────────────────────────────────────────┐
│              Validator                   │
├─────────────────────────────────────────┤
│  ┌─────────────────────────────────┐   │
│  │  Verifier                        │   │
│  │  - Quick check                   │   │
│  │  - VLC verification              │   │
│  │  - TEE proof verification        │   │
│  │  - Parent verification           │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │  DAG Manager                     │   │
│  │  - Add events to DAG             │   │
│  │  - Maintain topological order    │   │
│  │  - Track event dependencies      │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │  Sampling Verifier               │   │
│  │  - Probabilistic re-execution    │   │
│  │  - Fraud detection               │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │  Router (setu-router-core)       │   │
│  │  - Route transfers to Solvers    │   │
│  │  - Load balancing                │   │
│  └─────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

## Usage

### As a Binary

```bash
# Start validator with default config
cargo run -p setu-validator

# With an env file (auto-loads .env from working directory, or use --env-file)
cargo run -p setu-validator -- --env-file .env

# With inline environment variables
VALIDATOR_ID=validator_1 \
VALIDATOR_PORT=8001 \
IS_LEADER=true \
cargo run -p setu-validator
```

### As a Library

```rust
use setu_validator::Validator;
use setu_core::NodeConfig;
use tokio::sync::mpsc;

let config = NodeConfig {
    node_id: "validator-1".to_string(),
    // ...
};

let (event_tx, event_rx) = mpsc::unbounded_channel();
let validator = Validator::new(config, event_rx);

// Run the validator
validator.run().await;
```

## Configuration

| Parameter | Environment Variable | Default | Description |
|-----------|---------------------|---------|-------------|
| node_id | VALIDATOR_ID | auto-generated | Unique validator identifier |
| port | VALIDATOR_PORT | 8001 | Network port |
| is_leader | IS_LEADER | false | Whether this is the leader node |
| pocw_enabled | POCW_ENABLED | false | Enable PoCW economic layer |
| pocw_transfer_fee | POCW_TRANSFER_FEE | 21000 | Fee per transfer (burned) |

## Modules

- `verifier.rs` - Event verification logic
- `dag.rs` - DAG management
- `sampling.rs` - Sampling-based verification

## Dependencies

- `setu-core` - Core configuration
- `setu-types` - Type definitions
- `setu-vlc` - VLC clock
- `setu-router-core` - Routing logic
- `consensus` - Consensus engine

