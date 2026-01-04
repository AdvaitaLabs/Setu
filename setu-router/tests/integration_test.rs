//! Integration tests for Router

use setu_router::{Router, RouterConfig, LoadBalancingStrategy, RoutingStrategy};
use core_types::{Transfer, TransferType, Vlc};
use tokio::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn create_test_transfer(id: &str, from: &str, to: &str, amount: i128) -> Transfer {
    let mut vlc = Vlc::new();
    vlc.entries.insert("test-node".to_string(), 1);
    
    Transfer {
        id: id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        amount,
        transfer_type: TransferType::FluxTransfer,
        resources: vec![from.to_string()],
        vlc,
        power: 10,
        preferred_solver: None,
        shard_id: None,
    }
}

#[tokio::test]
async fn test_router_basic_routing() {
    // Create router
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let config = RouterConfig {
        node_id: "test-router".to_string(),
        max_pending_queue_size: 100,
        load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
        quick_check_timeout_ms: 100,
        enable_resource_routing: false,
        routing_strategy: RoutingStrategy::LoadBalanceOnly,
    };
    
    let router = Router::new(config, transfer_rx);
    
    // Register solver
    let (solver_tx, mut solver_rx) = mpsc::unbounded_channel();
    router.register_solver("solver-1".to_string(), solver_tx, 100);
    
    // Spawn router
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    // Send transfer
    let transfer = create_test_transfer("t1", "alice", "bob", 100);
    transfer_tx.send(transfer.clone()).unwrap();
    
    // Receive on solver side
    let received = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        solver_rx.recv()
    ).await.unwrap().unwrap();
    
    assert_eq!(received.id, "t1");
    assert_eq!(received.from, "alice");
    assert_eq!(received.to, "bob");
    assert_eq!(received.amount, 100);
    
    // Cleanup
    drop(transfer_tx);
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        router_handle
    ).await;
}

#[tokio::test]
async fn test_router_round_robin() {
    // Create router
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let config = RouterConfig {
        node_id: "test-router".to_string(),
        max_pending_queue_size: 100,
        load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
        quick_check_timeout_ms: 100,
        enable_resource_routing: false,
        routing_strategy: RoutingStrategy::LoadBalanceOnly,
    };
    
    let router = Router::new(config, transfer_rx);
    
    // Register 3 solvers
    let solver1_count = Arc::new(AtomicUsize::new(0));
    let solver2_count = Arc::new(AtomicUsize::new(0));
    let solver3_count = Arc::new(AtomicUsize::new(0));
    
    let (solver1_tx, mut solver1_rx) = mpsc::unbounded_channel();
    let (solver2_tx, mut solver2_rx) = mpsc::unbounded_channel();
    let (solver3_tx, mut solver3_rx) = mpsc::unbounded_channel();
    
    router.register_solver("solver-1".to_string(), solver1_tx, 100);
    router.register_solver("solver-2".to_string(), solver2_tx, 100);
    router.register_solver("solver-3".to_string(), solver3_tx, 100);
    
    // Spawn router
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    // Spawn solver receivers
    let count1 = solver1_count.clone();
    let h1 = tokio::spawn(async move {
        while solver1_rx.recv().await.is_some() {
            count1.fetch_add(1, Ordering::Relaxed);
        }
    });
    
    let count2 = solver2_count.clone();
    let h2 = tokio::spawn(async move {
        while solver2_rx.recv().await.is_some() {
            count2.fetch_add(1, Ordering::Relaxed);
        }
    });
    
    let count3 = solver3_count.clone();
    let h3 = tokio::spawn(async move {
        while solver3_rx.recv().await.is_some() {
            count3.fetch_add(1, Ordering::Relaxed);
        }
    });
    
    // Send 9 transfers (should be distributed 3-3-3)
    for i in 0..9 {
        let transfer = create_test_transfer(
            &format!("t{}", i),
            "alice",
            "bob",
            100 + i as i128,
        );
        transfer_tx.send(transfer).unwrap();
    }
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Check distribution
    let c1 = solver1_count.load(Ordering::Relaxed);
    let c2 = solver2_count.load(Ordering::Relaxed);
    let c3 = solver3_count.load(Ordering::Relaxed);
    
    println!("Distribution: solver1={}, solver2={}, solver3={}", c1, c2, c3);
    
    // Each solver should receive 3 transfers
    assert_eq!(c1, 3);
    assert_eq!(c2, 3);
    assert_eq!(c3, 3);
    
    // Cleanup
    drop(transfer_tx);
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        router_handle
    ).await;
    h1.abort();
    h2.abort();
    h3.abort();
}

#[tokio::test]
async fn test_router_resource_affinity() {
    // Create router with resource routing enabled
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let config = RouterConfig {
        node_id: "test-router".to_string(),
        max_pending_queue_size: 100,
        load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
        quick_check_timeout_ms: 100,
        enable_resource_routing: true,
        routing_strategy: RoutingStrategy::ResourceAffinityFirst,
    };
    
    let router = Router::new(config, transfer_rx);
    
    // Register solvers with resource affinity
    let (solver1_tx, mut solver1_rx) = mpsc::unbounded_channel();
    router.register_solver_with_affinity(
        "solver-1".to_string(),
        solver1_tx,
        100,
        None,
        vec!["alice".to_string()],
    );
    
    let (solver2_tx, mut solver2_rx) = mpsc::unbounded_channel();
    router.register_solver_with_affinity(
        "solver-2".to_string(),
        solver2_tx,
        100,
        None,
        vec!["bob".to_string()],
    );
    
    // Spawn router
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    // Send transfer from alice (should go to solver-1)
    let transfer1 = create_test_transfer("t1", "alice", "charlie", 100);
    transfer_tx.send(transfer1).unwrap();
    
    // Send transfer from bob (should go to solver-2)
    let transfer2 = create_test_transfer("t2", "bob", "charlie", 200);
    transfer_tx.send(transfer2).unwrap();
    
    // Check solver-1 received alice's transfer
    let received1 = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        solver1_rx.recv()
    ).await.unwrap().unwrap();
    assert_eq!(received1.from, "alice");
    
    // Check solver-2 received bob's transfer
    let received2 = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        solver2_rx.recv()
    ).await.unwrap().unwrap();
    assert_eq!(received2.from, "bob");
    
    // Cleanup
    drop(transfer_tx);
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        router_handle
    ).await;
}

#[tokio::test]
async fn test_router_quick_check_rejection() {
    // Create router
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let config = RouterConfig::default();
    let router = Router::new(config, transfer_rx);
    
    // Register solver
    let (solver_tx, mut solver_rx) = mpsc::unbounded_channel();
    router.register_solver("solver-1".to_string(), solver_tx, 100);
    
    // Spawn router
    let router_handle = tokio::spawn(async move {
        router.run().await;
    });
    
    // Send invalid transfer (empty ID)
    let mut invalid_transfer = create_test_transfer("", "alice", "bob", 100);
    invalid_transfer.id = "".to_string();
    transfer_tx.send(invalid_transfer).unwrap();
    
    // Send valid transfer
    let valid_transfer = create_test_transfer("t1", "alice", "bob", 100);
    transfer_tx.send(valid_transfer).unwrap();
    
    // Should only receive the valid transfer
    let received = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        solver_rx.recv()
    ).await.unwrap().unwrap();
    
    assert_eq!(received.id, "t1");
    
    // Cleanup
    drop(transfer_tx);
    let _ = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        router_handle
    ).await;
}

