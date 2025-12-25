//! Setu Solver - Execution node
//!
//! The solver is responsible for:
//! - Receiving Transfer intents
//! - Executing computations
//! - Generating events
//! - Broadcasting to the network

mod executor;
mod dependency;
mod tee;

pub use executor::Executor;
pub use dependency::{DependencyTracker, DependencyStats};
pub use tee::{TeeEnvironment, TeeProof, EnclaveInfo};

use core_types::Transfer;
use setu_core::{NodeConfig, ShardManager};
use setu_types::event::{Event, EventType, EventId};
use setu_vlc::{VLCSnapshot, VectorClock};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error, debug};

/// Solver node
pub struct Solver {
    config: NodeConfig,
    shard_manager: Arc<ShardManager>,
    transfer_rx: mpsc::UnboundedReceiver<Transfer>,
    event_tx: mpsc::UnboundedSender<Event>,
    /// Executor for running transfers
    executor: Executor,
    /// Dependency tracker for building DAG
    dependency_tracker: DependencyTracker,
    /// TEE environment for secure execution
    tee: TeeEnvironment,
    /// Current VLC state
    vlc: VectorClock,
}

impl Solver {
    /// Create a new solver with channels
    pub fn new(
        config: NodeConfig,
        transfer_rx: mpsc::UnboundedReceiver<Transfer>,
        event_tx: mpsc::UnboundedSender<Event>,
    ) -> Self {
        info!(
            node_id = %config.node_id,
            "Creating solver node"
        );
        
        let shard_manager = Arc::new(ShardManager::new());
        let executor = Executor::new(config.node_id.clone());
        let dependency_tracker = DependencyTracker::new(config.node_id.clone());
        let tee = TeeEnvironment::new(config.node_id.clone());
        let vlc = VectorClock::new();
        
        Self {
            config,
            shard_manager,
            transfer_rx,
            event_tx,
            executor,
            dependency_tracker,
            tee,
            vlc,
        }
    }
    
    /// Run the solver
    pub async fn run(mut self) {
        info!(
            node_id = %self.config.node_id,
            port = self.config.network.port,
            "Solver started, waiting for transfers..."
        );
        
        // Main loop: receive transfers and process them
        while let Some(transfer) = self.transfer_rx.recv().await {
            info!(
                transfer_id = %transfer.id,
                from = %transfer.from,
                to = %transfer.to,
                amount = %transfer.amount,
                "Received transfer"
            );
            
            // Execute the transfer
            match self.execute_transfer(&transfer).await {
                Ok(event) => {
                    info!(
                        event_id = %event.id,
                        "Transfer executed successfully, generated event"
                    );
                    
                    // Send event to validator
                    if let Err(e) = self.event_tx.send(event) {
                        error!("Failed to send event: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to execute transfer: {}", e);
                }
            }
        }
        
        info!("Solver stopped");
    }
    
    /// Execute a transfer and generate an event
    async fn execute_transfer(&mut self, transfer: &Transfer) -> anyhow::Result<Event> {
        info!(
            transfer_id = %transfer.id,
            "Starting transfer execution pipeline"
        );
        
        // Step 1: Find dependencies
        let parent_ids = self.find_dependencies(transfer).await;
        debug!(
            transfer_id = %transfer.id,
            dependencies_count = parent_ids.len(),
            "Dependencies resolved"
        );
        
        // Step 2: Execute in TEE
        let execution_result = self.execute_in_tee(transfer).await?;
        debug!(
            transfer_id = %transfer.id,
            success = execution_result.success,
            "TEE execution completed"
        );
        
        // Step 3: Apply state changes
        self.apply_state_changes(&execution_result.state_changes).await?;
        debug!(
            transfer_id = %transfer.id,
            changes_count = execution_result.state_changes.len(),
            "State changes applied"
        );
        
        // Step 4: Generate TEE proof
        let _proof = self.generate_proof(transfer, &execution_result).await?;
        debug!(
            transfer_id = %transfer.id,
            "TEE proof generated"
        );
        
        // Step 5: Update VLC
        let vlc_snapshot = self.update_vlc(transfer);
        debug!(
            transfer_id = %transfer.id,
            logical_time = vlc_snapshot.logical_time,
            "VLC updated"
        );
        
        // Step 6: Create event
        let mut event = Event::new(
            EventType::Transfer,
            parent_ids.clone(),
            vlc_snapshot,
            self.config.node_id.clone(),
        );
        
        // Attach transfer and execution result
        event = event.with_transfer(setu_types::event::Transfer {
            from: transfer.from.clone(),
            to: transfer.to.clone(),
            amount: transfer.amount as u64,
        });
        event.set_execution_result(execution_result);
        
        // Step 7: Record event in dependency tracker
        let resources = vec![
            format!("account:{}", transfer.from),
            format!("account:{}", transfer.to),
        ];
        self.dependency_tracker.record_event(event.id.clone(), resources);
        
        // Add dependency edges
        for parent_id in &parent_ids {
            self.dependency_tracker.add_dependency(event.id.clone(), parent_id.clone());
        }
        
        info!(
            event_id = %event.id,
            transfer_id = %transfer.id,
            "Transfer execution pipeline completed"
        );
        
        Ok(event)
    }
    
    /// Find dependencies for a transfer
    async fn find_dependencies(&self, transfer: &Transfer) -> Vec<EventId> {
        self.dependency_tracker.find_dependencies(transfer).await
    }
    
    /// Execute transfer in TEE environment
    async fn execute_in_tee(&self, transfer: &Transfer) -> anyhow::Result<setu_types::event::ExecutionResult> {
        self.executor.execute_in_tee(transfer).await
    }
    
    /// Apply state changes to local storage
    async fn apply_state_changes(&self, changes: &[setu_types::event::StateChange]) -> anyhow::Result<()> {
        self.executor.apply_state_changes(changes).await
    }
    
    /// Generate TEE proof for execution
    async fn generate_proof(
        &self,
        transfer: &Transfer,
        result: &setu_types::event::ExecutionResult,
    ) -> anyhow::Result<TeeProof> {
        self.tee.generate_proof(transfer, result).await
    }
    
    /// Update VLC and return snapshot
    fn update_vlc(&mut self, _transfer: &Transfer) -> VLCSnapshot {
        // Increment logical clock
        self.vlc.increment(&self.config.node_id);
        
        // Note: We would merge with transfer's VLC here if Transfer had a VectorClock field
        // For now, we just increment our own clock
        
        // Create snapshot
        let mut snapshot = VLCSnapshot::new_with_clock(self.vlc.clone());
        snapshot.logical_time += 1;
        snapshot
    }
    
    /// Get dependency tracker statistics
    pub fn dependency_stats(&self) -> DependencyStats {
        self.dependency_tracker.stats()
    }
    
    /// Get TEE enclave information
    pub fn enclave_info(&self) -> EnclaveInfo {
        self.tee.enclave_info()
    }
    
    /// Get node ID
    pub fn node_id(&self) -> &str {
        &self.config.node_id
    }
}
