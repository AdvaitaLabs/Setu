//! Runtime executor - Simple State Transition Executor

use serde::{Deserialize, Serialize};
use tracing::{info, debug, warn};
use setu_types::{
    ObjectId, Address, CoinType, create_typed_coin,
};
use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::transaction::{Transaction, TransactionType, TransferTx, QueryTx, QueryType};

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
            TransactionType::Transfer(transfer_tx) => {
                self.execute_transfer(tx, transfer_tx, ctx)
            }
            TransactionType::Query(query_tx) => {
                self.execute_query(tx, query_tx, ctx)
            }
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
        let mut coin = self.state.get_object(&coin_id)?
            .ok_or(RuntimeError::ObjectNotFound(coin_id))?;
        
        // 2. 验证所有权
        let owner = coin.metadata.owner.as_ref()
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
        
        let fee = transfer_tx.fee;

        // 3. Execute transfer logic
        match transfer_tx.amount {
            // Full transfer: transfer ownership (fee deducted from coin value, then burned)
            None => {
                // Deduct fee before transferring ownership
                if fee > 0 {
                    coin.data.balance.withdraw(fee)
                        .map_err(|e| RuntimeError::InvalidTransaction(
                            format!("Insufficient balance for fee: {}", e)
                        ))?;
                }

                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = coin.data.balance.value(),
                    fee = fee,
                    "Full transfer"
                );

                coin.metadata.owner = Some(recipient.clone());
                coin.metadata.version += 1;

                let new_state = serde_json::to_vec(&coin)?;
                self.state.set_object(coin_id, coin)?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });
            }

            // Partial transfer: split coin (fee deducted from sender and burned, not sent to recipient)
            Some(amount) => {
                let total_deduction = amount.checked_add(fee)
                    .ok_or_else(|| RuntimeError::InvalidTransaction(
                        "Amount + fee overflow".to_string()
                    ))?;

                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = amount,
                    fee = fee,
                    total_deduction = total_deduction,
                    "Partial transfer (split)"
                );

                // Withdraw amount + fee from sender
                coin.data.balance.withdraw(total_deduction)
                    .map_err(|e| RuntimeError::InvalidTransaction(e))?;

                // Update sender's coin (reduced by amount + fee)
                coin.metadata.version += 1;
                let new_state = serde_json::to_vec(&coin)?;
                self.state.set_object(coin_id, coin.clone())?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });

                // Create new coin for recipient with only the transfer amount
                // (fee is burned/destroyed — no coin created for it)
                let new_coin = create_typed_coin(
                    recipient.clone(),
                    amount,
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
                    query_tx.params.get("address")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'address' parameter".to_string()
                        ))?
                        .clone()
                )?;
                
                let owned_objects = self.state.get_owned_objects(&address)?;
                let mut total_balance: HashMap<CoinType, u64> = HashMap::new();
                
                for obj_id in owned_objects {
                    if let Some(coin) = self.state.get_object(&obj_id)? {
                        *total_balance.entry(coin.data.coin_type.clone()).or_insert(0) 
                            += coin.data.balance.value();
                    }
                }
                
                serde_json::to_value(&total_balance)?
            }
            
            QueryType::Object => {
                let object_id: ObjectId = serde_json::from_value(
                    query_tx.params.get("object_id")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'object_id' parameter".to_string()
                        ))?
                        .clone()
                )?;
                
                let object = self.state.get_object(&object_id)?;
                serde_json::to_value(&object)?
            }
            
            QueryType::OwnedObjects => {
                let address: Address = serde_json::from_value(
                    query_tx.params.get("address")
                        .ok_or(RuntimeError::InvalidTransaction(
                            "Missing 'address' parameter".to_string()
                        ))?
                        .clone()
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
        self.execute_transfer_with_coin_and_fee(coin_id, sender, recipient, amount, 0, ctx)
    }

    /// Execute a transfer with fee (solver-tee3 architecture).
    ///
    /// Fee is deducted from the sender and destroyed.
    /// Burn mechanism subject to revision.
    pub fn execute_transfer_with_coin_and_fee(
        &mut self,
        coin_id: ObjectId,
        sender: &str,
        recipient: &str,
        amount: Option<u64>,
        fee: u64,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let sender_addr = Address::from(sender);
        let recipient_addr = Address::from(recipient);

        info!(
            coin_id = %coin_id,
            from = %sender,
            to = %recipient,
            amount = ?amount,
            fee = fee,
            "Executing transfer with specified coin_id"
        );

        // Create and execute the transfer transaction
        let tx = Transaction::new_transfer_with_fee(
            sender_addr,
            coin_id,
            recipient_addr,
            amount,
            fee,
        );

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

use std::collections::HashMap;

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
        let tx = Transaction::new_transfer(
            sender.clone(),
            coin_id,
            recipient.clone(),
            Some(300),
        );
        
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
    fn test_partial_transfer_with_fee() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        let coin = setu_types::create_coin(sender.clone(), 100_000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        // Transfer 5000 with 21000 fee → sender loses 26000 total
        let tx = Transaction::new_transfer_with_fee(
            sender.clone(), coin_id, recipient.clone(), Some(5000), 21_000,
        );

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);

        // Sender: 100_000 - 5000 - 21000 = 74_000
        let original = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(original.data.balance.value(), 74_000);

        // Recipient gets only the transfer amount, not the burn
        let new_coin_id = output.created_objects[0];
        let new_coin = executor.state().get_object(&new_coin_id).unwrap().unwrap();
        assert_eq!(new_coin.data.balance.value(), 5000);
    }

    #[test]
    fn test_full_transfer_with_fee() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        let coin = setu_types::create_coin(sender.clone(), 50_000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        // Full transfer with 21000 fee → coin arrives with 50_000 - 21_000 = 29_000
        let tx = Transaction::new_transfer_with_fee(
            sender.clone(), coin_id, recipient.clone(), None, 21_000,
        );

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);

        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(coin.data.balance.value(), 29_000);
        assert_eq!(coin.metadata.owner.unwrap(), recipient);
    }

    #[test]
    fn test_fee_insufficient_balance() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        // Transfer 500 with 21000 fee → needs 21500, has 1000
        let tx = Transaction::new_transfer_with_fee(
            sender.clone(), coin_id, recipient.clone(), Some(500), 21_000,
        );

        let result = executor.execute_transaction(&tx, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_zero_fee_unchanged_behavior() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from("alice");
        let recipient = Address::from("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        // Zero fee = identical to old behavior
        let tx = Transaction::new_transfer_with_fee(
            sender.clone(), coin_id, recipient.clone(), Some(300), 0,
        );

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);

        let original = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(original.data.balance.value(), 700);

        let new_coin_id = output.created_objects[0];
        let new_coin = executor.state().get_object(&new_coin_id).unwrap().unwrap();
        assert_eq!(new_coin.data.balance.value(), 300);
    }
}
