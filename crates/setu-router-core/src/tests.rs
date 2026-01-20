//! Integration tests for setu-router

use crate::{Router, RouterConfig, DEFAULT_SHARD_ID};
use setu_types::{Transfer, TransferType};

fn create_test_transfer(id: &str, resources: Vec<String>) -> Transfer {
    Transfer::new(id, "alice", "bob", 100)
        .with_type(TransferType::FluxTransfer)
        .with_resources(resources)
}

#[test]
fn test_mvp_router_basic() {
    let router = Router::new_mvp();
    
    assert_eq!(router.solvers().count(), 6);
    assert_eq!(router.shard_id(), DEFAULT_SHARD_ID);
}

#[test]
fn test_route_transfer() {
    let router = Router::new_mvp();
    let transfer = create_test_transfer("tx-1", vec!["account:alice".to_string()]);
    
    let decision = router.route(&transfer).unwrap();
    
    assert_eq!(decision.shard_id, DEFAULT_SHARD_ID);
    assert!(!decision.solver_id.is_empty());
    assert!(decision.solver_address.starts_with("127.0.0.1:"));
}

#[test]
fn test_consistent_routing() {
    let router = Router::new_mvp();
    let transfer = create_test_transfer("tx-1", vec!["account:alice".to_string()]);
    
    // Same transfer should route to same solver
    let decision1 = router.route(&transfer).unwrap();
    let decision2 = router.route(&transfer).unwrap();
    
    assert_eq!(decision1.solver_id, decision2.solver_id);
}

#[test]
fn test_different_resources_may_route_differently() {
    let router = Router::new_mvp();
    
    let tx1 = create_test_transfer("tx-1", vec!["account:alice".to_string()]);
    let tx2 = create_test_transfer("tx-2", vec!["coin:btc".to_string()]);
    
    let decision1 = router.route(&tx1).unwrap();
    let decision2 = router.route(&tx2).unwrap();
    
    // Different resources may (but not necessarily) route to different solvers
    // Just verify both routes succeed
    assert!(!decision1.solver_id.is_empty());
    assert!(!decision2.solver_id.is_empty());
}

#[test]
fn test_batch_routing() {
    let router = Router::new_mvp();
    
    let transfers = vec![
        create_test_transfer("tx-1", vec!["account:alice".to_string()]),
        create_test_transfer("tx-2", vec!["account:bob".to_string()]),
        create_test_transfer("tx-3", vec!["coin:btc".to_string()]),
    ];
    
    let decisions = router.route_batch(&transfers);
    
    assert_eq!(decisions.len(), 3);
    assert!(decisions.iter().all(|d| d.is_ok()));
}

#[test]
fn test_route_by_key() {
    let router = Router::new_mvp();
    
    // Route by explicit key
    let decision1 = router.route_by_key("account:alice").unwrap();
    let decision2 = router.route_by_key("account:alice").unwrap();
    
    // Same key should route to same solver
    assert_eq!(decision1.solver_id, decision2.solver_id);
}

#[test]
fn test_router_config() {
    let config = RouterConfig {
        virtual_nodes: 200,
        load_aware: false,
        load_threshold: 0.9,
    };
    
    let solvers = std::sync::Arc::new(crate::SolverRegistry::new());
    for i in 1..=3 {
        solvers.register(crate::SolverInfo::new(
            format!("solver-{}", i),
            format!("127.0.0.1:{}", 9000 + i),
        ));
    }
    
    let router = Router::with_config(config, solvers);
    
    assert_eq!(router.config().virtual_nodes, 200);
    assert!(!router.config().load_aware);
}
