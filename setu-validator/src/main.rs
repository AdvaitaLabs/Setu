//! Setu Validator - Main entry point
//!
//! The Validator node is responsible for:
//! - Receiving and routing transfers to Solvers
//! - Verifying events from Solvers
//! - Managing the DAG with consensus
//! - Coordinating consensus (leader election, CF voting)
//! - Providing HTTP API for registration and transfer submission

use setu_core::NodeConfig;
use setu_validator::{
    RouterManager, 
    ValidatorNetworkService, NetworkServiceConfig,
    ConsensusValidator, ConsensusValidatorConfig,
};
use setu_types::NodeInfo;
use std::sync::Arc;
use std::net::SocketAddr;
use tracing::{info, error, Level};
use tracing_subscriber;

/// Validator configuration from environment
#[derive(Debug, Clone)]
struct ValidatorConfig {
    /// Node configuration
    node_config: NodeConfig,
    /// HTTP API listen address
    http_addr: SocketAddr,
    /// P2P listen address (for future use)
    p2p_addr: SocketAddr,
}

impl ValidatorConfig {
    fn from_env() -> Self {
        let node_config = NodeConfig::from_env();
        
        // HTTP API port (default: 8080)
        let http_port: u16 = std::env::var("VALIDATOR_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8080);
        
        // P2P port (default: 9000)
        let p2p_port: u16 = std::env::var("VALIDATOR_P2P_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9000);
        
        let listen_addr = std::env::var("VALIDATOR_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
        Self {
            node_config,
            http_addr: format!("{}:{}", listen_addr, http_port).parse().unwrap(),
            p2p_addr: format!("{}:{}", listen_addr, p2p_port).parse().unwrap(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with more detailed output
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_thread_ids(true)
        .init();

    // Load configuration
    let config = ValidatorConfig::from_env();
    
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Setu Validator Node Starting                  ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Node ID:    {:^44} ║", config.node_config.node_id);
    info!("║  HTTP API:   {:^44} ║", config.http_addr);
    info!("║  P2P Addr:   {:^44} ║", config.p2p_addr);
    info!("╚════════════════════════════════════════════════════════════╝");

    // Create router manager (shared between NetworkService components)
    let router_manager = Arc::new(RouterManager::new());
    
    // Create task preparer (solver-tee3 architecture)
    // Uses real MerkleStateProvider with pre-initialized test accounts
    let task_preparer = Arc::new(setu_validator::TaskPreparer::new_for_testing(
        config.node_config.node_id.clone(),
    ));
    
    info!("✓ TaskPreparer initialized with test accounts (alice, bob, charlie)");
    
    // Create ConsensusValidator for DAG + VLC + Consensus
    let node_info = NodeInfo::new_validator(
        config.node_config.node_id.clone(),
        config.http_addr.ip().to_string(),
        config.http_addr.port(),
    );
    let consensus_config = ConsensusValidatorConfig {
        node_info,
        is_leader: true, // Single node mode: always leader
        ..Default::default()
    };
    let consensus_validator = Arc::new(ConsensusValidator::new(consensus_config));
    
    info!("✓ ConsensusValidator initialized (single node mode)");
    
    // Create network service configuration
    let network_config = NetworkServiceConfig {
        http_listen_addr: config.http_addr,
        p2p_listen_addr: config.p2p_addr,
    };
    
    // Create network service with consensus enabled
    let network_service = Arc::new(ValidatorNetworkService::with_consensus(
        config.node_config.node_id.clone(),
        router_manager.clone(),
        task_preparer.clone(),
        consensus_validator.clone(),
        network_config,
    ));

    // ========================================
    // Components Status
    // ========================================
    
    info!("┌─────────────────────────────────────────────────────────────┐");
    info!("│                  Components Initialized                     │");
    info!("├─────────────────────────────────────────────────────────────┤");
    
    // VLC (Vector Logical Clock) - Real implementation
    info!("│ [VLC]       Vector Logical Clock initialized               │");
    info!("│             - Managed by ConsensusEngine                   │");
    info!("│             - Increments on each event                     │");
    
    // DAG Manager - Real implementation  
    info!("│ [DAG]       DAG Manager initialized                        │");
    info!("│             - Events stored in ConsensusEngine DAG         │");
    info!("│             - Parent tracking: enabled                     │");
    
    // Consensus - Real implementation
    info!("│ [CONSENSUS] Consensus module initialized                   │");
    info!("│             - Mode: Single validator (leader)              │");
    info!("│             - CF creation and finalization enabled         │");
    
    // AnchorBuilder
    info!("│ [ANCHOR]    AnchorBuilder initialized                      │");
    info!("│             - Merkle tree computation enabled              │");
    info!("│             - State persistence ready                      │");
    
    info!("└─────────────────────────────────────────────────────────────┘");

    // Spawn HTTP server
    let http_service = network_service.clone();
    let http_handle = tokio::spawn(async move {
        info!("Starting HTTP API server...");
        if let Err(e) = http_service.start_http_server().await {
            error!("HTTP server error: {}", e);
        }
    });

    // Log startup complete
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Validator Ready for Connections               ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  HTTP API Endpoints:                                       ║");
    info!("║    POST /api/v1/register/solver    - Register a solver     ║");
    info!("║    POST /api/v1/register/validator - Register a validator  ║");
    info!("║    GET  /api/v1/solvers            - List solvers          ║");
    info!("║    GET  /api/v1/validators         - List validators       ║");
    info!("║    GET  /api/v1/health             - Health check          ║");
    info!("║    POST /api/v1/transfer           - Submit transfer       ║");
    info!("║    POST /api/v1/event              - Submit event (Solver) ║");
    info!("║    GET  /api/v1/events             - List events           ║");
    info!("╚════════════════════════════════════════════════════════════╝");

    // Wait for shutdown signal
    tokio::select! {
        _ = http_handle => {
            info!("HTTP server stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
        }
    }

    info!("Validator shutdown complete");
    Ok(())
}