Storage
=======

-------
- Persistent DAG data: transfers, dependencies, node metadata.
- FoldGraph data: folds, fold-transfers mapping, parent/child edges.
- Postgres for relational queries and durability.
- RocksDB (or similar) for high-throughput key/value paths.
- Schema migrations tracked in repo.
- Favor append-only logs where possible; compaction for pruning.
# Setu Storage

This crate owns Setu's local storage boundary for DAG replay, committed state,
object indexes, and RocksDB-backed recovery. For testnet readiness, RocksDB is
the durability backend. In-memory backends are parity/test helpers only.

## Durable Backends

- `storage/src/rocks/` contains the production RocksDB stores used for durable
	local node state.
- `SetuDB` opens a fixed set of column families defined by `ColumnFamily::all()`.
- RocksDB data is validated by reopen probes in
	[STOR-T01](../docs/testnet-storage/runs/20260512-storage-rocksdb-probes.md),
	[STOR-T02](../docs/testnet-storage/runs/20260512-storage-dag-substrate.md),
	[STOR-T03](../docs/testnet-storage/runs/20260512-storage-object-indexes.md),
	[STOR-T04](../docs/testnet-storage/runs/20260512-storage-gsm-b4.md), and
	[STOR-T06](../docs/testnet-storage/runs/20260512-storage-fault-injection.md).

There is no Postgres storage backend in the current crate. Historical references
to Postgres or schema migrations should be treated as stale planning material,
not as implemented testnet behavior.

## Codec Contract

`SetuDB` uses two binary codecs:

- Typed keys are encoded with `bincode 2.0.0-rc.3`.
- Typed values are encoded with BCS through `bcs::to_bytes` and decoded through
	`bcs::from_bytes`.

Raw byte tools must use the producer/consumer codec for the key family they are
reading. BCS and bincode are not self-describing; STOR-T01 proved that direct
wrong-codec decoding can succeed for simple shapes. A successful manual decode
with the wrong codec is not proof that the bytes were produced by that codec.

## Memory Backend Boundary

`storage/src/memory/` backends are useful for trait parity, fast unit tests, and
in-process behavior checks. They are not restart durability evidence and must not
be used to support testnet replay or recovery claims.

Release or operator claims about restart behavior require RocksDB reopen evidence
from the run artifacts listed above. STOR-T02 explicitly records this as a
memory-backend boundary WARN, not a storage correctness bug.

## Prefix Scan Boundary

All raw RocksDB prefix iteration exposed by `SetuDB::prefix_iterator()` is
explicitly bounded with a `starts_with(prefix)` guard. Do not rely on RocksDB's
native `prefix_iterator_cf` to stop at the boundary unless the column family is
known to have a matching prefix extractor. STOR-T01 found and fixed a bug where
unbounded raw prefix iteration made MerkleMeta registry recovery over-claim an
anchor-only subnet.

## Replay And Recovery Boundary

Replay paths that follow durable indexes must decode primary records fail-loud.
Do not use `Option<T>` convenience lookup helpers when an index proves the body
should exist. STOR-T06 found and fixed a case where corrupt indexed event bytes
were hidden as an empty successful replay range.

Object index repair has two different contracts:

- `rebuild_coin_type_index()` adds index entries derivable from current primary
	`Coins` rows.
- `clear_and_rebuild_coin_type_index()` first deletes actual raw
	`CoinsByOwnerAndType` index keys, then rebuilds from current primary rows.

Operators recovering from suspected stale owner/type index corruption should use
the clear-and-rebuild path, not append-only rebuild. STOR-T03 found and fixed an
orphan-index case that append-only rebuild could not repair.

## State Boundary

`GlobalStateManager` persists dirty SMT leaves through the B4 delayed-persistence
path only at anchor commit. Before commit, dirty writes are in memory and are not
durable RocksDB evidence. `SharedStateManager::publish_snapshot()` publishes a
read snapshot; finalized reads must bypass speculative overlay state.

Committed state and provider/batch read behavior are covered by
[STOR-T04](../docs/testnet-storage/runs/20260512-storage-gsm-b4.md) and
[STOR-T05](../docs/testnet-storage/runs/20260512-storage-overlay-provider.md).

## Testnet Signoff Status

As of 2026-05-12, storage validation STOR-T00 through STOR-T10 has completed
with a CONDITIONAL GO for the selected testnet scope. See
[v1-storage-signoff](../docs/testnet-storage/v1-storage-signoff.md) and the
STOR-T09 dev-mult run artifact for the exact claim.

The durable claim covers RocksDB-backed event, index, Merkle state, subnet,
account, and balance evidence. It does not promote memory-only trackers such as
`transfer_status`, coin reservations, or live solver routing as restart-stable
storage guarantees, and it does not make `/api/v1/state/object/:key` a supported
raw object byte API.

Known limitation: `RocksDBEventStore::get_max_depth()` is not yet proven correct
for depths above `0xffffffff` because the current `depthidx:` key encoding uses
eight hex digits before expanding. This is outside the accepted current testnet
evidence range and is tracked in
`../docs/bugs/20260512-depthidx-max-depth-lex-order.md`.

