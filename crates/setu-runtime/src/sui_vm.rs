//! Direct Sui disassembly VM (subset).
//!
//! This module executes a subset of Sui Move disassembly opcodes directly,
//! instead of translating specific contract patterns into Setu VM programs.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use setu_types::{deterministic_coin_id, Address, Balance, CoinData, CoinType, Object, ObjectId};

use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::vm_object::SuiVmStoredObject;

/// Compile a Sui Move package and return disassembly text for `module_name`.
pub fn compile_package_to_disassembly(
    package_path: &Path,
    module_name: &str,
) -> RuntimeResult<String> {
    let status = Command::new("sui")
        .arg("move")
        .arg("build")
        .arg("--disassemble")
        .arg("--path")
        .arg(package_path)
        .status()
        .map_err(|e| RuntimeError::ProgramExecution(format!("Failed to run sui build: {}", e)))?;

    if !status.success() {
        return Err(RuntimeError::ProgramExecution(format!(
            "sui move build failed with status {}",
            status
        )));
    }

    let disassembly_file = find_disassembly_file(package_path, module_name)?;
    fs::read_to_string(&disassembly_file).map_err(|e| {
        RuntimeError::ProgramExecution(format!(
            "Failed reading disassembly file {}: {}",
            disassembly_file.display(),
            e
        ))
    })
}

fn find_disassembly_file(package_path: &Path, module_name: &str) -> RuntimeResult<PathBuf> {
    let build_dir = package_path.join("build");
    let entries = fs::read_dir(&build_dir).map_err(|e| {
        RuntimeError::ProgramExecution(format!(
            "Failed reading build dir {}: {}",
            build_dir.display(),
            e
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            RuntimeError::ProgramExecution(format!("Failed reading build entry: {}", e))
        })?;
        let candidate = entry
            .path()
            .join("disassembly")
            .join(format!("{}.mvb", module_name));
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(RuntimeError::ProgramExecution(format!(
        "Disassembly file for module '{}' not found under {}",
        module_name,
        build_dir.display()
    )))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SuiVmArg {
    U64(u64),
    Bool(bool),
    Address(Address),
    ObjectId(ObjectId),
    Opaque,
}

#[derive(Debug, Clone)]
enum SuiVmValue {
    U64(u64),
    Bool(bool),
    Address(Address),
    Coin(Object<CoinData>),
    VmObjectRef(ObjectId),
    VmFieldRef {
        object_id: ObjectId,
        field_name: String,
    },
    Opaque,
}

#[derive(Debug, Clone)]
enum SuiOpcode {
    MoveLoc(usize),
    CopyLoc(usize),
    StLoc(usize),
    LdU64(u64),
    LdU8(u8),
    LdTrue,
    LdFalse,
    Add,
    BrFalse(usize),
    BrTrue(usize),
    Branch(usize),
    ImmBorrowField(String),
    MutBorrowField(String),
    ReadRef,
    WriteRef,
    Call { function: String, arg_count: usize },
    Pop,
    Ret,
}

#[derive(Debug, Clone)]
struct ParsedFunction {
    param_types: Vec<String>,
    return_count: usize,
    locals_count: usize,
    instructions: Vec<(usize, SuiOpcode)>,
}

#[derive(Debug, Clone)]
struct CallFrame {
    instructions: Vec<(usize, SuiOpcode)>,
    instruction_pc: HashMap<usize, usize>,
    locals: Vec<Option<SuiVmValue>>,
    stack: Vec<SuiVmValue>,
}

#[derive(Debug, Clone)]
pub struct SuiVmWrite {
    pub object_id: ObjectId,
    pub old_state: Option<Vec<u8>>,
    pub new_state: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct SuiVmExecutionOutcome {
    pub writes: Vec<SuiVmWrite>,
}

pub fn execute_sui_entry_from_disassembly<S: StateStore>(
    state: &mut S,
    sender: &Address,
    disassembly: &str,
    function_name: &str,
    args: &[SuiVmArg],
) -> RuntimeResult<()> {
    let _ = execute_sui_entry_with_outcome(state, sender, disassembly, function_name, args)?;
    Ok(())
}

pub fn execute_sui_entry_with_outcome<S: StateStore>(
    state: &mut S,
    sender: &Address,
    disassembly: &str,
    function_name: &str,
    args: &[SuiVmArg],
) -> RuntimeResult<SuiVmExecutionOutcome> {
    let mut vm = SuiDisasmVm::new(state, sender, disassembly);
    vm.run(function_name, args)
}

struct SuiDisasmVm<'a, S: StateStore> {
    state: &'a mut S,
    sender: &'a Address,
    disassembly: &'a str,
    functions: HashMap<String, ParsedFunction>,
    temp_counter: u64,
    write_order: Vec<ObjectId>,
    old_states: HashMap<ObjectId, Option<Vec<u8>>>,
    final_states: HashMap<ObjectId, Option<Vec<u8>>>,
}

impl<'a, S: StateStore> SuiDisasmVm<'a, S> {
    fn new(state: &'a mut S, sender: &'a Address, disassembly: &'a str) -> Self {
        Self {
            state,
            sender,
            disassembly,
            functions: HashMap::new(),
            temp_counter: 1,
            write_order: Vec::new(),
            old_states: HashMap::new(),
            final_states: HashMap::new(),
        }
    }

    fn run(
        &mut self,
        function_name: &str,
        args: &[SuiVmArg],
    ) -> RuntimeResult<SuiVmExecutionOutcome> {
        let returned = self.execute_entry_function(function_name, args)?;
        if !returned.is_empty() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Entry function '{}' returned {} values unexpectedly",
                function_name,
                returned.len()
            )));
        }
        self.finish()
    }

    fn execute_entry_function(
        &mut self,
        function_name: &str,
        args: &[SuiVmArg],
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        let function = self.load_function(function_name)?.clone();
        if args.len() != function.param_types.len() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Arg count mismatch: expected {}, got {}",
                function.param_types.len(),
                args.len()
            )));
        }

        let mut locals = vec![None; function.locals_count];
        for (idx, (arg, param_ty)) in args.iter().zip(function.param_types.iter()).enumerate() {
            locals[idx] = Some(Self::coerce_arg(self.state, arg, param_ty)?);
        }

        let mut frame = Self::build_frame(&function, locals);
        self.run_frame(function_name, &function, &mut frame)
    }

    fn execute_user_function(
        &mut self,
        function_name: &str,
        args: &[SuiVmValue],
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        let function = self.load_function(function_name)?.clone();
        if args.len() != function.param_types.len() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Call {} expects {} args, got {}",
                function_name,
                function.param_types.len(),
                args.len()
            )));
        }

        let mut locals = vec![None; function.locals_count];
        for (idx, (arg, param_ty)) in args.iter().zip(function.param_types.iter()).enumerate() {
            locals[idx] = Some(Self::coerce_value(arg, param_ty)?);
        }

        let mut frame = Self::build_frame(&function, locals);
        self.run_frame(function_name, &function, &mut frame)
    }

    fn build_frame(function: &ParsedFunction, locals: Vec<Option<SuiVmValue>>) -> CallFrame {
        let mut instruction_pc = HashMap::new();
        for (pc, (inst_idx, _)) in function.instructions.iter().enumerate() {
            instruction_pc.insert(*inst_idx, pc);
        }

        CallFrame {
            instructions: function.instructions.clone(),
            instruction_pc,
            locals,
            stack: Vec::new(),
        }
    }

    fn run_frame(
        &mut self,
        function_name: &str,
        function: &ParsedFunction,
        frame: &mut CallFrame,
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        let mut pc = 0usize;
        let step_limit = 100_000usize;
        let mut steps = 0usize;

        while pc < frame.instructions.len() {
            if steps >= step_limit {
                return Err(RuntimeError::ProgramExecution(
                    "Sui VM step limit exceeded".to_string(),
                ));
            }
            steps += 1;

            let (_, op) = frame.instructions[pc].clone();
            pc += 1;

            match op {
                SuiOpcode::MoveLoc(idx) => {
                    let slot = frame.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    let v = slot.take().ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Local {} is uninitialized", idx))
                    })?;
                    frame.stack.push(v);
                }
                SuiOpcode::CopyLoc(idx) => {
                    let v = Self::local_get(frame, idx)?.clone();
                    frame.stack.push(v);
                }
                SuiOpcode::StLoc(idx) => {
                    let v = Self::pop(&mut frame.stack)?;
                    let slot = frame.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    *slot = Some(v);
                }
                SuiOpcode::LdU64(v) => frame.stack.push(SuiVmValue::U64(v)),
                SuiOpcode::LdU8(v) => frame.stack.push(SuiVmValue::U64(v as u64)),
                SuiOpcode::LdTrue => frame.stack.push(SuiVmValue::Bool(true)),
                SuiOpcode::LdFalse => frame.stack.push(SuiVmValue::Bool(false)),
                SuiOpcode::Add => {
                    let rhs = Self::pop_u64(&mut frame.stack)?;
                    let lhs = Self::pop_u64(&mut frame.stack)?;
                    frame
                        .stack
                        .push(SuiVmValue::U64(lhs.checked_add(rhs).ok_or_else(|| {
                            RuntimeError::ProgramExecution(
                                "u64 addition overflow in Sui VM".to_string(),
                            )
                        })?));
                }
                SuiOpcode::BrFalse(target) => {
                    if !Self::pop_bool(&mut frame.stack)? {
                        pc = Self::jump_target(frame, target)?;
                    }
                }
                SuiOpcode::BrTrue(target) => {
                    if Self::pop_bool(&mut frame.stack)? {
                        pc = Self::jump_target(frame, target)?;
                    }
                }
                SuiOpcode::Branch(target) => {
                    pc = Self::jump_target(frame, target)?;
                }
                SuiOpcode::ImmBorrowField(field_name) | SuiOpcode::MutBorrowField(field_name) => {
                    let object_id = Self::pop_object_ref(&mut frame.stack)?;
                    frame.stack.push(SuiVmValue::VmFieldRef {
                        object_id,
                        field_name,
                    });
                }
                SuiOpcode::ReadRef => {
                    let (object_id, field_name) = Self::pop_field_ref(&mut frame.stack)?;
                    let value = self.read_vm_u64_field(&object_id, &field_name)?;
                    frame.stack.push(SuiVmValue::U64(value));
                }
                SuiOpcode::WriteRef => {
                    let (object_id, field_name) = Self::pop_field_ref(&mut frame.stack)?;
                    let value = Self::pop_u64(&mut frame.stack)?;
                    self.write_vm_u64_field(object_id, &field_name, value)?;
                }
                SuiOpcode::Call {
                    function,
                    arg_count,
                } => {
                    let returned = self.execute_call(frame, &function, arg_count)?;
                    frame.stack.extend(returned);
                }
                SuiOpcode::Pop => {
                    let _ = Self::pop(&mut frame.stack)?;
                }
                SuiOpcode::Ret => return Self::finish_frame(function_name, function, frame),
            }
        }

        Err(RuntimeError::ProgramExecution(format!(
            "Function '{}' terminated without Ret",
            function_name
        )))
    }

    fn finish(&mut self) -> RuntimeResult<SuiVmExecutionOutcome> {
        let mut writes = Vec::new();
        for object_id in &self.write_order {
            let old_state = self.old_states.get(object_id).cloned().unwrap_or(None);
            let new_state = self.final_states.get(object_id).cloned().unwrap_or(None);
            if old_state == new_state {
                continue;
            }
            writes.push(SuiVmWrite {
                object_id: *object_id,
                old_state,
                new_state,
            });
        }
        Ok(SuiVmExecutionOutcome { writes })
    }

    fn coerce_arg(state: &mut S, arg: &SuiVmArg, param_type: &str) -> RuntimeResult<SuiVmValue> {
        if param_type == "u64" {
            return match arg {
                SuiVmArg::U64(v) => Ok(SuiVmValue::U64(*v)),
                _ => Err(RuntimeError::ProgramExecution(format!(
                    "Expected u64 arg, got {:?}",
                    arg
                ))),
            };
        }

        if param_type == "bool" {
            return match arg {
                SuiVmArg::Bool(v) => Ok(SuiVmValue::Bool(*v)),
                _ => Err(RuntimeError::ProgramExecution(format!(
                    "Expected bool arg, got {:?}",
                    arg
                ))),
            };
        }

        if param_type == "address" {
            return match arg {
                SuiVmArg::Address(v) => Ok(SuiVmValue::Address(*v)),
                _ => Err(RuntimeError::ProgramExecution(format!(
                    "Expected address arg, got {:?}",
                    arg
                ))),
            };
        }

        if param_type.starts_with("Coin<") {
            return match arg {
                SuiVmArg::ObjectId(object_id) => {
                    let coin = state
                        .get_object(object_id)?
                        .ok_or(RuntimeError::ObjectNotFound(*object_id))?;
                    Ok(SuiVmValue::Coin(coin))
                }
                _ => Err(RuntimeError::ProgramExecution(format!(
                    "Expected coin object id arg, got {:?}",
                    arg
                ))),
            };
        }

        if let SuiVmArg::ObjectId(object_id) = arg {
            let normalized = normalize_vm_object_type(param_type);
            if !normalized.is_empty() {
                let object = state
                    .get_vm_object(object_id)?
                    .ok_or(RuntimeError::ObjectNotFound(*object_id))?;
                if object.type_name != normalized {
                    return Err(RuntimeError::ProgramExecution(format!(
                        "Expected VM object type '{}', got '{}'",
                        normalized, object.type_name
                    )));
                }
                return Ok(SuiVmValue::VmObjectRef(*object_id));
            }
        }

        Ok(SuiVmValue::Opaque)
    }

    fn coerce_value(arg: &SuiVmValue, param_type: &str) -> RuntimeResult<SuiVmValue> {
        match (param_type, arg) {
            ("u64", SuiVmValue::U64(_))
            | ("bool", SuiVmValue::Bool(_))
            | ("address", SuiVmValue::Address(_)) => Ok(arg.clone()),
            (ty, SuiVmValue::Coin(_)) if ty.starts_with("Coin<") => Ok(arg.clone()),
            (ty, SuiVmValue::VmObjectRef(_)) if !normalize_vm_object_type(ty).is_empty() => {
                Ok(arg.clone())
            }
            _ => Ok(arg.clone()),
        }
    }

    fn load_function(&mut self, function_name: &str) -> RuntimeResult<&ParsedFunction> {
        if !self.functions.contains_key(function_name) {
            let parsed = parse_function_by_name(self.disassembly, function_name)?;
            self.functions.insert(function_name.to_string(), parsed);
        }
        self.functions.get(function_name).ok_or_else(|| {
            RuntimeError::ProgramExecution(format!("Function '{}' not found", function_name))
        })
    }

    fn jump_target(frame: &CallFrame, target: usize) -> RuntimeResult<usize> {
        frame.instruction_pc.get(&target).copied().ok_or_else(|| {
            RuntimeError::ProgramExecution(format!("Invalid branch target {}", target))
        })
    }

    fn execute_call(
        &mut self,
        frame: &mut CallFrame,
        function: &str,
        arg_count: usize,
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        if frame.stack.len() < arg_count {
            return Err(RuntimeError::ProgramExecution(format!(
                "Call {} expects {} args, stack has {}",
                function,
                arg_count,
                frame.stack.len()
            )));
        }

        let mut args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            args.push(Self::pop(&mut frame.stack)?);
        }
        args.reverse();

        if function.starts_with("coin::mint<") {
            self.call_coin_mint(function, &args)
        } else if function.starts_with("transfer::public_transfer<Coin<") {
            self.call_public_transfer(&args)
        } else if function.starts_with("coin::burn<") {
            self.call_coin_burn(&args)
        } else {
            self.execute_user_function(function, &args)
        }
    }

    fn call_coin_mint(
        &mut self,
        function: &str,
        args: &[SuiVmValue],
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        if args.len() != 3 {
            return Err(RuntimeError::ProgramExecution(format!(
                "coin::mint expects 3 args, got {}",
                args.len()
            )));
        }

        let amount = match args[1] {
            SuiVmValue::U64(v) => v,
            _ => {
                return Err(RuntimeError::ProgramExecution(
                    "coin::mint amount must be u64".to_string(),
                ))
            }
        };

        let coin_type = extract_generic_type(function).unwrap_or_else(|| "SUI".to_string());
        let object_id = self.next_temp_object_id();
        let coin = Object::new_owned(
            object_id,
            *self.sender,
            CoinData {
                coin_type: CoinType::new(coin_type),
                balance: Balance::new(amount),
            },
        );
        Ok(vec![SuiVmValue::Coin(coin)])
    }

    fn call_public_transfer(&mut self, args: &[SuiVmValue]) -> RuntimeResult<Vec<SuiVmValue>> {
        if args.len() != 2 {
            return Err(RuntimeError::ProgramExecution(format!(
                "transfer::public_transfer expects 2 args, got {}",
                args.len()
            )));
        }

        let coin = match &args[0] {
            SuiVmValue::Coin(v) => v.clone(),
            _ => {
                return Err(RuntimeError::ProgramExecution(
                    "public_transfer first arg must be coin".to_string(),
                ))
            }
        };
        let recipient = match args[1] {
            SuiVmValue::Address(v) => v,
            _ => {
                return Err(RuntimeError::ProgramExecution(
                    "public_transfer second arg must be address".to_string(),
                ))
            }
        };

        let amount = coin.data.balance.value();
        let coin_type = coin.data.coin_type.clone();
        let recipient_coin_id = deterministic_coin_id(&recipient, coin_type.as_str());

        // Consume moved source coin when it is a persisted object.
        let source_coin_id = *coin.id();
        if self.state.get_object(&source_coin_id)?.is_some() {
            self.delete_object_tracked(&source_coin_id)?;
        }

        if let Some(mut existing) = self.state.get_object(&recipient_coin_id)? {
            existing
                .data
                .balance
                .deposit(Balance::new(amount))
                .map_err(RuntimeError::InvalidTransaction)?;
            existing.increment_version();
            self.set_object_tracked(recipient_coin_id, existing)?;
        } else {
            let new_coin = Object::new_owned(
                recipient_coin_id,
                recipient,
                CoinData {
                    coin_type,
                    balance: Balance::new(amount),
                },
            );
            self.set_object_tracked(recipient_coin_id, new_coin)?;
        }

        Ok(vec![])
    }

    fn call_coin_burn(&mut self, args: &[SuiVmValue]) -> RuntimeResult<Vec<SuiVmValue>> {
        if args.len() != 2 {
            return Err(RuntimeError::ProgramExecution(format!(
                "coin::burn expects 2 args, got {}",
                args.len()
            )));
        }

        let coin = match &args[1] {
            SuiVmValue::Coin(v) => v.clone(),
            _ => {
                return Err(RuntimeError::ProgramExecution(
                    "coin::burn second arg must be coin".to_string(),
                ))
            }
        };

        let object_id = *coin.id();
        if self.state.get_object(&object_id)?.is_some() {
            self.delete_object_tracked(&object_id)?;
        }
        Ok(vec![SuiVmValue::U64(0)])
    }

    fn next_temp_object_id(&mut self) -> ObjectId {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&self.temp_counter.to_le_bytes());
        bytes[8..].copy_from_slice(&self.sender.as_bytes()[..24]);
        self.temp_counter += 1;
        ObjectId::new(bytes)
    }

    fn local_get(frame: &CallFrame, idx: usize) -> RuntimeResult<&SuiVmValue> {
        frame
            .locals
            .get(idx)
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("Invalid local index {}", idx)))?
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::ProgramExecution(format!("Local {} is uninitialized", idx))
            })
    }

    fn pop(stack: &mut Vec<SuiVmValue>) -> RuntimeResult<SuiVmValue> {
        stack
            .pop()
            .ok_or_else(|| RuntimeError::ProgramExecution("Stack underflow".to_string()))
    }

    fn pop_bool(stack: &mut Vec<SuiVmValue>) -> RuntimeResult<bool> {
        match Self::pop(stack)? {
            SuiVmValue::Bool(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected bool, got {:?}",
                other
            ))),
        }
    }

    fn pop_u64(stack: &mut Vec<SuiVmValue>) -> RuntimeResult<u64> {
        match Self::pop(stack)? {
            SuiVmValue::U64(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected u64, got {:?}",
                other
            ))),
        }
    }

    fn pop_object_ref(stack: &mut Vec<SuiVmValue>) -> RuntimeResult<ObjectId> {
        match Self::pop(stack)? {
            SuiVmValue::VmObjectRef(object_id) => Ok(object_id),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected object reference, got {:?}",
                other
            ))),
        }
    }

    fn pop_field_ref(stack: &mut Vec<SuiVmValue>) -> RuntimeResult<(ObjectId, String)> {
        match Self::pop(stack)? {
            SuiVmValue::VmFieldRef {
                object_id,
                field_name,
            } => Ok((object_id, field_name)),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected field reference, got {:?}",
                other
            ))),
        }
    }

    fn mark_touched(&mut self, object_id: ObjectId) -> RuntimeResult<()> {
        if self.old_states.contains_key(&object_id) {
            return Ok(());
        }
        let old_state = self
            .state
            .get_object(&object_id)?
            .map(|obj| obj.to_coin_state_bytes());
        self.old_states.insert(object_id, old_state.clone());
        self.final_states.insert(object_id, old_state);
        self.write_order.push(object_id);
        Ok(())
    }

    fn set_object_tracked(
        &mut self,
        object_id: ObjectId,
        object: Object<CoinData>,
    ) -> RuntimeResult<()> {
        self.mark_touched(object_id)?;
        let new_state = object.to_coin_state_bytes();
        self.state.set_object(object_id, object)?;
        self.final_states.insert(object_id, Some(new_state));
        Ok(())
    }

    fn read_vm_u64_field(&mut self, object_id: &ObjectId, field_name: &str) -> RuntimeResult<u64> {
        let object = self
            .state
            .get_vm_object(object_id)?
            .ok_or(RuntimeError::ObjectNotFound(*object_id))?;
        object.get_u64_field(field_name).ok_or_else(|| {
            RuntimeError::ProgramExecution(format!(
                "VM object {} missing u64 field '{}'",
                object_id, field_name
            ))
        })
    }

    fn mark_vm_object_touched(&mut self, object_id: ObjectId) -> RuntimeResult<()> {
        if self.old_states.contains_key(&object_id) {
            return Ok(());
        }

        let old_state = self
            .state
            .get_vm_object(&object_id)?
            .map(|object| serialize_vm_object(&object))
            .transpose()?;
        self.old_states.insert(object_id, old_state.clone());
        self.final_states.insert(object_id, old_state);
        self.write_order.push(object_id);
        Ok(())
    }

    fn set_vm_object_tracked(
        &mut self,
        object_id: ObjectId,
        object: SuiVmStoredObject,
    ) -> RuntimeResult<()> {
        self.mark_vm_object_touched(object_id)?;
        let new_state = serialize_vm_object(&object)?;
        self.state.set_vm_object(object_id, object)?;
        self.final_states.insert(object_id, Some(new_state));
        Ok(())
    }

    fn write_vm_u64_field(
        &mut self,
        object_id: ObjectId,
        field_name: &str,
        value: u64,
    ) -> RuntimeResult<()> {
        let mut object = self
            .state
            .get_vm_object(&object_id)?
            .ok_or(RuntimeError::ObjectNotFound(object_id))?;
        object.set_u64_field(field_name, value);
        self.set_vm_object_tracked(object_id, object)
    }

    fn delete_object_tracked(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.mark_touched(*object_id)?;
        self.state.delete_object(object_id)?;
        self.final_states.insert(*object_id, None);
        Ok(())
    }

    fn finish_frame(
        function_name: &str,
        function: &ParsedFunction,
        frame: &mut CallFrame,
    ) -> RuntimeResult<Vec<SuiVmValue>> {
        if frame.stack.len() < function.return_count {
            return Err(RuntimeError::ProgramExecution(format!(
                "Function '{}' expected {} return values, stack has {}",
                function_name,
                function.return_count,
                frame.stack.len()
            )));
        }

        let split = frame.stack.len() - function.return_count;
        let returns = frame.stack.split_off(split);
        if !frame.stack.is_empty() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Function '{}' left {} extra values on the stack",
                function_name,
                frame.stack.len()
            )));
        }
        Ok(returns)
    }
}

fn parse_function_by_name(disassembly: &str, function_name: &str) -> RuntimeResult<ParsedFunction> {
    let lines: Vec<&str> = disassembly.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if is_function_header(line) {
            let parsed_name = parse_function_name(line)?;
            if parsed_name == function_name {
                let param_types = parse_param_types(line)?;
                let return_count = parse_return_count(line)?;
                let mut instructions = Vec::new();
                i += 1;
                while i < lines.len() {
                    let cur = lines[i].trim();
                    if cur == "}" {
                        let max_local = instructions
                            .iter()
                            .filter_map(|(_, op)| match op {
                                SuiOpcode::MoveLoc(idx)
                                | SuiOpcode::CopyLoc(idx)
                                | SuiOpcode::StLoc(idx) => Some(*idx),
                                _ => None,
                            })
                            .max()
                            .unwrap_or(0);
                        return Ok(ParsedFunction {
                            locals_count: usize::max(param_types.len(), max_local + 1),
                            param_types,
                            return_count,
                            instructions,
                        });
                    }
                    if let Some(inst) = parse_instruction(cur)? {
                        instructions.push(inst);
                    }
                    i += 1;
                }
                return Err(RuntimeError::ProgramExecution(format!(
                    "Function '{}' body not closed",
                    function_name
                )));
            }
        }
        i += 1;
    }

    Err(RuntimeError::ProgramExecution(format!(
        "Entry function '{}' not found in disassembly",
        function_name
    )))
}

fn is_function_header(line: &str) -> bool {
    if !line.ends_with('{') || !line.contains('(') || line.starts_with("module ") {
        return false;
    }
    if line.starts_with("struct ") || line.starts_with("Constants ") {
        return false;
    }
    true
}

fn parse_function_name(line: &str) -> RuntimeResult<String> {
    let open = line.find('(').ok_or_else(|| {
        RuntimeError::ProgramExecution("Malformed function header: missing '('".to_string())
    })?;
    let prefix = line[..open].trim();
    prefix
        .split_whitespace()
        .last()
        .map(str::to_string)
        .ok_or_else(|| {
            RuntimeError::ProgramExecution(
                "Malformed function header: missing function name".to_string(),
            )
        })
}

fn parse_param_types(line: &str) -> RuntimeResult<Vec<String>> {
    let open = line.find('(').ok_or_else(|| {
        RuntimeError::ProgramExecution("Malformed function header: missing '('".to_string())
    })?;
    let close = line.rfind(')').ok_or_else(|| {
        RuntimeError::ProgramExecution("Malformed function header: missing ')'".to_string())
    })?;
    let raw = &line[open + 1..close];
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    let parts = split_top_level(raw, ',');
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let (_, ty) = part.split_once(':').ok_or_else(|| {
            RuntimeError::ProgramExecution(format!("Malformed parameter '{}'", part))
        })?;
        out.push(ty.trim().to_string());
    }
    Ok(out)
}

fn parse_return_count(line: &str) -> RuntimeResult<usize> {
    let close = line.rfind(')').ok_or_else(|| {
        RuntimeError::ProgramExecution("Malformed function header: missing ')'".to_string())
    })?;
    let suffix = line[close + 1..].trim();
    let suffix = suffix.strip_suffix('{').unwrap_or(suffix).trim();
    let returns = suffix.strip_prefix(':').unwrap_or(suffix).trim();
    if returns.is_empty() {
        return Ok(0);
    }
    Ok(split_top_level(returns, '*').len())
}

fn parse_instruction(line: &str) -> RuntimeResult<Option<(usize, SuiOpcode)>> {
    if line.is_empty() || line.ends_with(':') {
        return Ok(None);
    }

    let Some((idx_str, rhs)) = line.split_once(':') else {
        return Ok(None);
    };
    let idx = match idx_str.trim().parse::<usize>() {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let body = rhs.trim();

    if body.starts_with("MoveLoc[") {
        return Ok(Some((idx, SuiOpcode::MoveLoc(parse_bracket_usize(body)?))));
    }
    if body.starts_with("CopyLoc[") {
        return Ok(Some((idx, SuiOpcode::CopyLoc(parse_bracket_usize(body)?))));
    }
    if body.starts_with("StLoc[") {
        return Ok(Some((idx, SuiOpcode::StLoc(parse_bracket_usize(body)?))));
    }
    if body.starts_with("LdU64(") {
        return Ok(Some((idx, SuiOpcode::LdU64(parse_paren_u64(body)?))));
    }
    if body.starts_with("LdU8(") {
        return Ok(Some((idx, SuiOpcode::LdU8(parse_paren_u64(body)? as u8))));
    }
    if body == "LdTrue" {
        return Ok(Some((idx, SuiOpcode::LdTrue)));
    }
    if body == "LdFalse" {
        return Ok(Some((idx, SuiOpcode::LdFalse)));
    }
    if body == "Add" {
        return Ok(Some((idx, SuiOpcode::Add)));
    }
    if body.starts_with("BrFalse(") {
        return Ok(Some((idx, SuiOpcode::BrFalse(parse_paren_usize(body)?))));
    }
    if body.starts_with("BrTrue(") {
        return Ok(Some((idx, SuiOpcode::BrTrue(parse_paren_usize(body)?))));
    }
    if body.starts_with("Branch(") {
        return Ok(Some((idx, SuiOpcode::Branch(parse_paren_usize(body)?))));
    }
    if body.starts_with("ImmBorrowField[") {
        return Ok(Some((
            idx,
            SuiOpcode::ImmBorrowField(parse_field_name(body)?),
        )));
    }
    if body.starts_with("MutBorrowField[") {
        return Ok(Some((
            idx,
            SuiOpcode::MutBorrowField(parse_field_name(body)?),
        )));
    }
    if body == "ReadRef" {
        return Ok(Some((idx, SuiOpcode::ReadRef)));
    }
    if body == "WriteRef" {
        return Ok(Some((idx, SuiOpcode::WriteRef)));
    }
    if body.starts_with("Call ") {
        let call = body.trim_start_matches("Call ").trim();
        let open = call
            .find('(')
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed call '{}'", body)))?;
        let function = call[..open].trim().to_string();
        let close = call
            .find("):")
            .or_else(|| call.rfind(')'))
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed call '{}'", body)))?;
        let args_raw = &call[open + 1..close];
        let arg_count = if args_raw.trim().is_empty() {
            0
        } else {
            split_top_level(args_raw, ',').len()
        };
        return Ok(Some((
            idx,
            SuiOpcode::Call {
                function,
                arg_count,
            },
        )));
    }
    if body == "Pop" {
        return Ok(Some((idx, SuiOpcode::Pop)));
    }
    if body == "Ret" {
        return Ok(Some((idx, SuiOpcode::Ret)));
    }
    if body == "FreezeRef" {
        return Ok(None);
    }

    Err(RuntimeError::ProgramExecution(format!(
        "Unsupported Sui opcode line '{}'",
        line
    )))
}

fn parse_bracket_usize(body: &str) -> RuntimeResult<usize> {
    let start = body.find('[').ok_or_else(|| {
        RuntimeError::ProgramExecution(format!("Malformed local access '{}'", body))
    })?;
    let end = body.find(']').ok_or_else(|| {
        RuntimeError::ProgramExecution(format!("Malformed local access '{}'", body))
    })?;
    body[start + 1..end]
        .trim()
        .parse::<usize>()
        .map_err(|_| RuntimeError::ProgramExecution(format!("Malformed local index in '{}'", body)))
}

fn parse_paren_u64(body: &str) -> RuntimeResult<u64> {
    let start = body
        .find('(')
        .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed literal '{}'", body)))?;
    let end = body
        .find(')')
        .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed literal '{}'", body)))?;
    body[start + 1..end]
        .trim()
        .parse::<u64>()
        .map_err(|_| RuntimeError::ProgramExecution(format!("Malformed u64 literal in '{}'", body)))
}

fn parse_paren_usize(body: &str) -> RuntimeResult<usize> {
    let start = body
        .find('(')
        .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed jump '{}'", body)))?;
    let end = body
        .find(')')
        .ok_or_else(|| RuntimeError::ProgramExecution(format!("Malformed jump '{}'", body)))?;
    body[start + 1..end]
        .trim()
        .parse::<usize>()
        .map_err(|_| RuntimeError::ProgramExecution(format!("Malformed jump target in '{}'", body)))
}

fn parse_field_name(body: &str) -> RuntimeResult<String> {
    let start = body.find('(').ok_or_else(|| {
        RuntimeError::ProgramExecution(format!("Malformed field borrow '{}'", body))
    })?;
    let end = body.rfind(':').ok_or_else(|| {
        RuntimeError::ProgramExecution(format!("Malformed field borrow '{}'", body))
    })?;
    let field = body[start + 1..end]
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim();
    if field.is_empty() {
        return Err(RuntimeError::ProgramExecution(format!(
            "Missing field name in '{}'",
            body
        )));
    }
    Ok(field.to_string())
}

fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut angle_depth = 0usize;
    let mut paren_depth = 0usize;

    for ch in text.chars() {
        match ch {
            '<' => {
                angle_depth += 1;
                buf.push(ch);
            }
            '>' => {
                angle_depth = angle_depth.saturating_sub(1);
                buf.push(ch);
            }
            '(' => {
                paren_depth += 1;
                buf.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                buf.push(ch);
            }
            _ if ch == sep && angle_depth == 0 && paren_depth == 0 => {
                let part = buf.trim();
                if !part.is_empty() {
                    out.push(part.to_string());
                }
                buf.clear();
            }
            _ => buf.push(ch),
        }
    }

    let tail = buf.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn extract_generic_type(function: &str) -> Option<String> {
    let start = function.find('<')?;
    let end = function[start + 1..].find('>')?;
    Some(function[start + 1..start + 1 + end].to_string())
}

fn normalize_vm_object_type(param_type: &str) -> &str {
    let trimmed = param_type.trim();
    let trimmed = trimmed.strip_prefix("&mut ").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix('&').unwrap_or(trimmed);
    let trimmed = trimmed.trim();

    if trimmed.is_empty()
        || trimmed == "u64"
        || trimmed == "bool"
        || trimmed == "address"
        || trimmed.starts_with("Coin<")
        || trimmed.starts_with("TreasuryCap<")
        || trimmed == "TxContext"
    {
        return "";
    }

    trimmed
}

fn serialize_vm_object(object: &SuiVmStoredObject) -> RuntimeResult<Vec<u8>> {
    serde_json::to_vec(object).map_err(|e| {
        RuntimeError::ProgramExecution(format!("Failed to serialize VM object state: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::state::InMemoryStateStore;
    use crate::vm_object::{SuiVmStoredObject, SuiVmStoredValue};

    const DISASSEMBLY: &str = r#"
entry public mint(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, amount#0#0: u64, recipient#0#0: address, ctx#0#0: &mut TxContext) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](amount#0#0: u64)
	2: MoveLoc[3](ctx#0#0: &mut TxContext)
	3: Call coin::mint<MY_COIN>(&mut TreasuryCap<MY_COIN>, u64, &mut TxContext): Coin<MY_COIN>
	4: MoveLoc[2](recipient#0#0: address)
	5: Call transfer::public_transfer<Coin<MY_COIN>>(Coin<MY_COIN>, address)
	6: Ret
}

entry public conditional_transfer(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, amount#0#0: u64, recipient#0#0: address, should_transfer#0#0: bool, ctx#0#0: &mut TxContext) {
B0:
	0: MoveLoc[3](should_transfer#0#0: bool)
	1: BrFalse(9)
B1:
	2: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	3: MoveLoc[1](amount#0#0: u64)
	4: MoveLoc[4](ctx#0#0: &mut TxContext)
	5: Call coin::mint<MY_COIN>(&mut TreasuryCap<MY_COIN>, u64, &mut TxContext): Coin<MY_COIN>
	6: MoveLoc[2](recipient#0#0: address)
	7: Call transfer::public_transfer<Coin<MY_COIN>>(Coin<MY_COIN>, address)
	8: Branch(13)
B2:
	9: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	10: Pop
	11: MoveLoc[4](ctx#0#0: &mut TxContext)
	12: Pop
B3:
	13: Ret
}

entry public burn(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, coin#0#0: Coin<MY_COIN>) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](coin#0#0: Coin<MY_COIN>)
	2: Call coin::burn<MY_COIN>(&mut TreasuryCap<MY_COIN>, Coin<MY_COIN>): u64
	3: Pop
	4: Ret
}
"#;

    const HELPER_CALL_DISASSEMBLY: &str = r#"
mint_to(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, amount#0#0: u64, recipient#0#0: address, ctx#0#0: &mut TxContext) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](amount#0#0: u64)
	2: MoveLoc[3](ctx#0#0: &mut TxContext)
	3: Call coin::mint<MY_COIN>(&mut TreasuryCap<MY_COIN>, u64, &mut TxContext): Coin<MY_COIN>
	4: MoveLoc[2](recipient#0#0: address)
	5: Call transfer::public_transfer<Coin<MY_COIN>>(Coin<MY_COIN>, address)
	6: Ret
}

maybe_mint_to(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, amount#0#0: u64, recipient#0#0: address, should_transfer#0#0: bool, ctx#0#0: &mut TxContext) {
B0:
	0: MoveLoc[3](should_transfer#0#0: bool)
	1: BrFalse(8)
B1:
	2: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	3: MoveLoc[1](amount#0#0: u64)
	4: MoveLoc[2](recipient#0#0: address)
	5: MoveLoc[4](ctx#0#0: &mut TxContext)
	6: Call mint_to(&mut TreasuryCap<MY_COIN>, u64, address, &mut TxContext)
	7: Branch(12)
B2:
	8: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	9: Pop
	10: MoveLoc[4](ctx#0#0: &mut TxContext)
	11: Pop
B3:
	12: Ret
}

entry public complex_flow(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, amount#0#0: u64, recipient#0#0: address, should_transfer#0#0: bool, ctx#0#0: &mut TxContext) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](amount#0#0: u64)
	2: MoveLoc[2](recipient#0#0: address)
	3: MoveLoc[3](should_transfer#0#0: bool)
	4: MoveLoc[4](ctx#0#0: &mut TxContext)
	5: Call maybe_mint_to(&mut TreasuryCap<MY_COIN>, u64, address, bool, &mut TxContext)
	6: Ret
}

burn_inner(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, coin#0#0: Coin<MY_COIN>) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](coin#0#0: Coin<MY_COIN>)
	2: Call coin::burn<MY_COIN>(&mut TreasuryCap<MY_COIN>, Coin<MY_COIN>): u64
	3: Pop
	4: Ret
}

entry public burn_via_helper(treasury_cap#0#0: &mut TreasuryCap<MY_COIN>, coin#0#0: Coin<MY_COIN>) {
B0:
	0: MoveLoc[0](treasury_cap#0#0: &mut TreasuryCap<MY_COIN>)
	1: MoveLoc[1](coin#0#0: Coin<MY_COIN>)
	2: Call burn_inner(&mut TreasuryCap<MY_COIN>, Coin<MY_COIN>)
	3: Ret
}
"#;

    const COUNTER_DISASSEMBLY: &str = r#"
entry increment(counter#0#0: &mut Counter) {
B0:
	0: CopyLoc[0](counter#0#0: &mut Counter)
	1: ImmBorrowField[0](Counter.value: u64)
	2: ReadRef
	3: LdU64(1)
	4: Add
	5: MoveLoc[0](counter#0#0: &mut Counter)
	6: MutBorrowField[0](Counter.value: u64)
	7: WriteRef
	8: Ret
}
"#;

    #[test]
    fn test_execute_conditional_transfer_subset() {
        let mut state = InMemoryStateStore::new();
        let alice = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            DISASSEMBLY,
            "mint",
            &[
                SuiVmArg::Opaque,
                SuiVmArg::U64(100),
                SuiVmArg::Address(alice),
                SuiVmArg::Opaque,
            ],
        )
        .unwrap();

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            DISASSEMBLY,
            "conditional_transfer",
            &[
                SuiVmArg::Opaque,
                SuiVmArg::U64(40),
                SuiVmArg::Address(bob),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
        )
        .unwrap();

        let bob_coin_id = deterministic_coin_id(&bob, "MY_COIN");
        let bob_coin = state.get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin.data.balance.value(), 40);

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            DISASSEMBLY,
            "conditional_transfer",
            &[
                SuiVmArg::Opaque,
                SuiVmArg::U64(55),
                SuiVmArg::Address(bob),
                SuiVmArg::Bool(false),
                SuiVmArg::Opaque,
            ],
        )
        .unwrap();

        let bob_coin_after = state.get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin_after.data.balance.value(), 40);

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            DISASSEMBLY,
            "burn",
            &[SuiVmArg::Opaque, SuiVmArg::ObjectId(bob_coin_id)],
        )
        .unwrap();

        assert!(state.get_object(&bob_coin_id).unwrap().is_none());
    }

    #[test]
    fn test_execute_user_defined_helper_calls() {
        let mut state = InMemoryStateStore::new();
        let alice = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            HELPER_CALL_DISASSEMBLY,
            "complex_flow",
            &[
                SuiVmArg::Opaque,
                SuiVmArg::U64(75),
                SuiVmArg::Address(bob),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
        )
        .unwrap();

        let bob_coin_id = deterministic_coin_id(&bob, "MY_COIN");
        let bob_coin = state.get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin.data.balance.value(), 75);

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            HELPER_CALL_DISASSEMBLY,
            "complex_flow",
            &[
                SuiVmArg::Opaque,
                SuiVmArg::U64(25),
                SuiVmArg::Address(bob),
                SuiVmArg::Bool(false),
                SuiVmArg::Opaque,
            ],
        )
        .unwrap();

        let bob_coin_after = state.get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin_after.data.balance.value(), 75);

        execute_sui_entry_from_disassembly(
            &mut state,
            &alice,
            HELPER_CALL_DISASSEMBLY,
            "burn_via_helper",
            &[SuiVmArg::Opaque, SuiVmArg::ObjectId(bob_coin_id)],
        )
        .unwrap();

        assert!(state.get_object(&bob_coin_id).unwrap().is_none());
    }

    #[test]
    fn test_increment_persisted_counter_object() {
        let mut state = InMemoryStateStore::new();
        let alice = Address::from_str_id("alice");
        let counter_id = ObjectId::new([0x44; 32]);

        state
            .set_vm_object(
                counter_id,
                SuiVmStoredObject::new_owned(
                    counter_id,
                    "Counter",
                    alice,
                    BTreeMap::from([("value".to_string(), SuiVmStoredValue::U64(41))]),
                ),
            )
            .unwrap();

        execute_sui_entry_with_outcome(
            &mut state,
            &alice,
            COUNTER_DISASSEMBLY,
            "increment",
            &[SuiVmArg::ObjectId(counter_id)],
        )
        .unwrap();

        let counter = state.get_vm_object(&counter_id).unwrap().unwrap();
        assert_eq!(counter.get_u64_field("value"), Some(42));
        assert_eq!(counter.version, 2);
    }
}
