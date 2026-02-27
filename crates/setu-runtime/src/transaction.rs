//! Transaction types for simple runtime

use serde::{Deserialize, Serialize};
use setu_types::{Address, ObjectId};
use std::collections::BTreeMap;

/// Transaction types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionType {
    /// Transfer transaction
    Transfer(TransferTx),
    /// Query transaction (read-only)
    Query(QueryTx),
    /// Program transaction (small deterministic instruction set)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryType {
    /// Query balance
    Balance,
    /// Query object
    Object,
    /// Query objects owned by an account
    OwnedObjects,
}

/// Program transaction payload.
///
/// This enables simple deterministic programmability without integrating a full VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramTx {
    /// Program bytecode (instruction sequence)
    pub instructions: Vec<Instruction>,
    /// Named input values available via `LoadInput`
    pub inputs: BTreeMap<String, ProgramValue>,
    /// Optional execution step limit override
    pub max_steps: Option<u64>,
}

/// Value type used by the program runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgramValue {
    U64(u64),
    Bool(bool),
}

/// Arithmetic and bitwise binary operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
}

/// Comparison operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Instruction set (10 opcodes total).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    Nop,
    Const {
        dst: u8,
        value: ProgramValue,
    },
    Mov {
        dst: u8,
        src: u8,
    },
    BinOp {
        op: BinaryOp,
        dst: u8,
        lhs: u8,
        rhs: u8,
    },
    Cmp {
        op: CompareOp,
        dst: u8,
        lhs: u8,
        rhs: u8,
    },
    LoadInput {
        dst: u8,
        key: String,
    },
    StoreOutput {
        key: String,
        src: u8,
    },
    Jump {
        pc: u16,
    },
    JumpIf {
        cond: u8,
        pc: u16,
    },
    Halt {
        success: bool,
        message: Option<String>,
    },
}

impl Transaction {
    /// Create a new transfer transaction
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

    /// Create a new programmable transaction.
    pub fn new_program(
        sender: Address,
        instructions: Vec<Instruction>,
        inputs: BTreeMap<String, ProgramValue>,
        max_steps: Option<u64>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = format!("prog_{:x}", timestamp);

        Self {
            id,
            sender,
            tx_type: TransactionType::Program(ProgramTx {
                instructions,
                inputs,
                max_steps,
            }),
            input_objects: vec![],
            timestamp,
        }
    }
}
