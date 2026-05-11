# Move Example Contracts

Example Move contracts for the Setu network. Each demonstrates different aspects of the setu-framework. Contracts marked as development coverage exercise testnet behavior and should not be treated as stable API examples.

## Contracts

| Contract | Demonstrates |
|----------|-------------|
| [counter](counter/) | Owned objects, mutation via entry functions, transfer |
| [custom_token](custom_token/) | Custom fungible token (Coin\<GOLD\>), TreasuryCap, mint/burn |
| [nft](nft/) | Unique NFT objects, freeze (immutable), burn (delete) |
| [lightning](lightning/) | Payment channel (Sui contract adaptation), Balance lifecycle, parallel-vector patterns |
| [pwoo_counter](pwoo_counter/) | Development example for Setu shared ownership/PWOO single-hotspot pressure; not PTB shared-object support |
| [df_registry](df_registry/) | Dynamic Field call-flow coverage (`add` / `remove` / `borrow` / `borrow_mut` / `exists_`); persistent in-place `borrow_mut` writeback remains outside the v1 stable contract |
| [dex_pool](dex_pool/) | Dogfood example for DF-based hotspot mitigation; beta/development unless promoted by the v1 boundary |
| [bad_df](bad_df/) | Negative case: `V: key` rejected at DF native entry |

## Prerequisites

These contracts depend on `setu-framework` (the Setu stdlib at address `0x1`).

## Building

Move contracts are compiled and published as bytecode via the `/api/v1/move/publish` endpoint.
Use the local Move compiler tooling under `tools/move-compile` to produce bytecode before publishing.

## Framework API Reference

The contracts above use these stdlib modules:

- `setu::object` — UID lifecycle: `new()`, `delete()`, `uid_to_address()`
- `setu::transfer` — Ownership: `transfer()`, `freeze_object()`, `share_object()` (Phase 5+)
- `setu::tx_context` — Context: `sender()`, `tx_hash()`, `epoch()`, `derive_id()`
- `setu::coin` — Fungible tokens: `mint()`, `burn()`, `split()`, `join()`, `transfer()`
- `setu::balance` — Balance arithmetic: `value()`, `split()`, `join()`, `zero()`
- `setu::setu` — Native SETU token type identifier
- `setu::dynamic_field` — Per-key child state: `add()`, `remove()`, `borrow()`, `borrow_mut()`, `exists_()` (Phase 8); use explicit update/remove+add patterns for persistent v1 writes

## Limitations

- **Package upgrade scope** — Simple examples usually treat published modules as fixed. Package upgrade flows are experimental unless explicitly documented in the public API surface.
- **No on-chain events** — Move `emit()` is a placeholder; surface signals via object fields.
- **DF value ability** — `dynamic_field::*<K, V>` rejects any `V` that has the `key` ability; object-typed DF values are not supported.
- **DF pre-declaration** — Every DF access must be listed in `MoveCallPayload.dynamic_field_accesses`; the Move VM aborts `E_DF_NOT_PRELOADED` otherwise.
