//! Setu Explorer - Blockchain Browser Service
//!
//! Independent service providing read-only API for blockchain exploration.
//! Can be deployed separately from validator nodes.

mod api;
mod config;
mod storage;

use crate::config::{ExplorerConfig, StorageMode};
use crate::storage::ExplorerStorage;
use std::sync::Arc;
use tower_http::cors::{CorsLayer, Any};
use tracing::{info, error};
use tracing_subscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(true)
        .init();

    // Load configuration
    let config = ExplorerConfig::from_env();
    
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Setu Explorer Service Starting                ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Listen Addr:  {:^44} ║", config.listen_addr);
    info!("║  Storage Mode: {:^44} ║", format!("{:?}", config.storage_mode));
    
    // Validate configuration
    if let Err(e) = config.validate() {
        error!("Configuration validation failed: {}", e);
        return Err(e);
    }
    
    // Initialize storage
    let storage = match config.storage_mode {
        StorageMode::DirectRocksDB => {
            let db_path = config.db_path.as_ref().unwrap();
            info!("║  DB Path:      {:^44} ║", db_path.display());
            info!("╚════════════════════════════════════════════════════════════╝");
            
            info!("Opening RocksDB in read-only mode...");
            match ExplorerStorage::open_readonly(db_path) {
                Ok(storage) => {
                    info!("✓ Storage initialized successfully");
                    Arc::new(storage)
                }
                Err(e) => {
                    error!("Failed to open storage: {}", e);
                    error!("Make sure the validator is running and the DB path is correct");
                    return Err(e);
                }
            }
        }
        StorageMode::RPC => {
            let rpc_url = config.validator_rpc_url.as_ref().unwrap();
            info!("║  RPC URL:      {:^44} ║", rpc_url);
            info!("╚════════════════════════════════════════════════════════════╝");
            
            error!("RPC mode is not yet implemented");
            anyhow::bail!("RPC mode is not yet implemented");
        }
    };
    
    // Create API router
    let mut app = api::create_explorer_router(storage);
    
    // Add CORS if enabled
    if config.enable_cors {
        info!("CORS enabled for all origins");
        app = app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        );
    }
    
    // Start HTTP server
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Explorer API Ready                            ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Explorer Endpoints:                                       ║");
    info!("║    GET  /api/v1/explorer/stats                             ║");
    info!("║    GET  /api/v1/explorer/anchors                           ║");
    info!("║    GET  /api/v1/explorer/anchor/:id                        ║");
    info!("║    GET  /api/v1/explorer/events                            ║");
    info!("║    GET  /api/v1/explorer/event/:id                         ║");
    info!("║    GET  /api/v1/explorer/dag/live                          ║");
    info!("║    GET  /api/v1/explorer/dag/path/:event_id                ║");
    info!("║    GET  /api/v1/explorer/search                            ║");
    info!("║                                                            ║");
    info!("║  Account Endpoints (Wallet):                               ║");
    info!("║    GET  /api/v1/explorer/account/:address/balance          ║");
    info!("║    GET  /api/v1/explorer/account/:address/coins            ║");
    info!("║    GET  /api/v1/explorer/account/:address/activity         ║");
    info!("║    GET  /api/v1/explorer/transaction/:event_id             ║");
    info!("╚════════════════════════════════════════════════════════════╝");
    
    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!("Listening on {}", config.listen_addr);
    
    axum::serve(listener, app).await?;
    
    Ok(())
}
