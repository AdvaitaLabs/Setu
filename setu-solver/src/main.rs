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

use setu_solver::{
    TeeExecutor, TeeExecutionResult,
    SolverNetworkClient, SolverNetworkConfig,
    SolverTask,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn, error, Level};

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
}

impl SolverConfig {
    fn from_env() -> Self {
        let solver_id = std::env::var("SOLVER_ID")
            .unwrap_or_else(|_| format!("solver-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()));
        
        let address = std::env::var("SOLVER_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
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
        
        let validator_address = std::env::var("VALIDATOR_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
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
        }
    }
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
    info!("║  Address:    {:^44} ║", format!("{}:{}", config.address, config.port));
    info!("║  Capacity:   {:^44} ║", config.capacity);
    info!("║  Shard:      {:^44} ║", config.shard_id.as_deref().unwrap_or("default"));
    info!("║  Validator:  {:^44} ║", format!("{}:{}", config.validator_address, config.validator_port));
    info!("╚════════════════════════════════════════════════════════════╝");

    // Initialize TEE Executor
    let tee_executor = Arc::new(TeeExecutor::new(config.solver_id.clone()));
    
    info!("┌─────────────────────────────────────────────────────────────┐");
    info!("│                  TEE Executor Initialized                   │");
    info!("├─────────────────────────────────────────────────────────────┤");
    info!("│ Enclave:     {:^44} │", tee_executor.enclave_info().enclave_id);
    info!("│ Version:     {:^44} │", tee_executor.enclave_info().version);
    info!("│ Mode:        {:^44} │", if tee_executor.enclave_info().is_simulated { "Mock (Development)" } else { "Production" });
    info!("└─────────────────────────────────────────────────────────────┘");

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

    // Create channel for receiving SolverTasks from Validator
    // In production, this would come via RPC/network
    let (task_tx, mut task_rx) = mpsc::unbounded_channel::<SolverTask>();
    
    // Create channel for sending results back
    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<TeeExecutionResult>();

    // Task processor - the core of solver-tee3 architecture
    let executor = tee_executor.clone();
    let result_sender = result_tx.clone();
    let task_handle = tokio::spawn(async move {
        info!("[TASK_RX] Task processor ready, waiting for SolverTasks...");
        
        while let Some(task) = task_rx.recv().await {
            let task_id_hex = hex::encode(&task.task_id[..8]);
            
            info!("╔════════════════════════════════════════════════════════════╗");
            info!("║              Processing SolverTask                         ║");
            info!("╠════════════════════════════════════════════════════════════╣");
            info!("║  Task ID:    {:^44} ║", task_id_hex);
            info!("║  Event ID:   {:^44} ║", &task.event.id[..20.min(task.event.id.len())]);
            info!("║  Inputs:     {:^44} ║", format!("{} objects", task.resolved_inputs.input_objects.len()));
            info!("╚════════════════════════════════════════════════════════════╝");
            
            // Execute in TEE (this is all the Solver does in solver-tee3)
            match executor.execute_solver_task(task).await {
                Ok(result) => {
                    info!("╔════════════════════════════════════════════════════════════╗");
                    info!("║              TEE Execution Complete                        ║");
                    info!("╠════════════════════════════════════════════════════════════╣");
                    info!("║  Status:     {:^44} ║", if result.is_success() { "SUCCESS" } else { "FAILED" });
                    info!("║  Events:     {:^44} ║", result.events_processed);
                    info!("║  Gas Used:   {:^44} ║", result.gas_usage.gas_used);
                    info!("╚════════════════════════════════════════════════════════════╝");
                    
                    if let Err(e) = result_sender.send(result) {
                        error!("Failed to send result: {}", e);
                    }
                }
                Err(e) => {
                    error!("TEE execution failed: {}", e);
                }
            }
        }
        
        info!("[TASK_RX] Task processor stopped");
    });

    // Result sender - sends results back to Validator
    let result_client = network_client.clone();
    let result_handle = tokio::spawn(async move {
        info!("[RESULT_TX] Result sender ready...");
        
        while let Some(result) = result_rx.recv().await {
            // In solver-tee3, we send the TeeExecutionResult back to Validator
            // The Validator then creates the Event from the result
            info!("[RESULT_TX] Sending result to Validator...");
            
            // TODO: Implement actual RPC call to Validator
            // For now, just log it
            info!("[RESULT_TX] Result sent (task_id: {})", hex::encode(&result.task_id[..8]));
        }
        
        info!("[RESULT_TX] Result sender stopped");
    });

    // Log startup complete
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Solver Ready (solver-tee3 Mode)               ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Status: Waiting for SolverTasks from Validator            ║");
    info!("║  Mode:   Pass-through to TEE                               ║");
    info!("║  Note:   Validator handles coin selection & VLC            ║");
    info!("╚════════════════════════════════════════════════════════════╝");

    // Wait for shutdown signal
    tokio::select! {
        _ = task_handle => {
            info!("Task processor stopped");
        }
        _ = result_handle => {
            info!("Result sender stopped");
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
