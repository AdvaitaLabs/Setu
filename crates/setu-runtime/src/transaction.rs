//! Transaction types for simple runtime

use serde::{Deserialize, Serialize};
use setu_types::{Address, ObjectId};

/// Transaction 类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionType {
    /// 转账交易
    Transfer(TransferTx),
    /// 查询交易（只读）
    Query(QueryTx),
}

/// 简化的交易结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// 交易 ID
    pub id: String,
    /// 发送者地址
    pub sender: Address,
    /// 交易类型
    pub tx_type: TransactionType,
    /// 输入对象（依赖的对象）
    pub input_objects: Vec<ObjectId>,
    /// 时间戳
    pub timestamp: u64,
}

/// 转账交易
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTx {
    /// Coin 对象 ID
    pub coin_id: ObjectId,
    /// 接收者地址
    pub recipient: Address,
    /// 转账金额（如果是部分转账）
    pub amount: Option<u64>,
}

/// 查询交易（只读）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTx {
    /// 查询类型
    pub query_type: QueryType,
    /// 查询参数
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryType {
    /// 查询余额
    Balance,
    /// 查询对象
    Object,
    /// 查询账户拥有的对象
    OwnedObjects,
}

impl Transaction {
    /// 创建一个新的转账交易
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
    
    /// 创建一个余额查询交易
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
