//! Runtime executor - Simple State Transition Executor
//!
//! ## State Serialization Format
//!
//! **Important**: All Coin state changes use BCS serialization (via `CoinState`),
//! not JSON. This ensures compatibility with the storage layer's Merkle tree.
//!
//! - Use `coin.to_coin_state_bytes()` for StateChange.new_state
//! - Non-Coin objects (SubnetMetadata, UserMembership) still use JSON

use serde::{Deserialize, Serialize};
use setu_types::{deterministic_coin_id, Address, Balance, CoinData, CoinType, Object, ObjectId};
use tracing::{debug, info, warn};
// Note: Coin::to_coin_state_bytes() is used via trait method on Object<CoinData>
use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::sui_vm::execute_sui_entry_with_outcome;
use crate::transaction::{ProgramTx, QueryTx, QueryType, Transaction, TransactionType, TransferTx};

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

impl StateChange {
    /// Convert runtime StateChange to event-layer StateChange for storage/network.
    ///
    /// Uses canonical "oid:{hex}" key format.
    /// NOTE: `change_type` is intentionally NOT carried over — storage layer
    /// derives operation type from new_state: Some(_) → Create/Update, None → Delete.
    pub fn to_event_state_change(&self) -> setu_types::StateChange {
        setu_types::StateChange {
            key: setu_types::object_key(&self.object_id),
            old_value: self.old_state.clone(),
            new_value: self.new_state.clone(),
        }
    }
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
            TransactionType::Program(program_tx) => self.execute_program(tx, program_tx, ctx),
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

        // 1.5. 确保 Coin 是 Owned 对象（防御性检查）
        // transfer_to() 对非 Owned 对象会静默跳过，导致生成无效 StateChange。
        // 所有 Coin 都应该是 OwnedObject，如果不是则说明数据已损坏。
        if !coin.is_owned() {
            return Err(RuntimeError::InvalidTransaction(format!(
                "Coin {} is not an owned object — cannot transfer",
                coin_id
            )));
        }

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

        // 记录旧状态 (BCS format for Merkle tree compatibility)
        let old_state = coin.to_coin_state_bytes();

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

                // transfer_to updates owner, ownership, version, and digest
                coin.transfer_to(recipient.clone());

                // Use BCS serialization for storage compatibility
                let new_state = coin.to_coin_state_bytes();

                // 保存更新后的对象
                self.state.set_object(coin_id, coin)?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });
            }

            // 部分转账：使用 get-or-deposit 模式，确定性 coin ID
            Some(amount) if amount < coin.data.balance.value() => {
                let coin_type_str = coin.data.coin_type.as_str().to_string();

                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = amount,
                    remaining = coin.data.balance.value() - amount,
                    "Partial transfer (get-or-deposit)"
                );

                // 从原 Coin 中提取金额
                let transferred_balance = coin
                    .data
                    .balance
                    .withdraw(amount)
                    .map_err(|e| RuntimeError::InvalidTransaction(e))?;

                // 更新原 Coin: increment_version 同时更新 version 和 digest
                // (之前用 `version += 1` 未更新 digest——虽然 CoinState 不含 digest,
                //  但保持内存 Object 状态一致性，避免后续维护隐患)
                coin.increment_version();
                let new_state = coin.to_coin_state_bytes();
                self.state.set_object(coin_id, coin)?; // move, 不再 clone

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });

                // Use deterministic coin ID for recipient (1:1 model: one coin per address per subnet)
                let recipient_coin_id = deterministic_coin_id(recipient, &coin_type_str);

                if let Some(mut existing_coin) = self.state.get_object(&recipient_coin_id)? {
                    // Recipient already has a coin of this type → deposit into existing
                    let old_recipient_state = existing_coin.to_coin_state_bytes();
                    existing_coin
                        .data
                        .balance
                        .deposit(transferred_balance)
                        .map_err(|e| RuntimeError::InvalidTransaction(e))?;
                    existing_coin.increment_version();
                    let new_recipient_state = existing_coin.to_coin_state_bytes();

                    self.state.set_object(recipient_coin_id, existing_coin)?;

                    state_changes.push(StateChange {
                        change_type: StateChangeType::Update,
                        object_id: recipient_coin_id,
                        old_state: Some(old_recipient_state),
                        new_state: Some(new_recipient_state),
                    });
                } else {
                    // Recipient doesn't have this coin type → create new coin with deterministic ID
                    let data = CoinData {
                        coin_type: CoinType::new(&coin_type_str),
                        balance: Balance::new(amount),
                    };
                    let new_coin = Object::new_owned(recipient_coin_id, recipient.clone(), data);
                    let new_coin_state = new_coin.to_coin_state_bytes();

                    self.state.set_object(recipient_coin_id, new_coin)?;

                    created_objects.push(recipient_coin_id);
                    state_changes.push(StateChange {
                        change_type: StateChangeType::Create,
                        object_id: recipient_coin_id,
                        old_state: None,
                        new_state: Some(new_coin_state),
                    });
                }
            }

            // amount == full balance → treat as full transfer (avoid zombie 0-balance coin)
            Some(amount) if amount == coin.data.balance.value() => {
                debug!(
                    coin_id = %coin_id,
                    from = %tx.sender,
                    to = %recipient,
                    amount = amount,
                    "Full transfer (amount == balance, ownership transfer)"
                );

                coin.transfer_to(recipient.clone());
                let new_state = coin.to_coin_state_bytes();
                self.state.set_object(coin_id, coin)?;

                state_changes.push(StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                });
            }

            // amount > balance → insufficient funds
            Some(amount) => {
                return Err(RuntimeError::InvalidTransaction(format!(
                    "Insufficient balance: requested {}, available {}",
                    amount,
                    coin.data.balance.value()
                )));
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

    /// 执行 Sui Program 交易（通过 sui_vm 子集解释器）
    fn execute_program(
        &mut self,
        tx: &Transaction,
        program_tx: &ProgramTx,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let outcome = execute_sui_entry_with_outcome(
            &mut self.state,
            &tx.sender,
            &program_tx.disassembly,
            &program_tx.function_name,
            &program_tx.args,
        )?;

        let mut state_changes = Vec::new();
        let mut created_objects = Vec::new();
        let mut deleted_objects = Vec::new();

        for write in outcome.writes {
            let change_type = match (write.old_state.is_some(), write.new_state.is_some()) {
                (false, true) => {
                    created_objects.push(write.object_id);
                    StateChangeType::Create
                }
                (true, false) => {
                    deleted_objects.push(write.object_id);
                    StateChangeType::Delete
                }
                (true, true) => StateChangeType::Update,
                (false, false) => continue,
            };

            state_changes.push(StateChange {
                change_type,
                object_id: write.object_id,
                old_state: write.old_state,
                new_state: write.new_state,
            });
        }

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!(
                "Program '{}' executed successfully",
                program_tx.function_name
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
        let sender_addr = Address::from_hex(sender)
            .map_err(|_| RuntimeError::InvalidAddress(sender.to_string()))?;
        let recipient_addr = Address::from_hex(recipient)
            .map_err(|_| RuntimeError::InvalidAddress(recipient.to_string()))?;

        info!(
            coin_id = %coin_id,
            from = %sender,
            to = %recipient,
            amount = ?amount,
            "Executing transfer with specified coin_id"
        );

        // Create and execute the transfer transaction
        // ⚠️ Use deterministic constructor — this is a TEE/consensus path.
        // Transaction::new_transfer uses SystemTime::now() which would produce
        // different values across validators, breaking consensus.
        let tx = Transaction::new_transfer_deterministic(
            sender_addr,
            coin_id,
            recipient_addr,
            amount,
            ctx.timestamp,
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
        // ⚠️ Safety: execute_simple_transfer auto-selects coins, which is
        // non-deterministic across validators. TEE/consensus paths MUST use
        // execute_transfer_with_coin (coin pre-selected by TaskPreparer).
        if ctx.in_tee {
            return Err(RuntimeError::InvalidTransaction(
                "execute_simple_transfer must not be called in TEE path — \
                 use execute_transfer_with_coin with pre-selected coin_id"
                    .to_string(),
            ));
        }

        let sender =
            Address::from_hex(from).map_err(|_| RuntimeError::InvalidAddress(from.to_string()))?;
        let recipient =
            Address::from_hex(to).map_err(|_| RuntimeError::InvalidAddress(to.to_string()))?;

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
        // ⚠️ Use deterministic constructor — this is a TEE/consensus path.
        let tx = Transaction::new_transfer_deterministic(
            sender,
            coin_id,
            recipient,
            Some(amount), // Always partial transfer for simple API
            ctx.timestamp,
        );

        self.execute_transaction(&tx, ctx)
    }

    // ========== Subnet & User Registration Handlers ==========

    /// Execute subnet registration - initializes subnet token if configured
    ///
    /// This handles the SubnetRegister event and:
    /// 1. Records subnet metadata
    /// 2. Mints initial token supply to subnet owner (if token configured)
    /// 3. Returns state changes for both subnet registration and token creation
    pub fn execute_subnet_register(
        &mut self,
        subnet_id: &str,
        name: &str,
        owner: &Address,
        token_symbol: Option<&str>,
        initial_supply: Option<u64>,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let mut state_changes = Vec::new();
        let mut created_objects = Vec::new();

        // 1. Record subnet metadata
        let subnet_key = format!("subnet:{}", subnet_id);
        let subnet_data = serde_json::json!({
            "subnet_id": subnet_id,
            "name": name,
            "owner": owner.to_string(),
            "token_symbol": token_symbol,
            "created_at": ctx.timestamp,
        });

        // Generate deterministic ObjectId from subnet key (domain-separated)
        let subnet_object_id = ObjectId::new(setu_types::hash_utils::setu_hash_with_domain(
            b"SETU_SUBNET_META:",
            subnet_key.as_bytes(),
        ));

        // Note: SubnetMetadata is NOT a Coin, so we keep JSON format for it
        // Only Coin objects use BCS format
        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: subnet_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&subnet_data)?),
        });

        // 2. Mint initial token supply to owner if configured
        // Note: token_symbol is for display only, we use subnet_id as the coin namespace
        if let Some(supply) = initial_supply {
            if supply > 0 {
                // Use subnet_id as the coin namespace (1:1 binding)
                // token_symbol is only for display purposes (stored in SubnetConfig)
                let coin_id = deterministic_coin_id(owner, subnet_id);

                // Create token coin for subnet owner with deterministic ID
                // (not via create_typed_coin, whose internal ID would differ from coin_id)
                let data = CoinData {
                    coin_type: CoinType::new(subnet_id),
                    balance: Balance::new(supply),
                };
                let token_coin = Object::new_owned(coin_id, owner.clone(), data);
                // Use BCS serialization for Coin storage
                let coin_state = token_coin.to_coin_state_bytes();

                self.state.set_object(coin_id, token_coin)?;

                created_objects.push(coin_id);
                state_changes.push(StateChange {
                    change_type: StateChangeType::Create,
                    object_id: coin_id,
                    old_state: None,
                    new_state: Some(coin_state),
                });

                info!(
                    subnet_id = %subnet_id,
                    owner = %owner,
                    token_symbol = ?token_symbol,
                    initial_supply = supply,
                    "Minted initial subnet token supply"
                );
            }
        }

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!(
                "Subnet '{}' registered with owner {}{}",
                name,
                owner,
                token_symbol.map_or(String::new(), |s| format!(", token: {}", s))
            )),
            state_changes,
            created_objects,
            deleted_objects: vec![],
            query_result: None,
        })
    }

    /// Execute user registration (pure infrastructure primitive)
    ///
    /// This is a basic infrastructure operation that only records user membership.
    ///
    /// **Note**: Token airdrops are application-layer logic and should be handled
    /// by Subnet applications (future: MoveVM smart contracts). The Setu core
    /// only provides primitives like `mint_tokens()` and `transfer()` that
    /// applications can compose.
    ///
    /// # Arguments
    /// * `user_address` - Address of the user to register
    /// * `subnet_id` - Subnet the user is joining
    /// * `ctx` - Execution context
    pub fn execute_user_register(
        &mut self,
        user_address: &Address,
        subnet_id: &str,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let mut state_changes = Vec::new();

        // Record user membership (pure infrastructure operation)
        let membership_key = format!("user:{}:subnet:{}", user_address, subnet_id);
        let membership_data = serde_json::json!({
            "user": user_address.to_string(),
            "subnet_id": subnet_id,
            "joined_at": ctx.timestamp,
        });

        // Generate deterministic ObjectId from membership key (domain-separated)
        let membership_object_id = ObjectId::new(setu_types::hash_utils::setu_hash_with_domain(
            b"SETU_MEMBERSHIP:",
            membership_key.as_bytes(),
        ));

        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: membership_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&membership_data)?),
        });

        info!(
            user = %user_address,
            subnet_id = %subnet_id,
            "User registered in subnet"
        );

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!(
                "User {} registered in subnet '{}'",
                user_address, subnet_id,
            )),
            state_changes,
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: None,
        })
    }

    /// Mint tokens to an address (pure infrastructure primitive)
    ///
    /// This is a basic token minting operation. Applications can use this
    /// to implement airdrops, rewards, or other token distribution logic.
    ///
    /// # Arguments
    /// * `to` - Address to mint tokens to
    /// * `subnet_id` - Subnet ID (determines token type, 1:1 binding)
    /// * `amount` - Amount to mint
    /// * `ctx` - Execution context
    pub fn mint_tokens(
        &mut self,
        to: &Address,
        subnet_id: &str,
        amount: u64,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        if amount == 0 {
            return Ok(ExecutionOutput {
                success: true,
                message: Some("No tokens to mint (amount=0)".to_string()),
                state_changes: vec![],
                created_objects: vec![],
                deleted_objects: vec![],
                query_result: None,
            });
        }

        // Use deterministic coin ID with subnet_id as namespace
        let coin_id = deterministic_coin_id(to, subnet_id);

        // Check if coin already exists
        let existing = self.state.get_object(&coin_id)?;

        let (state_change, created) = if let Some(mut existing_coin) = existing {
            // Add to existing balance - use BCS format
            let old_state = existing_coin.to_coin_state_bytes();
            existing_coin
                .data
                .balance
                .deposit(setu_types::Balance::new(amount))
                .map_err(|e| RuntimeError::InvalidTransaction(e))?;
            existing_coin.increment_version();
            let new_state = existing_coin.to_coin_state_bytes();

            self.state.set_object(coin_id, existing_coin)?;

            (
                StateChange {
                    change_type: StateChangeType::Update,
                    object_id: coin_id,
                    old_state: Some(old_state),
                    new_state: Some(new_state),
                },
                false,
            )
        } else {
            // Create new coin with deterministic ID - use BCS format
            let data = CoinData {
                coin_type: CoinType::new(subnet_id),
                balance: Balance::new(amount),
            };
            let coin = Object::new_owned(coin_id, to.clone(), data);
            let new_state = coin.to_coin_state_bytes();

            self.state.set_object(coin_id, coin)?;

            (
                StateChange {
                    change_type: StateChangeType::Create,
                    object_id: coin_id,
                    old_state: None,
                    new_state: Some(new_state),
                },
                true,
            )
        };

        info!(
            to = %to,
            subnet_id = %subnet_id,
            amount = amount,
            created = created,
            "Tokens minted"
        );

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!(
                "Minted {} tokens to {} in subnet {}",
                amount, to, subnet_id
            )),
            state_changes: vec![state_change],
            created_objects: if created { vec![coin_id] } else { vec![] },
            deleted_objects: vec![],
            query_result: None,
        })
    }

    /// Get or create a coin for an address in specific subnet
    ///
    /// Uses deterministic coin ID generation for consistency with storage layer.
    /// Returns (coin_id, was_created).
    ///
    /// # Arguments
    /// * `owner` - Owner address
    /// * `subnet_id` - Subnet ID (determines token type)
    /// * `ctx` - Execution context
    pub fn get_or_create_coin(
        &mut self,
        owner: &Address,
        subnet_id: &str,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<(ObjectId, bool)> {
        // Use deterministic coin ID with subnet_id
        let coin_id = deterministic_coin_id(owner, subnet_id);

        // Check if coin already exists
        if self.state.get_object(&coin_id)?.is_some() {
            return Ok((coin_id, false));
        }

        // Create new coin with 0 balance using deterministic ID
        let data = CoinData {
            coin_type: CoinType::new(subnet_id),
            balance: Balance::new(0),
        };
        let coin = Object::new_owned(coin_id, owner.clone(), data);
        self.state.set_object(coin_id, coin)?;

        info!(
            owner = %owner,
            subnet_id = %subnet_id,
            coin_id = %coin_id,
            "Created empty coin for recipient with deterministic ID"
        );

        Ok((coin_id, true))
    }
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InMemoryStateStore;
    use crate::sui_vm::SuiVmArg;

    const PROGRAM_TX_DISASSEMBLY: &str = r#"
entry public transfer_coin_by_flag(coin#0#0: Coin<SUI>, recipient_true#0#0: address, recipient_false#0#0: address, flag#0#0: bool) {
B0:
    0: MoveLoc[3](flag#0#0: bool)
    1: BrFalse(6)
B1:
    2: MoveLoc[0](coin#0#0: Coin<SUI>)
    3: MoveLoc[1](recipient_true#0#0: address)
    4: Call transfer::public_transfer<Coin<SUI>>(Coin<SUI>, address)
    5: Branch(9)
B2:
    6: MoveLoc[0](coin#0#0: Coin<SUI>)
    7: MoveLoc[2](recipient_false#0#0: address)
    8: Call transfer::public_transfer<Coin<SUI>>(Coin<SUI>, address)
B3:
    9: Ret
}
"#;

    #[test]
    fn test_full_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

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
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

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
        // get-or-deposit: new coin created for recipient
        assert_eq!(output.created_objects.len(), 1);

        // 验证原 Coin 余额减少
        let original_coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(original_coin.data.balance.value(), 700);

        // 验证 recipient coin 使用确定性 ID
        let expected_coin_id = deterministic_coin_id(&recipient, "ROOT");
        assert_eq!(output.created_objects[0], expected_coin_id);
        let new_coin = executor
            .state()
            .get_object(&expected_coin_id)
            .unwrap()
            .unwrap();
        assert_eq!(new_coin.data.balance.value(), 300);
        assert_eq!(new_coin.metadata.owner.unwrap(), recipient);
    }

    #[test]
    fn test_partial_transfer_deposit_into_existing() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        // Alice has 1000
        let alice_coin = setu_types::create_coin(sender.clone(), 1000);
        let alice_coin_id = *alice_coin.id();
        store.set_object(alice_coin_id, alice_coin).unwrap();

        // Bob already has 500 (with deterministic ID)
        let bob_coin_id = deterministic_coin_id(&recipient, "ROOT");
        let bob_data = CoinData {
            coin_type: CoinType::native(),
            balance: Balance::new(500),
        };
        let bob_coin = Object::new_owned(bob_coin_id, recipient.clone(), bob_data);
        store.set_object(bob_coin_id, bob_coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);

        // Transfer 300 from Alice to Bob
        let tx =
            Transaction::new_transfer(sender.clone(), alice_coin_id, recipient.clone(), Some(300));

        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let output = executor.execute_transaction(&tx, &ctx).unwrap();

        assert!(output.success);
        // No new coin created — deposited into existing
        assert_eq!(output.created_objects.len(), 0);
        // 2 state changes: sender Update + recipient Update
        assert_eq!(output.state_changes.len(), 2);
        assert_eq!(output.state_changes[1].change_type, StateChangeType::Update);

        // Verify balances
        let alice_coin = executor
            .state()
            .get_object(&alice_coin_id)
            .unwrap()
            .unwrap();
        assert_eq!(alice_coin.data.balance.value(), 700);

        let bob_coin = executor.state().get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin.data.balance.value(), 800); // 500 + 300
    }

    #[test]
    fn test_program_tx_executes_sui_entry_true_branch() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");
        let carol = Address::from_str_id("carol");

        let source_coin_id = ObjectId::new([0x11; 32]);
        let source_coin = Object::new_owned(
            source_coin_id,
            sender,
            CoinData {
                coin_type: CoinType::new("SUI"),
                balance: Balance::new(120),
            },
        );
        store.set_object(source_coin_id, source_coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_program(
            sender,
            PROGRAM_TX_DISASSEMBLY.to_string(),
            "transfer_coin_by_flag",
            vec![
                SuiVmArg::ObjectId(source_coin_id),
                SuiVmArg::Address(bob),
                SuiVmArg::Address(carol),
                SuiVmArg::Bool(true),
            ],
        );
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);
        assert_eq!(output.created_objects.len(), 1);
        assert_eq!(output.deleted_objects.len(), 1);

        let bob_coin_id = deterministic_coin_id(&bob, "SUI");
        let bob_coin = executor.state().get_object(&bob_coin_id).unwrap().unwrap();
        assert_eq!(bob_coin.data.balance.value(), 120);
        assert!(executor
            .state()
            .get_object(&source_coin_id)
            .unwrap()
            .is_none());
        assert!(executor
            .state()
            .get_object(&deterministic_coin_id(&carol, "SUI"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_program_tx_executes_sui_entry_false_branch() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let bob = Address::from_str_id("bob");
        let carol = Address::from_str_id("carol");

        let source_coin_id = ObjectId::new([0x12; 32]);
        let source_coin = Object::new_owned(
            source_coin_id,
            sender,
            CoinData {
                coin_type: CoinType::new("SUI"),
                balance: Balance::new(70),
            },
        );
        store.set_object(source_coin_id, source_coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_program(
            sender,
            PROGRAM_TX_DISASSEMBLY.to_string(),
            "transfer_coin_by_flag",
            vec![
                SuiVmArg::ObjectId(source_coin_id),
                SuiVmArg::Address(bob),
                SuiVmArg::Address(carol),
                SuiVmArg::Bool(false),
            ],
        );
        let ctx = ExecutionContext {
            executor_id: "solver1".to_string(),
            timestamp: 1000,
            in_tee: false,
        };

        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        assert!(output.success);
        assert_eq!(output.created_objects.len(), 1);
        assert_eq!(output.deleted_objects.len(), 1);

        let carol_coin_id = deterministic_coin_id(&carol, "SUI");
        let carol_coin = executor
            .state()
            .get_object(&carol_coin_id)
            .unwrap()
            .unwrap();
        assert_eq!(carol_coin.data.balance.value(), 70);
        assert!(executor
            .state()
            .get_object(&source_coin_id)
            .unwrap()
            .is_none());
        assert!(executor
            .state()
            .get_object(&deterministic_coin_id(&bob, "SUI"))
            .unwrap()
            .is_none());
    }

    /// Balance conservation: sum of all balances must be unchanged after any transfer.
    #[test]
    fn test_balance_conservation_full_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let before_total: u64 = 1000; // only alice has balance

        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), None);
        let ctx = ExecutionContext {
            executor_id: "s".into(),
            timestamp: 1,
            in_tee: false,
        };
        executor.execute_transaction(&tx, &ctx).unwrap();

        // After full transfer: sender coin now owned by recipient, balance unchanged
        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        let after_total = coin.data.balance.value();
        assert_eq!(
            before_total, after_total,
            "Balance conservation violated in full transfer"
        );
    }

    #[test]
    fn test_balance_conservation_partial_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let before_total: u64 = 1000;

        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(300));
        let ctx = ExecutionContext {
            executor_id: "s".into(),
            timestamp: 1,
            in_tee: false,
        };
        executor.execute_transaction(&tx, &ctx).unwrap();

        // Sum: sender remaining + recipient new coin
        let sender_coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        let recipient_coin_id = deterministic_coin_id(&recipient, "ROOT");
        let recipient_coin = executor
            .state()
            .get_object(&recipient_coin_id)
            .unwrap()
            .unwrap();
        let after_total = sender_coin.data.balance.value() + recipient_coin.data.balance.value();
        assert_eq!(
            before_total, after_total,
            "Balance conservation violated in partial transfer"
        );
    }

    #[test]
    fn test_balance_conservation_deposit_into_existing() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        let alice_coin = setu_types::create_coin(sender.clone(), 1000);
        let alice_coin_id = *alice_coin.id();
        store.set_object(alice_coin_id, alice_coin).unwrap();

        let bob_coin_id = deterministic_coin_id(&recipient, "ROOT");
        let bob_data = CoinData {
            coin_type: CoinType::native(),
            balance: Balance::new(500),
        };
        let bob_coin = Object::new_owned(bob_coin_id, recipient.clone(), bob_data);
        store.set_object(bob_coin_id, bob_coin).unwrap();

        let before_total: u64 = 1000 + 500;

        let mut executor = RuntimeExecutor::new(store);
        let tx =
            Transaction::new_transfer(sender.clone(), alice_coin_id, recipient.clone(), Some(300));
        let ctx = ExecutionContext {
            executor_id: "s".into(),
            timestamp: 1,
            in_tee: false,
        };
        executor.execute_transaction(&tx, &ctx).unwrap();

        let alice = executor
            .state()
            .get_object(&alice_coin_id)
            .unwrap()
            .unwrap();
        let bob = executor.state().get_object(&bob_coin_id).unwrap().unwrap();
        let after_total = alice.data.balance.value() + bob.data.balance.value();
        assert_eq!(
            before_total, after_total,
            "Balance conservation violated in deposit-into-existing"
        );
    }

    #[test]
    fn test_balance_conservation_amount_equals_balance() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let before_total: u64 = 1000;

        let mut executor = RuntimeExecutor::new(store);
        // amount == balance → treated as full transfer
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(1000));
        let ctx = ExecutionContext {
            executor_id: "s".into(),
            timestamp: 1,
            in_tee: false,
        };
        executor.execute_transaction(&tx, &ctx).unwrap();

        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        let after_total = coin.data.balance.value();
        assert_eq!(
            before_total, after_total,
            "Balance conservation violated in amount==balance transfer"
        );
        // Verify ownership transferred
        assert_eq!(coin.metadata.owner.unwrap(), recipient);
    }

    #[test]
    fn test_insufficient_balance_rejected() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");

        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();

        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(2000));
        let ctx = ExecutionContext {
            executor_id: "s".into(),
            timestamp: 1,
            in_tee: false,
        };

        let result = executor.execute_transaction(&tx, &ctx);
        assert!(result.is_err(), "Should reject transfer exceeding balance");
    }
}
