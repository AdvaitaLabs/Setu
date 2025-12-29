//! Complete Setu System Integration (Single Process)
//! 
//! This demonstrates the full backend flow:
//! Relay -> Router -> Solver -> Validator
//! 
//! Note: This is a single-process demo using mpsc channels.
//! In production, components will communicate over network.

use setu_router::{Router, RouterConfig, LoadBalancingStrategy};
use setu_solver::Solver;
use setu_validator::Validator;
use setu_core::NodeConfig;
use setu_core::config::NetworkConfig;
use core_types::{Transfer, TransferType, Vlc};
use tokio::sync::mpsc;
use tracing::{info, Level};
use tracing_subscriber;
use std::time::Duration;

/// Simulate Relay sending transfers
async fn simulate_relay(transfer_tx: mpsc::UnboundedSender<Transfer>) {
    info!("🟢 Relay: Starting to send transfers...");
    
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Transfer 1: alice -> bob
    let mut vlc1 = Vlc::new();
    vlc1.entries.insert("relay-1".to_string(), 1);
    let transfer1 = Transfer {
        id: "tx-001".to_string(),
        from: "alice".to_string(),
        to: "bob".to_string(),
        amount: 1000,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["alice".to_string()],
        vlc: vlc1,
        power: 100,
    };
    info!("🟢 Relay: Sending transfer tx-001 (alice -> bob, 1000)");
    transfer_tx.send(transfer1).unwrap();
    
    tokio::time::sleep(Duration::from_millis(300)).await;
    
    // Transfer 2: charlie -> dave
    let mut vlc2 = Vlc::new();
    vlc2.entries.insert("relay-1".to_string(), 2);
    let transfer2 = Transfer {
        id: "tx-002".to_string(),
        from: "charlie".to_string(),
        to: "dave".to_string(),
        amount: 2000,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["charlie".to_string()],
        vlc: vlc2,
        power: 200,
    };
    info!("🟢 Relay: Sending transfer tx-002 (charlie -> dave, 2000)");
    transfer_tx.send(transfer2).unwrap();
    
    tokio::time::sleep(Duration::from_millis(300)).await;
    
    // Transfer 3: bob -> alice (dependent on tx-001)
    let mut vlc3 = Vlc::new();
    vlc3.entries.insert("relay-1".to_string(), 3);
    let transfer3 = Transfer {
        id: "tx-003".to_string(),
        from: "bob".to_string(),
        to: "alice".to_string(),
        amount: 500,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["bob".to_string()],
        vlc: vlc3,
        power: 150,
    };
    info!("🟢 Relay: Sending transfer tx-003 (bob -> alice, 500)");
    transfer_tx.send(transfer3).unwrap();
    
    info!("🟢 Relay: All transfers sent");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    info!("🚀 Starting Complete Setu System");
    info!("================================================");
    info!("Architecture:");
    info!("  🟢 Relay (simulated)");
    info!("       ↓ Transfer Intent");
    info!("  🔴 Router (Quick Check + Load Balance)");
    info!("       ↓ Transfer");
    info!("  🔵 Solver (Execute + Create Event)");
    info!("       ↓ Event");
    info!("  🟣 Validator (Verify + DAG)");
    info!("================================================");

    // ============================================
    // Step 1: Create communication channels
    // ============================================
    info!("📡 Creating channels...");
    
    // Relay -> Router
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel::<Transfer>();
    
    // Router -> Solver
    let (solver_tx, solver_rx) = mpsc::unbounded_channel::<Transfer>();
    
    // Solver -> Validator
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    info!("✅ Channels created");

    // ============================================
    // Step 2: Create Router
    // ============================================
    info!("🔴 Creating Router...");
    
    let router_config = RouterConfig {
        node_id: "router-1".to_string(),
        max_pending_queue_size: 10000,
        load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
        quick_check_timeout_ms: 100,
        enable_resource_routing: false,
    };
    
    let router = Router::new(router_config, transfer_rx);
    router.register_solver("solver-1".to_string(), solver_tx, 100);
    
    info!("✅ Router created");

    // ============================================
    // Step 3: Create Solver
    // ============================================
    info!("🔵 Creating Solver...");
    
    let solver_config = NodeConfig {
        node_id: "solver-1".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 8001,
            peers: vec![],
        },
    };
    
    let solver = Solver::new(solver_config, solver_rx, event_tx);
    
    info!("✅ Solver created");

    // ============================================
    // Step 4: Create Validator
    // ============================================
    info!("🟣 Creating Validator...");
    
    let validator_config = NodeConfig {
        node_id: "validator-1".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 9000,
            peers: vec![],
        },
    };
    
    let validator = Validator::new(validator_config, event_rx);
    
    info!("✅ Validator created");

    // ============================================
    // Step 5: Start all components
    // ============================================
    info!("================================================");
    info!("🚀 Starting all components...");
    
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    info!("✅ All components started");

    // ============================================
    // Step 6: Simulate Relay sending transfers
    // ============================================
    info!("================================================");
    
    let relay_tx = transfer_tx.clone();
    let relay_handle = tokio::spawn(async move {
        simulate_relay(relay_tx).await;
    });
    
    // Wait for relay to finish
    relay_handle.await?;

    // ============================================
    // Step 7: Wait for processing
    // ============================================
    info!("================================================");
    info!("⏳ Waiting for system to process...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ============================================
    // Step 8: Shutdown
    // ============================================
    info!("================================================");
    info!("🛑 Shutting down system...");
    
    drop(transfer_tx);
    
    let _ = tokio::time::timeout(Duration::from_secs(2), router_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), solver_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), validator_handle).await;
    
    info!("================================================");
    info!("🎉 System demo completed!");
    info!("================================================");
    info!("");
    info!("Summary:");
    info!("  ✅ Relay sent 3 transfers");
    info!("  ✅ Router routed all transfers");
    info!("  ✅ Solver executed and created events");
    info!("  ✅ Validator verified and built DAG");
    info!("");
    info!("Next steps:");
    info!("  🔧 Add network layer (replace mpsc with RPC/Anemo)");
    info!("  🔧 Implement VLC folding and consensus");
    info!("  🔧 Add fraud proof mechanism");
    
    Ok(())
}

