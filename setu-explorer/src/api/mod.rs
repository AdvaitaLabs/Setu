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
        
        // Search
        .route("/api/v1/explorer/search", get(search::search))
        
        .with_state(storage)
}

