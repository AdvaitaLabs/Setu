# Setu Explorer

Independent blockchain explorer service providing read-only query APIs for the Setu network.

## Features

- 🔍 **Network Statistics**: Total anchors, events, TPS, etc.
- 📦 **Anchor Queries**: Similar to "blocks" in traditional blockchains
- 📝 **Event Queries**: Similar to "transactions" in traditional blockchains
- 🌐 **DAG Visualization**: Real-time causal graph data
- 🔎 **Search**: Search by ID/address
- 🚀 **Independent Deployment**: Does not affect validator core nodes

## Architecture

```
┌─────────────────────────────────────────┐
│      Browser Frontend (React)           │
└─────────────────────────────────────────┘
                  ↓ HTTP
┌─────────────────────────────────────────┐
│      setu-explorer (This Service)       │
│      - Read-only API                    │
│      - Independent deployment           │
└─────────────────────────────────────────┘
                  ↓ Direct read
┌─────────────────────────────────────────┐
│      setu-validator                     │
│      - RocksDB (read-only mode)         │
└─────────────────────────────────────────┘
```

## Deployment

### Method 1: Co-located Deployment (Recommended)

Explorer and Validator on the same machine, directly reading RocksDB.

```bash
# 1. Start Validator
cd /path/to/setu
./target/release/setu-validator

# 2. Start Explorer (directly reading Validator's database)
export EXPLORER_DB_PATH=./data/validator
export EXPLORER_LISTEN_ADDR=0.0.0.0:8081
./target/release/setu-explorer
```

### Method 2: Remote Deployment (Future Support)

Explorer connects to remote Validator via RPC.

```bash
export EXPLORER_STORAGE_MODE=rpc
export EXPLORER_VALIDATOR_RPC_URL=http://validator:8080
export EXPLORER_LISTEN_ADDR=0.0.0.0:8081
./target/release/setu-explorer
```

## Configuration

Configure via environment variables:

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `EXPLORER_LISTEN_ADDR` | `0.0.0.0:8081` | HTTP listen address |
| `EXPLORER_STORAGE_MODE` | `direct` | Storage mode: `direct` or `rpc` |
| `EXPLORER_DB_PATH` | - | RocksDB path (required for direct mode) |
| `EXPLORER_VALIDATOR_RPC_URL` | - | Validator RPC URL (required for rpc mode) |
| `EXPLORER_ENABLE_CORS` | `true` | Enable CORS |
| `RUST_LOG` | `info` | Log level |

## API Endpoints

### Statistics

- `GET /api/v1/explorer/stats` - Network statistics

### Anchors (Blocks)

- `GET /api/v1/explorer/anchors?page=1&limit=20` - List anchors
- `GET /api/v1/explorer/anchor/:id` - Anchor details

### Events (Transactions)

- `GET /api/v1/explorer/events?page=1&limit=50&type=Transfer&status=Finalized` - List events
- `GET /api/v1/explorer/event/:id` - Event details

### DAG Visualization

- `GET /api/v1/explorer/dag/live?anchor_id=xxx&limit=100` - Live causal graph data
- `GET /api/v1/explorer/dag/path/:event_id` - Event causal path

### Search

- `GET /api/v1/explorer/search?q=xxx` - Search anchors/events/accounts

## Development

### Build

```bash
cargo build --release
```

### Run

```bash
# Development mode
EXPLORER_DB_PATH=./data/validator cargo run

# Production mode
./target/release/setu-explorer
```

### Test

```bash
cargo test
```

## Differences from Validator

| Feature | Validator | Explorer |
|---------|-----------|----------|
| Responsibility | Consensus & Verification | Read-only queries |
| API | Write + Read | Read-only |
| Users | Middleware/Wallets/DApps | Public/Browsers |
| Deployment | Required | Optional |
| Port | 8080 | 8081 |
| Authentication | Required | Not required |

## Performance Optimization

- ✅ Direct RocksDB read (read-only mode)
- ✅ Lock-free concurrent access
- ✅ Paginated queries
- 🔄 Cache layer (planned)
- 🔄 Index optimization (planned)

## Security

- ✅ Read-only mode, no data modification
- ✅ Independent process, fault isolation
- ✅ CORS support
- 🔄 Rate limiting (planned)
- 🔄 API keys (planned)

## License

Apache-2.0
