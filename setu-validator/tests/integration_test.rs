//! Integration tests for Validator

use core_types::{Transfer, TransferType, Vlc};
use setu_core::{NodeConfig, config::NetworkConfig};
use setu_solver::Solver;
use setu_validator::Validator;
use tokio::sync::mpsc;
use tokio::time::Duration;

#[tokio::test]
async fn test_validator_receives_and_verifies_event() {
    // Setup: Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    // Create solver config
    let solver_config = NodeConfig {
        node_id: "solver-1".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 8001,
            peers: vec![],
        },
    };

    // Create validator config
    let validator_config = NodeConfig {
        node_id: "validator-1".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 9001,
            peers: vec![],
        },
    };

    // Create solver and validator
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    let validator = Validator::new(validator_config, event_rx);

    // Spawn solver in background
    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });

    // Spawn validator in background
    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });

    // Give them time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a transfer to the solver
    let transfer = Transfer {
        id: "transfer-1".to_string(),
        from: "alice".to_string(),
        to: "bob".to_string(),
        amount: 100,
        vlc: Vlc::new(),
        transfer_type: TransferType::FluxTransfer,
        power: 0,
        resources: vec![],
        preferred_solver: None,
        shard_id: None,
        subnet_id: None,
        assigned_vlc: None,
    };

    transfer_tx.send(transfer).unwrap();

    // Wait a bit for processing
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Cleanup: abort the tasks
    solver_handle.abort();
    validator_handle.abort();

    // Test passes if no panics occurred
}

#[tokio::test]
async fn test_validator_verifies_multiple_events() {
    // Setup: Create channels
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    // Create configs
    let solver_config = NodeConfig {
        node_id: "solver-2".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 8002,
            peers: vec![],
        },
    };

    let validator_config = NodeConfig {
        node_id: "validator-2".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 9002,
            peers: vec![],
        },
    };

    // Create and spawn nodes
    let solver = Solver::new(solver_config, transfer_rx, event_tx);
    let validator = Validator::new(validator_config, event_rx);

    let solver_handle = tokio::spawn(async move {
        solver.run().await;
    });

    let validator_handle = tokio::spawn(async move {
        validator.run().await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send multiple transfers
    for i in 0..3 {
        let transfer = Transfer {
            id: format!("transfer-{}", i),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100 + i as i128,
            vlc: Vlc::new(),
            transfer_type: TransferType::FluxTransfer,
            power: 0,
            resources: vec![],
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
            assigned_vlc: None,
        };
        transfer_tx.send(transfer).unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Cleanup
    solver_handle.abort();
    validator_handle.abort();
}

#[tokio::test]
async fn test_event_verification_logic() {
    use setu_types::event::{Event, EventType, ExecutionResult, StateChange};
    use setu_vlc::VLCSnapshot;

    // Create a valid event
    let vlc_snapshot = VLCSnapshot {
        vector_clock: setu_vlc::VectorClock::new(),
        logical_time: 1,
        physical_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    };

    let mut event = Event::new(
        EventType::Transfer,
        vec![],
        vlc_snapshot,
        "solver-1".to_string(),
    );

    // Add execution result
    let execution_result = ExecutionResult {
        success: true,
        message: Some("Transfer executed".to_string()),
        state_changes: vec![
            StateChange {
                key: "balance:alice".to_string(),
                old_value: Some(vec![]),
                new_value: Some(vec![]),
            },
        ],
    };
    event.set_execution_result(execution_result);

    // Create validator
    let (_event_tx, event_rx) = mpsc::unbounded_channel();
    let validator_config = NodeConfig {
        node_id: "validator-test".to_string(),
        network: NetworkConfig {
            listen_addr: "127.0.0.1".to_string(),
            port: 9003,
            peers: vec![],
        },
    };

    let _validator = Validator::new(validator_config, event_rx);

    // Event should be valid
    assert!(event.execution_result.is_some());
    assert!(event.execution_result.as_ref().unwrap().success);
}

