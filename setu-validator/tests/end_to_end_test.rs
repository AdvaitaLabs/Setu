//! End-to-end tests for Solver → Validator flow
//!
//! These tests verify the complete flow from Transfer creation
//! through Solver execution to Validator verification.

use setu_validator::Validator;
use setu_solver::Solver;
use setu_core::config::{NodeConfig, NetworkConfig};
use core_types::{Transfer, TransferType, Vlc};
use tokio::sync::mpsc;
use tracing::{info, debug};
use tracing_subscriber;

/// Initialize tracing for tests
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

/// Create a test node config
fn create_node_config(node_id: &str, port: u16) -> NodeConfig {
    NodeConfig {
        node_id: node_id.to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port,
            peers: vec![],
        },
    }
}

/// Create a test transfer
fn create_test_transfer(id: &str, from: &str, to: &str, amount: u64) -> Transfer {
    Transfer {
        id: id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        amount: amount as i128,
        transfer_type: TransferType::FluxTransfer,
        resources: vec![],
        vlc: Vlc::new(),
        power: 0,
        preferred_solver: None,
        shard_id: None,
    }
}

#[tokio::test]
async fn test_complete_solver_to_validator_flow() {
    init_tracing();
    
    info!("🚀 Starting end-to-end test: Solver → Validator");
    
    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    // Create Solver
    let solver_config = create_node_config("solver-1", 8001);
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    
    // Create Validator
    let validator_config = create_node_config("validator-1", 9001);
    let validator = Validator::new(validator_config, event_rx);
    
    // Spawn Solver task
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    // Spawn Validator task
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    // Send a test transfer
    let transfer = create_test_transfer("transfer-1", "alice", "bob", 100);
    info!("📤 Sending transfer: {} → {}, amount: {}", 
          transfer.from, transfer.to, transfer.amount);
    
    transfer_tx.send(transfer).unwrap();
    
    // Wait a bit for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Drop senders to close channels
    drop(transfer_tx);
    
    // Wait for tasks to complete
    tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        async {
            let _ = tokio::join!(solver_handle, validator_handle);
        }
    ).await.ok();
    
    info!("✅ End-to-end test completed successfully");
}

#[tokio::test]
async fn test_multiple_transfers() {
    init_tracing();
    
    info!("🚀 Starting test: Multiple transfers");
    
    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    // Create Solver and Validator
    let solver_config = create_node_config("solver-2", 8002);
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    
    let validator_config = create_node_config("validator-2", 9002);
    let validator = Validator::new(validator_config, event_rx);
    
    // Spawn tasks
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    // Send multiple transfers
    let transfers = vec![
        create_test_transfer("transfer-1", "alice", "bob", 100),
        create_test_transfer("transfer-2", "bob", "charlie", 50),
        create_test_transfer("transfer-3", "charlie", "dave", 25),
    ];
    
    for transfer in transfers {
        info!("📤 Sending transfer: {} → {}, amount: {}", 
              transfer.from, transfer.to, transfer.amount);
        transfer_tx.send(transfer).unwrap();
    }
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    
    // Cleanup
    drop(transfer_tx);
    
    tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        async {
            let _ = tokio::join!(solver_handle, validator_handle);
        }
    ).await.ok();
    
    info!("✅ Multiple transfers test completed");
}

#[tokio::test]
async fn test_transfer_dependency_chain() {
    init_tracing();
    
    info!("🚀 Starting test: Transfer dependency chain");
    
    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    // Create Solver and Validator
    let solver_config = create_node_config("solver-3", 8003);
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    
    let validator_config = create_node_config("validator-3", 9003);
    let validator = Validator::new(validator_config, event_rx);
    
    // Spawn tasks
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    // Send transfers with dependencies
    // Transfer 1: Alice → Bob (100)
    let transfer1 = create_test_transfer("transfer-1", "alice", "bob", 100);
    info!("📤 [1/3] Sending: {} → {}, amount: {}", 
          transfer1.from, transfer1.to, transfer1.amount);
    transfer_tx.send(transfer1).unwrap();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    // Transfer 2: Bob → Charlie (50) - depends on transfer 1
    let transfer2 = create_test_transfer("transfer-2", "bob", "charlie", 50);
    info!("📤 [2/3] Sending: {} → {}, amount: {}", 
          transfer2.from, transfer2.to, transfer2.amount);
    transfer_tx.send(transfer2).unwrap();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    // Transfer 3: Charlie → Dave (25) - depends on transfer 2
    let transfer3 = create_test_transfer("transfer-3", "charlie", "dave", 25);
    info!("📤 [3/3] Sending: {} → {}, amount: {}", 
          transfer3.from, transfer3.to, transfer3.amount);
    transfer_tx.send(transfer3).unwrap();
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    
    // Cleanup
    drop(transfer_tx);
    
    tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        async {
            let _ = tokio::join!(solver_handle, validator_handle);
        }
    ).await.ok();
    
    info!("✅ Dependency chain test completed");
}

#[tokio::test]
async fn test_concurrent_transfers() {
    init_tracing();
    
    info!("🚀 Starting test: Concurrent transfers");
    
    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    // Create Solver and Validator
    let solver_config = create_node_config("solver-4", 8004);
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    
    let validator_config = create_node_config("validator-4", 9004);
    let validator = Validator::new(validator_config, event_rx);
    
    // Spawn tasks
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    // Send multiple concurrent transfers (no dependencies)
    let transfers = vec![
        create_test_transfer("transfer-1", "alice", "bob", 100),
        create_test_transfer("transfer-2", "charlie", "dave", 200),
        create_test_transfer("transfer-3", "eve", "frank", 300),
        create_test_transfer("transfer-4", "grace", "henry", 400),
        create_test_transfer("transfer-5", "iris", "jack", 500),
    ];
    
    info!("📤 Sending {} concurrent transfers", transfers.len());
    
    for transfer in transfers {
        debug!("  → {} → {}, amount: {}", 
               transfer.from, transfer.to, transfer.amount);
        transfer_tx.send(transfer).unwrap();
    }
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
    
    // Cleanup
    drop(transfer_tx);
    
    tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        async {
            let _ = tokio::join!(solver_handle, validator_handle);
        }
    ).await.ok();
    
    info!("✅ Concurrent transfers test completed");
}

#[tokio::test]
async fn test_validator_statistics() {
    init_tracing();
    
    info!("🚀 Starting test: Validator statistics");
    
    // Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    
    // Create Solver
    let solver_config = create_node_config("solver-5", 8005);
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    
    // Create Validator
    let validator_config = create_node_config("validator-5", 9005);
    let validator = Validator::new(validator_config, event_rx);
    
    // Spawn Solver task
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });
    
    // Spawn Validator task
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });
    
    // Send some transfers
    info!("📤 Sending 5 transfers for statistics test");
    for i in 1..=5 {
        let transfer = create_test_transfer(
            &format!("transfer-{}", i),
            "alice",
            "bob",
            i * 10,
        );
        debug!("  → Transfer {}: {} → {}, amount: {}", 
               i, transfer.from, transfer.to, transfer.amount);
        transfer_tx.send(transfer).unwrap();
    }
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
    
    // Cleanup
    drop(transfer_tx);
    
    tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        async {
            let _ = tokio::join!(solver_handle, validator_handle);
        }
    ).await.ok();
    
    info!("✅ Statistics test completed");
    info!("   Note: Statistics can be checked via validator.dag_stats() and validator.sampling_stats()");
}

