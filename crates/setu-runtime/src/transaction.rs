//! Transaction types for simple runtime

use serde::{Deserialize, Serialize};
use setu_types::{Address, ObjectId};

/// Transaction types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionType {
    /// Transfer transaction
    Transfer(TransferTx),
    /// Query transaction (read-only)
    Query(QueryTx),
    /// Move-style script transaction (typed stack VM)
    MoveScript(MoveScriptTx),
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

/// Move-style script payload for programmable execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveScriptTx {
    /// Bytecode instruction stream
    pub code: Vec<Bytecode>,
    /// Local variable signature table (slot types)
    pub locals_sig: Vec<SignatureToken>,
    /// Parameter signature (first N locals are parameters)
    pub params_sig: Vec<SignatureToken>,
    /// Return signature (stack shape expected at `Ret`)
    pub return_sig: Vec<SignatureToken>,
    /// Runtime argument values
    pub args: Vec<MoveValue>,
    /// Generic type arguments (reserved for phase-2+ features)
    pub type_args: Vec<TypeTag>,
    /// Gas budget for script execution
    pub max_gas: u64,
    /// Declared input objects this script can access
    pub input_objects: Vec<ObjectId>,
}

/// Runtime value type for the Move-style interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveValue {
    U64(u64),
    Bool(bool),
    Address(Address),
    Vector(Vec<MoveValue>),
}

/// Type signatures used by verifier/interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureToken {
    Bool,
    U64,
    Address,
    Vector(Box<SignatureToken>),
}

/// Type tags for script generic arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeTag {
    Bool,
    U64,
    Address,
    Vector(Box<TypeTag>),
}

/// Move-style phase-1 bytecode set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Bytecode {
    // Constants
    LdU64(u64),
    LdTrue,
    LdFalse,
    // Locals
    CopyLoc(u8),
    MoveLoc(u8),
    StLoc(u8),
    Pop,
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Comparison
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    // Control flow
    BrTrue(u16),
    BrFalse(u16),
    Branch(u16),
    // Termination
    Ret,
    Abort { code: u64, message: Option<String> },
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

    /// Create a new Move-style script transaction.
    pub fn new_move_script(sender: Address, script: MoveScriptTx) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = format!("move_{:x}", timestamp);

        Self {
            id,
            sender,
            input_objects: script.input_objects.clone(),
            tx_type: TransactionType::MoveScript(script),
            timestamp,
        }
    }
}
