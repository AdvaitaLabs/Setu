//! Runtime executor - Simple State Transition Executor

use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::transaction::{
    Bytecode, MoveScriptTx, MoveValue, QueryTx, QueryType, SignatureToken, Transaction,
    TransactionType, TransferTx,
};
use serde::{Deserialize, Serialize};
use setu_types::{create_typed_coin, Address, CoinType, ObjectId};
use std::collections::{HashMap, VecDeque};
use tracing::{debug, info, warn};

/// Execution context
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Executor (usually the solver)
    pub executor_id: String,
    /// Execution timestamp
    pub timestamp: u64,
    /// Whether executed in TEE (future implementation)
    pub in_tee: bool,
}

/// Execution output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutput {
    /// Whether the execution was successful
    pub success: bool,
    /// Execution message
    pub message: Option<String>,
    /// List of state changes
    pub state_changes: Vec<StateChange>,
    /// Newly created objects (if any)
    pub created_objects: Vec<ObjectId>,
    /// Deleted objects (if any)
    pub deleted_objects: Vec<ObjectId>,
    /// Query result (for read-only queries)
    pub query_result: Option<serde_json::Value>,
}

/// State change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    /// Change type
    pub change_type: StateChangeType,
    /// Object ID
    pub object_id: ObjectId,
    /// Old state (serialized object data)
    pub old_state: Option<Vec<u8>>,
    /// New state (serialized object data)
    pub new_state: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StateChangeType {
    /// Object creation
    Create,
    /// Object modification
    Update,
    /// Object deletion
    Delete,
}

/// Runtime executor
pub struct RuntimeExecutor<S: StateStore> {
    /// State storage
    state: S,
}

impl<S: StateStore> RuntimeExecutor<S> {
    /// 创建新的执行器
    pub fn new(state: S) -> Self {
        Self { state }
    }

    /// 执行交易
    ///
    /// 这是主要的执行入口，会根据交易类型调用对应的处理函数
    pub fn execute_transaction(
        &mut self,
        tx: &Transaction,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        info!(
            tx_id = %tx.id,
            sender = %tx.sender,
            executor = %ctx.executor_id,
            "Executing transaction"
        );

        let result = match &tx.tx_type {
            TransactionType::Transfer(transfer_tx) => self.execute_transfer(tx, transfer_tx, ctx),
            TransactionType::Query(query_tx) => self.execute_query(tx, query_tx, ctx),
            TransactionType::MoveScript(script_tx) => self.execute_move_script(tx, script_tx, ctx),
        };

        match &result {
            Ok(output) => {
                info!(
                    tx_id = %tx.id,
                    success = output.success,
                    changes = output.state_changes.len(),
                    "Transaction execution completed"
                );
            }
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "Transaction execution failed"
                );
            }
        }

        result
    }

    /// 执行转账交易
    fn execute_transfer(
        &mut self,
        tx: &Transaction,
        transfer_tx: &TransferTx,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let coin_id = transfer_tx.coin_id;
        let recipient = &transfer_tx.recipient;

        // 1. 读取 Coin 对象
        let mut coin = self
            .state
            .get_object(&coin_id)?
            .ok_or(RuntimeError::ObjectNotFound(coin_id))?;

        // 2. 验证所有权
        let owner = coin
            .metadata
            .owner
            .as_ref()
            .ok_or(RuntimeError::InvalidOwnership {
                object_id: coin_id,
                address: tx.sender.to_string(),
            })?;

        if owner != &tx.sender {
            return Err(RuntimeError::InvalidOwnership {
                object_id: coin_id,
                address: tx.sender.to_string(),
            });
        }

        // 记录旧状态
        let old_state = serde_json::to_vec(&coin)?;

        let mut state_changes = Vec::new();
        let mut created_objects = Vec::new();
        let deleted_objects = Vec::new();

        // 3. 执行转账逻辑
        match transfer_tx.amount {
            // 完整转账：直接转移对象所有权
            None => {
                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = coin.data.balance.value(),
                    "Full transfer"
                );

                // 更改所有者
                coin.metadata.owner = Some(recipient.clone());
                coin.metadata.version += 1;

                let new_state = serde_json::to_vec(&coin)?;

                // 保存更新后的对象
                self.state.set_object(coin_id, coin)?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });
            }

            // 部分转账：需要分割 Coin
            Some(amount) => {
                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = amount,
                    remaining = coin.data.balance.value() - amount,
                    "Partial transfer (split)"
                );

                // 从原 Coin 中提取金额
                let transferred_balance = coin
                    .data
                    .balance
                    .withdraw(amount)
                    .map_err(|e| RuntimeError::InvalidTransaction(e))?;

                // 更新原 Coin
                coin.metadata.version += 1;
                let new_state = serde_json::to_vec(&coin)?;
                self.state.set_object(coin_id, coin.clone())?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });

                // 创建新 Coin 给接收者
                let new_coin = create_typed_coin(
                    recipient.clone(),
                    transferred_balance.value(),
                    coin.data.coin_type.as_str(),
                );
                let new_coin_id = *new_coin.id();
                let new_coin_state = serde_json::to_vec(&new_coin)?;

                self.state.set_object(new_coin_id, new_coin)?;

                created_objects.push(new_coin_id);
                state_changes.push(StateChange {
                    change_type: StateChangeType::Create,
                    object_id: new_coin_id,
                    old_state: None,
                    new_state: Some(new_coin_state),
                });
            }
        }

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!(
                "Transfer completed: {} -> {}",
                tx.sender, recipient
            )),
            state_changes,
            created_objects,
            deleted_objects,
            query_result: None,
        })
    }

    /// 执行查询交易（只读）
    fn execute_query(
        &self,
        _tx: &Transaction,
        query_tx: &QueryTx,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let result = match query_tx.query_type {
            QueryType::Balance => {
                let address: Address = serde_json::from_value(
                    query_tx
                        .params
                        .get("address")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'address' parameter".to_string(),
                        ))?
                        .clone(),
                )?;

                let owned_objects = self.state.get_owned_objects(&address)?;
                let mut total_balance: HashMap<CoinType, u64> = HashMap::new();

                for obj_id in owned_objects {
                    if let Some(coin) = self.state.get_object(&obj_id)? {
                        *total_balance
                            .entry(coin.data.coin_type.clone())
                            .or_insert(0) += coin.data.balance.value();
                    }
                }

                serde_json::to_value(&total_balance)?
            }

            QueryType::Object => {
                let object_id: ObjectId = serde_json::from_value(
                    query_tx
                        .params
                        .get("object_id")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'object_id' parameter".to_string(),
                        ))?
                        .clone(),
                )?;

                let object = self.state.get_object(&object_id)?;
                serde_json::to_value(&object)?
            }

            QueryType::OwnedObjects => {
                let address: Address = serde_json::from_value(
                    query_tx
                        .params
                        .get("address")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'address' parameter".to_string(),
                        ))?
                        .clone(),
                )?;

                let owned_objects = self.state.get_owned_objects(&address)?;
                serde_json::to_value(&owned_objects)?
            }
        };

        Ok(ExecutionOutput {
            success: true,
            message: Some("Query executed successfully".to_string()),
            state_changes: vec![],
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: Some(result),
        })
    }

    /// Execute Move-style script (typed stack + locals).
    fn execute_move_script(
        &mut self,
        _tx: &Transaction,
        script_tx: &MoveScriptTx,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        const MAX_CODE_LENGTH: usize = 4_096;
        const MAX_LOCALS: usize = 256;
        const MAX_STACK_DEPTH: usize = 1_024;
        const MAX_STEPS: usize = 100_000;

        self.verify_move_script(script_tx, MAX_CODE_LENGTH, MAX_LOCALS)?;

        let mut gas_remaining = script_tx.max_gas;
        let mut steps = 0usize;
        let mut pc = 0usize;
        let mut stack: Vec<MoveValue> = Vec::new();
        let mut locals: Vec<Option<MoveValue>> = vec![None; script_tx.locals_sig.len()];

        for (i, arg) in script_tx.args.iter().enumerate() {
            locals[i] = Some(arg.clone());
        }

        loop {
            if pc >= script_tx.code.len() {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Program counter out of range: {}",
                    pc
                )));
            }

            if steps >= MAX_STEPS {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Step limit exceeded at pc={}",
                    pc
                )));
            }
            steps += 1;

            let op = &script_tx.code[pc];
            let gas_cost = Self::opcode_gas_cost(op);
            if gas_remaining < gas_cost {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Out of gas at pc={} (required={}, remaining={})",
                    pc, gas_cost, gas_remaining
                )));
            }
            gas_remaining -= gas_cost;

            match op {
                Bytecode::LdU64(v) => {
                    stack.push(MoveValue::U64(*v));
                    pc += 1;
                }
                Bytecode::LdTrue => {
                    stack.push(MoveValue::Bool(true));
                    pc += 1;
                }
                Bytecode::LdFalse => {
                    stack.push(MoveValue::Bool(false));
                    pc += 1;
                }
                Bytecode::CopyLoc(local_idx) => {
                    let idx = *local_idx as usize;
                    let val = locals[idx].clone().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "CopyLoc on uninitialized local {} at pc={}",
                            idx, pc
                        ))
                    })?;
                    stack.push(val);
                    pc += 1;
                }
                Bytecode::MoveLoc(local_idx) => {
                    let idx = *local_idx as usize;
                    let val = locals[idx].take().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "MoveLoc on uninitialized local {} at pc={}",
                            idx, pc
                        ))
                    })?;
                    stack.push(val);
                    pc += 1;
                }
                Bytecode::StLoc(local_idx) => {
                    let idx = *local_idx as usize;
                    let val = Self::pop_value(&mut stack, pc)?;
                    let expected_ty = &script_tx.locals_sig[idx];
                    Self::ensure_value_type(&val, expected_ty, pc)?;
                    locals[idx] = Some(val);
                    pc += 1;
                }
                Bytecode::Pop => {
                    let _ = Self::pop_value(&mut stack, pc)?;
                    pc += 1;
                }
                Bytecode::Add => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    let result = lhs.checked_add(rhs).ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Arithmetic overflow at pc={}",
                            pc
                        ))
                    })?;
                    stack.push(MoveValue::U64(result));
                    pc += 1;
                }
                Bytecode::Sub => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    let result = lhs.checked_sub(rhs).ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Arithmetic underflow at pc={}",
                            pc
                        ))
                    })?;
                    stack.push(MoveValue::U64(result));
                    pc += 1;
                }
                Bytecode::Mul => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    let result = lhs.checked_mul(rhs).ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Arithmetic overflow at pc={}",
                            pc
                        ))
                    })?;
                    stack.push(MoveValue::U64(result));
                    pc += 1;
                }
                Bytecode::Div => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    if rhs == 0 {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Division by zero at pc={}",
                            pc
                        )));
                    }
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::U64(lhs / rhs));
                    pc += 1;
                }
                Bytecode::Mod => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    if rhs == 0 {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Modulo by zero at pc={}",
                            pc
                        )));
                    }
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::U64(lhs % rhs));
                    pc += 1;
                }
                Bytecode::Eq => {
                    let rhs = Self::pop_value(&mut stack, pc)?;
                    let lhs = Self::pop_value(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs == rhs));
                    pc += 1;
                }
                Bytecode::Neq => {
                    let rhs = Self::pop_value(&mut stack, pc)?;
                    let lhs = Self::pop_value(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs != rhs));
                    pc += 1;
                }
                Bytecode::Lt => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs < rhs));
                    pc += 1;
                }
                Bytecode::Le => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs <= rhs));
                    pc += 1;
                }
                Bytecode::Gt => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs > rhs));
                    pc += 1;
                }
                Bytecode::Ge => {
                    let rhs = Self::pop_u64(&mut stack, pc)?;
                    let lhs = Self::pop_u64(&mut stack, pc)?;
                    stack.push(MoveValue::Bool(lhs >= rhs));
                    pc += 1;
                }
                Bytecode::BrTrue(target) => {
                    let cond = Self::pop_bool(&mut stack, pc)?;
                    if cond {
                        pc = *target as usize;
                    } else {
                        pc += 1;
                    }
                }
                Bytecode::BrFalse(target) => {
                    let cond = Self::pop_bool(&mut stack, pc)?;
                    if !cond {
                        pc = *target as usize;
                    } else {
                        pc += 1;
                    }
                }
                Bytecode::Branch(target) => {
                    pc = *target as usize;
                }
                Bytecode::Ret => {
                    if stack.len() != script_tx.return_sig.len() {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Return stack length mismatch at pc={}: expected {}, got {}",
                            pc,
                            script_tx.return_sig.len(),
                            stack.len()
                        )));
                    }
                    for (value, ty) in stack.iter().zip(script_tx.return_sig.iter()) {
                        Self::ensure_value_type(value, ty, pc)?;
                    }

                    let return_values_json: Vec<serde_json::Value> =
                        stack.into_iter().map(Self::move_value_to_json).collect();
                    let query_result = serde_json::json!({
                        "returns": return_values_json,
                        "gas_used": script_tx.max_gas.saturating_sub(gas_remaining),
                    });

                    return Ok(ExecutionOutput {
                        success: true,
                        message: Some(format!("Move script executed in {} steps", steps)),
                        state_changes: vec![],
                        created_objects: vec![],
                        deleted_objects: vec![],
                        query_result: Some(query_result),
                    });
                }
                Bytecode::Abort { code, message } => {
                    return Ok(ExecutionOutput {
                        success: false,
                        message: Some(match message {
                            Some(m) => format!("Abort({}): {}", code, m),
                            None => format!("Abort({})", code),
                        }),
                        state_changes: vec![],
                        created_objects: vec![],
                        deleted_objects: vec![],
                        query_result: Some(serde_json::json!({
                            "abort_code": code,
                            "gas_used": script_tx.max_gas.saturating_sub(gas_remaining),
                        })),
                    });
                }
            }

            if stack.len() > MAX_STACK_DEPTH {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Stack depth exceeded limit {} at pc={}",
                    MAX_STACK_DEPTH, pc
                )));
            }
        }
    }

    fn verify_move_script(
        &self,
        script_tx: &MoveScriptTx,
        max_code_len: usize,
        max_locals: usize,
    ) -> RuntimeResult<()> {
        if script_tx.code.is_empty() {
            return Err(RuntimeError::InvalidTransaction(
                "Move script has empty code".to_string(),
            ));
        }
        if script_tx.code.len() > max_code_len {
            return Err(RuntimeError::InvalidTransaction(format!(
                "Move script too large: {} > {} instructions",
                script_tx.code.len(),
                max_code_len
            )));
        }
        if script_tx.locals_sig.len() > max_locals {
            return Err(RuntimeError::InvalidTransaction(format!(
                "Too many locals: {} > {}",
                script_tx.locals_sig.len(),
                max_locals
            )));
        }
        if script_tx.params_sig.len() > script_tx.locals_sig.len() {
            return Err(RuntimeError::InvalidTransaction(format!(
                "params_sig longer than locals_sig: {} > {}",
                script_tx.params_sig.len(),
                script_tx.locals_sig.len()
            )));
        }
        if script_tx.args.len() != script_tx.params_sig.len() {
            return Err(RuntimeError::InvalidTransaction(format!(
                "Argument length mismatch: expected {}, got {}",
                script_tx.params_sig.len(),
                script_tx.args.len()
            )));
        }
        if script_tx.max_gas == 0 {
            return Err(RuntimeError::InvalidTransaction(
                "max_gas must be greater than zero".to_string(),
            ));
        }

        for (arg, sig) in script_tx.args.iter().zip(script_tx.params_sig.iter()) {
            Self::ensure_value_matches_sig(arg, sig)?;
        }

        self.verify_control_flow_and_types(script_tx)
    }

    fn verify_control_flow_and_types(&self, script_tx: &MoveScriptTx) -> RuntimeResult<()> {
        let mut queue: VecDeque<(usize, Vec<SignatureToken>)> = VecDeque::new();
        let mut seen: HashMap<usize, Vec<SignatureToken>> = HashMap::new();
        let mut has_terminal = false;
        queue.push_back((0, Vec::new()));

        while let Some((pc, stack_state)) = queue.pop_front() {
            if pc >= script_tx.code.len() {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Invalid pc {} during verification",
                    pc
                )));
            }

            if let Some(existing) = seen.get(&pc) {
                if existing != &stack_state {
                    return Err(RuntimeError::InvalidTransaction(format!(
                        "Incompatible stack state at join point pc={}",
                        pc
                    )));
                }
                continue;
            }
            seen.insert(pc, stack_state.clone());

            let mut stack = stack_state;
            let op = &script_tx.code[pc];
            let mut push_succ =
                |next_pc: usize, next_stack: Vec<SignatureToken>| -> RuntimeResult<()> {
                    if next_pc >= script_tx.code.len() {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Control flow exits code range at pc={}",
                            pc
                        )));
                    }
                    queue.push_back((next_pc, next_stack));
                    Ok(())
                };

            match op {
                Bytecode::LdU64(_) => {
                    stack.push(SignatureToken::U64);
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::LdTrue | Bytecode::LdFalse => {
                    stack.push(SignatureToken::Bool);
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::CopyLoc(idx) | Bytecode::MoveLoc(idx) => {
                    let local_idx = *idx as usize;
                    let ty = script_tx.locals_sig.get(local_idx).ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Local index out of bounds {} at pc={}",
                            local_idx, pc
                        ))
                    })?;
                    stack.push(ty.clone());
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::StLoc(idx) => {
                    let local_idx = *idx as usize;
                    let ty = script_tx.locals_sig.get(local_idx).ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Local index out of bounds {} at pc={}",
                            local_idx, pc
                        ))
                    })?;
                    let top = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Stack underflow at StLoc pc={}",
                            pc
                        ))
                    })?;
                    if &top != ty {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Type mismatch at StLoc pc={}",
                            pc
                        )));
                    }
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::Pop => {
                    let _ = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Stack underflow at Pop pc={}",
                            pc
                        ))
                    })?;
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::Add | Bytecode::Sub | Bytecode::Mul | Bytecode::Div | Bytecode::Mod => {
                    let rhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    let lhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    if rhs != SignatureToken::U64 || lhs != SignatureToken::U64 {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Arithmetic expects U64 at pc={}",
                            pc
                        )));
                    }
                    stack.push(SignatureToken::U64);
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::Eq | Bytecode::Neq => {
                    let rhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    let lhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    if rhs != lhs {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Eq/Neq operand type mismatch at pc={}",
                            pc
                        )));
                    }
                    stack.push(SignatureToken::Bool);
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::Lt | Bytecode::Le | Bytecode::Gt | Bytecode::Ge => {
                    let rhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    let lhs = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
                    })?;
                    if rhs != SignatureToken::U64 || lhs != SignatureToken::U64 {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Comparison expects U64 at pc={}",
                            pc
                        )));
                    }
                    stack.push(SignatureToken::Bool);
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::BrTrue(target) | Bytecode::BrFalse(target) => {
                    let cond = stack.pop().ok_or_else(|| {
                        RuntimeError::InvalidTransaction(format!(
                            "Stack underflow at branch pc={}",
                            pc
                        ))
                    })?;
                    if cond != SignatureToken::Bool {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Branch condition must be Bool at pc={}",
                            pc
                        )));
                    }
                    let target_pc = *target as usize;
                    if target_pc >= script_tx.code.len() {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Branch target {} out of bounds at pc={}",
                            target_pc, pc
                        )));
                    }
                    push_succ(target_pc, stack.clone())?;
                    push_succ(pc + 1, stack)?;
                }
                Bytecode::Branch(target) => {
                    let target_pc = *target as usize;
                    if target_pc >= script_tx.code.len() {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Branch target {} out of bounds at pc={}",
                            target_pc, pc
                        )));
                    }
                    push_succ(target_pc, stack)?;
                }
                Bytecode::Ret => {
                    if stack != script_tx.return_sig {
                        return Err(RuntimeError::InvalidTransaction(format!(
                            "Return signature mismatch at pc={}",
                            pc
                        )));
                    }
                    has_terminal = true;
                }
                Bytecode::Abort { .. } => {
                    has_terminal = true;
                }
            }
        }

        if !has_terminal {
            return Err(RuntimeError::InvalidTransaction(
                "Script has no reachable terminal instruction (Ret/Abort)".to_string(),
            ));
        }

        Ok(())
    }

    fn opcode_gas_cost(op: &Bytecode) -> u64 {
        match op {
            Bytecode::LdU64(_) | Bytecode::LdTrue | Bytecode::LdFalse => 1,
            Bytecode::CopyLoc(_) | Bytecode::MoveLoc(_) | Bytecode::StLoc(_) | Bytecode::Pop => 1,
            Bytecode::Add | Bytecode::Sub | Bytecode::Mul => 2,
            Bytecode::Div | Bytecode::Mod => 3,
            Bytecode::Eq
            | Bytecode::Neq
            | Bytecode::Lt
            | Bytecode::Le
            | Bytecode::Gt
            | Bytecode::Ge => 1,
            Bytecode::BrTrue(_) | Bytecode::BrFalse(_) | Bytecode::Branch(_) => 1,
            Bytecode::Ret => 1,
            Bytecode::Abort { .. } => 1,
        }
    }

    fn pop_value(stack: &mut Vec<MoveValue>, pc: usize) -> RuntimeResult<MoveValue> {
        stack.pop().ok_or_else(|| {
            RuntimeError::InvalidTransaction(format!("Stack underflow at pc={}", pc))
        })
    }

    fn pop_u64(stack: &mut Vec<MoveValue>, pc: usize) -> RuntimeResult<u64> {
        match Self::pop_value(stack, pc)? {
            MoveValue::U64(v) => Ok(v),
            other => Err(RuntimeError::InvalidTransaction(format!(
                "Type error at pc={}: expected U64, found {:?}",
                pc, other
            ))),
        }
    }

    fn pop_bool(stack: &mut Vec<MoveValue>, pc: usize) -> RuntimeResult<bool> {
        match Self::pop_value(stack, pc)? {
            MoveValue::Bool(v) => Ok(v),
            other => Err(RuntimeError::InvalidTransaction(format!(
                "Type error at pc={}: expected Bool, found {:?}",
                pc, other
            ))),
        }
    }

    fn ensure_value_type(
        value: &MoveValue,
        expected: &SignatureToken,
        pc: usize,
    ) -> RuntimeResult<()> {
        if Self::value_matches_sig(value, expected) {
            Ok(())
        } else {
            Err(RuntimeError::InvalidTransaction(format!(
                "Type mismatch at pc={}: value {:?} does not match {:?}",
                pc, value, expected
            )))
        }
    }

    fn ensure_value_matches_sig(value: &MoveValue, expected: &SignatureToken) -> RuntimeResult<()> {
        if Self::value_matches_sig(value, expected) {
            Ok(())
        } else {
            Err(RuntimeError::InvalidTransaction(format!(
                "Argument type mismatch: value {:?} does not match {:?}",
                value, expected
            )))
        }
    }

    fn value_matches_sig(value: &MoveValue, expected: &SignatureToken) -> bool {
        match (value, expected) {
            (MoveValue::Bool(_), SignatureToken::Bool) => true,
            (MoveValue::U64(_), SignatureToken::U64) => true,
            (MoveValue::Address(_), SignatureToken::Address) => true,
            (MoveValue::Vector(values), SignatureToken::Vector(inner)) => {
                values.iter().all(|v| Self::value_matches_sig(v, inner))
            }
            _ => false,
        }
    }

    fn move_value_to_json(value: MoveValue) -> serde_json::Value {
        match value {
            MoveValue::U64(v) => serde_json::json!(v),
            MoveValue::Bool(v) => serde_json::json!(v),
            MoveValue::Address(addr) => serde_json::json!(addr.to_string()),
            MoveValue::Vector(vals) => {
                serde_json::Value::Array(vals.into_iter().map(Self::move_value_to_json).collect())
            }
        }
    }

    /// Execute a transfer using a specific coin_id (solver-tee3 architecture)
    ///
    /// This method is called when Validator has already selected the coin_id
    /// via ResolvedInputs. The TEE should use this method instead of
    /// execute_simple_transfer to honor the Validator's coin selection.
    ///
    /// # Arguments
    /// * `coin_id` - The specific coin object ID selected by Validator
    /// * `sender` - Sender address (for ownership verification)
    /// * `recipient` - Recipient address
    /// * `amount` - Amount to transfer (None for full transfer)
    /// * `ctx` - Execution context
    pub fn execute_transfer_with_coin(
        &mut self,
        coin_id: ObjectId,
        sender: &str,
        recipient: &str,
        amount: Option<u64>,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let sender_addr = Address::from(sender);
        let recipient_addr = Address::from(recipient);

        info!(
            coin_id = %coin_id,
            from = %sender,
            to = %recipient,
            amount = ?amount,
            "Executing transfer with specified coin_id"
        );

        // Create and execute the transfer transaction
        let tx = Transaction::new_transfer(sender_addr, coin_id, recipient_addr, amount);

        self.execute_transaction(&tx, ctx)
    }

    /// 获取状态存储的引用（用于外部查询）
    pub fn state(&self) -> &S {
        &self.state
    }

    /// 获取状态存储的可变引用
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Execute a simple account-based transfer (convenience method)
    ///
    /// This method accepts a simple `Transfer` request (from/to/amount) from users,
    /// automatically finds suitable Coin objects from the sender, and executes the transfer.
    ///
    /// This bridges the gap between user-facing account model and internal object model.
    ///
    /// # Arguments
    /// * `from` - Sender address (account)
    /// * `to` - Recipient address (account)  
    /// * `amount` - Amount to transfer
    /// * `ctx` - Execution context
    ///
    /// # Returns
    /// * `ExecutionOutput` with state changes in object model format
    pub fn execute_simple_transfer(
        &mut self,
        from: &str,
        to: &str,
        amount: u64,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let sender = Address::from(from);
        let recipient = Address::from(to);

        info!(
            from = %from,
            to = %to,
            amount = amount,
            "Executing simple transfer"
        );

        // 1. Find sender's Coin objects
        let owned_objects = self.state.get_owned_objects(&sender)?;

        if owned_objects.is_empty() {
            return Err(RuntimeError::InsufficientBalance {
                address: sender.to_string(),
                required: amount,
                available: 0,
            });
        }

        // 2. Calculate total balance and find a suitable coin
        let mut total_balance = 0u64;
        let mut selected_coin_id: Option<ObjectId> = None;
        let mut selected_coin_balance = 0u64;

        for obj_id in &owned_objects {
            if let Some(coin) = self.state.get_object(obj_id)? {
                let balance = coin.data.balance.value();
                total_balance += balance;

                // Select a coin that can cover the amount (prefer exact match or smallest sufficient)
                if balance >= amount {
                    if selected_coin_id.is_none() || balance < selected_coin_balance {
                        selected_coin_id = Some(*obj_id);
                        selected_coin_balance = balance;
                    }
                }
            }
        }

        // Check total balance
        if total_balance < amount {
            return Err(RuntimeError::InsufficientBalance {
                address: sender.to_string(),
                required: amount,
                available: total_balance,
            });
        }

        // 3. If no single coin is sufficient, we need to merge (future: for now, error out)
        let coin_id = selected_coin_id.ok_or_else(|| {
            RuntimeError::InvalidTransaction(format!(
                "No single coin with sufficient balance. Total: {}, Required: {}. Coin merging not yet implemented.",
                total_balance, amount
            ))
        })?;

        // 4. Create and execute the transfer transaction
        let tx = Transaction::new_transfer(
            sender,
            coin_id,
            recipient,
            Some(amount), // Always partial transfer for simple API
        );

        self.execute_transaction(&tx, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InMemoryStateStore;

    #[test]
    fn test_full_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        // 创建初始 Coin
        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        // 创建执行器
        let mut executor = RuntimeExecutor::new(store);

        // 创建转账交易
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), None);

        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        // 执行转账
        let output = executor.execute_transaction(&tx, &ctx).unwrap();

        assert!(output.success);
        assert_eq!(output.state_changes.len(), 1);

        // 验证所有权变更
        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(coin.metadata.owner.unwrap(), recipient);
    }

    #[test]
    fn test_partial_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);

        // 转账 300
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(300));

        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let output = executor.execute_transaction(&tx, &ctx).unwrap();

        assert!(output.success);
        assert_eq!(output.created_objects.len(), 1);

        // 验证原 Coin 余额减少
        let original_coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(original_coin.data.balance.value(), 700);

        // 验证新 Coin 创建
        let new_coin_id = output.created_objects[0];
        let new_coin = executor.state().get_object(&new_coin_id).unwrap().unwrap();
        assert_eq!(new_coin.data.balance.value(), 300);
        assert_eq!(new_coin.metadata.owner.unwrap(), recipient);
    }

    #[test]
    fn test_move_script_branch_and_return() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let sender = Address::from("alice");

        let script = MoveScriptTx {
            code: vec![
                Bytecode::CopyLoc(0),
                Bytecode::CopyLoc(1),
                Bytecode::Add,
                Bytecode::LdU64(12),
                Bytecode::Gt,
                Bytecode::BrFalse(8),
                Bytecode::LdU64(1),
                Bytecode::Ret,
                Bytecode::LdU64(0),
                Bytecode::Ret,
            ],
            locals_sig: vec![SignatureToken::U64, SignatureToken::U64],
            params_sig: vec![SignatureToken::U64, SignatureToken::U64],
            return_sig: vec![SignatureToken::U64],
            args: vec![MoveValue::U64(10), MoveValue::U64(3)],
            type_args: vec![],
            max_gas: 200,
            input_objects: vec![],
        };

        let tx = Transaction::new_move_script(sender, script);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);
        let result = output.query_result.unwrap();
        assert_eq!(result["returns"][0], serde_json::json!(1));
    }

    #[test]
    fn test_move_script_out_of_gas() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let sender = Address::from("alice");

        let script = MoveScriptTx {
            code: vec![
                Bytecode::LdTrue,
                Bytecode::BrFalse(3),
                Bytecode::Branch(0),
                Bytecode::Ret,
            ],
            locals_sig: vec![],
            params_sig: vec![],
            return_sig: vec![],
            args: vec![],
            type_args: vec![],
            max_gas: 8,
            input_objects: vec![],
        };

        let tx = Transaction::new_move_script(sender, script);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let err = executor.execute_transaction(&tx, &ctx).unwrap_err();
        assert!(
            err.to_string().contains("Out of gas"),
            "unexpected error: {}",
            err
        );
    }
}
