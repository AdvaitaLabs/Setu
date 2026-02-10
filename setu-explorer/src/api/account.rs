//! Account query endpoints for wallet

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use setu_types::{EventType, EventStatus, EventPayload, Address};
use hex;

// ========== Request/Response Types ==========

#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub address: String,
    pub balances: Vec<BalanceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_usd_value: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BalanceInfo {
    pub coin_type: String,
    pub amount: String,
    pub coin_count: usize,
    pub subnet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CoinQueryParams {
    #[serde(rename = "coinType")]
    pub coin_type: Option<String>,
    #[serde(rename = "subnetId")]
    pub subnet_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CoinsResponse {
    pub address: String,
    pub coins: Vec<CoinInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct CoinInfo {
    pub object_id: String,
    pub coin_type: String,
    pub balance: String,
    pub owner: String,
    pub version: u64,
    pub subnet_id: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct ActivityQueryParams {
    pub page: Option<usize>,
    pub limit: Option<usize>,
    #[serde(rename = "type")]
    pub activity_type: Option<String>, // "all", "sent", "received"
    #[serde(rename = "coinType")]
    pub coin_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActivityResponse {
    pub address: String,
    pub activities: Vec<ActivityItem>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
pub struct ActivityItem {
    pub id: String,
    #[serde(rename = "type")]
    pub activity_type: String, // "received" or "sent"
    pub status: String,
    pub coin_type: String,
    pub amount: String,
    pub from: String,
    pub to: String,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_time: Option<String>,
    pub subnet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_depth: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct TransactionDetailResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub tx_type: String,
    pub status: String,
    pub coin_type: String,
    pub amount: String,
    pub from: String,
    pub to: String,
    pub timestamp: u64,
    pub subnet_id: String,
    pub event_info: EventInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_result: Option<ExecutionResultInfo>,
}

#[derive(Debug, Serialize)]
pub struct EventInfo {
    pub event_id: String,
    pub creator: String,
    pub parent_ids: Vec<String>,
    pub vlc_time: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_depth: Option<u64>,
}

// ========== API Handlers ==========

/// GET /api/v1/explorer/account/:address/balance
/// 
/// Query account balance (aggregated by coin type)
pub async fn get_account_balance(
    Path(address): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<BalanceResponse>, StatusCode> {
    tracing::info!("GET /account/{}/balance", address);
    
    // Parse address (support both 20-byte and 32-byte addresses)
    let addr = parse_address(&address)
        .map_err(|e| {
            tracing::error!("Failed to parse address {}: {:?}", address, e);
            StatusCode::BAD_REQUEST
        })?;
    
    tracing::debug!("Parsed address: {:?}", addr);
    
    // Get all coins owned by this address
    let coins = storage.get_coins_by_owner(&addr)
        .map_err(|e| {
            tracing::error!("Failed to get coins for {}: {:?}", address, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    tracing::info!("Found {} coins for address {}", coins.len(), address);
    
    // Filter only FLUX coins from subnet-0
    let flux_coins: Vec<_> = coins
        .into_iter()
        .filter(|coin| coin.coin_type().as_str() == "FLUX")
        .collect();
    
    // Calculate total balance
    let total_balance: u64 = flux_coins.iter().map(|c| c.value()).sum();
    
    let balance_info = BalanceInfo {
        coin_type: "FLUX".to_string(),
        amount: total_balance.to_string(),
        coin_count: flux_coins.len(),
        subnet_id: "subnet-0".to_string(),
        usd_value: None,
    };
    
    tracing::info!("Returning balance response for {}: {} FLUX", address, total_balance);
    
    Ok(Json(BalanceResponse {
        address: address.clone(),
        balances: vec![balance_info],
        total_usd_value: None,
    }))
}

/// GET /api/v1/explorer/account/:address/coins
/// 
/// Query account's Coin Objects
pub async fn get_account_coins(
    Path(address): Path<String>,
    Query(params): Query<CoinQueryParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<CoinsResponse>, StatusCode> {
    // Parse address
    let addr = parse_address(&address)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    
    // Get coins
    let coins = if let Some(coin_type_str) = params.coin_type {
        let coin_type = setu_types::CoinType::new(coin_type_str);
        storage.get_coins_by_owner_and_type(&addr, &coin_type)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        storage.get_coins_by_owner(&addr)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    
    // Convert to response format
    let coin_infos: Vec<CoinInfo> = coins
        .iter()
        .map(|coin| CoinInfo {
            object_id: coin.id().to_string(),
            coin_type: coin.coin_type().as_str().to_string(),
            balance: coin.value().to_string(),
            owner: coin.owner().map(|a| a.to_string()).unwrap_or_default(),
            version: coin.version(),
            subnet_id: "subnet-0".to_string(), // TODO: get from coin metadata
            created_at: coin.metadata.created_at,
            updated_at: coin.metadata.updated_at,
        })
        .collect();
    
    let total = coin_infos.len();
    
    Ok(Json(CoinsResponse {
        address: address.clone(),
        coins: coin_infos,
        total,
    }))
}

/// GET /api/v1/explorer/account/:address/activity
/// 
/// Query account's transaction history (activity)
pub async fn get_account_activity(
    Path(address): Path<String>,
    Query(params): Query<ActivityQueryParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<ActivityResponse>, StatusCode> {
    // 获取该地址相关的所有 Transfer 事件
    let all_events = storage.get_events_by_creator(&address).await;
    
    // 筛选 Transfer 事件
    let mut transfer_events: Vec<_> = all_events
        .into_iter()
        .filter(|e| matches!(e.event_type, EventType::Transfer))
        .collect();
    
    // 按时间戳排序（最新的在前）
    transfer_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    
    // 根据 type 参数筛选
    let activity_type = params.activity_type.as_deref().unwrap_or("all");
    let filtered_events: Vec<_> = transfer_events
        .into_iter()
        .filter(|event| {
            if let EventPayload::Transfer(transfer) = &event.payload {
                match activity_type {
                    "sent" => transfer.from == address,
                    "received" => transfer.to == address,
                    _ => transfer.from == address || transfer.to == address,
                }
            } else {
                false
            }
        })
        .collect();
    
    let total = filtered_events.len();
    
    // 分页
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).min(100);
    let total_pages = (total + limit - 1) / limit;
    let start = (page - 1) * limit;
    let end = (start + limit).min(total);
    
    let page_events = &filtered_events[start..end];
    
    // 构建响应
    let mut activities = Vec::new();
    for event in page_events {
        if let EventPayload::Transfer(transfer) = &event.payload {
            let is_received = transfer.to == address;
            
            activities.push(ActivityItem {
                id: event.id.to_string(),
                activity_type: if is_received { "received" } else { "sent" }.to_string(),
                status: format!("{:?}", event.status).to_lowercase(),
                coin_type: "FLUX".to_string(), // 简化：假设都是 FLUX
                amount: transfer.amount.to_string(),
                from: transfer.from.clone(),
                to: transfer.to.clone(),
                timestamp: event.timestamp,
                relative_time: Some(format_relative_time(event.timestamp)),
                subnet_id: event.subnet_id.as_ref()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "subnet-0".to_string()),
                anchor_id: None, // TODO: 从 EventStore 获取
                anchor_depth: None,
            });
        }
    }
    
    Ok(Json(ActivityResponse {
        address: address.clone(),
        activities,
        pagination: Pagination {
            page,
            limit,
            total,
            total_pages,
        },
    }))
}

/// GET /api/v1/explorer/transaction/:event_id
/// 
/// Query transaction details
pub async fn get_transaction_detail(
    Path(event_id): Path<String>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Result<Json<TransactionDetailResponse>, StatusCode> {
    // 获取 Event
    let event = storage
        .get_event(&event_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    
    // 只处理 Transfer 事件
    if let EventPayload::Transfer(transfer) = &event.payload {
        let execution_result = event.execution_result.as_ref().map(|result| {
            ExecutionResultInfo {
                success: result.success,
                message: result.message.clone().unwrap_or_default(),
                state_changes: result
                    .state_changes
                    .iter()
                    .map(|change| StateChange {
                        key: change.key.clone(),
                        old_value: change.old_value.as_ref().map(|v| String::from_utf8_lossy(v).to_string()),
                        new_value: change.new_value.as_ref().map(|v| String::from_utf8_lossy(v).to_string()),
                    })
                    .collect(),
            }
        });
        
        Ok(Json(TransactionDetailResponse {
            id: event.id.to_string(),
            tx_type: "transfer".to_string(),
            status: format!("{:?}", event.status).to_lowercase(),
            coin_type: "FLUX".to_string(),
            amount: transfer.amount.to_string(),
            from: transfer.from.clone(),
            to: transfer.to.clone(),
            timestamp: event.timestamp,
            subnet_id: event.subnet_id.as_ref()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "subnet-0".to_string()),
            event_info: EventInfo {
                event_id: event.id.to_string(),
                creator: event.creator.clone(),
                parent_ids: event.parent_ids.iter().map(|id| id.to_string()).collect(),
                vlc_time: event.vlc_snapshot.logical_time,
                anchor_id: None, // TODO: 从 EventStore 获取
                anchor_depth: None,
            },
            execution_result,
        }))
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

// ========== Helper Functions ==========

/// Parse address from hex string (supports both 20-byte and 32-byte addresses)
fn parse_address(hex_str: &str) -> Result<Address, &'static str> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str).map_err(|_| "Invalid hex string")?;
    
    match bytes.len() {
        20 => {
            // Ethereum-style 20-byte address: pad with zeros to 32 bytes
            let mut padded = [0u8; 32];
            padded[12..32].copy_from_slice(&bytes);
            Address::from_bytes(&padded)
        }
        32 => {
            // Native 32-byte address
            Address::from_bytes(&bytes)
        }
        _ => Err("Address must be 20 or 32 bytes")
    }
}

/// Format relative time (e.g., "2 hours ago")
fn format_relative_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    
    let diff = now.saturating_sub(timestamp);
    let seconds = diff / 1000;
    
    if seconds < 60 {
        format!("{} seconds ago", seconds)
    } else if seconds < 3600 {
        format!("{} minutes ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{} hours ago", seconds / 3600)
    } else {
        format!("{} days ago", seconds / 86400)
    }
}

