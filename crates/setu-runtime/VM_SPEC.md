# Setu Runtime VM Specification (V1)

## 1. Scope

This document specifies the behavior of the currently implemented VM in `setu-runtime` (`program_vm.rs`).

It is a deterministic, stack-based interpreter used by `TransactionType::Program`.
It is **not** full MoveVM yet; it is a V1 programmable execution layer with Move-like instruction categories.

## 2. Entry Points

- Program transaction type:
  - `TransactionType::Program(ProgramTx)`
- Program structure:
  - `Program { locals_count, instructions }`
- Executor dispatch:
  - `RuntimeExecutor::execute_transaction` routes `Program` transactions to VM execution.

## 3. Execution Model

### 3.1 Core Runtime State

- Program counter (`pc`), starts at `0`
- Operand stack (`Vec<RuntimeValue>`)
- Locals (`Vec<Option<RuntimeValue>>`) of fixed length `locals_count`
- Overlay storage map (`ObjectId -> Option<CoinObject>`) for transactional writes
- Original-value snapshot map for touched object IDs
- Write order list (first-touch order)

### 3.2 Termination

- `Ret`: success path; commits overlay writes.
- `Abort(msg)`: immediate failure with `RuntimeError::ProgramAbort(msg)`.
- Falling off end without `Ret`: `RuntimeError::ProgramExecution("Program terminated without Ret")`.
- Step limit: max `100_000` instructions; exceeding it aborts with `ProgramExecution`.

### 3.3 Determinism Rules

- No wall-clock reads inside VM execution.
- No randomness.
- Storage reads/writes are deterministic given input state and instruction stream.
- Branch targets are absolute instruction indices.

## 4. Value Types

### 4.1 Constants (`VmConstant`)

- `U64(u64)`
- `Bool(bool)`
- `Address(Address)`
- `ObjectId(ObjectId)`
- `String(String)`

### 4.2 Runtime Values (`RuntimeValue`)

- `U64`, `Bool`, `Address`, `ObjectId`, `String`
- `Coin(Object<CoinData>)`
- `Ref(usize)` (local reference handle)
- `Vector(Vec<RuntimeValue>)`

## 5. Storage Model

### 5.1 Read Path

`Exists` / `BorrowGlobal` / `MoveFrom` resolve objects as:

1. Overlay value if present.
2. Base `StateStore` value otherwise.

### 5.2 Write Path

- `MoveFrom(id)` marks object as touched and sets overlay to `None` (logical delete in overlay).
- `MoveTo(id, coin)` marks touched and sets overlay to `Some(coin)`.
- Only touched IDs are considered at commit.

### 5.3 Commit Semantics

For each touched ID (in first-touch order):

1. Compute `old_state` from original snapshot (`CoinState` bytes).
2. Compute `new_state` from final overlay value.
3. If bytes equal, skip write.
4. Else:
   - `new_state = Some` -> `set_object`
   - `new_state = None` -> `delete_object`

The VM returns write tuples (`object_id`, `old_state`, `new_state`), then executor maps them to:

- `Create`: `old=None`, `new=Some`
- `Update`: `old=Some`, `new=Some`
- `Delete`: `old=Some`, `new=None`

## 6. Instruction Set Specification

Notation:

- Stack shown as `[..., a, b]` where `b` is top-of-stack.
- `pop` order is top first.
- Errors below are `RuntimeError::ProgramExecution` unless otherwise noted.

### 6.1 Control Flow

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `Nop` | no change | No operation | none |
| `Branch(target)` | no change | Set `pc = target` | target out of range |
| `BrTrue(target)` | `[..., bool] -> [...]` | Jump if popped bool is true | type mismatch, target out of range |
| `BrFalse(target)` | `[..., bool] -> [...]` | Jump if popped bool is false | type mismatch, target out of range |
| `Abort(msg)` | no change | Fail immediately with `ProgramAbort(msg)` | none |
| `Ret` | no change | Commit overlay and return success | commit/write errors |

### 6.2 Constants / Locals / References

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `LoadConst(c)` | `[...] -> [..., c]` | Push constant | none |
| `CopyLoc(i)` | `[...] -> [..., v]` | Clone initialized local `i` | invalid index, uninitialized local |
| `MoveLoc(i)` | `[...] -> [..., v]` | Move local `i` and set it uninitialized | invalid index, uninitialized local |
| `StLoc(i)` | `[..., v] -> [...]` | Store popped value into local `i` | invalid index, stack underflow |
| `BorrowLoc(i)` | `[...] -> [..., ref(i)]` | Create local reference handle | invalid index, uninitialized local |
| `ReadRef` | `[..., ref(i)] -> [..., v]` | Read referenced local (clone) | type mismatch, invalid/uninitialized local |
| `WriteRef` | `[..., ref(i), v] -> [...]` | Write `v` into referenced local | type mismatch, invalid index |
| `FreezeRef` | `[..., ref(i)] -> [..., ref(i)]` | No-op in V1 | type mismatch |

### 6.3 Arithmetic / Comparison / Boolean

All arithmetic operations are `u64` only.

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `Add` | `[..., a, b] -> [..., a+b]` | Checked add | type mismatch, overflow |
| `Sub` | `[..., a, b] -> [..., a-b]` | Checked sub | type mismatch, underflow |
| `Mul` | `[..., a, b] -> [..., a*b]` | Checked mul | type mismatch, overflow |
| `Div` | `[..., a, b] -> [..., a/b]` | Integer division | type mismatch, divide by zero |
| `Mod` | `[..., a, b] -> [..., a%b]` | Integer modulo | type mismatch, modulo by zero |
| `Lt/Le/Gt/Ge` | `[..., a, b] -> [..., bool]` | `u64` comparison | type mismatch |
| `Eq` | `[..., a, b] -> [..., bool]` | Equality | unsupported type pair |
| `Neq` | `[..., a, b] -> [..., bool]` | Inequality | unsupported type pair |
| `Not` | `[..., bool] -> [..., !bool]` | Boolean not | type mismatch |
| `And` | `[..., a, b] -> [..., a&&b]` | Boolean and | type mismatch |
| `Or` | `[..., a, b] -> [..., a\|\|b]` | Boolean or | type mismatch |

`Eq/Neq` currently support:

- `u64`, `bool`, `address`, `object_id`, `string`

### 6.4 Global Storage Instructions

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `Exists` | `[..., object_id] -> [..., bool]` | True if object exists in overlay/base state | type mismatch |
| `BorrowGlobal` | `[..., object_id] -> [..., coin]` | Clone current coin object | type mismatch, object not found |
| `MoveFrom` | `[..., object_id] -> [..., coin]` | Read and remove object in overlay (touch + `None`) | type mismatch, object not found |
| `MoveTo` | `[..., object_id, coin] -> [...]` | Write coin to overlay (touch + `Some`) | type mismatch |

### 6.5 Resource-Like Instructions (Coin)

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `PackCoin` | `[..., object_id, owner, amount, coin_type] -> [..., coin]` | Build owned coin object | type mismatch |
| `UnpackCoin` | `[..., coin] -> [..., object_id, owner, amount, coin_type]` | Decompose coin fields | type mismatch, missing owner |

### 6.6 Vector Instructions

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `VecPack(n)` | `[..., v1, ... vn] -> [..., vec]` | Pops `n` values and preserves original order in vector | insufficient stack |
| `VecLen` | `[..., vec] -> [..., len]` | Vector length as `u64` | type mismatch |
| `VecPushBack` | `[..., vec, v] -> [..., vec']` | Appends value to vector | type mismatch |

### 6.7 Call Instructions

| Instruction | Stack Effect | Semantics | Errors |
|---|---|---|---|
| `Call { function, arg_count }` | builtin-specific | Executes builtin implementation | bad arg count, type errors, builtin errors |
| `CallGeneric { function, type_args, arg_count }` | builtin-specific | Same as `Call` in V1; `type_args` currently ignored | same as above |

## 7. Builtin Function Specification

| Builtin | Expected Stack Input | Stack Output | Behavior |
|---|---|---|---|
| `ReadCoinBalance` | `[..., coin]` | `[..., u64]` | Reads coin balance |
| `ReadCoinOwner` | `[..., coin]` | `[..., address]` | Reads owner |
| `ReadCoinType` | `[..., coin]` | `[..., string]` | Reads coin type string |
| `CoinWithdraw` | `[..., coin, amount]` | `[..., coin']` | Withdraws amount, increments version |
| `CoinDeposit` | `[..., coin, amount]` | `[..., coin']` | Deposits amount, increments version |
| `CoinTransferTo` | `[..., coin, recipient]` | `[..., coin']` | Transfers owner, increments version |
| `DeterministicCoinId` | `[..., owner, coin_type]` | `[..., object_id]` | Calls `deterministic_coin_id(owner, coin_type)` |

Notes:

- `CoinWithdraw` uses checked balance logic (`Insufficient balance` on failure).
- `CoinDeposit` uses checked add (`Balance overflow` on failure).
- Builtins consume their arguments and push their outputs.

## 8. Error Model

### 8.1 VM-Specific Errors

- `RuntimeError::ProgramExecution(String)`
- `RuntimeError::ProgramAbort(String)`

### 8.2 Common Failure Cases

- Stack underflow
- Type mismatch (`Expected X, got Y`)
- Invalid local index
- Uninitialized local access
- Invalid jump target
- Missing object on storage operations
- Arithmetic overflow/underflow/divide-by-zero

## 9. Example: Conditional Transfer Program

Implemented test program (see `executor.rs` tests) demonstrates:

1. `MoveFrom` sender coin.
2. Owner check via branch (`BrFalse -> Abort`).
3. Balance check via branch (`BrFalse -> Abort`).
4. Withdraw sender amount and `MoveTo` sender coin back.
5. Compute recipient deterministic coin ID.
6. If recipient coin exists:
   - `BorrowGlobal`, `CoinDeposit`, `MoveTo`.
7. Else:
   - `PackCoin`, `MoveTo`.

This is validated by unit tests:

- success path
- insufficient-balance abort path

## 10. Non-Goals in V1

- Full Move bytecode verifier
- Full Sui Move native function set
- Type-checked generics beyond builtin dispatch
- Gas accounting/enforcement
- Module publish/upgrade semantics

