//! Explorer API module
//!
//! Provides HTTP endpoints for blockchain explorer functionality:
//! - Network statistics
//! - Anchor (block) queries
//! - Event (transaction) queries
//! - DAG visualization data
//! - Real-time causal graph streaming
//! - Search functionality

pub mod types;
pub mod stats;
pub mod anchors;
pub mod events;
pub mod dag;
pub mod search;
pub mod nodes;
pub mod account;

pub use types::*;

use crate::storage::ExplorerStorage;
use axum::{
    routing::get,
    Router,
};
use std::sync::Arc;

/// Create explorer API router
pub fn create_explorer_router(storage: Arc<ExplorerStorage>) -> Router {
    Router::new()
        // Statistics
        .route("/api/v1/explorer/stats", get(stats::get_stats))
        
        // Anchors
        .route("/api/v1/explorer/anchors", get(anchors::list_anchors))
        .route("/api/v1/explorer/anchor/:id", get(anchors::get_anchor_detail))
        
        // Events
        .route("/api/v1/explorer/events", get(events::list_events))
        .route("/api/v1/explorer/event/:id", get(events::get_event_detail))
        
        // DAG visualization
        .route("/api/v1/explorer/dag/live", get(dag::get_dag_live))
        .route("/api/v1/explorer/dag/path/:event_id", get(dag::get_causal_path))
        
        // Validators and Solvers
        .route("/api/v1/explorer/validators", get(nodes::get_validators))
        .route("/api/v1/explorer/validator/:id", get(nodes::get_validator))
        .route("/api/v1/explorer/solvers", get(nodes::get_solvers))
        .route("/api/v1/explorer/solver/:id", get(nodes::get_solver))
        
        // Search
        .route("/api/v1/explorer/search", get(search::search))
        
        // Account APIs (for wallet)
        .route("/api/v1/explorer/account/:address/balance", get(account::get_account_balance))
        .route("/api/v1/explorer/account/:address/coins", get(account::get_account_coins))
        .route("/api/v1/explorer/account/:address/activity", get(account::get_account_activity))
        .route("/api/v1/explorer/account/:address/transactions", get(account::get_account_activity))  // Alias for activity
        .route("/api/v1/explorer/transaction/:event_id", get(account::get_transaction_detail))
        
        // Token info (optional)
        .route("/api/v1/explorer/token/:coin_type", get(account::get_token_info))
        
        .with_state(storage)
}

