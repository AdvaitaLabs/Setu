# Setu

> **Hetu Project** | A high-performance DAG-based distributed consensus network

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org/)

---

## Overview

Setu is a next-generation distributed consensus network designed for high throughput and low latency. It combines **DAG-based consensus**, **Vector Logical Clocks (VLC)**, and **TEE-based execution** to achieve secure and efficient transaction processing.

### Key Features

- **DAG-Based Consensus**: DAG-BFT consensus with leader rotation
- **VLC Hybrid Clock**: Vector Logical Clock for causal ordering in distributed events
- **TEE Execution**: Trusted Execution Environment (AWS Nitro) for secure computation
- **Subnet Architecture**: Multi-subnet support for horizontal scalability *(in development)*
- **Merkle State Commitment**: Binary + Sparse Merkle Trees for verifiable state

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Setu Network                                   │
├─────────────────────────────────┬────────────────────────────────────────── ┤ 
│         Validator Nodes         │           Solver Nodes                    │
│                                 │                                           │
│  ┌──────────────────────────┐   │   ┌──────────────────────────┐            │
│  │    ConsensusEngine       │   │   │      TeeExecutor         │            │
│  │  ┌────────┐ ┌─────────┐  │   │   │                          │            │
│  │  │  DAG   │ │   VLC   │  │   │   │  ┌────────────────────┐  │            │
│  │  └────────┘ └─────────┘  │   │   │  │   EnclaveRuntime   │  │            │
│  │  ┌────────────────────┐  │   │   │  │  (Mock / Nitro)    │  │            │
│  │  │  ValidatorSet      │  │   │   │  └────────────────────┘  │            │
│  │  │  (Leader Election) │  │   │   │                          │            │
│  │  └────────────────────┘  │   │   └──────────────────────────┘            │
│  │  ┌────────────────────┐  │   │                                           │
│  │  │   AnchorBuilder    │  │   │   ┌──────────────────────────┐            │
│  │  │  (Merkle Roots)    │  │   │   │   SolverNetworkClient    │            │
│  │  └────────────────────┘  │   │   └──────────────────────────┘            │
│  └──────────────────────────┘   │                                           │
│                                 │                                           │
│  ┌──────────────────────────┐   │                                           │
│  │  GlobalStateManager      │   │                                           │
│  │  (Sparse Merkle Trees)   │   │                                           │
│  └──────────────────────────┘   │                                           │
├─────────────────────────────────┴────────────────────────────────────────── ┤
│                           P2P Network (Anemo/QUIC)                          │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Description |
|-----------|-------------|
| **Validator** | Verification and coordination node for consensus |
| **Solver** | TEE-based execution node (pass-through to EnclaveRuntime) |
| **ConsensusEngine** | DAG-based consensus with VLC timing |
| **AnchorBuilder** | Creates Anchors with Merkle root computation |
| **GlobalStateManager** | Manages state across all subnets via Sparse Merkle Trees |

---

## Consensus Flow

Setu implements a DAG-BFT consensus protocol:

```
1. Event Submission
   Client → Validator → TaskPreparer → SolverTask

2. TEE Execution
   SolverTask → Solver → TEE (EnclaveRuntime) → TeeExecutionResult

3. Event Verification
   TeeExecutionResult → Validator → TeeVerifier → Event added to DAG

4. DAG Folding (Anchor Creation)
   VLC delta threshold reached → AnchorBuilder → Anchor with Merkle roots

5. Consensus Finalization
   ConsensusFrame proposal → Explicit voting (quorum 2/3+1) → CF finalized → State committed
```

### Key Concepts

- **Event**: Atomic unit of state change with TEE attestation
- **Anchor**: Checkpoint containing events and Merkle roots
- **ConsensusFrame (CF)**: Voting unit for consensus finalization
- **VLC**: Vector Logical Clock for causal ordering

---

## Project Structure

```
Setu/
├── consensus/              # DAG-based consensus implementation
│   ├── dag.rs             # DAG data structure
│   ├── engine.rs          # Main consensus engine
│   ├── anchor_builder.rs  # Anchor creation with Merkle trees
│   ├── folder.rs          # ConsensusManager for CF management
│   └── vlc.rs             # VLC integration
│
├── types/                  # Core type definitions
│   ├── event.rs           # Event, EventId, EventStatus
│   ├── consensus.rs       # Anchor, ConsensusFrame, Vote
│   ├── object.rs          # Object model (Coin, Profile, etc.)
│   └── merkle.rs          # Merkle tree types
│
├── storage/                # Storage layer
│   ├── memory/            # In-memory implementations (DashMap)
│   ├── rocks/             # RocksDB persistent storage
│   └── state/             # GlobalStateManager, StateProvider
│
├── crates/
│   ├── setu-vlc/          # VLC Hybrid Logical Clock library
│   ├── setu-merkle/       # Merkle trees (Binary + Sparse)
│   ├── setu-keys/         # Cryptographic key management
│   ├── setu-enclave/      # TEE abstraction (Mock + Nitro)
│   ├── setu-network-anemo/# Anemo-based P2P network
│   ├── setu-transport/    # HTTP/WS/gRPC transport layer
│   ├── setu-protocol/     # Protocol message definitions
│   └── setu-core/         # Shared core utilities
│
├── setu-validator/         # Validator node binary
├── setu-solver/            # Solver node binary
├── setu-cli/               # CLI tool
├── setu-rpc/               # RPC layer
├── setu-benchmark/         # TPS benchmark tool
│
├── api/                    # HTTP API layer
├── docker/                 # Docker deployment configs
├── scripts/                # Deployment & test scripts
└── docs/                   # Design documents
```

---

## Getting Started

### Prerequisites

- **Rust**: 1.75+ (2021 edition)
- **RocksDB**: For persistent storage
- **Docker**: For containerized deployment (optional)

### Build

```bash
# Clone the repository
git clone https://github.com/advaitaLabs/Setu.git
cd Setu

# Build all components (release mode)
cargo build --release

# Run tests
cargo test --all
```

### Run Locally

#### 1. Start Validator

```bash
# Set environment variables
export VALIDATOR_ID=validator-1
export VALIDATOR_HTTP_PORT=8080
export VALIDATOR_P2P_PORT=9000
export VALIDATOR_DB_PATH=/tmp/setu/validator

# Start validator
./target/release/setu-validator
```

#### 2. Start Solver

```bash
# Set environment variables
export SOLVER_ID=solver-1
export SOLVER_PORT=9001
export VALIDATOR_ADDRESS=127.0.0.1
export VALIDATOR_HTTP_PORT=8080

# Start solver
./target/release/setu-solver
```

#### 3. Submit Transactions (CLI)

```bash
# Check balance
./target/release/setu balance --address <ADDRESS>

# Transfer
./target/release/setu transfer --from <FROM> --to <TO> --amount 100
```

### Docker Deployment

```bash
cd docker

# Build images
./scripts/build.sh

# Start multi-validator setup
docker-compose -f docker-compose.multi-validator.yml up -d

# View logs
docker-compose logs -f
```

---

## Configuration

### Validator Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `VALIDATOR_ID` | `validator-1` | Unique validator identifier |
| `VALIDATOR_HTTP_PORT` | `8080` | HTTP API port |
| `VALIDATOR_P2P_PORT` | `9000` | P2P network port |
| `VALIDATOR_DB_PATH` | (memory) | RocksDB path for persistence |
| `VALIDATOR_KEY_FILE` | - | Path to keypair file |

### Solver Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SOLVER_ID` | `solver-{uuid}` | Unique solver identifier |
| `SOLVER_PORT` | `9001` | Solver listen port |
| `SOLVER_CAPACITY` | `100` | Max concurrent tasks |
| `VALIDATOR_ADDRESS` | `127.0.0.1` | Validator address |
| `AUTO_REGISTER` | `true` | Auto-register on startup |

### Consensus Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `vlc_delta_threshold` | `10` | VLC delta to trigger folding |
| `min_events_per_cf` | `1` | Minimum events per ConsensusFrame |
| `max_events_per_cf` | `1000` | Maximum events per ConsensusFrame |
| `vote_timeout_ms` | `5000` | Voting timeout in milliseconds |

---

## API Reference

### HTTP Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/api/v1/transfer` | POST | Submit transfer |
| `/api/v1/balance/{address}` | GET | Query balance |
| `/api/v1/object/{id}` | GET | Query object |
| `/api/v1/events` | GET | List events |
| `/api/v1/register/solver` | POST | Register solver |

### RPC Services

- **ConsensusService**: Event submission, CF proposal, voting
- **SyncService**: Event/CF synchronization between validators
- **DiscoveryService**: Peer discovery and management

---

## Benchmarks

Run TPS benchmark:

```bash
# Simple TPS test
python scripts/tps_test_simple.py

# Full benchmark
cargo bench --package setu-benchmark
```

### Expected Performance (MVP)

| Metric | Target | Notes |
|--------|--------|-------|
| TPS | 200,000-300,000 | dag-bft consensus |
| Latency | 50-100ms | End-to-end confirmation |
| Validators | 7 | BFT consensus quorum |
| Solvers | 21 | Horizontal scaling |

---

## Contributing

We welcome contributions! Please see our [Contributing Guidelines](CONTRIBUTING.md) for details.

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests and linting
5. Submit a pull request

---

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

---

## Contact

- **Project**: Hetu Project
- **GitHub**: [advaitaLabs/Setu](https://github.com/advaitaLabs/Setu)

---

<p align="center">
  Built with ❤️ by <a href="https://github.com/advaitaLabs">Advaita Labs</a>
</p>
