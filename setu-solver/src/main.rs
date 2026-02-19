//! Setu Solver - Main entry point (solver-tee3 Architecture)
//!
//! ## solver-tee3 Architecture
//!
//! In this architecture, Solver is a **pass-through** node that:
//! 1. Receives fully-prepared `SolverTask` from Validator
//! 2. Passes it to TEE for execution
//! 3. Returns `TeeExecutionResult` back to Validator
//!
//! ## What Solver does NOT do (handled by Validator)
//! - Coin selection
//! - State reads
//! - VLC management
//! - Event creation

use setu_keys::{load_keypair, KeyPair};
use setu_solver::{
    start_server, SolverNetworkClient, SolverNetworkConfig, SolverTask, TeeExecutionResult,
    TeeExecutor,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn, Level};

/// Solver configuration from environment
#[derive(Debug, Clone)]
struct SolverConfig {
    /// Solver ID
    solver_id: String,
    /// Solver listen address
    address: String,
    /// Solver listen port
    port: u16,
    /// Maximum capacity (concurrent tasks)
    capacity: u32,
    /// Shard assignment
    shard_id: Option<String>,
    /// Resource types this solver handles
    resources: Vec<String>,
    /// Validator address
    validator_address: String,
    /// Validator HTTP port
    validator_port: u16,
    /// Heartbeat interval
    heartbeat_interval_secs: u64,
    /// Auto-register on startup
    auto_register: bool,
    /// Key file path (optional)
    key_file: Option<String>,
}

impl SolverConfig {
    fn from_env() -> Self {
        let solver_id = std::env::var("SOLVER_ID").unwrap_or_else(|_| {
            format!(
                "solver-{}",
                uuid::Uuid::new_v4().to_string()[..8].to_string()
            )
        });

        let address =
            std::env::var("SOLVER_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1".to_string());

        let port: u16 = std::env::var("SOLVER_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9001);

        let capacity: u32 = std::env::var("SOLVER_CAPACITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);

        let shard_id = std::env::var("SOLVER_SHARD_ID").ok();

        let resources: Vec<String> = std::env::var("SOLVER_RESOURCES")
            .ok()
            .map(|s| s.split(',').map(|r| r.trim().to_string()).collect())
            .unwrap_or_default();

        let validator_address =
            std::env::var("VALIDATOR_ADDRESS").unwrap_or_else(|_| "127.0.0.1".to_string());

        let validator_port: u16 = std::env::var("VALIDATOR_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8080);

        let heartbeat_interval_secs: u64 = std::env::var("HEARTBEAT_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let auto_register = std::env::var("AUTO_REGISTER")
            .map(|s| s.to_lowercase() != "false" && s != "0")
            .unwrap_or(true);

        let key_file = std::env::var("SOLVER_KEY_FILE").ok();

        Self {
            solver_id,
            address,
            port,
            capacity,
            shard_id,
            resources,
            validator_address,
            validator_port,
            heartbeat_interval_secs,
            auto_register,
            key_file,
        }
    }
}

/// Load keypair from file and extract registration info
fn load_key_info(key_file: &str) -> anyhow::Result<(String, Vec<u8>, Vec<u8>)> {
    info!("Loading keypair from: {}", key_file);

    let keypair = load_keypair(key_file)?;
    let account_address = keypair.address().to_string();
    let public_key = keypair.public().as_bytes();

    // Create registration message to sign
    let message = format!("Register Solver: {}", account_address);
    let signature = keypair.sign(message.as_bytes()).as_bytes();

    info!("Keypair loaded successfully");
    info!("  Account Address: {}", account_address);
    info!("  Public Key: {}", hex::encode(&public_key));

    Ok((account_address, public_key, signature))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_thread_ids(true)
        .init();

    // Load configuration
    let config = SolverConfig::from_env();

    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║         Setu Solver Node (solver-tee3 Architecture)        ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Solver ID:  {:^44} ║", &config.solver_id);
    info!(
        "║  Address:    {:^44} ║",
        format!("{}:{}", config.address, config.port)
    );
    info!("║  Capacity:   {:^44} ║", config.capacity);
    info!(
        "║  Shard:      {:^44} ║",
        config.shard_id.as_deref().unwrap_or("default")
    );
    info!(
        "║  Validator:  {:^44} ║",
        format!("{}:{}", config.validator_address, config.validator_port)
    );
    info!("╚════════════════════════════════════════════════════════════╝");

    // Initialize TEE Executor
    let tee_executor = Arc::new(TeeExecutor::new(config.solver_id.clone()));

    info!("┌─────────────────────────────────────────────────────────────┐");
    info!("│                  TEE Executor Initialized                   │");
    info!("├─────────────────────────────────────────────────────────────┤");
    info!(
        "│ Enclave:     {:^44} │",
        tee_executor.enclave_info().enclave_id
    );
    info!(
        "│ Version:     {:^44} │",
        tee_executor.enclave_info().version
    );
    info!(
        "│ Mode:        {:^44} │",
        if tee_executor.enclave_info().is_simulated {
            "Mock (Development)"
        } else {
            "Production"
        }
    );
    info!("└─────────────────────────────────────────────────────────────┘");

    // Load key info if key file is provided
    let (account_address, public_key, signature) = if let Some(ref key_file) = config.key_file {
        match load_key_info(key_file) {
            Ok(info) => info,
            Err(e) => {
                error!("Failed to load key file: {}", e);
                error!("Using placeholder values for registration");
                (
                    "0x0000000000000000000000000000000000000000".to_string(),
                    vec![],
                    vec![],
                )
            }
        }
    } else {
        warn!("No key file provided (SOLVER_KEY_FILE not set)");
        warn!("Using placeholder values for registration");
        (
            "0x0000000000000000000000000000000000000000".to_string(),
            vec![],
            vec![],
        )
    };

    // Create network client for Validator communication
    let network_config = SolverNetworkConfig {
        solver_id: config.solver_id.clone(),
        address: config.address.clone(),
        port: config.port,
        capacity: config.capacity,
        shard_id: config.shard_id.clone(),
        resources: config.resources.clone(),
        validator_address: config.validator_address.clone(),
        validator_port: config.validator_port,
        heartbeat_interval_secs: config.heartbeat_interval_secs,
        account_address: account_address.clone(),
        public_key: public_key.clone(),
        signature: signature.clone(),
    };

    let network_client = Arc::new(SolverNetworkClient::new(network_config));

    // Auto-register with Validator
    if config.auto_register {
        info!("┌─────────────────────────────────────────────────────────────┐");
        info!("│              Registering with Validator                     │");
        info!("└─────────────────────────────────────────────────────────────┘");

        match network_client.register().await {
            Ok(response) => {
                if response.success {
                    info!("╔════════════════════════════════════════════════════════════╗");
                    info!("║           ✓ Registration Successful                        ║");
                    info!("╚════════════════════════════════════════════════════════════╝");
                } else {
                    warn!("Registration failed: {}", response.message);
                }
            }
            Err(e) => {
                error!("Failed to register with validator: {}", e);
            }
        }
    }

    // Start heartbeat loop
    let heartbeat_client = network_client.clone();
    let heartbeat_handle = tokio::spawn(async move {
        heartbeat_client.start_heartbeat_loop().await;
    });

    // ============================================
    // HTTP Server for Sync Task Execution (solver-tee3)
    // ============================================
    //
    // This is the **synchronous HTTP** approach:
    // - Validator sends POST /api/v1/execute-task with SolverTask
    // - Solver executes in TEE synchronously
    // - Solver returns TeeExecutionResult in HTTP response
    // - Validator keeps Event in scope, no pending_tasks needed
    //
    let http_solver_id = config.solver_id.clone();
    let http_executor = tee_executor.clone();

    let http_address = config.address.clone();
    let http_port = config.port;
    let http_handle = tokio::spawn(async move {
        info!(
            address = %http_address,
            port = http_port,
            "Starting Solver HTTP server for sync task execution"
        );

        if let Err(e) = start_server(&http_address, http_port, http_solver_id, http_executor).await
        {
            error!("HTTP server failed: {}", e);
        }
    });

    // Log startup complete
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Solver Ready (solver-tee3 Sync HTTP)          ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!(
        "║  HTTP Server:  http://{}:{}/api/v1/execute-task {:>11}║",
        config.address, config.port, ""
    );
    info!("║  Mode:         Synchronous HTTP (no callback)             ║");
    info!("║  Note:         Validator sends task, waits for result     ║");
    info!("╚════════════════════════════════════════════════════════════╝");

    // Wait for shutdown signal
    tokio::select! {
        _ = http_handle => {
            info!("HTTP server stopped");
        }
        _ = heartbeat_handle => {
            info!("Heartbeat loop stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
            network_client.shutdown();
        }
    }

    info!("Solver shutdown complete");
    Ok(())
}
