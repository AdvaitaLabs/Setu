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
use tracing::{info, debug, warn};
use setu_types::{
    ObjectId, Address, CoinType, CoinData, Object,
    coin_id_from_tx, create_coin_with_id,
};
// Note: Coin::to_coin_state_bytes() is used via trait method on Object<CoinData>
use crate::error::{RuntimeError, RuntimeResult};
use crate::state::StateStore;
use crate::transaction::{Transaction, TransactionType, TransferTx, QueryTx, QueryType};

/// Execution context for a single transaction.
///
/// SAFETY: Do NOT clone this struct — the output_counter is per-transaction
/// and cloning would cause duplicate ObjectId generation.
#[derive(Debug)]  // 不 derive Clone!
pub struct ExecutionContext {
    /// Executor (usually the solver)
    pub executor_id: String,
    /// Execution timestamp
    pub timestamp: u64,
    /// Whether executed in TEE (future implementation)
    pub in_tee: bool,
    /// Transaction hash — used for deterministic ID generation
    pub tx_hash: [u8; 32],
    /// Output counter — tracks number of objects created in this tx.
    /// Uses Cell for interior mutability (single-threaded execution context).
    output_counter: std::cell::Cell<u32>,
}

impl ExecutionContext {
    /// Create a new ExecutionContext.
    ///
    /// For STF/Enclave path: tx_hash derived from task_id (see design doc §3.3.2)
    /// For genesis/validator path: tx_hash = BLAKE3("SETU_TX_HASH:GENESIS:" || event_id)
    /// For tests: tx_hash = [0u8; 32] or deterministic test value
    pub fn new(
        executor_id: String,
        timestamp: u64,
        in_tee: bool,
        tx_hash: [u8; 32],
    ) -> Self {
        Self {
            executor_id,
            timestamp,
            in_tee,
            tx_hash,
            output_counter: std::cell::Cell::new(0),
        }
    }

    /// Get the next output index and increment counter.
    ///
    /// Panics if counter overflows u32 (> 4 billion coins per tx — impossible
    /// in practice, but guarded defensively).
    pub fn next_output_index(&self) -> u32 {
        let idx = self.output_counter.get();
        self.output_counter.set(
            idx.checked_add(1).expect("output_counter overflow: > u32::MAX coins in one tx")
        );
        idx
    }

    /// Generate deterministic ObjectId for a new coin
    pub fn new_coin_id(&self) -> ObjectId {
        coin_id_from_tx(&self.tx_hash, self.next_output_index())
    }
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
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let coin_id = transfer_tx.coin_id;
        let recipient = &transfer_tx.recipient;
        
        // 1. 读取 Coin 对象
        let mut coin = self.state.get_object(&coin_id)?
            .ok_or(RuntimeError::ObjectNotFound(coin_id))?;
        
        // 1.5. 确保 Coin 是 Owned 对象（防御性检查）
        if !coin.is_owned() {
            return Err(RuntimeError::InvalidTransaction(
                format!("Coin {} is not an owned object — cannot transfer", coin_id)
            ));
        }
        
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
        
        // 记录旧状态 (BCS format for Merkle tree compatibility)
        let old_state = coin.to_coin_state_bytes();
        
        let mut state_changes = Vec::new();
        let mut created_objects = Vec::new();
        let deleted_objects = Vec::new();
        
        // 🔴 R13: 拒绝 amount == 0（防止创建 0 余额僵尸 Coin）
        if let Some(0) = transfer_tx.amount {
            return Err(RuntimeError::InvalidTransaction(
                "Transfer amount must be > 0".into()
            ));
        }
        
        // 判断是否全额转账:
        // - None: 显式全额
        // - Some(amount) where amount == balance: 隐式全额（避免 0 余额僵尸 Coin）
        let is_full_transfer = match transfer_tx.amount {
            None => true,
            Some(amount) => amount == coin.data.balance.value(),
        };
        
        if is_full_transfer {
            // 全额转账：直接转移所有权（不创建新 Coin，不留僵尸）
            debug!(
                coin_id = %coin_id,
                from = %tx.sender,
                to = %recipient,
                amount = coin.data.balance.value(),
                "Full transfer (ownership transfer)"
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
        } else {
            // 部分转账 (amount < balance): always-create-new pattern
            let amount = transfer_tx.amount.unwrap(); // safe: is_full_transfer=false ⟹ Some
            let coin_type_str = coin.data.coin_type.as_str().to_string();
            
            debug!(
                coin_id = %coin_id,
                from = %tx.sender,
                to = %recipient,
                amount = amount,
                remaining = coin.data.balance.value() - amount,
                "Partial transfer (always-create-new)"
            );
            
            // 1. 扣减 sender 的 Coin
            let _ = coin.data.balance.withdraw(amount)
                .map_err(|e| RuntimeError::InvalidTransaction(e))?;
            coin.increment_version();
            let new_state = coin.to_coin_state_bytes();
            self.state.set_object(coin_id, coin)?;
            
            state_changes.push(StateChange {
                change_type: StateChangeType::Update,
                object_id: coin_id,
                old_state: Some(old_state),
                new_state: Some(new_state),
            });
            
            // 2. 为 recipient 创建新 Coin（确定性 ID）
            let new_coin_id = ctx.new_coin_id();
            let new_coin = create_coin_with_id(
                new_coin_id,
                recipient.clone(),
                amount,
                &coin_type_str,
                ctx.timestamp,
            );
            let new_coin_state = new_coin.to_coin_state_bytes();
            self.state.set_object(new_coin_id, new_coin)?;
            
            created_objects.push(new_coin_id);
            state_changes.push(StateChange {
                change_type: StateChangeType::Create,
                object_id: new_coin_id,
                old_state: None,
                new_state: Some(new_coin_state),
            });
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
                        let entry = total_balance.entry(coin.data.coin_type.clone()).or_insert(0);
                        *entry = entry.checked_add(coin.data.balance.value())
                            .ok_or_else(|| RuntimeError::InvalidTransaction(
                                "Balance overflow in query".to_string()
                            ))?;
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
    
    /// Execute a simple account-based transfer with auto-merge support.
    ///
    /// When a single Coin is insufficient, automatically merges all coins of
    /// the same type, then executes the transfer. This is Setu's "auto-PTB".
    ///
    /// # Arguments
    /// * `from` - Sender address (account)
    /// * `to` - Recipient address (account)
    /// * `amount` - Amount to transfer
    /// * `ctx` - Execution context
    /// * `coin_type` - Optional coin type filter (None = "ROOT")
    pub fn execute_simple_transfer(
        &mut self,
        from: &str,
        to: &str,
        amount: u64,
        ctx: &ExecutionContext,
        coin_type: Option<&str>,
    ) -> RuntimeResult<ExecutionOutput> {
        if ctx.in_tee {
            return Err(RuntimeError::InvalidTransaction(
                "execute_simple_transfer must not be called in TEE path — \
                 use execute_transfer_with_coin with pre-selected coin_id".to_string()
            ));
        }
        
        let sender = Address::from_hex(from)
            .map_err(|_| RuntimeError::InvalidAddress(from.to_string()))?;
        let recipient = Address::from_hex(to)
            .map_err(|_| RuntimeError::InvalidAddress(to.to_string()))?;
        
        info!(from = %from, to = %to, amount = amount, "Executing simple transfer");
        
        let owned_objects = self.state.get_owned_objects(&sender)?;
        let coin_type_filter = coin_type.unwrap_or("ROOT");
        
        // Collect coins of the matching type
        let mut coins: Vec<(ObjectId, Object<CoinData>)> = Vec::new();
        for obj_id in &owned_objects {
            if let Some(coin) = self.state.get_object(obj_id)? {
                if coin.data.coin_type.as_str() == coin_type_filter {
                    coins.push((*obj_id, coin));
                }
            }
        }
        
        let total_balance: u64 = coins.iter()
            .try_fold(0u64, |acc, (_, c)| acc.checked_add(c.data.balance.value()))
            .ok_or(RuntimeError::InvalidTransaction("Total balance overflow".into()))?;
        
        if total_balance < amount {
            return Err(RuntimeError::InsufficientBalance {
                address: sender.to_string(),
                required: amount,
                available: total_balance,
            });
        }
        
        // Try to find a single coin that's sufficient (smallest sufficient)
        coins.sort_by_key(|(_, c)| c.data.balance.value());
        let single_sufficient = coins.iter()
            .find(|(_, c)| c.data.balance.value() >= amount);
        
        if let Some((id, _)) = single_sufficient {
            // Single coin is enough — direct transfer
            let tx = Transaction::new_transfer_deterministic(
                sender, *id, recipient, Some(amount), ctx.timestamp,
            );
            self.execute_transaction(&tx, ctx)
        } else {
            // Need to merge: merge all coins into the largest, then transfer
            coins.sort_by(|(_, a), (_, b)| b.data.balance.value().cmp(&a.data.balance.value()));
            let (target_id, _) = coins[0];
            let source_ids: Vec<ObjectId> = coins[1..].iter().map(|(id, _)| *id).collect();
            
            let mut merge_output = self.execute_merge_coins(
                &sender, target_id, &source_ids, ctx
            )?;
            
            // Transfer from merged coin
            let tx = Transaction::new_transfer_deterministic(
                sender, target_id, recipient, Some(amount), ctx.timestamp,
            );
            let transfer_output = self.execute_transaction(&tx, ctx)?;
            
            // Combine outputs
            merge_output.state_changes.extend(transfer_output.state_changes);
            merge_output.created_objects.extend(transfer_output.created_objects);
            merge_output.deleted_objects.extend(transfer_output.deleted_objects);
            merge_output.message = Some(format!(
                "Auto-merged {} coins, then transferred {} to {}",
                source_ids.len() + 1, amount, recipient
            ));
            
            Ok(merge_output)
        }
    }
    
    // ========== Subnet & User Registration Handlers ==========
    
    /// Execute subnet registration - initializes subnet token if configured
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
        
        let subnet_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(b"SETU_SUBNET_META:", subnet_key.as_bytes())
        );
        
        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: subnet_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&subnet_data)?),
        });
        
        // 2. Mint initial token supply via mint_tokens (unified path)
        if let Some(supply) = initial_supply {
            if supply > 0 {
                let mint_output = self.mint_tokens(owner, subnet_id, supply, ctx)?;
                state_changes.extend(mint_output.state_changes);
                created_objects.extend(mint_output.created_objects);
                
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
                name, owner,
                token_symbol.map_or(String::new(), |s| format!(", token: {}", s))
            )),
            state_changes,
            created_objects,
            deleted_objects: vec![],
            query_result: None,
        })
    }
    
    /// Execute user registration (pure infrastructure primitive)
    pub fn execute_user_register(
        &mut self,
        user_address: &Address,
        subnet_id: &str,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let mut state_changes = Vec::new();
        
        let membership_key = format!("user:{}:subnet:{}", user_address, subnet_id);
        let membership_data = serde_json::json!({
            "user": user_address.to_string(),
            "subnet_id": subnet_id,
            "joined_at": ctx.timestamp,
        });
        
        let membership_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes())
        );
        
        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: membership_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&membership_data)?),
        });
        
        info!(user = %user_address, subnet_id = %subnet_id, "User registered in subnet");
        
        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("User {} registered in subnet '{}'", user_address, subnet_id)),
            state_changes,
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: None,
        })
    }
    
    // ========== Profile & Subnet Membership (Phase 3) ==========

    /// Execute profile creation or update (Create-or-Update semantics)
    pub fn execute_profile_update(
        &mut self,
        user_address: &Address,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        bio: Option<&str>,
        attributes: &std::collections::HashMap<String, String>,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let profile_key = format!("profile:{}", user_address);
        let profile_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(
                b"SETU_PROFILE:", profile_key.as_bytes()
            )
        );

        let profile_data = serde_json::json!({
            "owner": user_address.to_string(),
            "display_name": display_name,
            "avatar_url": avatar_url,
            "bio": bio,
            "attributes": attributes,
            "created_at": ctx.timestamp,
            "updated_at": ctx.timestamp,
        });

        let state_changes = vec![StateChange {
            change_type: StateChangeType::Create,
            object_id: profile_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&profile_data)?),
        }];

        info!(user = %user_address, "Profile updated");

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("Profile updated for {}", user_address)),
            state_changes,
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: None,
        })
    }

    /// Execute subnet join — creates forward membership + reverse member index
    pub fn execute_subnet_join(
        &mut self,
        user_address: &Address,
        subnet_id: &str,
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let mut state_changes = Vec::new();

        // 1. Forward index: user → subnet
        let membership_key = format!("user:{}:subnet:{}", user_address, subnet_id);
        let membership_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(
                b"SETU_MEMBERSHIP:", membership_key.as_bytes()
            )
        );
        let membership_data = serde_json::json!({
            "user": user_address.to_string(),
            "subnet_id": subnet_id,
            "joined_at": ctx.timestamp,
        });
        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: membership_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&membership_data)?),
        });

        // 2. Reverse index: subnet → member
        let reverse_key = format!("subnet:{}:member:{}", subnet_id, user_address);
        let reverse_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(
                b"SETU_SUBNET_MEMBER:", reverse_key.as_bytes()
            )
        );
        let reverse_data = serde_json::json!({
            "user": user_address.to_string(),
            "subnet_id": subnet_id,
            "joined_at": ctx.timestamp,
        });
        state_changes.push(StateChange {
            change_type: StateChangeType::Create,
            object_id: reverse_object_id,
            old_state: None,
            new_state: Some(serde_json::to_vec(&reverse_data)?),
        });

        info!(user = %user_address, subnet_id = %subnet_id, "User joined subnet");

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("User {} joined subnet '{}'", user_address, subnet_id)),
            state_changes,
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: None,
        })
    }

    /// Execute subnet leave — deletes forward membership + reverse member index
    pub fn execute_subnet_leave(
        &mut self,
        user_address: &Address,
        subnet_id: &str,
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        let mut state_changes = Vec::new();

        // 1. Delete forward index
        let membership_key = format!("user:{}:subnet:{}", user_address, subnet_id);
        let membership_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(
                b"SETU_MEMBERSHIP:", membership_key.as_bytes()
            )
        );
        state_changes.push(StateChange {
            change_type: StateChangeType::Delete,
            object_id: membership_object_id,
            old_state: None,
            new_state: None,
        });

        // 2. Delete reverse index
        let reverse_key = format!("subnet:{}:member:{}", subnet_id, user_address);
        let reverse_object_id = ObjectId::new(
            setu_types::hash_utils::setu_hash_with_domain(
                b"SETU_SUBNET_MEMBER:", reverse_key.as_bytes()
            )
        );
        state_changes.push(StateChange {
            change_type: StateChangeType::Delete,
            object_id: reverse_object_id,
            old_state: None,
            new_state: None,
        });

        info!(user = %user_address, subnet_id = %subnet_id, "User left subnet");

        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("User {} left subnet '{}'", user_address, subnet_id)),
            state_changes,
            created_objects: vec![],
            deleted_objects: vec![],
            query_result: None,
        })
    }
    
    /// Mint tokens — unified single path (R14 simplified)
    ///
    /// All mint operations use `ctx.new_coin_id()` (i.e., `coin_id_from_tx`)
    /// to create new Coins. No more get-or-deposit pattern.
    pub fn mint_tokens(
        &mut self,
        to: &Address,
        subnet_id: &str,
        amount: u64,
        ctx: &ExecutionContext,
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
        
        let coin_id = ctx.new_coin_id();
        let coin = create_coin_with_id(coin_id, to.clone(), amount, subnet_id, ctx.timestamp);
        let new_state = coin.to_coin_state_bytes();
        self.state.set_object(coin_id, coin)?;
        
        info!(to = %to, subnet_id = %subnet_id, amount = amount, "Tokens minted");
        
        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("Minted {} tokens to {} in subnet {}", amount, to, subnet_id)),
            state_changes: vec![StateChange {
                change_type: StateChangeType::Create,
                object_id: coin_id,
                old_state: None,
                new_state: Some(new_state),
            }],
            created_objects: vec![coin_id],
            deleted_objects: vec![],
            query_result: None,
        })
    }
    
    // ========== Multi-Coin Operations ==========
    
    /// Maximum number of source coins in a single merge operation.
    const MAX_MERGE_SOURCES: usize = 50;
    
    /// Merge multiple coins into a target coin.
    ///
    /// All source coins must belong to the same owner and have the same coin_type
    /// as the target. After merge, target balance += sum(source balances), sources deleted.
    pub fn execute_merge_coins(
        &mut self,
        owner: &Address,
        target_coin_id: ObjectId,
        source_coin_ids: &[ObjectId],
        _ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        // Parameter validation
        if source_coin_ids.is_empty() {
            return Err(RuntimeError::InvalidTransaction(
                "Must provide at least one source coin to merge".into()
            ));
        }
        if source_coin_ids.len() > Self::MAX_MERGE_SOURCES {
            return Err(RuntimeError::InvalidTransaction(
                format!("Too many source coins: {} (max {})",
                    source_coin_ids.len(), Self::MAX_MERGE_SOURCES)
            ));
        }
        // R11: target must not appear in sources
        if source_coin_ids.contains(&target_coin_id) {
            return Err(RuntimeError::InvalidTransaction(
                format!("Target coin {} cannot also be a source", target_coin_id)
            ));
        }
        // R11: no duplicate sources
        {
            let mut seen = std::collections::HashSet::with_capacity(source_coin_ids.len());
            for id in source_coin_ids {
                if !seen.insert(id) {
                    return Err(RuntimeError::InvalidTransaction(
                        format!("Duplicate source coin: {}", id)
                    ));
                }
            }
        }
        
        let mut state_changes = Vec::new();
        let mut deleted_objects = Vec::new();
        
        // 1. Read target coin
        let mut target = self.state.get_object(&target_coin_id)?
            .ok_or(RuntimeError::ObjectNotFound(target_coin_id))?;
        
        let target_owner = target.metadata.owner.as_ref()
            .ok_or(RuntimeError::InvalidOwnership {
                object_id: target_coin_id,
                address: owner.to_string(),
            })?;
        if target_owner != owner {
            return Err(RuntimeError::InvalidOwnership {
                object_id: target_coin_id,
                address: owner.to_string(),
            });
        }
        
        let target_old_state = target.to_coin_state_bytes();
        let target_coin_type = target.data.coin_type.clone();
        
        // 2. Merge source coins one by one
        for &source_id in source_coin_ids {
            let source = self.state.get_object(&source_id)?
                .ok_or(RuntimeError::ObjectNotFound(source_id))?;
            
            let source_owner = source.metadata.owner.as_ref()
                .ok_or(RuntimeError::InvalidOwnership {
                    object_id: source_id,
                    address: owner.to_string(),
                })?;
            if source_owner != owner {
                return Err(RuntimeError::InvalidTransaction(
                    format!("Source coin {} not owned by {}", source_id, owner)
                ));
            }
            
            if source.data.coin_type != target_coin_type {
                return Err(RuntimeError::InvalidTransaction(
                    format!("Coin type mismatch: target={}, source={}",
                        target_coin_type, source.data.coin_type)
                ));
            }
            
            let source_old_state = source.to_coin_state_bytes();
            target.data.balance.deposit(source.data.balance)
                .map_err(|e| RuntimeError::InvalidTransaction(e))?;
            
            self.state.delete_object(&source_id)?;
            deleted_objects.push(source_id);
            state_changes.push(StateChange {
                change_type: StateChangeType::Delete,
                object_id: source_id,
                old_state: Some(source_old_state),
                new_state: None,
            });
        }
        
        // 3. Update target coin
        target.increment_version();
        let target_new_state = target.to_coin_state_bytes();
        self.state.set_object(target_coin_id, target)?;
        
        state_changes.insert(0, StateChange {
            change_type: StateChangeType::Update,
            object_id: target_coin_id,
            old_state: Some(target_old_state),
            new_state: Some(target_new_state),
        });
        
        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("Merged {} coins", source_coin_ids.len())),
            state_changes,
            created_objects: vec![],
            deleted_objects,
            query_result: None,
        })
    }
    
    /// Maximum number of split outputs in a single split operation.
    const MAX_SPLIT_OUTPUTS: usize = 50;
    
    /// Split a coin into multiple new coins.
    ///
    /// If total_split == balance (exact split), the source coin is deleted.
    /// Otherwise, the source coin's balance is reduced.
    pub fn execute_split_coin(
        &mut self,
        owner: &Address,
        source_coin_id: ObjectId,
        amounts: &[u64],
        ctx: &ExecutionContext,
    ) -> RuntimeResult<ExecutionOutput> {
        if amounts.is_empty() {
            return Err(RuntimeError::InvalidTransaction(
                "Split amounts cannot be empty".into()
            ));
        }
        if amounts.iter().any(|&a| a == 0) {
            return Err(RuntimeError::InvalidTransaction(
                "Split amount must be > 0 (zero-balance coins are not allowed)".into()
            ));
        }
        if amounts.len() > Self::MAX_SPLIT_OUTPUTS {
            return Err(RuntimeError::InvalidTransaction(
                format!("Too many split outputs: {} (max {})",
                    amounts.len(), Self::MAX_SPLIT_OUTPUTS)
            ));
        }
        
        let mut state_changes = Vec::new();
        let mut created_objects = Vec::new();
        let mut deleted_objects = Vec::new();
        
        // 1. Read and validate source coin
        let mut source = self.state.get_object(&source_coin_id)?
            .ok_or(RuntimeError::ObjectNotFound(source_coin_id))?;
        
        let source_owner = source.metadata.owner.as_ref()
            .ok_or(RuntimeError::InvalidOwnership {
                object_id: source_coin_id,
                address: owner.to_string(),
            })?;
        if source_owner != owner {
            return Err(RuntimeError::InvalidOwnership {
                object_id: source_coin_id,
                address: owner.to_string(),
            });
        }
        
        let source_old_state = source.to_coin_state_bytes();
        let total_split: u64 = amounts.iter()
            .try_fold(0u64, |acc, &a| acc.checked_add(a))
            .ok_or(RuntimeError::InvalidTransaction("Split amounts overflow u64".into()))?;
        
        if source.data.balance.value() < total_split {
            return Err(RuntimeError::InsufficientBalance {
                address: owner.to_string(),
                required: total_split,
                available: source.data.balance.value(),
            });
        }
        
        let is_exact_split = source.data.balance.value() == total_split;
        
        // 2. Create new coins
        for &amount in amounts {
            let new_coin_id = ctx.new_coin_id();
            let new_coin = create_coin_with_id(
                new_coin_id,
                owner.clone(),
                amount,
                source.data.coin_type.as_str(),
                ctx.timestamp,
            );
            let new_coin_state = new_coin.to_coin_state_bytes();
            
            self.state.set_object(new_coin_id, new_coin)?;
            created_objects.push(new_coin_id);
            
            state_changes.push(StateChange {
                change_type: StateChangeType::Create,
                object_id: new_coin_id,
                old_state: None,
                new_state: Some(new_coin_state),
            });
            
            let _ = source.data.balance.withdraw(amount)
                .map_err(|e| RuntimeError::InvalidTransaction(e))?;
        }
        
        // 3. Handle source coin
        if is_exact_split {
            // Exact split: delete source coin (prevent 0-balance zombie)
            self.state.delete_object(&source_coin_id)?;
            deleted_objects.push(source_coin_id);
            state_changes.insert(0, StateChange {
                change_type: StateChangeType::Delete,
                object_id: source_coin_id,
                old_state: Some(source_old_state),
                new_state: None,
            });
        } else {
            // Partial split: update source coin
            source.increment_version();
            let source_new_state = source.to_coin_state_bytes();
            self.state.set_object(source_coin_id, source)?;
            state_changes.insert(0, StateChange {
                change_type: StateChangeType::Update,
                object_id: source_coin_id,
                old_state: Some(source_old_state),
                new_state: Some(source_new_state),
            });
        }
        
        Ok(ExecutionOutput {
            success: true,
            message: Some(format!("Split into {} coins{}",
                amounts.len(),
                if is_exact_split { " (source deleted)" } else { "" }
            )),
            state_changes,
            created_objects,
            deleted_objects,
            query_result: None,
        })
    }
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InMemoryStateStore;
    
    /// Helper: create test ExecutionContext with deterministic tx_hash
    fn test_ctx(seed: &str) -> ExecutionContext {
        let tx_hash = *blake3::hash(format!("test-tx:{}", seed).as_bytes()).as_bytes();
        ExecutionContext::new("test-solver".to_string(), 1000, false, tx_hash)
    }
    
    #[test]
    fn test_full_transfer() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");
        
        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), None);
        let ctx = test_ctx("full-transfer");
        
        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        
        assert!(output.success);
        assert_eq!(output.state_changes.len(), 1);
        
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
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(300));
        let ctx = test_ctx("partial-transfer");
        
        let output = executor.execute_transaction(&tx, &ctx).unwrap();
        
        assert!(output.success);
        // always-create-new: new coin created for recipient
        assert_eq!(output.created_objects.len(), 1);
        // 2 state changes: sender Update + recipient Create
        assert_eq!(output.state_changes.len(), 2);
        assert_eq!(output.state_changes[1].change_type, StateChangeType::Create);
        
        // Verify sender balance reduced
        let original_coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(original_coin.data.balance.value(), 700);
        
        // Verify recipient coin created with tx-derived ID
        let new_coin_id = output.created_objects[0];
        let new_coin = executor.state().get_object(&new_coin_id).unwrap().unwrap();
        assert_eq!(new_coin.data.balance.value(), 300);
        assert_eq!(new_coin.metadata.owner.unwrap(), recipient);
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
        
        let before_total: u64 = 1000;
        
        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), None);
        let ctx = test_ctx("conservation-full");
        executor.execute_transaction(&tx, &ctx).unwrap();
        
        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        let after_total = coin.data.balance.value();
        assert_eq!(before_total, after_total, "Balance conservation violated in full transfer");
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
        let ctx = test_ctx("conservation-partial");
        executor.execute_transaction(&tx, &ctx).unwrap();
        
        // Sum: sender remaining + recipient new coin (tx-derived ID)
        let sender_coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        // Find recipient coin from get_owned_objects
        let recipient_objs = executor.state().get_owned_objects(&recipient).unwrap();
        let recipient_balance: u64 = recipient_objs.iter()
            .filter_map(|id| executor.state().get_object(id).ok().flatten())
            .map(|c| c.data.balance.value())
            .sum();
        let after_total = sender_coin.data.balance.value() + recipient_balance;
        assert_eq!(before_total, after_total, "Balance conservation violated in partial transfer");
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
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(1000));
        let ctx = test_ctx("conservation-exact");
        executor.execute_transaction(&tx, &ctx).unwrap();
        
        let coin = executor.state().get_object(&coin_id).unwrap().unwrap();
        let after_total = coin.data.balance.value();
        assert_eq!(before_total, after_total, "Balance conservation violated in amount==balance transfer");
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
        let ctx = test_ctx("insufficient");
        
        let result = executor.execute_transaction(&tx, &ctx);
        assert!(result.is_err(), "Should reject transfer exceeding balance");
    }
    
    #[test]
    fn test_transfer_amount_zero_rejected() {
        let mut store = InMemoryStateStore::new();
        let sender = Address::from_str_id("alice");
        let recipient = Address::from_str_id("bob");
        
        let coin = setu_types::create_coin(sender.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let tx = Transaction::new_transfer(sender.clone(), coin_id, recipient.clone(), Some(0));
        let ctx = test_ctx("zero-amount");
        
        let result = executor.execute_transaction(&tx, &ctx);
        assert!(result.is_err(), "Should reject transfer with amount == 0");
    }
    
    #[test]
    fn test_merge_coins() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        // Create 3 coins
        let coin1 = setu_types::create_coin(owner.clone(), 500);
        let id1 = *coin1.id();
        store.set_object(id1, coin1).unwrap();
        
        let coin2 = setu_types::create_coin(owner.clone(), 300);
        let id2 = *coin2.id();
        store.set_object(id2, coin2).unwrap();
        
        let coin3 = setu_types::create_coin(owner.clone(), 200);
        let id3 = *coin3.id();
        store.set_object(id3, coin3).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("merge");
        
        let output = executor.execute_merge_coins(&owner, id1, &[id2, id3], &ctx).unwrap();
        
        assert!(output.success);
        assert_eq!(output.deleted_objects.len(), 2);
        
        // Target coin has accumulated balance
        let merged = executor.state().get_object(&id1).unwrap().unwrap();
        assert_eq!(merged.data.balance.value(), 1000); // 500 + 300 + 200
        
        // Source coins deleted
        assert!(executor.state().get_object(&id2).unwrap().is_none());
        assert!(executor.state().get_object(&id3).unwrap().is_none());
    }
    
    #[test]
    fn test_merge_target_in_sources_rejected() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        let coin = setu_types::create_coin(owner.clone(), 500);
        let id = *coin.id();
        store.set_object(id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("merge-self");
        
        let result = executor.execute_merge_coins(&owner, id, &[id], &ctx);
        assert!(result.is_err(), "Target cannot be in sources");
    }
    
    #[test]
    fn test_split_coin() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        let coin = setu_types::create_coin(owner.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("split");
        
        let output = executor.execute_split_coin(&owner, coin_id, &[200, 300], &ctx).unwrap();
        
        assert!(output.success);
        assert_eq!(output.created_objects.len(), 2);
        
        // Source coin has reduced balance
        let source = executor.state().get_object(&coin_id).unwrap().unwrap();
        assert_eq!(source.data.balance.value(), 500); // 1000 - 200 - 300
        
        // New coins created with correct balances
        let new1 = executor.state().get_object(&output.created_objects[0]).unwrap().unwrap();
        assert_eq!(new1.data.balance.value(), 200);
        let new2 = executor.state().get_object(&output.created_objects[1]).unwrap().unwrap();
        assert_eq!(new2.data.balance.value(), 300);
        
        // Balance conservation
        let total = source.data.balance.value() + new1.data.balance.value() + new2.data.balance.value();
        assert_eq!(total, 1000);
    }
    
    #[test]
    fn test_split_exact_deletes_source() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        let coin = setu_types::create_coin(owner.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("exact-split");
        
        let output = executor.execute_split_coin(&owner, coin_id, &[600, 400], &ctx).unwrap();
        
        assert!(output.success);
        assert_eq!(output.deleted_objects.len(), 1);
        assert_eq!(output.deleted_objects[0], coin_id);
        
        // Source coin deleted
        assert!(executor.state().get_object(&coin_id).unwrap().is_none());
        
        // New coins balance conservation
        let new1 = executor.state().get_object(&output.created_objects[0]).unwrap().unwrap();
        let new2 = executor.state().get_object(&output.created_objects[1]).unwrap().unwrap();
        assert_eq!(new1.data.balance.value() + new2.data.balance.value(), 1000);
    }
    
    #[test]
    fn test_split_zero_amount_rejected() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        let coin = setu_types::create_coin(owner.clone(), 1000);
        let coin_id = *coin.id();
        store.set_object(coin_id, coin).unwrap();
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("split-zero");
        
        let result = executor.execute_split_coin(&owner, coin_id, &[0, 500], &ctx);
        assert!(result.is_err(), "Should reject split with zero amount");
    }
    
    #[test]
    fn test_mint_tokens_creates_new_coin() {
        let mut store = InMemoryStateStore::new();
        let owner = Address::from_str_id("alice");
        
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("mint");
        
        let output = executor.mint_tokens(&owner, "ROOT", 1000, &ctx).unwrap();
        
        assert!(output.success);
        assert_eq!(output.created_objects.len(), 1);
        
        let minted = executor.state().get_object(&output.created_objects[0]).unwrap().unwrap();
        assert_eq!(minted.data.balance.value(), 1000);
        assert_eq!(minted.metadata.owner.unwrap(), owner);
    }
    
    #[test]
    fn test_coin_id_deterministic_from_tx() {
        // Same tx_hash + output_index → same coin_id
        let tx_hash = *blake3::hash(b"test-tx").as_bytes();
        let id1 = coin_id_from_tx(&tx_hash, 0);
        let id2 = coin_id_from_tx(&tx_hash, 0);
        assert_eq!(id1, id2);
        
        // Different output_index → different coin_id
        let id3 = coin_id_from_tx(&tx_hash, 1);
        assert_ne!(id1, id3);
    }

    // ========== Phase 3: Profile & Subnet Membership Tests ==========

    #[test]
    fn test_execute_profile_update_creates_state_change() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("profile-1");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();
        let attrs = std::collections::HashMap::new();

        let output = executor.execute_profile_update(
            &addr, Some("Alice"), Some("https://img.example.com/a.png"), Some("Hello"), &attrs, &ctx,
        ).unwrap();

        assert!(output.success);
        assert_eq!(output.state_changes.len(), 1);
        assert_eq!(output.state_changes[0].change_type, StateChangeType::Create);
        assert!(output.state_changes[0].new_state.is_some());

        // Verify ObjectId uses SETU_PROFILE: domain hash
        let expected_oid = ObjectId::new(setu_types::hash_utils::setu_hash_with_domain(
            b"SETU_PROFILE:", format!("profile:{}", addr).as_bytes(),
        ));
        assert_eq!(output.state_changes[0].object_id, expected_oid);

        // Verify JSON content
        let data: serde_json::Value = serde_json::from_slice(output.state_changes[0].new_state.as_ref().unwrap()).unwrap();
        assert_eq!(data["display_name"], "Alice");
        assert_eq!(data["bio"], "Hello");
        assert_eq!(data["created_at"], ctx.timestamp);
    }

    #[test]
    fn test_execute_profile_update_idempotent() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("profile-idem");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();
        let attrs = std::collections::HashMap::new();

        let out1 = executor.execute_profile_update(&addr, Some("V1"), None, None, &attrs, &ctx).unwrap();
        let out2 = executor.execute_profile_update(&addr, Some("V2"), None, None, &attrs, &ctx).unwrap();

        assert!(out1.success);
        assert!(out2.success);
        assert_eq!(out1.state_changes.len(), 1);
        assert_eq!(out2.state_changes.len(), 1);
        // Same ObjectId (idempotent key)
        assert_eq!(out1.state_changes[0].object_id, out2.state_changes[0].object_id);
    }

    #[test]
    fn test_execute_subnet_join_creates_two_state_changes() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("join-1");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();

        let output = executor.execute_subnet_join(&addr, "defi-subnet", &ctx).unwrap();

        assert!(output.success);
        assert_eq!(output.state_changes.len(), 2);
        // Forward index uses SETU_MEMBERSHIP:
        let fwd_key = format!("user:{}:subnet:defi-subnet", addr);
        let expected_fwd = ObjectId::new(setu_types::hash_utils::setu_hash_with_domain(
            b"SETU_MEMBERSHIP:", fwd_key.as_bytes(),
        ));
        assert_eq!(output.state_changes[0].object_id, expected_fwd);
        assert_eq!(output.state_changes[0].change_type, StateChangeType::Create);

        // Reverse index uses SETU_SUBNET_MEMBER:
        let rev_key = format!("subnet:defi-subnet:member:{}", addr);
        let expected_rev = ObjectId::new(setu_types::hash_utils::setu_hash_with_domain(
            b"SETU_SUBNET_MEMBER:", rev_key.as_bytes(),
        ));
        assert_eq!(output.state_changes[1].object_id, expected_rev);
        assert_eq!(output.state_changes[1].change_type, StateChangeType::Create);
    }

    #[test]
    fn test_execute_subnet_join_state_change_content() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("join-content");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();

        let output = executor.execute_subnet_join(&addr, "gaming", &ctx).unwrap();

        // Check forward index content
        let fwd: serde_json::Value = serde_json::from_slice(output.state_changes[0].new_state.as_ref().unwrap()).unwrap();
        assert_eq!(fwd["user"], addr.to_string());
        assert_eq!(fwd["subnet_id"], "gaming");
        assert_eq!(fwd["joined_at"], ctx.timestamp);

        // Check reverse index content
        let rev: serde_json::Value = serde_json::from_slice(output.state_changes[1].new_state.as_ref().unwrap()).unwrap();
        assert_eq!(rev["user"], addr.to_string());
        assert_eq!(rev["subnet_id"], "gaming");
    }

    #[test]
    fn test_execute_subnet_leave_deletes_two_state_changes() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("leave-1");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();

        let output = executor.execute_subnet_leave(&addr, "defi-subnet", &ctx).unwrap();

        assert!(output.success);
        assert_eq!(output.state_changes.len(), 2);
        assert_eq!(output.state_changes[0].change_type, StateChangeType::Delete);
        assert_eq!(output.state_changes[1].change_type, StateChangeType::Delete);
        assert!(output.state_changes[0].new_state.is_none());
        assert!(output.state_changes[1].new_state.is_none());
    }

    #[test]
    fn test_execute_subnet_leave_object_ids_match_join() {
        let store = InMemoryStateStore::new();
        let mut executor = RuntimeExecutor::new(store);
        let ctx = test_ctx("symmetry");
        let addr = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();

        let join_out = executor.execute_subnet_join(&addr, "test-net", &ctx).unwrap();
        let leave_out = executor.execute_subnet_leave(&addr, "test-net", &ctx).unwrap();

        // Same ObjectIds for forward and reverse
        assert_eq!(join_out.state_changes[0].object_id, leave_out.state_changes[0].object_id);
        assert_eq!(join_out.state_changes[1].object_id, leave_out.state_changes[1].object_id);
    }
}
