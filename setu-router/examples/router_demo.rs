//! Router demonstration
//! 
//! This example shows how to:
//! 1. Create a Router
//! 2. Register multiple Solvers
//! 3. Send Transfer intents
//! 4. Route transfers to Solvers
//! 5. Monitor statistics

use setu_router::{Router, RouterConfig, LoadBalancingStrategy};
use core_types::{Transfer, TransferType, Vlc};
use tokio::sync::mpsc;
use tracing::{info, Level};
use tracing_subscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    info!("🚀 Starting Router Demo");

    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    
    // Create router configuration
    let config = RouterConfig {
        node_id: "router-1".to_string(),
        max_pending_queue_size: 1000,
        load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
        quick_check_timeout_ms: 100,
        enable_resource_routing: true,
    };

    // Create router
    let router = Router::new(config, transfer_rx);
    
    // Register solvers
    info!("📝 Registering Solvers...");
    
    let (solver1_tx, mut solver1_rx) = mpsc::unbounded_channel();
    router.register_solver_with_affinity(
        "solver-1".to_string(),
        solver1_tx,
        100,
        Some("shard-1".to_string()),
        vec!["alice".to_string(), "bob".to_string()],
    );
    
    let (solver2_tx, mut solver2_rx) = mpsc::unbounded_channel();
    router.register_solver_with_affinity(
        "solver-2".to_string(),
        solver2_tx,
        100,
        Some("shard-1".to_string()),
        vec!["charlie".to_string(), "dave".to_string()],
    );
    
    let (solver3_tx, mut solver3_rx) = mpsc::unbounded_channel();
    router.register_solver_with_affinity(
        "solver-3".to_string(),
        solver3_tx,
        100,
        Some("shard-2".to_string()),
        vec!["eve".to_string(), "frank".to_string()],
    );
    
    info!("✅ Registered 3 solvers");
    
    // Spawn router task
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    // Spawn solver tasks (simulate solvers processing transfers)
    let solver1_handle = tokio::spawn(async move {
        while let Some(transfer) = solver1_rx.recv().await {
            info!(
                solver = "solver-1",
                transfer_id = %transfer.id,
                from = %transfer.from,
                to = %transfer.to,
                amount = %transfer.amount,
                "Solver received transfer"
            );
            // Simulate processing
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });
    
    let solver2_handle = tokio::spawn(async move {
        while let Some(transfer) = solver2_rx.recv().await {
            info!(
                solver = "solver-2",
                transfer_id = %transfer.id,
                from = %transfer.from,
                to = %transfer.to,
                amount = %transfer.amount,
                "Solver received transfer"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });
    
    let solver3_handle = tokio::spawn(async move {
        while let Some(transfer) = solver3_rx.recv().await {
            info!(
                solver = "solver-3",
                transfer_id = %transfer.id,
                from = %transfer.from,
                to = %transfer.to,
                amount = %transfer.amount,
                "Solver received transfer"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });
    
    // Send test transfers
    info!("📤 Sending test transfers...");
    
    // Transfer 1: alice -> bob (should route to solver-1)
    let mut vlc1 = Vlc::new();
    vlc1.entries.insert("client-1".to_string(), 1);
    let transfer1 = Transfer {
        id: "transfer-1".to_string(),
        from: "alice".to_string(),
        to: "bob".to_string(),
        amount: 100,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["alice".to_string()],
        vlc: vlc1,
        power: 10,
    };
    transfer_tx.send(transfer1)?;
    
    // Transfer 2: charlie -> dave (should route to solver-2)
    let mut vlc2 = Vlc::new();
    vlc2.entries.insert("client-1".to_string(), 2);
    let transfer2 = Transfer {
        id: "transfer-2".to_string(),
        from: "charlie".to_string(),
        to: "dave".to_string(),
        amount: 200,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["charlie".to_string()],
        vlc: vlc2,
        power: 20,
    };
    transfer_tx.send(transfer2)?;
    
    // Transfer 3: eve -> frank (should route to solver-3)
    let mut vlc3 = Vlc::new();
    vlc3.entries.insert("client-1".to_string(), 3);
    let transfer3 = Transfer {
        id: "transfer-3".to_string(),
        from: "eve".to_string(),
        to: "frank".to_string(),
        amount: 300,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["eve".to_string()],
        vlc: vlc3,
        power: 30,
    };
    transfer_tx.send(transfer3)?;
    
    // Transfer 4: unknown resource (should use load balancing)
    let mut vlc4 = Vlc::new();
    vlc4.entries.insert("client-1".to_string(), 4);
    let transfer4 = Transfer {
        id: "transfer-4".to_string(),
        from: "grace".to_string(),
        to: "henry".to_string(),
        amount: 400,
        transfer_type: TransferType::FluxTransfer,
        resources: vec!["grace".to_string()],
        vlc: vlc4,
        power: 40,
    };
    transfer_tx.send(transfer4)?;
    
    info!("✅ Sent 4 test transfers");
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    info!("🎉 Router demo completed!");
    
    // Cleanup
    drop(transfer_tx);
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        router_handle
    ).await;
    
    solver1_handle.abort();
    solver2_handle.abort();
    solver3_handle.abort();
    
    Ok(())
}

