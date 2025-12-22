# Setu MVP Architecture Documentation

## Overview

Setu is a DAG-based distributed system that implements causality-driven event processing and periodic folding consensus.

## Network Topology

```
┌─────────────────────────────────────────────────────────────────┐
│                      MVP Network Topology                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  3 Validator Nodes (BFT Minimum Configuration)                  │
│  ════════════════════════════════════════════                   │
│                                                                 │
│      Validator_1 (Leader)                                       │
│           │                                                     │
│           ├──────────┬──────────┐                               │
│           │          │          │                               │
│      Validator_2  Validator_3                                   │
│           │          │                                          │
│           └──────────┴──────────┘                               │
│          (Full Mesh Connectivity)                               │
│                                                                 │
│  BFT Threshold: 2/3 + 1 = 3 nodes agree to reach consensus     │
│                                                                 │
│  ─────────────────────────────────────────────────────────────  │
│                                                                 │
│  10 Solver Nodes (Single Shard)                                 │
│  ═══════════════════════════════                                │
│                                                                 │
│      Solver_1  Solver_2  ...  Solver_10                         │
│          │        │              │                              │
│          └────────┴──────────────┘                              │
│                   │                                             │
│        Connect to all 3 Validators                              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Core Mechanisms

### Mechanism 1: Causality-Driven DAG Construction

```rust
// Event expresses dependencies through parent_ids
pub struct Event {
    pub id: EventId,
    pub parent_ids: Vec<EventId>,  // Causal dependencies
    pub vlc_snapshot: VLCSnapshot, // VLC clock snapshot
    // ...
}
```

- Events express dependency relationships through parent events
- VLC clock accumulates at each event creation
- DAG maintains topological ordering

### Mechanism 2: Optimistic Execution + Fast Confirmation

```
Solver Execution Flow:
1. Receive Transfer
2. Execute Script (if any)
3. State changes
4. Generate TEE Attestation (optional)
5. Broadcast execution result
6. Event enters Pending state

Validator Verification Flow:
1. Receive Event
2. Verify signature and format
3. Verify TEE Attestation
4. Add to DAG
5. Wait for consensus confirmation
```

### Mechanism 3: Periodic Folding Consensus

```
VLC Delta Trigger Condition:
- current_vlc.logical_time - last_fold_vlc >= threshold

Folding Process:
1. Leader detects VLC Delta reaching threshold
2. Create Anchor containing event_ids to be folded
3. Construct ConsensusFrame (CF)
4. Broadcast CF to all Validators
5. Validators vote
6. Confirm after reaching quorum (2/3 + 1)
7. Update state root
```

## Module Structure

```
setu/
├── types/           # Core type definitions
│   ├── event.rs     # Event, EventId, EventStatus
│   ├── transfer.rs  # Transfer, Bill, Script
│   ├── vlc.rs       # VLC hybrid clock
│   ├── dag.rs       # DAG data structure
│   ├── consensus.rs # Anchor, CF, Vote
│   └── node.rs      # NodeInfo, ValidatorInfo, SolverInfo
│
├── consensus/       # Consensus engine
│   ├── engine.rs    # ConsensusEngine
│   ├── folder.rs    # DagFolder, ConsensusManager
│   └── validator_set.rs # ValidatorSet
│
├── netcore/         # Network communication
│   ├── message.rs   # NetworkMessage definitions
│   ├── peer.rs      # PeerManager
│   ├── transport.rs # Transport abstraction
│   └── network.rs   # NetworkService
│
├── storage/         # Storage layer
│   ├── state.rs     # StateStore (account balances)
│   ├── event_store.rs # EventStore
│   └── anchor_store.rs # AnchorStore, CFStore
│
├── setu-solver/     # Solver node
│   ├── solver.rs    # Solver main logic
│   ├── executor.rs  # TransferExecutor
│   └── tee.rs       # TEE simulation
│
└── setu-validator/  # Validator node
    ├── validator.rs # Validator main logic
    └── verifier.rs  # EventVerifier, CFVerifier
```

## Running

### Start Validator

```bash
# Validator 1 (Leader)
VALIDATOR_ID=validator_1 VALIDATOR_PORT=8001 IS_LEADER=true cargo run -p setu-validator

# Validator 2
VALIDATOR_ID=validator_2 VALIDATOR_PORT=8002 cargo run -p setu-validator

# Validator 3
VALIDATOR_ID=validator_3 VALIDATOR_PORT=8003 cargo run -p setu-validator
```

### Start Solver

```bash
SOLVER_ID=solver_1 SOLVER_PORT=9001 SOLVER_STAKE=1000 cargo run -p setu-solver
```

## Testing

```bash
cargo test
```

## Configuration Parameters

### ConsensusConfig

| Parameter | Default Value | Description |
|------|--------|------|
| vlc_delta_threshold | 10 | VLC delta threshold to trigger folding |
| min_events_per_cf | 1 | Minimum number of events per CF |
| max_events_per_cf | 1000 | Maximum number of events per CF |
| cf_timeout_ms | 5000 | CF timeout duration |
| validator_count | 3 | Number of validators |

### SolverConfig

| Parameter | Default Value | Description |
|------|--------|------|
| processing_capacity | 50 | Processing capacity (TPS) |
| tee_enabled | false | Whether TEE is enabled |
| batch_size | 100 | Batch processing size |
| execution_interval_ms | 100 | Execution interval |
