//! Transaction types for simple runtime

use crate::program_vm::Program;
use serde::{Deserialize, Serialize};
use setu_types::{Address, ObjectId};

/// Transaction types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionType {
    /// Transfer transaction
    Transfer(TransferTx),
    /// Query transaction (read-only)
    Query(QueryTx),
    /// Program transaction (VM execution)
    Program(ProgramTx),
}

/// Simplified transaction structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction ID
    pub id: String,
    /// Sender address
    pub sender: Address,
    /// Transaction type
    pub tx_type: TransactionType,
    /// Input objects (dependent objects)
    pub input_objects: Vec<ObjectId>,
    /// Timestamp
    pub timestamp: u64,
}

/// Transfer transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTx {
    /// Coin object ID
    pub coin_id: ObjectId,
    /// Recipient address
    pub recipient: Address,
    /// Transfer amount (if partial transfer)
    pub amount: Option<u64>,
}

/// Query transaction (read-only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTx {
    /// Query type
    pub query_type: QueryType,
    /// Query parameters
    pub params: serde_json::Value,
}

/// Program transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramTx {
    /// Program bytecode/instructions
    pub program: Program,
    /// Optional gas budget (reserved for future accounting)
    pub gas_budget: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryType {
    /// Query balance
    Balance,
    /// Query object
    Object,
    /// Query objects owned by an account
    OwnedObjects,
}

impl Transaction {
    /// Create a new transfer transaction (non-consensus path only).
    ///
    /// ⚠️ Uses `SystemTime::now()` — **NOT** safe for TEE/consensus paths.
    /// For TEE/Enclave execution, use [`new_transfer_deterministic`] instead.
    pub fn new_transfer(
        sender: Address,
        coin_id: ObjectId,
        recipient: Address,
        amount: Option<u64>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = format!("tx_{:x}", timestamp);

        Self {
            id,
            sender,
            tx_type: TransactionType::Transfer(TransferTx {
                coin_id,
                recipient,
                amount,
            }),
            input_objects: vec![coin_id],
            timestamp,
        }
    }

    /// Deterministic transfer constructor for TEE/consensus paths.
    ///
    /// Unlike [`new_transfer`] which uses `SystemTime::now()`, this constructor
    /// derives `id` and `timestamp` from the execution context, ensuring all
    /// validator nodes produce identical `Transaction` values for the same input.
    ///
    /// # Arguments
    /// * `ctx_timestamp` — deterministic timestamp from `ExecutionContext.timestamp`
    ///   (originally sourced from the Event, identical across all validators)
    ///
    /// # When to use
    /// - `execute_transfer_with_coin` (Enclave entry point)
    /// - `execute_simple_transfer` (auto-select path)
    /// - `MergeThenTransfer` Step 2
    /// - Any path where multiple validators must produce the same state
    ///
    /// # When NOT to use
    /// - RPC/CLI convenience paths (non-consensus, `SystemTime` acceptable)
    /// - Tests (use `new_transfer` for brevity)
    pub fn new_transfer_deterministic(
        sender: Address,
        coin_id: ObjectId,
        recipient: Address,
        amount: Option<u64>,
        ctx_timestamp: u64,
    ) -> Self {
        // Derive id from coin_id + timestamp — both deterministic across nodes
        let id = format!("tx_{}_{:x}", &coin_id.to_string()[..8], ctx_timestamp);

        Self {
            id,
            sender,
            tx_type: TransactionType::Transfer(TransferTx {
                coin_id,
                recipient,
                amount,
            }),
            input_objects: vec![coin_id],
            timestamp: ctx_timestamp,
        }
    }

    /// Create a new balance query transaction
    pub fn new_balance_query(address: Address) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = format!("query_{:x}", timestamp);

        Self {
            id,
            sender: address.clone(),
            tx_type: TransactionType::Query(QueryTx {
                query_type: QueryType::Balance,
                params: serde_json::json!({ "address": address }),
            }),
            input_objects: vec![],
            timestamp,
        }
    }

    /// Create a new VM program transaction
    pub fn new_program(sender: Address, program: Program) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = format!("prog_{:x}", timestamp);

        Self {
            id,
            sender,
            tx_type: TransactionType::Program(ProgramTx {
                program,
                gas_budget: None,
            }),
            input_objects: vec![],
            timestamp,
        }
    }

    /// Create a deterministic VM program transaction
    pub fn new_program_deterministic(
        sender: Address,
        program: Program,
        ctx_timestamp: u64,
    ) -> Self {
        let id = format!("prog_{:x}", ctx_timestamp);

        Self {
            id,
            sender,
            tx_type: TransactionType::Program(ProgramTx {
                program,
                gas_budget: None,
            }),
            input_objects: vec![],
            timestamp: ctx_timestamp,
        }
    }
}
