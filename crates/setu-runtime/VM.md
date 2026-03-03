# Setu Runtime Move-Style VM

This document specifies the current programmable VM in `setu-runtime`.

## Overview

Setu VM is a **Move-style typed stack VM with local slots**.

- Stack-based evaluation (`LdU64`, `Add`, `BrTrue`, `Ret`, etc.)
- Indexed locals (`CopyLoc`, `MoveLoc`, `StLoc`)
- Pre-execution verifier (control-flow + type checks)
- Deterministic interpreter with gas metering

This VM powers `TransactionType::MoveScript`.

## Transaction Format

Programmable execution uses `MoveScriptTx`:

- `code: Vec<Bytecode>`
- `locals_sig: Vec<SignatureToken>`
- `params_sig: Vec<SignatureToken>`
- `return_sig: Vec<SignatureToken>`
- `args: Vec<MoveValue>`
- `type_args: Vec<TypeTag>` (reserved for generic extensions)
- `max_gas: u64`
- `input_objects: Vec<ObjectId>`

`Transaction::new_move_script(...)` copies `input_objects` into transaction-level dependencies.

## Runtime Types

`MoveValue`:

- `U64(u64)`
- `Bool(bool)`
- `Address(Address)`
- `Vector(Vec<MoveValue>)`

`SignatureToken`:

- `U64`
- `Bool`
- `Address`
- `Vector(Box<SignatureToken>)`

`TypeTag` currently mirrors simple base/vector types and is reserved for future generic handling.

## Bytecode Set (Current)

Constants:

- `LdU64(u64)`, `LdTrue`, `LdFalse`

Locals/stack:

- `CopyLoc(u8)`, `MoveLoc(u8)`, `StLoc(u8)`, `Pop`

Arithmetic:

- `Add`, `Sub`, `Mul`, `Div`, `Mod`

Comparison:

- `Eq`, `Neq`, `Lt`, `Le`, `Gt`, `Ge`

Control flow:

- `BrTrue(u16)`, `BrFalse(u16)`, `Branch(u16)`

Termination:

- `Ret`
- `Abort { code: u64, message: Option<String> }`

## Verification Pipeline

Before execution, the VM verifies:

1. Structural envelope:
- non-empty code
- code length bound
- locals count bound
- `params_sig.len() <= locals_sig.len()`
- `args.len() == params_sig.len()`
- `max_gas > 0`

2. Argument type compatibility:
- each `arg` matches `params_sig`

3. Bytecode control-flow and type safety:
- valid branch targets
- stack underflow prevention via abstract interpretation
- opcode operand type checks
- join-point stack state consistency
- return stack matches `return_sig`
- at least one reachable terminal instruction (`Ret` or `Abort`)

If verification fails, execution is rejected with `RuntimeError::InvalidTransaction`.

## Execution Semantics

Interpreter model:

- `locals: Vec<Option<MoveValue>>`, initialized from `args`
- `stack: Vec<MoveValue>`
- `pc: usize`
- per-op gas charging

Current runtime bounds:

- `MAX_CODE_LENGTH = 4096`
- `MAX_LOCALS = 256`
- `MAX_STACK_DEPTH = 1024`
- `MAX_STEPS = 100000`

Arithmetic is checked:

- overflow/underflow -> error
- division/mod by zero -> error

Gas behavior:

- fixed gas cost table by opcode
- execution aborts with out-of-gas error when budget is insufficient

Termination behavior:

- `Ret`: success, stack must match `return_sig`, return values emitted in `query_result.returns`
- `Abort`: non-success `ExecutionOutput` with `abort_code` in `query_result`

## Output Contract

For `MoveScript` execution:

- `ExecutionOutput.success`: `true` on `Ret`, `false` on `Abort`
- `ExecutionOutput.message`: execution/abort message
- `ExecutionOutput.query_result`:
  - `returns`: return values (on `Ret`)
  - `gas_used`: consumed gas
  - `abort_code`: on `Abort`
- `state_changes`, `created_objects`, `deleted_objects`: empty in current phase

Transfer/query paths are unchanged.

## Example

High-level shape of a script:

1. load params from locals
2. compute and compare
3. branch
4. push return value
5. `Ret`

Reference tests:

- `test_move_script_branch_and_return`
- `test_move_script_out_of_gas`

in `crates/setu-runtime/src/executor.rs`.

## Current Scope and Next Steps

Implemented scope:

- typed stack VM core
- locals handling
- verifier
- gas metering

Not yet implemented:

- module/function call model (`Call`)
- storage/resource bytecodes (`Exists`, `MoveFrom`, `MoveTo`)
- struct/resource semantics (`Pack`, `Unpack`, abilities)
