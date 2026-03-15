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
    BrFalse(usize),
    BrTrue(usize),
    Branch(usize),
    Call { function: String, arg_count: usize },
    Pop,
    Ret,
}

#[derive(Debug, Clone)]
struct ParsedFunction {
    param_types: Vec<String>,
    instructions: Vec<(usize, SuiOpcode)>,
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
    let function = parse_entry_function(disassembly, function_name)?;
    let mut vm = SuiDisasmVm::new(state, sender, function, args)?;
    vm.run()
}

struct SuiDisasmVm<'a, S: StateStore> {
    state: &'a mut S,
    sender: &'a Address,
    instructions: Vec<(usize, SuiOpcode)>,
    instruction_pc: HashMap<usize, usize>,
    locals: Vec<Option<SuiVmValue>>,
    stack: Vec<SuiVmValue>,
    temp_counter: u64,
    write_order: Vec<ObjectId>,
    old_states: HashMap<ObjectId, Option<Vec<u8>>>,
    final_states: HashMap<ObjectId, Option<Vec<u8>>>,
}

impl<'a, S: StateStore> SuiDisasmVm<'a, S> {
    fn new(
        state: &'a mut S,
        sender: &'a Address,
        function: ParsedFunction,
        args: &[SuiVmArg],
    ) -> RuntimeResult<Self> {
        if args.len() != function.param_types.len() {
            return Err(RuntimeError::ProgramExecution(format!(
                "Arg count mismatch: expected {}, got {}",
                function.param_types.len(),
                args.len()
            )));
        }

        let max_local = function
            .instructions
            .iter()
            .filter_map(|(_, op)| match op {
                SuiOpcode::MoveLoc(idx) | SuiOpcode::CopyLoc(idx) | SuiOpcode::StLoc(idx) => {
                    Some(*idx)
                }
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let locals_count = usize::max(function.param_types.len(), max_local + 1);
        let mut locals = vec![None; locals_count];

        for (idx, (arg, param_ty)) in args.iter().zip(function.param_types.iter()).enumerate() {
            locals[idx] = Some(Self::coerce_arg(state, arg, param_ty)?);
        }

        let mut instruction_pc = HashMap::new();
        for (pc, (inst_idx, _)) in function.instructions.iter().enumerate() {
            instruction_pc.insert(*inst_idx, pc);
        }

        Ok(Self {
            state,
            sender,
            instructions: function.instructions,
            instruction_pc,
            locals,
            stack: Vec::new(),
            temp_counter: 1,
            write_order: Vec::new(),
            old_states: HashMap::new(),
            final_states: HashMap::new(),
        })
    }

    fn run(&mut self) -> RuntimeResult<SuiVmExecutionOutcome> {
        let mut pc = 0usize;
        let step_limit = 100_000usize;
        let mut steps = 0usize;

        while pc < self.instructions.len() {
            if steps >= step_limit {
                return Err(RuntimeError::ProgramExecution(
                    "Sui VM step limit exceeded".to_string(),
                ));
            }
            steps += 1;

            let (_, op) = self.instructions[pc].clone();
            pc += 1;

            match op {
                SuiOpcode::MoveLoc(idx) => {
                    let slot = self.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    let v = slot.take().ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Local {} is uninitialized", idx))
                    })?;
                    self.stack.push(v);
                }
                SuiOpcode::CopyLoc(idx) => {
                    let v = self.local_get(idx)?.clone();
                    self.stack.push(v);
                }
                SuiOpcode::StLoc(idx) => {
                    let v = self.pop()?;
                    let slot = self.locals.get_mut(idx).ok_or_else(|| {
                        RuntimeError::ProgramExecution(format!("Invalid local index {}", idx))
                    })?;
                    *slot = Some(v);
                }
                SuiOpcode::LdU64(v) => self.stack.push(SuiVmValue::U64(v)),
                SuiOpcode::LdU8(v) => self.stack.push(SuiVmValue::U64(v as u64)),
                SuiOpcode::LdTrue => self.stack.push(SuiVmValue::Bool(true)),
                SuiOpcode::LdFalse => self.stack.push(SuiVmValue::Bool(false)),
                SuiOpcode::BrFalse(target) => {
                    if !self.pop_bool()? {
                        pc = self.jump_target(target)?;
                    }
                }
                SuiOpcode::BrTrue(target) => {
                    if self.pop_bool()? {
                        pc = self.jump_target(target)?;
                    }
                }
                SuiOpcode::Branch(target) => {
                    pc = self.jump_target(target)?;
                }
                SuiOpcode::Call {
                    function,
                    arg_count,
                } => {
                    self.execute_call(&function, arg_count)?;
                }
                SuiOpcode::Pop => {
                    let _ = self.pop()?;
                }
                SuiOpcode::Ret => return self.finish(),
            }
        }

        Err(RuntimeError::ProgramExecution(
            "Function terminated without Ret".to_string(),
        ))
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

        Ok(SuiVmValue::Opaque)
    }

    fn jump_target(&self, target: usize) -> RuntimeResult<usize> {
        self.instruction_pc.get(&target).copied().ok_or_else(|| {
            RuntimeError::ProgramExecution(format!("Invalid branch target {}", target))
        })
    }

    fn execute_call(&mut self, function: &str, arg_count: usize) -> RuntimeResult<()> {
        if self.stack.len() < arg_count {
            return Err(RuntimeError::ProgramExecution(format!(
                "Call {} expects {} args, stack has {}",
                function,
                arg_count,
                self.stack.len()
            )));
        }

        let mut args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            args.push(self.pop()?);
        }
        args.reverse();

        if function.starts_with("coin::mint<") {
            self.call_coin_mint(function, &args)
        } else if function.starts_with("transfer::public_transfer<Coin<") {
            self.call_public_transfer(&args)
        } else if function.starts_with("coin::burn<") {
            self.call_coin_burn(&args)
        } else {
            Err(RuntimeError::ProgramExecution(format!(
                "Unsupported Sui native call '{}'",
                function
            )))
        }
    }

    fn call_coin_mint(&mut self, function: &str, args: &[SuiVmValue]) -> RuntimeResult<()> {
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
        self.stack.push(SuiVmValue::Coin(coin));
        Ok(())
    }

    fn call_public_transfer(&mut self, args: &[SuiVmValue]) -> RuntimeResult<()> {
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

        Ok(())
    }

    fn call_coin_burn(&mut self, args: &[SuiVmValue]) -> RuntimeResult<()> {
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
        self.stack.push(SuiVmValue::U64(0));
        Ok(())
    }

    fn next_temp_object_id(&mut self) -> ObjectId {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&self.temp_counter.to_le_bytes());
        bytes[8..].copy_from_slice(&self.sender.as_bytes()[..24]);
        self.temp_counter += 1;
        ObjectId::new(bytes)
    }

    fn local_get(&self, idx: usize) -> RuntimeResult<&SuiVmValue> {
        self.locals
            .get(idx)
            .ok_or_else(|| RuntimeError::ProgramExecution(format!("Invalid local index {}", idx)))?
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::ProgramExecution(format!("Local {} is uninitialized", idx))
            })
    }

    fn pop(&mut self) -> RuntimeResult<SuiVmValue> {
        self.stack
            .pop()
            .ok_or_else(|| RuntimeError::ProgramExecution("Stack underflow".to_string()))
    }

    fn pop_bool(&mut self) -> RuntimeResult<bool> {
        match self.pop()? {
            SuiVmValue::Bool(v) => Ok(v),
            other => Err(RuntimeError::ProgramExecution(format!(
                "Expected bool, got {:?}",
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

    fn delete_object_tracked(&mut self, object_id: &ObjectId) -> RuntimeResult<()> {
        self.mark_touched(*object_id)?;
        self.state.delete_object(object_id)?;
        self.final_states.insert(*object_id, None);
        Ok(())
    }
}

fn parse_entry_function(disassembly: &str, function_name: &str) -> RuntimeResult<ParsedFunction> {
    let lines: Vec<&str> = disassembly.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if is_function_header(line) {
            let parsed_name = parse_function_name(line)?;
            if parsed_name == function_name {
                let param_types = parse_param_types(line)?;
                let mut instructions = Vec::new();
                i += 1;
                while i < lines.len() {
                    let cur = lines[i].trim();
                    if cur == "}" {
                        return Ok(ParsedFunction {
                            param_types,
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
    let lower = line.to_ascii_lowercase();
    lower.starts_with("entry ") || lower.starts_with("public ") || line.starts_with("init(")
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
    if body.starts_with("BrFalse(") {
        return Ok(Some((idx, SuiOpcode::BrFalse(parse_paren_usize(body)?))));
    }
    if body.starts_with("BrTrue(") {
        return Ok(Some((idx, SuiOpcode::BrTrue(parse_paren_usize(body)?))));
    }
    if body.starts_with("Branch(") {
        return Ok(Some((idx, SuiOpcode::Branch(parse_paren_usize(body)?))));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InMemoryStateStore;

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
}
