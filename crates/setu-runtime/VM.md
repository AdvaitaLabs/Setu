# Setu Runtime VM (Short Spec)

This VM is a small deterministic instruction executor used by `ProgramTx` in `setu-runtime`.

## Goals

- Add programmable execution without full Move VM integration.
- Keep behavior deterministic and easy to audit.
- Support basic arithmetic plus control flow (branch/jump).

## Transaction Entry

`ProgramTx` contains:

- `instructions: Vec<Instruction>`
- `inputs: BTreeMap<String, ProgramValue>`
- `max_steps: Option<u64>`

Execution is triggered via `TransactionType::Program`.

## Value Model

`ProgramValue`:

- `U64(u64)`
- `Bool(bool)`

Registers are fixed-size (`16`) and initialized to `U64(0)`.

## Instruction Set (10 opcodes)

1. `Nop`
2. `Const { dst, value }`
3. `Mov { dst, src }`
4. `BinOp { op, dst, lhs, rhs }`
5. `Cmp { op, dst, lhs, rhs }`
6. `LoadInput { dst, key }`
7. `StoreOutput { key, src }`
8. `Jump { pc }`
9. `JumpIf { cond, pc }`
10. `Halt { success, message }`

`BinOp` supports checked arithmetic/bitwise ops:
`Add`, `Sub`, `Mul`, `Div`, `Mod`, `BitAnd`, `BitOr`, `BitXor`.

`Cmp` supports:
`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`.

## Determinism and Safety

Before execution:

- Validate instruction count (`<= 4096`).
- Validate jump targets are in range.
- Validate register indices are valid.
- Validate `max_steps` (`1..=100000`, default `10000`).

During execution:

- Enforce step limit.
- Fail on overflow/div-by-zero/mod-by-zero.
- Fail on missing input key.
- Require explicit `Halt`; falling off program is an error.

## Output

Current VM phase returns:

- `ExecutionOutput.success` from `Halt.success`
- `ExecutionOutput.message` from `Halt.message` (or default message)
- `ExecutionOutput.query_result` containing stored output key-values
- No state writes yet (`state_changes` is empty in this phase)

## Minimal Example

```rust
use std::collections::BTreeMap;
use setu_runtime::{Instruction, ProgramValue, BinaryOp, CompareOp, Transaction};
use setu_types::Address;

let tx = Transaction::new_program(
    Address::from("alice"),
    vec![
        Instruction::Const { dst: 0, value: ProgramValue::U64(2) },
        Instruction::Const { dst: 1, value: ProgramValue::U64(3) },
        Instruction::BinOp { op: BinaryOp::Add, dst: 2, lhs: 0, rhs: 1 }, // r2 = 5
        Instruction::Const { dst: 3, value: ProgramValue::U64(4) },
        Instruction::Cmp { op: CompareOp::Gt, dst: 4, lhs: 2, rhs: 3 },    // r4 = true
        Instruction::JumpIf { cond: 4, pc: 8 },
        Instruction::Const { dst: 5, value: ProgramValue::U64(0) },
        Instruction::Jump { pc: 9 },
        Instruction::Const { dst: 5, value: ProgramValue::U64(1) },
        Instruction::StoreOutput { key: "ok".to_string(), src: 5 },
        Instruction::Halt { success: true, message: Some("done".to_string()) },
    ],
    BTreeMap::new(),
    None,
);
```

