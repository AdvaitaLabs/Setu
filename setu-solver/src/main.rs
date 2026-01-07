//! Setu Solver - Main entry point
//!
//! The Solver node is responsible for:
//! - Registering with Validator on startup
//! - Receiving transfers from Validator
//! - Executing transfers (simulated for now)
//! - Generating events and sending back to Validator
//! - Maintaining heartbeat with Validator

use core_types::{Transfer, TransferType, Vlc};
use setu_core::NodeConfig;
use setu_solver::{Solver, SolverNetworkClient, SolverNetworkConfig};
use setu_types::event::{Event, EventType, ExecutionResult, StateChange};
use setu_vlc::{VLCSnapshot, VectorClock};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{info, warn, error, debug, Level};
use tracing_subscriber;

/// Solver configuration from environment
#[derive(Debug, Clone)]
struct SolverConfig {
    /// Node configuration
    node_config: NodeConfig,
    /// Solver ID
    solver_id: String,
    /// Solver listen address
    address: String,
    /// Solver listen port
    port: u16,
    /// Maximum capacity
    capacity: u32,
    /// Shard assignment
    shard_id: Option<String>,
    /// Resource types
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
        let node_config = NodeConfig::from_env();
        
        let solver_id = std::env::var("SOLVER_ID")
            .unwrap_or_else(|_| format!("solver-{}", &node_config.node_id[..8.min(node_config.node_id.len())]));
        
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
            node_config,
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

/// Event counter for generating unique IDs
static EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);
static VLC_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with detailed output
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_thread_ids(true)
        .init();

    // Load configuration
    let config = SolverConfig::from_env();
    
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║                Setu Solver Node Starting                   ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Solver ID:  {:^44} ║", config.solver_id);
    info!("║  Address:    {:^44} ║", format!("{}:{}", config.address, config.port));
    info!("║  Capacity:   {:^44} ║", config.capacity);
    info!("║  Shard:      {:^44} ║", config.shard_id.as_deref().unwrap_or("default"));
    info!("║  Validator:  {:^44} ║", format!("{}:{}", config.validator_address, config.validator_port));
    info!("╚════════════════════════════════════════════════════════════╝");

    // ========================================
    // Simulated Components (placeholder logs)
    // ========================================
    
    info!("┌─────────────────────────────────────────────────────────────┐");
    info!("│                  Initializing Components                    │");
    info!("├─────────────────────────────────────────────────────────────┤");
    
    // TEE Environment - Simulated
    info!("│ [TEE]       Trusted Execution Environment (simulated)      │");
    info!("│             - Enclave: mock-enclave-001                    │");
    info!("│             - Attestation: disabled (simulation mode)      │");
    
    // Executor - Simulated
    info!("│ [EXECUTOR]  Transfer Executor initialized                  │");
    info!("│             - Mode: simulation                             │");
    info!("│             - State storage: in-memory                     │");
    
    // Dependency Tracker - Simulated
    info!("│ [DEP]       Dependency Tracker initialized                 │");
    info!("│             - Tracking: resource-based                     │");
    info!("│             - Conflict detection: enabled                  │");
    
    info!("└─────────────────────────────────────────────────────────────┘");

    // Create channels for communication
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel::<Transfer>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

    // Create network client for registration
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
        
        let client = network_client.clone();
        match client.register().await {
            Ok(response) => {
                if response.success {
                    info!("╔════════════════════════════════════════════════════════════╗");
                    info!("║           ✓ Registration Successful                        ║");
                    info!("╠════════════════════════════════════════════════════════════╣");
                    info!("║  Assigned ID: {:^43} ║", response.assigned_id.as_deref().unwrap_or("N/A"));
                    info!("║  Message:     {:^43} ║", &response.message[..43.min(response.message.len())]);
                    info!("╚════════════════════════════════════════════════════════════╝");
                } else {
                    warn!("Registration failed: {}", response.message);
                }
            }
            Err(e) => {
                error!("Failed to register with validator: {}", e);
                error!("Solver will continue without registration (transfers won't be routed)");
            }
        }
    }

    // Start heartbeat loop
    let heartbeat_client = network_client.clone();
    let heartbeat_handle = tokio::spawn(async move {
        heartbeat_client.start_heartbeat_loop().await;
    });

    // Spawn event sender (sends events back to validator)
    let solver_id = config.solver_id.clone();
    let event_handle = tokio::spawn(async move {
        info!("[EVENT_TX] Event sender ready, waiting for execution results...");
        while let Some(event) = event_rx.recv().await {
            info!("╔════════════════════════════════════════════════════════════╗");
            info!("║              Event Generated by Solver                     ║");
            info!("╠════════════════════════════════════════════════════════════╣");
            info!("║  Event ID:   {:^44} ║", &event.id[..20.min(event.id.len())]);
            info!("║  Creator:    {:^44} ║", &event.creator);
            info!("║  Type:       {:^44} ║", format!("{:?}", event.event_type));
            info!("║  Status:     {:^44} ║", format!("{:?}", event.status));
            info!("╚════════════════════════════════════════════════════════════╝");
            
            // TODO: Send event to validator via HTTP/RPC
            info!("[EVENT_TX] Event ready to send to validator (HTTP forwarding not implemented)");
        }
    });

    // Create transfer processor with simulated execution
    let solver_id_clone = config.solver_id.clone();
    let event_tx_clone = event_tx.clone();
    let transfer_handle = tokio::spawn(async move {
        process_transfers(transfer_rx, event_tx_clone, solver_id_clone).await;
    });

    // Log startup complete
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Solver Ready for Transfers                    ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Status: Waiting for transfers from Validator              ║");
    info!("║  Mode:   Simulation (TEE disabled)                         ║");
    info!("╚════════════════════════════════════════════════════════════╝");

    // Wait for shutdown signal
    tokio::select! {
        _ = transfer_handle => {
            info!("Transfer processor stopped");
        }
        _ = event_handle => {
            info!("Event sender stopped");
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

/// Process incoming transfers with simulated execution pipeline
async fn process_transfers(
    mut transfer_rx: mpsc::UnboundedReceiver<Transfer>,
    event_tx: mpsc::UnboundedSender<Event>,
    solver_id: String,
) {
    info!("[TRANSFER_RX] Transfer processor ready, waiting for transfers...");
    
    while let Some(transfer) = transfer_rx.recv().await {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Processing Transfer                           ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Transfer ID: {:^43} ║", &transfer.id[..20.min(transfer.id.len())]);
        info!("║  From:        {:^43} ║", &transfer.from);
        info!("║  To:          {:^43} ║", &transfer.to);
        info!("║  Amount:      {:^43} ║", transfer.amount);
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Dependency Resolution (Simulated)
        info!("[STEP 1/6] 🔍 [DEP] Resolving dependencies...");
        info!("           └─ Resources: {:?}", transfer.resources);
        info!("           └─ Checking for conflicts...");
        info!("           └─ No conflicts found (simulated)");
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        // Step 2: TEE Execution (Simulated)
        info!("[STEP 2/6] 🔐 [TEE] Executing in Trusted Environment...");
        info!("           └─ Enclave: mock-enclave-001");
        info!("           └─ Loading state for accounts...");
        info!("           └─ Executing transfer logic...");
        info!("           └─ Generating execution proof...");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Step 3: State Changes (Simulated)
        info!("[STEP 3/6] 📝 Applying state changes...");
        let state_changes = vec![
            StateChange {
                key: format!("balance:{}", transfer.from),
                old_value: Some(vec![0, 0, 0, 100]), // Simulated old balance
                new_value: Some(vec![0, 0, 0, (100 - transfer.amount as u8).max(0)]),
            },
            StateChange {
                key: format!("balance:{}", transfer.to),
                old_value: Some(vec![0, 0, 0, 0]),
                new_value: Some(vec![0, 0, 0, transfer.amount as u8]),
            },
        ];
        for change in &state_changes {
            info!("           └─ {}: {:?} → {:?}", change.key, change.old_value, change.new_value);
        }
        
        // Step 4: VLC Update (Simulated)
        let vlc_time = VLC_COUNTER.fetch_add(1, Ordering::SeqCst);
        info!("[STEP 4/6] ⏰ [VLC] Updating Vector Logical Clock...");
        info!("           └─ Previous: {}", vlc_time);
        info!("           └─ New: {}", vlc_time + 1);
        info!("           └─ Merging with transfer VLC...");
        
        let mut vector_clock = VectorClock::new();
        vector_clock.increment(&solver_id);
        let vlc_snapshot = VLCSnapshot {
            vector_clock,
            logical_time: vlc_time + 1,
            physical_time: now,
        };
        
        // Step 5: Generate Event
        let event_id = EVENT_COUNTER.fetch_add(1, Ordering::SeqCst);
        info!("[STEP 5/6] 📦 Generating event...");
        info!("           └─ Event ID: evt-{}-{}", now, event_id);
        info!("           └─ Type: Transfer");
        info!("           └─ Parents: [] (genesis)");
        
        let mut event = Event::new(
            EventType::Transfer,
            vec![], // No parents for now (simulated)
            vlc_snapshot,
            solver_id.clone(),
        );
        
        // Attach transfer info
        event = event.with_transfer(setu_types::event::Transfer {
            from: transfer.from.clone(),
            to: transfer.to.clone(),
            amount: transfer.amount as u64,
        });
        
        // Set execution result
        let execution_result = ExecutionResult {
            success: true,
            message: Some("Transfer executed successfully (simulated)".to_string()),
            state_changes,
        };
        event.set_execution_result(execution_result);
        
        // Step 6: TEE Proof (Simulated)
        info!("[STEP 6/6] 🔏 [TEE] Generating attestation proof...");
        info!("           └─ Proof type: mock-attestation");
        info!("           └─ Proof size: 256 bytes (simulated)");
        info!("           └─ Signature: [mock-signature]");
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              Transfer Execution Complete                   ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Status:     SUCCESS                                       ║");
        info!("║  Event ID:   {:^44} ║", &event.id[..20.min(event.id.len())]);
        info!("║  VLC Time:   {:^44} ║", vlc_time + 1);
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Send event to validator
        if let Err(e) = event_tx.send(event) {
            error!("Failed to send event: {}", e);
        }
    }
    
    info!("[TRANSFER_RX] Transfer processor stopped");
}
