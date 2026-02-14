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
    /// Burn fee deducted from sender (destroyed, not transferred)
    #[serde(default)]
    pub burn_fee: u64,
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

impl Transaction {
    /// Create a new transfer transaction
    pub fn new_transfer(
        sender: Address,
        coin_id: ObjectId,
        recipient: Address,
        amount: Option<u64>,
    ) -> Self {
        Self::new_transfer_with_burn(sender, coin_id, recipient, amount, 0)
    }

    /// Create a transfer transaction with burn fee
    pub fn new_transfer_with_burn(
        sender: Address,
        coin_id: ObjectId,
        recipient: Address,
        amount: Option<u64>,
        burn_fee: u64,
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
                burn_fee,
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
}
