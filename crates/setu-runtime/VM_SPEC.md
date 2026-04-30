# Setu Runtime VM Specification

## 1. Scope

This document specifies the runtime execution model based on direct Sui subset execution.

Current executable components:

1. Runtime executor for transaction flows (`Transfer`, `Query`, `Program`)
2. Direct Sui disassembly subset VM (`sui_vm.rs`) for smart-contract entry execution
3. Published Sui disassembly storage for module-like contract execution

## 2. Runtime Transaction Model

`TransactionType` currently includes:

1. `Transfer(TransferTx)`
2. `Query(QueryTx)`
3. `Program(ProgramTx)`
4. `ContractPublish(ContractPublishTx)`

`ProgramTx` payload:

1. `disassembly` (module `.mvb` text)
2. `function_name` (entry to execute)
3. `args` (`Vec<SuiVmArg>`)
4. optional `gas_budget` (reserved)

`ContractPublishTx` payload:

1. `module_name` (module identifier for later calls)
2. `disassembly` (module `.mvb` instructions to store)

## 3. State Model

State backend uses `StateStore` over `Object<CoinData>`:

- key: `ObjectId`
- owner: `metadata.owner`
- value: `CoinData { coin_type, balance }`

Deterministic recipient placement:

- `deterministic_coin_id(address, coin_type)`

## 4. Executor Routing

## 4.1 Transfer

`RuntimeExecutor::execute_transaction` routes `Transfer` to transfer logic with:

1. Ownership verification
2. Full transfer or partial transfer
3. Recipient get-or-create by deterministic coin id
4. State-change emission (`Create`, `Update`, `Delete`)

## 4.2 Query

Supported query types:

1. `Balance`
2. `Object`
3. `OwnedObjects`

## 4.3 Program

`RuntimeExecutor` routes `Program` transactions to `sui_vm`:

1. Execute target entry via `execute_sui_entry_with_outcome(...)`
2. Convert VM write records into `StateChange` values
3. Classify writes as `Create` / `Update` / `Delete`

## 4.4 Contract Publish

`RuntimeExecutor` routes `ContractPublish` transactions to state storage:

1. Validate module name and disassembly instructions
2. Store `PublishedSuiContract` under deterministic `published_contract_id(module_name)`
3. Emit a `Create` or `Update` state change for the published instructions

## 5. Direct Sui Disassembly Subset VM (`sui_vm.rs`)

## 5.1 Purpose

Execute Sui Move entries directly from `.mvb` disassembly for a supported opcode/native subset.

Entry APIs:

1. `compile_package_to_disassembly(package_path, module_name)`
2. `execute_sui_entry_from_disassembly(state, sender, disassembly, function_name, args)`

## 5.2 Opcode Subset

Currently implemented opcodes:

1. `MoveLoc[idx]`
2. `CopyLoc[idx]`
3. `StLoc[idx]`
4. `LdU64(x)`
5. `LdU8(x)`
6. `LdTrue`
7. `LdFalse`
8. `BrFalse(target)`
9. `BrTrue(target)`
10. `Branch(target)`
11. `Call ...`
12. `Pop`
13. `Ret`

Parser handling notes:

- `FreezeRef` is accepted as no-op for current fixtures.
- Unknown opcodes fail execution.

## 5.3 Native Call Subset

Implemented call handlers:

1. `coin::mint<T>(...)`
- Produces a temporary coin value.

2. `transfer::public_transfer<Coin<T>>(coin, recipient)`
- Deposits/creates recipient deterministic coin.
- Consumes source coin object when persisted in state.

3. `coin::burn<T>(..., coin)`
- Deletes source coin object from state if present.

## 5.4 Argument Types (`SuiVmArg`)

Supported invocation arguments:

1. `U64`
2. `Bool`
3. `Address`
4. `ObjectId` (for `Coin<T>` object params)
5. `Opaque` (cap/context placeholders not modeled)

## 6. Example A: My Coin Conditional Transfer

Reference:

- [sui_contract_e2e.rs](/home/gyp/repo/Setu/crates/setu-runtime/examples/sui_contract_e2e.rs)

Flow:

1. Compile + disassemble `my_coin`
2. Execute `mint`
3. Execute `conditional_transfer(..., true, ...)`
4. Execute `conditional_transfer(..., false, ...)`
5. Execute `burn`

Expected behavior:

1. `mint` creates sender-side balance
2. true branch transfers minted amount
3. false branch is no-op
4. `burn` removes recipient coin object

Run:

```bash
cargo run -p setu-runtime --example sui_contract_e2e
```

## 7. Example B: Lightning Module under New Paradigm

Reference:

- [sui_lightning_contract_e2e.rs](/home/gyp/repo/Setu/crates/setu-runtime/examples/sui_lightning_contract_e2e.rs)

The full Lightning module is compiled/disassembled and verified for function presence.
Because full Lightning opcodes/natives are not yet all implemented, the module includes a subset-compatible entry:

- `vm_subset_branch_transfer`

This entry executes directly in `sui_vm` and validates branch-based transfer semantics in the Lightning module context.

Run:

```bash
cargo run -p setu-runtime --example sui_lightning_contract_e2e
```

## 8. Example C: Publish Then Execute

Reference:

- [sui_contract_publish_e2e.rs](/home/gyp/repo/Setu/crates/setu-runtime/examples/sui_contract_publish_e2e.rs)

Flow:

1. Compile + disassemble `published_coin`
2. Publish the disassembly instructions into runtime state
3. Resolve the published module by name
4. Execute mint and burn entries through the existing program execution utility

Run:

```bash
cargo run -p setu-runtime --example sui_contract_publish_e2e
```

## 9. Determinism and Safety

Determinism constraints:

1. No randomness in VM stepping
2. Explicit branch targets from disassembly indices
3. Deterministic coin-id derivation

Safety checks:

1. stack underflow checks
2. local index validation
3. argument/param coercion checks
4. unsupported opcode/call rejection

## 10. Current Limitations

Not implemented yet:

1. Full Sui opcode set
2. Full Sui native function set (`table`, `object`, `event`, `hash`, `ecdsa`, etc.)
3. Full capability/object-kind semantics
4. Bytecode verifier and gas metering
5. Full Sui on-chain package publish/upgrade semantics

## 11. Roadmap

Near-term priorities:

1. Expand opcode subset needed by target contracts
2. Expand native-call coverage for Lightning functions
3. Improve context/capability modeling beyond `Opaque`
4. Add conformance tests from real `.mvb` fixtures
