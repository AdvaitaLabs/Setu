//! Minimal V1 program VM for runtime execution.
//!
//! This is a deterministic interpreter intended as the first step toward
//! MoveVM-style programmable execution. It supports:
//! - Control flow (`Branch`, `BrTrue`, `BrFalse`, `Abort`, `Ret`)
//! - Locals / references (`CopyLoc`, `MoveLoc`, `StLoc`, `BorrowLoc`, `ReadRef`, `WriteRef`, `FreezeRef`)
//! - Arithmetic / comparisons / boolean ops
//! - `Call` / `CallGeneric` via builtin functions
//! - Global storage operations (`Exists`, `BorrowGlobal`, `MoveFrom`, `MoveTo`)
//! - Basic resource ops (`PackCoin`, `UnpackCoin`)
//! - Vector basics (`VecPack`, `VecLen`, `VecPushBack`)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use setu_types::{deterministic_coin_id, Address, Balance, CoinData, CoinType, Object, ObjectId};

use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub locals_count: usize,
    pub instructions: Vec<Instruction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VmConstant {
    U64(u64),
    Bool(bool),
    Address(Address),
    ObjectId(ObjectId),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BuiltinFunction {
    ReadCoinBalance,
    ReadCoinOwner,
    ReadCoinType,
    CoinWithdraw,
    CoinDeposit,
    CoinTransferTo,
    DeterministicCoinId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    Nop,
    LoadConst(VmConstant),

    CopyLoc(usize),
    MoveLoc(usize),
    StLoc(usize),
    BorrowLoc(usize),
    ReadRef,
    WriteRef,
    FreezeRef,

    Add,
    Sub,
    Mul,
    Div,
    Mod,

    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    Not,
    And,
    Or,

    Branch(usize),
    BrTrue(usize),
    BrFalse(usize),

    Exists,
    BorrowGlobal,
    MoveFrom,
    MoveTo,

    PackCoin,
    UnpackCoin,

    VecPack(usize),
    VecLen,
    VecPushBack,

    Call {
        function: BuiltinFunction,
        arg_count: usize,
    },
    CallGeneric {
        function: BuiltinFunction,
        type_args: Vec<String>,
        arg_count: usize,
    },

    Abort(String),
    Ret,
}

#[derive(Debug, Clone)]
pub struct ProgramWrite {
    pub object_id: ObjectId,
    pub old_state: Option<Vec<u8>>,
    pub new_state: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct ProgramExecutionOutcome {
    pub message: Option<String>,
    pub writes: Vec<ProgramWrite>,
}

#[derive(Debug, Clone)]
enum RuntimeValue {
    U64(u64),
    Bool(bool),
    Address(Address),
    ObjectId(ObjectId),
    String(String),
    Coin(Object<CoinData>),
    Ref(usize),
    Vector(Vec<RuntimeValue>),
}

impl RuntimeValue {
    fn type_name(&self) -> &'static str {
        match self {
            Self::U64(_) => "u64",
            Self::Bool(_) => "bool",
            Self::Address(_) => "address",
            Self::ObjectId(_) => "object_id",
            Self::String(_) => "string",
            Self::Coin(_) => "coin",
            Self::Ref(_) => "ref",
            Self::Vector(_) => "vector",
        }
    }
}

impl From<VmConstant> for RuntimeValue {
    fn from(value: VmConstant) -> Self {
        match value {
            VmConstant::U64(v) => RuntimeValue::U64(v),
            VmConstant::Bool(v) => RuntimeValue::Bool(v),
            VmConstant::Address(v) => RuntimeValue::Address(v),
            VmConstant::ObjectId(v) => RuntimeValue::ObjectId(v),
            VmConstant::String(v) => RuntimeValue::String(v),
        }
    }
}

struct ProgramVm<'a, S: StateStore> {
    state: &'a mut S,
    program: &'a Program,
    pc: usize,
    stack: Vec<RuntimeValue>,
    locals: Vec<Option<RuntimeValue>>,
    overlay: HashMap<ObjectId, Option<Object<CoinData>>>,
    original: HashMap<ObjectId, Option<Object<CoinData>>>,
    write_order: Vec<ObjectId>,
}

impl<'a, S: StateStore> ProgramVm<'a, S> {
    fn new(state: &'a mut S, program: &'a Program) -> Self {
        Self {
            state,
            program,
            pc: 0,
            stack: Vec::new(),
            locals: vec![None; program.locals_count],
            overlay: HashMap::new(),
            original: HashMap::new(),
            write_order: Vec::new(),
        }
    }

    fn run(mut self) -> RuntimeResult<ProgramExecutionOutcome> {
        let step_limit = 100_000usize;
        let mut steps = 0usize;

        while self.pc < self.program.instructions.len() {
            if steps >= step_limit {
                return Err(RuntimeError::ProgramExecution(
                    "Program step limit exceeded".to_string(),
                ));
            }
            steps += 1;

            let instruction = self.program.instructions[self.pc].clone();
            self.pc += 1;
            match instruction {
                Instruction::Nop => {}
                Instruction::LoadConst(v) => self.stack.push(v.into()),
                Instruction::CopyLoc(idx) => {
                    let value = self.local_get(idx)?.clone();
                    self.stack.push(value);
                }
                Instruction::MoveLoc(idx) => {
                    let value = self
                        .locals
                        .get_mut(idx)
                        .ok_or_else(|| {
                            RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                        })?
                        .take()
                        .ok_or_else(|| {
                            RuntimeError::ProgramExecution(format!(
                                "Local {} is uninitialized",
                                idx
                            ))
                        })?;
                    self.stack.push(value);
                }
                Instruction::StLoc(idx) => {
                    let value = self.pop()?;
                    let slot = self.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    *slot = Some(value);
                }
                Instruction::BorrowLoc(idx) => {
                    self.local_get(idx)?;
                    self.stack.push(RuntimeValue::Ref(idx));
                }
                Instruction::ReadRef => {
                    let idx = self.pop_ref()?;
                    let value = self.local_get(idx)?.clone();
                    self.stack.push(value);
                }
                Instruction::WriteRef => {
                    let value = self.pop()?;
                    let idx = self.pop_ref()?;
                    let slot = self.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    *slot = Some(value);
                }
                Instruction::FreezeRef => {
                    let idx = self.pop_ref()?;
                    self.stack.push(RuntimeValue::Ref(idx));
                }
                Instruction::Add => self.binary_u64_op("Add", |a, b| a.checked_add(b))?,
                Instruction::Sub => self.binary_u64_op("Sub", |a, b| a.checked_sub(b))?,
                Instruction::Mul => self.binary_u64_op("Mul", |a, b| a.checked_mul(b))?,
                Instruction::Div => {
                    let rhs = self.pop_u64()?;
                    let lhs = self.pop_u64()?;
                    if rhs == 0 {
                        return Err(RuntimeError::ProgramExecution(
                            "Division by zero".to_string(),
                        ));
                    }
                    self.stack.push(RuntimeValue::U64(lhs / rhs));
                }
                Instruction::Mod => {
                    let rhs = self.pop_u64()?;
                    let lhs = self.pop_u64()?;
                    if rhs == 0 {
                        return Err(RuntimeError::ProgramExecution("Modulo by zero".to_string()));
                    }
                    self.stack.push(RuntimeValue::U64(lhs % rhs));
                }
                Instruction::Eq => {
                    let rhs = self.pop()?;
                    let lhs = self.pop()?;
                    self.stack
                        .push(RuntimeValue::Bool(self.eq_values(&lhs, &rhs)?));
                }
                Instruction::Neq => {
                    let rhs = self.pop()?;
                    let lhs = self.pop()?;
                    self.stack
                        .push(RuntimeValue::Bool(!self.eq_values(&lhs, &rhs)?));
                }
                Instruction::Lt => self.compare_u64(|a, b| a < b)?,
                Instruction::Le => self.compare_u64(|a, b| a <= b)?,
                Instruction::Gt => self.compare_u64(|a, b| a > b)?,
                Instruction::Ge => self.compare_u64(|a, b| a >= b)?,
                Instruction::Not => {
                    let v = self.pop_bool()?;
                    self.stack.push(RuntimeValue::Bool(!v));
                }
                Instruction::And => {
                    let rhs = self.pop_bool()?;
                    let lhs = self.pop_bool()?;
                    self.stack.push(RuntimeValue::Bool(lhs && rhs));
                }
                Instruction::Or => {
                    let rhs = self.pop_bool()?;
                    let lhs = self.pop_bool()?;
                    self.stack.push(RuntimeValue::Bool(lhs || rhs));
                }
                Instruction::Branch(target) => self.jump(target)?,
                Instruction::BrTrue(target) => {
                    if self.pop_bool()? {
                        self.jump(target)?;
                    }
                }
                Instruction::BrFalse(target) => {
                    if !self.pop_bool()? {
                        self.jump(target)?;
                    }
                }
                Instruction::Exists => {
                    let object_id = self.pop_object_id()?;
                    let exists = self.read_object(&object_id)?.is_some();
                    self.stack.push(RuntimeValue::Bool(exists));
                }
                Instruction::BorrowGlobal => {
                    let object_id = self.pop_object_id()?;
                    let coin = self
                        .read_object(&object_id)?
                        .ok_or(RuntimeError::ObjectNotFound(object_id))?;
                    self.stack.push(RuntimeValue::Coin(coin));
                }
                Instruction::MoveFrom => {
                    let object_id = self.pop_object_id()?;
                    let coin = self.take_object(object_id)?;
                    self.stack.push(RuntimeValue::Coin(coin));
                }
                Instruction::MoveTo => {
                    let coin = self.pop_coin()?;
                    let object_id = self.pop_object_id()?;
                    self.put_object(object_id, coin)?;
                }
                Instruction::PackCoin => {
                    let coin_type = self.pop_string()?;
                    let amount = self.pop_u64()?;
                    let owner = self.pop_address()?;
                    let object_id = self.pop_object_id()?;

                    let coin = Object::new_owned(
                        object_id,
                        owner,
                        CoinData {
                            coin_type: CoinType::new(coin_type),
                            balance: Balance::new(amount),
                        },
                    );
                    self.stack.push(RuntimeValue::Coin(coin));
                }
                Instruction::UnpackCoin => {
                    let coin = self.pop_coin()?;
                    let owner = coin.owner().copied().ok_or_else(|| {
                        RuntimeError::ProgramExecution("Coin has no owner".to_string())
                    })?;
                    self.stack.push(RuntimeValue::ObjectId(*coin.id()));
                    self.stack.push(RuntimeValue::Address(owner));
                    self.stack
                        .push(RuntimeValue::U64(coin.data.balance.value()));
                    self.stack.push(RuntimeValue::String(
                        coin.data.coin_type.as_str().to_string(),
                    ));
                }
                Instruction::VecPack(count) => {
                    if self.stack.len() < count {
                        return Err(RuntimeError::ProgramExecution(format!(
                            "VecPack requires {} stack values, found {}",
                            count,
                            self.stack.len()
                        )));
                    }
                    let mut values = Vec::with_capacity(count);
                    for _ in 0..count {
                        values.push(self.pop()?);
                    }
                    values.reverse();
                    self.stack.push(RuntimeValue::Vector(values));
                }
                Instruction::VecLen => {
                    let values = self.pop_vector()?;
                    self.stack.push(RuntimeValue::U64(values.len() as u64));
                }
                Instruction::VecPushBack => {
                    let value = self.pop()?;
                    let mut values = self.pop_vector()?;
                    values.push(value);
                    self.stack.push(RuntimeValue::Vector(values));
                }
                Instruction::Call {
                    function,
                    arg_count,
                } => {
                    self.execute_builtin(function, arg_count)?;
                }
                Instruction::CallGeneric {
                    function,
                    type_args: _,
                    arg_count,
                } => {
                    self.execute_builtin(function, arg_count)?;
                }
                Instruction::Abort(msg) => return Err(RuntimeError::ProgramAbort(msg)),
                Instruction::Ret => return self.commit(),
            }
        }

        Err(RuntimeError::ProgramExecution(
            "Program terminated without Ret".to_string(),
        ))
    }

    fn commit(self) -> RuntimeResult<ProgramExecutionOutcome> {
        let mut writes = Vec::new();

        for object_id in self.write_order {
            let old_obj = self.original.get(&object_id).cloned().unwrap_or(None);
            let new_obj = self
                .overlay
                .get(&object_id)
                .cloned()
                .unwrap_or(old_obj.clone());

            let old_state = old_obj.as_ref().map(|obj| obj.to_coin_state_bytes());
            let new_state = new_obj.as_ref().map(|obj| obj.to_coin_state_bytes());

            if old_state == new_state {
                continue;
            }

            match new_obj {
                Some(obj) => {
                    self.state.set_object(object_id, obj)?;
                }
                None => {
                    self.state.delete_object(&object_id)?;
                }
            }

            writes.push(ProgramWrite {
                object_id,
                old_state,
                new_state,
            });
        }

        Ok(ProgramExecutionOutcome {
            message: Some("Program executed successfully".to_string()),
            writes,
        })
    }

    fn execute_builtin(
        &mut self,
        function: BuiltinFunction,
        arg_count: usize,
    ) -> RuntimeResult<()> {
        match function {
            BuiltinFunction::ReadCoinBalance => {
                self.expect_args("ReadCoinBalance", arg_count, 1)?;
                let coin = self.pop_coin()?;
                self.stack
                    .push(RuntimeValue::U64(coin.data.balance.value()));
            }
            BuiltinFunction::ReadCoinOwner => {
                self.expect_args("ReadCoinOwner", arg_count, 1)?;
                let coin = self.pop_coin()?;
                let owner = coin.owner().copied().ok_or_else(|| {
                    RuntimeError::ProgramExecution("Coin has no owner".to_string())
                })?;
                self.stack.push(RuntimeValue::Address(owner));
            }
            BuiltinFunction::ReadCoinType => {
                self.expect_args("ReadCoinType", arg_count, 1)?;
                let coin = self.pop_coin()?;
                self.stack.push(RuntimeValue::String(
                    coin.data.coin_type.as_str().to_string(),
                ));
            }
            BuiltinFunction::CoinWithdraw => {
                self.expect_args("CoinWithdraw", arg_count, 2)?;
                let amount = self.pop_u64()?;
                let mut coin = self.pop_coin()?;
                coin.data
                    .balance
                    .withdraw(amount)
                    .map_err(RuntimeError::InvalidTransaction)?;
                coin.increment_version();
                self.stack.push(RuntimeValue::Coin(coin));
            }
            BuiltinFunction::CoinDeposit => {
                self.expect_args("CoinDeposit", arg_count, 2)?;
                let amount = self.pop_u64()?;
                let mut coin = self.pop_coin()?;
                coin.data
                    .balance
                    .deposit(Balance::new(amount))
                    .map_err(RuntimeError::InvalidTransaction)?;
                coin.increment_version();
                self.stack.push(RuntimeValue::Coin(coin));
            }
            BuiltinFunction::CoinTransferTo => {
                self.expect_args("CoinTransferTo", arg_count, 2)?;
                let recipient = self.pop_address()?;
                let mut coin = self.pop_coin()?;
                coin.transfer_to(recipient);
                self.stack.push(RuntimeValue::Coin(coin));
            }
            BuiltinFunction::DeterministicCoinId => {
                self.expect_args("DeterministicCoinId", arg_count, 2)?;
                let coin_type = self.pop_string()?;
                let owner = self.pop_address()?;
                self.stack
                    .push(RuntimeValue::ObjectId(deterministic_coin_id(
                        &owner, &coin_type,
                    )));
            }
        }
        Ok(())
    }

    fn expect_args(&self, name: &str, got: usize, expected: usize) -> RuntimeResult<()> {
        if got != expected {
            return Err(RuntimeError::ProgramExecution(format!(
                "{} expects {} args, got {}",
                name, expected, got
            )));
        }
        Ok(())
    }

    fn mark_touched(&mut self, object_id: ObjectId) -> RuntimeResult<()> {
        if !self.original.contains_key(&object_id) {
            let original = self.state.get_object(&object_id)?;
            self.original.insert(object_id, original);
            self.write_order.push(object_id);
        }
        Ok(())
    }

    fn read_object(&self, object_id: &ObjectId) -> RuntimeResult<Option<Object<CoinData>>> {
        if let Some(value) = self.overlay.get(object_id) {
            return Ok(value.clone());
        }
        self.state.get_object(object_id)
    }

    fn take_object(&mut self, object_id: ObjectId) -> RuntimeResult<Object<CoinData>> {
        let value = self
            .read_object(&object_id)?
            .ok_or(RuntimeError::ObjectNotFound(object_id))?;
        self.mark_touched(object_id)?;
        self.overlay.insert(object_id, None);
        Ok(value)
    }

    fn put_object(&mut self, object_id: ObjectId, object: Object<CoinData>) -> RuntimeResult<()> {
        self.mark_touched(object_id)?;
        self.overlay.insert(object_id, Some(object));
        Ok(())
    }

    fn pop(&mut self) -> RuntimeResult<RuntimeValue> {
        self.stack
            .pop()
            .ok_or_else(|| RuntimeError::ProgramExecution("Stack underflow".to_string()))
    }

    fn pop_u64(&mut self) -> RuntimeResult<u64> {
        match self.pop()? {
            RuntimeValue::U64(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected u64, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_bool(&mut self) -> RuntimeResult<bool> {
        match self.pop()? {
            RuntimeValue::Bool(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected bool, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_address(&mut self) -> RuntimeResult<Address> {
        match self.pop()? {
            RuntimeValue::Address(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected address, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_object_id(&mut self) -> RuntimeResult<ObjectId> {
        match self.pop()? {
            RuntimeValue::ObjectId(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected object_id, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_string(&mut self) -> RuntimeResult<String> {
        match self.pop()? {
            RuntimeValue::String(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected string, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_coin(&mut self) -> RuntimeResult<Object<CoinData>> {
        match self.pop()? {
            RuntimeValue::Coin(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected coin, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_ref(&mut self) -> RuntimeResult<usize> {
        match self.pop()? {
            RuntimeValue::Ref(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected ref, got {}",
                other.type_name()
            ))),
        }
    }

    fn pop_vector(&mut self) -> RuntimeResult<Vec<RuntimeValue>> {
        match self.pop()? {
            RuntimeValue::Vector(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected vector, got {}",
                other.type_name()
            ))),
        }
    }

    fn local_get(&self, idx: usize) -> RuntimeResult<&RuntimeValue> {
        self.locals
            .get(idx)
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("Invalid local index {}", idx)))?
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::ProgramExecution(format!("Local {} is uninitialized", idx))
            })
    }

    fn jump(&mut self, target: usize) -> RuntimeResult<()> {
        if target >= self.program.instructions.len() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Jump target out of range: {}",
                target
            )));
        }
        self.pc = target;
        Ok(())
    }

    fn binary_u64_op<F>(&mut self, op: &str, f: F) -> RuntimeResult<()>
    where
        F: FnOnce(u64, u64) -> Option<u64>,
    {
        let rhs = self.pop_u64()?;
        let lhs = self.pop_u64()?;
        let out = f(lhs, rhs)
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("{} overflow/underflow", op)))?;
        self.stack.push(RuntimeValue::U64(out));
        Ok(())
    }

    fn compare_u64<F>(&mut self, f: F) -> RuntimeResult<()>
    where
        F: FnOnce(u64, u64) -> bool,
    {
        let rhs = self.pop_u64()?;
        let lhs = self.pop_u64()?;
        self.stack.push(RuntimeValue::Bool(f(lhs, rhs)));
        Ok(())
    }

    fn eq_values(&self, lhs: &RuntimeValue, rhs: &RuntimeValue) -> RuntimeResult<bool> {
        match (lhs, rhs) {
            (RuntimeValue::U64(a), RuntimeValue::U64(b)) => Ok(a == b),
            (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => Ok(a == b),
            (RuntimeValue::Address(a), RuntimeValue::Address(b)) => Ok(a == b),
            (RuntimeValue::ObjectId(a), RuntimeValue::ObjectId(b)) => Ok(a == b),
            (RuntimeValue::String(a), RuntimeValue::String(b)) => Ok(a == b),
            _ => Err(RuntimeError::ProgramExecution(format!(
                "Eq/Neq unsupported for {} and {}",
                lhs.type_name(),
                rhs.type_name()
            ))),
        }
    }
}

pub fn execute_program<S: StateStore>(
    state: &mut S,
    _sender: &Address,
    program: &Program,
) -> RuntimeResult<ProgramExecutionOutcome> {
    if program.instructions.is_empty() {
        return Err(RuntimeError::ProgramExecution(
            "Program has no instructions".to_string(),
        ));
    }

    let vm = ProgramVm::new(state, program);
    vm.run()
}
