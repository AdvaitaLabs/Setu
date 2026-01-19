//! Setu Solver - Execution node
//!
//! The solver is responsible for:
//! - Receiving Transfer intents
//! - Executing computations
//! - Generating events
//! - Broadcasting to the network
//! - Registering with Validator

mod dependency;
mod tee;
mod network_client;

pub use dependency::{DependencyTracker, DependencyStats};
pub use tee::{TeeExecutor, TeeExecutionResult};
pub use setu_enclave::{EnclaveInfo, Attestation};
pub use network_client::{SolverNetworkClient, SolverNetworkConfig, SubmitEventRequest, SubmitEventResponse};

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
    /// Dependency tracker for building DAG
    dependency_tracker: DependencyTracker,
    /// TEE executor for secure execution and state management
    tee: TeeExecutor,
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
        let dependency_tracker = DependencyTracker::new(config.node_id.clone());
        let tee = TeeExecutor::new(config.node_id.clone());
        let vlc = VectorClock::new();
        
        Self {
            config,
            shard_manager,
            transfer_rx,
            event_tx,
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
            state_changes_count = execution_result.state_changes.len(),
            "TEE execution completed"
        );
        
        // NOTE: Solver does NOT apply state changes locally (Scheme B)
        // - Solver is stateless, Validator is the single source of truth
        // - State changes are included in the Event and sent to Validator
        // - Validator applies state changes after CF (ConsensusFrame) is finalized
        // - This ensures Solver/Validator state consistency
        
        // Step 3: Update VLC
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
        self.tee.execute_in_tee(transfer).await
    }
    
    // NOTE: apply_state_changes() removed - Solver is stateless in Scheme B
    // State changes are applied only by Validator after CF finalization
    // See: storage/src/subnet_state.rs - apply_committed_events()

    /// Update VLC and return snapshot
    /// 
    /// NOTE: In the new architecture, VLC is managed by Validator.
    /// Solver should use the assigned_vlc from Transfer when available.
    fn update_vlc(&mut self, transfer: &Transfer) -> VLCSnapshot {
        // Check if Validator assigned a VLC
        if let Some(ref assigned) = transfer.assigned_vlc {
            // Use Validator-assigned VLC
            let mut vlc = VectorClock::new();
            vlc.increment(&assigned.validator_id);
            
            return VLCSnapshot {
                vector_clock: vlc,
                logical_time: assigned.logical_time,
                physical_time: assigned.physical_time,
            };
        }
        
        // Fallback: Increment local clock (for backward compatibility)
        // This should not happen in normal operation
        self.vlc.increment(&self.config.node_id);
        
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
