# Setu Merkle

High-performance Merkle tree implementations for Setu.

## Features

- **Sparse Merkle Tree (SMT)**: 256-bit key space for object state storage
- **Incremental SMT**: O(log N) updates (vs O(N) for basic SMT)
- **Binary Merkle Tree**: For event list commitments
- **Subnet Aggregation Tree**: Aggregates all subnet state roots

## Performance: BLAKE3 Hashing

This module uses **BLAKE3** instead of SHA256 for all hash operations:

| Metric | SHA256 | BLAKE3 | Improvement |
|--------|--------|--------|-------------|
| Small data (< 1KB) | ~400 MB/s | ~1.2 GB/s | **3x faster** |
| Large data (SIMD) | ~400 MB/s | ~8+ GB/s | **20x faster** |
| Security | 128-bit | 128-bit | Equal |

### Benchmark Results

With BLAKE3 optimization (single Validator + single Solver):

- **TPS**: 14,634 (was 10,714 with SHA256) → **+36.6%**
- **P99 Latency**: 16.56ms (was 23.70ms) → **-30%**

## Usage

```rust
use setu_merkle::{SparseMerkleTree, IncrementalSparseMerkleTree, HashValue};

// Create an incremental SMT (recommended for high-frequency updates)
let mut tree = IncrementalSparseMerkleTree::new();

// Insert key-value pairs
let key = HashValue::from_slice(&[1u8; 32]).unwrap();
tree.insert(key, b"value".to_vec());

// Get the state root
let root = tree.root();

// Generate inclusion proof
let proof = tree.get_proof(&key).unwrap();
```

## Hash Functions

All hash functions use BLAKE3 with domain separation:

```rust
use setu_merkle::hash::{hash_leaf, hash_internal, blake3_hash};

// Direct hash
let h = blake3_hash(b"data");

// Leaf hash (with 0x00 prefix)
let leaf = hash_leaf(b"leaf data");

// Internal node hash (with 0x01 prefix)
let internal = hash_internal(&left, &right);
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Setu Merkle Architecture                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  hash.rs          - BLAKE3 hash primitives with domain separation           │
│       │                                                                      │
│       ├── sparse.rs         - Sparse Merkle Tree (256-bit keys)             │
│       │       ├── SparseMerkleTree       - Basic SMT (O(N) updates)         │
│       │       └── IncrementalSparseMerkleTree - Optimized (O(log N))        │
│       │                                                                      │
│       ├── binary.rs         - Binary Merkle Tree for event lists            │
│       │                                                                      │
│       └── aggregation.rs    - Subnet state root aggregation                 │
│                                                                              │
│  storage.rs       - Merkle node persistence layer                           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## License

Apache-2.0
