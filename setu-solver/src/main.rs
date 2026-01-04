//! Setu Solver - Main entry point

use core_types::{Transfer, TransferType, Vlc};
use setu_core::NodeConfig;
use setu_solver::Solver;
use setu_types::event::Event;
use tokio::sync::mpsc;
use tracing::Level;
use tracing_subscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    // Load configuration from environment
    let config = NodeConfig::from_env();

    // Create channels for communication
    let (transfer_tx, transfer_rx) = mpsc::unbounded_channel::<Transfer>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

    // Spawn a task to receive events (simulating validator)
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            tracing::info!(
                event_id = %event.id,
                creator = %event.creator,
                status = ?event.status,
                "Received event from solver"
            );
        }
    });

    // Create and spawn solver
    let solver = Solver::new(config, transfer_rx, event_tx);
    
    // Spawn solver in background
    tokio::spawn(async move {
        solver.run().await;
    });

    // Simulate sending some transfers for testing
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    tracing::info!("Sending test transfers...");
    
    for i in 1..=3 {
        let mut vlc = Vlc::new();
        vlc.entries.insert("node1".to_string(), i);
        
        let transfer = Transfer {
            id: format!("transfer_{}", i),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100 * i as i128,
            transfer_type: TransferType::FluxTransfer,
            resources: vec!["alice".to_string(), "bob".to_string()],
            vlc,
            power: 10,
            preferred_solver: None,
            shard_id: None,
        };
        
        transfer_tx.send(transfer)?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // Keep running for a bit to see the results
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    
    tracing::info!("Test completed");

    Ok(())
}
