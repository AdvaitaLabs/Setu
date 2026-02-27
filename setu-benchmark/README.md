# Setu Benchmark Tool

A high-performance benchmarking tool for testing Setu validator throughput and latency.

## Features

- **Multiple benchmark modes**: Burst, Sustained, and Ramp
- **Batch API support**: Test the new batch transfer endpoint for higher throughput
- **Dynamic account initialization**: Create test accounts on-the-fly via transfers
- **Configurable concurrency**: Control parallel request count
- **Detailed metrics**: Latency percentiles, success/failure rates, TPS

## Usage

### Basic Usage

```bash
# Burst mode - send all requests as fast as possible
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --total 10000 \
  --concurrency 100

# Sustained mode - maintain target TPS for duration
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --mode sustained \
  --target-tps 500 \
  --duration 60

# Ramp mode - gradually increase load
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --mode ramp \
  --ramp-start 100 \
  --ramp-step 100 \
  --ramp-step-duration 30 \
  --duration 300
```

### High-Concurrency Testing with Account Initialization

For high-concurrency testing, you need more accounts to avoid coin reservation conflicts.
Use `--init-accounts` to create test accounts from seed accounts (alice, bob, charlie):

```bash
# Initialize 100 test accounts and run benchmark
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --init-accounts 100 \
  --init-account-balance 100000 \
  --use-test-accounts \
  --total 10000 \
  --concurrency 100

# For maximum concurrency, match account count to concurrency
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --init-accounts 200 \
  --use-test-accounts \
  --total 50000 \
  --concurrency 200
```

**Why account initialization matters:**
- Setu uses 1:1 binding between (owner, subnet) and coin
- Each account can only process one transfer at a time
- Success rate ≈ min(num_accounts / concurrency, 1) * 100%

### Batch Mode (New)

Use batch mode to test the batch transfer API endpoint (`/api/v1/transfers/batch`):

```bash
# Burst mode with batching
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --total 10000 \
  --concurrency 50 \
  --use-batch \
  --batch-size 100

# Combined: account init + batch mode
cargo run -p setu-benchmark -- \
  --url http://localhost:8080 \
  --init-accounts 100 \
  --use-test-accounts \
  --use-batch \
  --batch-size 50 \
  --total 10000 \
  --concurrency 50
```

## Command Line Options

| Option | Description | Default |
|--------|-------------|---------|
| `--url` | Validator URL | `http://localhost:8080` |
| `--mode` | Benchmark mode: burst, sustained, ramp | `burst` |
| `--total` | Total number of requests (burst mode) | `1000` |
| `--concurrency` | Number of concurrent requests | `10` |
| `--timeout` | Request timeout in milliseconds | `30000` |
| `--warmup` | Number of warmup requests | `100` |
| `--init-accounts` | Number of test accounts to initialize (0=skip) | `0` |
| `--init-account-balance` | Balance for each initialized account | `100000` |
| `--use-test-accounts` | Use test accounts instead of random addresses | `false` |
| `--use-batch` | Use batch API endpoint | `false` |
| `--batch-size` | Number of transfers per batch | `50` |
| `--target-tps` | Target TPS (sustained mode) | `100` |
| `--duration` | Duration in seconds (sustained/ramp) | `60` |
| `--ramp-start` | Starting TPS (ramp mode) | `100` |
| `--ramp-step` | TPS increment per step (ramp mode) | `100` |
| `--ramp-step-duration` | Duration per step in seconds | `30` |
| `--amount` | Transfer amount | `100` |

## Account Architecture

### Seed Accounts (Validator-side)
The validator initializes 3 seed accounts with high balance (1B tokens each):
- `alice`
- `bob`  
- `charlie`

### Test Accounts (Benchmark-side)
When `--init-accounts N` is specified:
1. Benchmark creates `user_001` to `user_N` by transferring from seed accounts
2. Each test account receives `--init-account-balance` tokens
3. Transfers are distributed across seed accounts using round-robin

This design decouples the Validator from benchmark-specific requirements.

## Batch API Benefits

The batch API (`/api/v1/transfers/batch`) provides several advantages over single-request mode:

1. **Reduced network overhead**: Fewer HTTP round-trips
2. **Better throughput**: Single reservation per batch instead of per-transfer
3. **Atomic batch processing**: All transfers in a batch share the same state snapshot
4. **Lower latency variance**: Amortized per-transfer latency

## Output

The benchmark outputs:
- Real-time progress updates
- Final summary with:
  - Total requests, success/failure counts
  - TPS (transactions per second)
  - Latency percentiles (p50, p95, p99)
  - Min/Max/Average latency

## Example Output

```
════════════════════════════════════════════════════════════
Benchmark Results
════════════════════════════════════════════════════════════
Total Requests:    10000
Successful:        9987
Failed:            13
Timeouts:          0
Success Rate:      99.87%
────────────────────────────────────────────────────────────
Duration:          12.34s
TPS:               809.56
────────────────────────────────────────────────────────────
Latency (ms):
  Min:             2.1
  Max:             156.3
  Avg:             12.4
  P50:             10.2
  P95:             28.7
  P99:             45.1
════════════════════════════════════════════════════════════
```
