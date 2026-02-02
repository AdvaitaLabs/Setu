//! Search endpoint

use super::types::*;
use crate::storage::ExplorerStorage;
use axum::{
    extract::{Query, State},
    Json,
};
use std::sync::Arc;

/// GET /api/v1/explorer/search
/// 
/// Search for anchors, events, or accounts by ID/address
pub async fn search(
    Query(params): Query<SearchParams>,
    State(storage): State<Arc<ExplorerStorage>>,
) -> Json<SearchResponse> {
    let query = params.q.trim();
    let mut results = Vec::new();
    
    if query.is_empty() {
        return Json(SearchResponse { results });
    }
    
    // Try to find anchor
    if let Some(anchor) = storage.get_anchor(query).await {
        let mut extra = std::collections::HashMap::new();
        extra.insert("depth".to_string(), serde_json::json!(anchor.depth));
        extra.insert("event_count".to_string(), serde_json::json!(anchor.event_ids.len()));
        
        results.push(SearchResult {
            result_type: "anchor".to_string(),
            id: anchor.id.to_string(),
            url: format!("/anchor/{}", anchor.id),
            extra,
        });
    }
    
    // Try to find event
    if let Some(event) = storage.get_event(query).await {
        let mut extra = std::collections::HashMap::new();
        extra.insert("type".to_string(), serde_json::json!(format!("{:?}", event.event_type)));
        extra.insert("status".to_string(), serde_json::json!(format!("{:?}", event.status)));
        
        results.push(SearchResult {
            result_type: "event".to_string(),
            id: event.id.to_string(),
            url: format!("/event/{}", event.id),
            extra,
        });
    }
    
    // Try to find account (if query looks like an address)
    if query.starts_with("0x") && query.len() == 42 {
        // TODO: Check if account exists in storage
        let mut extra = std::collections::HashMap::new();
        extra.insert("address".to_string(), serde_json::json!(query));
        
        results.push(SearchResult {
            result_type: "account".to_string(),
            id: query.to_string(),
            url: format!("/account/{}", query),
            extra,
        });
    }
    
    Json(SearchResponse { results })
}

