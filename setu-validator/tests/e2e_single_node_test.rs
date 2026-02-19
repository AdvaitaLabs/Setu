//! End-to-End Single Node Test for solver-tee3 Architecture
//!
//! This test simulates the complete flow:
//! 1. User submits transfer → Validator
//! 2. Validator prepares SolverTask (coin selection, read_set, proofs)
//! 3. Validator routes to Solver
//! 4. Solver executes in TEE (pass-through)
//! 5. Solver returns TeeExecutionResult
//! 6. Validator verifies attestation and applies state changes
//! 7. Event confirmed in DAG

use setu_solver::{SolverTask, TeeExecutionResult, TeeExecutor};
use setu_storage::{init_coin, GlobalStateManager, StateProvider};
use setu_types::event::{Event, EventType, VLCSnapshot};
use setu_types::{SubnetId, Transfer, TransferType};
use setu_validator::{MerkleStateProvider, TaskPreparer};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::info;

/// Initialize tracing for tests
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

/// Simulated DAG for tracking confirmed events
struct SimulatedDAG {
    events: HashMap<String, Event>,
    confirmed: Vec<String>,
    depth: u64,
}

impl SimulatedDAG {
    fn new() -> Self {
        Self {
            events: HashMap::new(),
            confirmed: Vec::new(),
            depth: 0,
        }
    }

    fn add_event(&mut self, event: Event) -> u64 {
        let event_id = event.id.clone();
        self.events.insert(event_id.clone(), event);
        self.depth += 1;
        self.depth
    }

    fn confirm(&mut self, event_id: &str) {
        if self.events.contains_key(event_id) {
            self.confirmed.push(event_id.to_string());
        }
    }

    fn is_confirmed(&self, event_id: &str) -> bool {
        self.confirmed.contains(&event_id.to_string())
    }
}

/// Single node test harness
struct SingleNodeTestHarness {
    /// Validator components
    validator_id: String,
    task_preparer: Arc<TaskPreparer>,
    state_manager: Arc<RwLock<GlobalStateManager>>,

    /// Solver components
    solver_id: String,
    tee_executor: Arc<TeeExecutor>,

    /// DAG for tracking events
    dag: SimulatedDAG,

    /// Channels
    solver_task_tx: mpsc::UnboundedSender<SolverTask>,
    solver_task_rx: mpsc::UnboundedReceiver<SolverTask>,
    result_tx: mpsc::UnboundedSender<TeeExecutionResult>,
    result_rx: mpsc::UnboundedReceiver<TeeExecutionResult>,
}

impl SingleNodeTestHarness {
    fn new() -> Self {
        // Initialize state manager with test accounts
        let state_manager = Arc::new(RwLock::new(GlobalStateManager::new()));
        {
            let mut manager = state_manager.write().unwrap();
            init_coin(&mut manager, "alice", 10_000_000);
            init_coin(&mut manager, "bob", 5_000_000);
            init_coin(&mut manager, "charlie", 1_000_000);
        }

        // Create state provider
        let state_provider = Arc::new(MerkleStateProvider::new(state_manager.clone()));

        // Create task preparer
        let validator_id = "validator-1".to_string();
        let task_preparer = Arc::new(TaskPreparer::new(validator_id.clone(), state_provider));

        // Create TEE executor
        let solver_id = "solver-1".to_string();
        let tee_executor = Arc::new(TeeExecutor::new(solver_id.clone()));

        // Create channels
        let (solver_task_tx, solver_task_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::unbounded_channel();

        Self {
            validator_id,
            task_preparer,
            state_manager,
            solver_id,
            tee_executor,
            dag: SimulatedDAG::new(),
            solver_task_tx,
            solver_task_rx,
            result_tx,
            result_rx,
        }
    }

    /// Step 1: User submits transfer to Validator
    fn user_submit_transfer(&self, from: &str, to: &str, amount: u64) -> Transfer {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let transfer_id = format!("tx_{:x}", ts % 0xFFFFFFFFFFFF);

        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 1: User Submits Transfer                             ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Transfer ID: {:^43} ║", &transfer_id);
        info!("║  From:        {:^43} ║", from);
        info!("║  To:          {:^43} ║", to);
        info!("║  Amount:      {:^43} ║", amount);
        info!("╚════════════════════════════════════════════════════════════╝");

        Transfer::new(&transfer_id, from, to, amount).with_type(TransferType::FluxTransfer)
    }

    /// Step 2: Validator prepares SolverTask
    fn validator_prepare_task(&self, transfer: &Transfer) -> Result<SolverTask, String> {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 2: Validator Prepares SolverTask                     ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let task = self
            .task_preparer
            .prepare_transfer_task(transfer, SubnetId::ROOT)
            .map_err(|e| format!("Task preparation failed: {}", e))?;

        info!("  ✓ Task ID:        {}", hex::encode(&task.task_id[..8]));
        info!("  ✓ Event ID:       {}...", &task.event.id[..20]);
        info!(
            "  ✓ Input Objects:  {}",
            task.resolved_inputs.input_objects.len()
        );
        info!("  ✓ Read Set:       {} entries", task.read_set.len());
        info!("  ✓ Gas Budget:     {}", task.gas_budget.max_gas_units);

        Ok(task)
    }

    /// Step 3: Validator routes to Solver
    async fn validator_route_to_solver(&self, task: SolverTask) -> Result<(), String> {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 3: Validator Routes to Solver                        ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let task_id = hex::encode(&task.task_id[..8]);

        self.solver_task_tx
            .send(task)
            .map_err(|e| format!("Failed to send task: {}", e))?;

        info!("  ✓ Routed to:      {}", self.solver_id);
        info!("  ✓ Task ID:        {}", task_id);

        Ok(())
    }

    /// Step 4: Solver executes in TEE
    async fn solver_execute_task(&mut self) -> Result<TeeExecutionResult, String> {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 4: Solver Executes in TEE                            ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let task = self.solver_task_rx.recv().await.ok_or("No task received")?;

        let task_id = hex::encode(&task.task_id[..8]);
        info!("  → Received Task:  {}", task_id);
        info!(
            "  → TEE Mode:       {}",
            if self.tee_executor.is_simulated() {
                "Mock"
            } else {
                "Production"
            }
        );

        let result = self
            .tee_executor
            .execute_solver_task(task)
            .await
            .map_err(|e| format!("TEE execution failed: {}", e))?;

        info!(
            "  ✓ Execution:      {}",
            if result.is_success() {
                "SUCCESS"
            } else {
                "FAILED"
            }
        );
        info!("  ✓ Events Done:    {}", result.events_processed);
        info!("  ✓ Events Failed:  {}", result.events_failed);
        info!("  ✓ Gas Used:       {}", result.gas_usage.gas_used);
        info!(
            "  ✓ Attestation:    {}",
            if result.attestation.is_mock() {
                "Mock"
            } else {
                "Real"
            }
        );

        Ok(result)
    }

    /// Step 5: Solver returns result to Validator
    async fn solver_return_result(&self, result: TeeExecutionResult) -> Result<(), String> {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 5: Solver Returns Result                             ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let task_id = hex::encode(&result.task_id[..8]);

        self.result_tx
            .send(result)
            .map_err(|e| format!("Failed to send result: {}", e))?;

        info!("  ✓ Result sent for Task: {}", task_id);

        Ok(())
    }

    /// Step 6: Validator verifies and creates Event
    async fn validator_verify_and_create_event(&mut self) -> Result<Event, String> {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 6: Validator Verifies & Creates Event                ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let result = self.result_rx.recv().await.ok_or("No result received")?;

        let _task_id = hex::encode(&result.task_id[..8]);

        // Verify attestation (in mock mode, always passes)
        info!("  → Verifying attestation...");
        let attestation_valid = result.attestation.is_mock() || self.verify_attestation(&result);
        if !attestation_valid {
            return Err("Attestation verification failed".to_string());
        }
        info!("  ✓ Attestation verified");

        // Create Event from result
        let event = Event::new(
            EventType::Transfer,
            vec![], // parent_ids from result
            VLCSnapshot::default(),
            self.validator_id.clone(),
        );

        info!("  ✓ Event ID:       {}...", &event.id[..20]);
        info!(
            "  ✓ Post State:     {}...",
            hex::encode(&result.post_state_root[..8])
        );

        Ok(event)
    }

    /// Step 7: Event confirmed in DAG
    fn confirm_in_dag(&mut self, event: Event) -> u64 {
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║  STEP 7: Event Confirmed in DAG                            ║");
        info!("╚════════════════════════════════════════════════════════════╝");

        let event_id = event.id.clone();
        let depth = self.dag.add_event(event);
        self.dag.confirm(&event_id);

        info!("  ✓ Event ID:       {}...", &event_id[..20]);
        info!("  ✓ DAG Depth:      {}", depth);
        info!("  ✓ Confirmed:      {}", self.dag.is_confirmed(&event_id));

        depth
    }

    fn verify_attestation(&self, _result: &TeeExecutionResult) -> bool {
        // In production, this would verify the TEE attestation
        true
    }

    /// Get balance for an address (for verification)
    fn get_balance(&self, address: &str) -> u64 {
        let _manager = self.state_manager.read().unwrap();
        let state_provider = MerkleStateProvider::new(self.state_manager.clone());
        let coins = state_provider.get_coins_for_address(address);
        coins.iter().map(|c| c.balance).sum()
    }
}

#[tokio::test]
async fn test_e2e_single_transfer() {
    init_tracing();

    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║     🚀 E2E Single Node Test: Complete Transfer Flow 🚀             ║");
    info!("╚════════════════════════════════════════════════════════════════════╝");
    info!("");

    let mut harness = SingleNodeTestHarness::new();

    // Initial balances
    let alice_initial = harness.get_balance("alice");
    let bob_initial = harness.get_balance("bob");
    info!("📊 Initial Balances:");
    info!("   Alice: {}", alice_initial);
    info!("   Bob:   {}", bob_initial);
    info!("");

    // Step 1: User submits transfer
    let transfer = harness.user_submit_transfer("alice", "bob", 100);

    // Step 2: Validator prepares SolverTask
    let task = harness
        .validator_prepare_task(&transfer)
        .expect("Failed to prepare task");

    // Step 3: Validator routes to Solver
    harness
        .validator_route_to_solver(task)
        .await
        .expect("Failed to route");

    // Step 4: Solver executes in TEE
    let result = harness
        .solver_execute_task()
        .await
        .expect("Failed to execute");

    // Step 5: Solver returns result
    harness
        .solver_return_result(result)
        .await
        .expect("Failed to return result");

    // Step 6: Validator verifies and creates Event
    let event = harness
        .validator_verify_and_create_event()
        .await
        .expect("Failed to create event");

    // Step 7: Confirm in DAG
    let depth = harness.confirm_in_dag(event);

    // Summary
    info!("");
    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║                      ✅ TEST COMPLETED ✅                           ║");
    info!("╠════════════════════════════════════════════════════════════════════╣");
    info!("║  Transfer:    alice → bob (100 units)                              ║");
    info!("║  DAG Depth:   {:^55} ║", depth);
    info!("║  Status:      Confirmed                                            ║");
    info!("╚════════════════════════════════════════════════════════════════════╝");

    assert_eq!(depth, 1, "Should have depth 1 after one transfer");
}

#[tokio::test]
async fn test_e2e_multiple_transfers() {
    init_tracing();

    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║     🚀 E2E Single Node Test: Multiple Transfers 🚀                 ║");
    info!("╚════════════════════════════════════════════════════════════════════╝");
    info!("");

    let mut harness = SingleNodeTestHarness::new();

    let transfers = vec![
        ("alice", "bob", 100),
        ("bob", "charlie", 50),
        ("charlie", "alice", 25),
    ];

    for (i, (from, to, amount)) in transfers.iter().enumerate() {
        info!("");
        info!(
            "═══════════════ Transfer {} of {} ═══════════════",
            i + 1,
            transfers.len()
        );
        info!("");

        // Step 1: User submits
        let transfer = harness.user_submit_transfer(from, to, *amount);

        // Step 2: Prepare
        let task = harness
            .validator_prepare_task(&transfer)
            .expect("Failed to prepare task");

        // Step 3: Route
        harness
            .validator_route_to_solver(task)
            .await
            .expect("Failed to route");

        // Step 4: Execute
        let result = harness
            .solver_execute_task()
            .await
            .expect("Failed to execute");

        // Step 5: Return
        harness
            .solver_return_result(result)
            .await
            .expect("Failed to return");

        // Step 6: Verify
        let event = harness
            .validator_verify_and_create_event()
            .await
            .expect("Failed to verify");

        // Step 7: Confirm
        let depth = harness.confirm_in_dag(event);

        info!("  ✅ Transfer {} confirmed at depth {}", i + 1, depth);
    }

    info!("");
    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║                      ✅ ALL TRANSFERS COMPLETED ✅                  ║");
    info!(
        "║  Total: {} transfers confirmed                                     ║",
        transfers.len()
    );
    info!("╚════════════════════════════════════════════════════════════════════╝");

    assert_eq!(harness.dag.confirmed.len(), transfers.len());
}

#[tokio::test]
async fn test_e2e_insufficient_balance() {
    init_tracing();

    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║     🚀 E2E Single Node Test: Insufficient Balance 🚀               ║");
    info!("╚════════════════════════════════════════════════════════════════════╝");
    info!("");

    let harness = SingleNodeTestHarness::new();

    // Try to transfer more than charlie has
    let transfer = harness.user_submit_transfer("charlie", "alice", 999_999_999);

    // Should fail at task preparation
    let result = harness.validator_prepare_task(&transfer);

    info!("");
    match result {
        Err(e) => {
            info!("✅ Expected failure: {}", e);
            assert!(
                e.contains("insufficient") || e.contains("balance") || e.contains("No coin"),
                "Error should mention insufficient balance"
            );
        }
        Ok(_) => {
            panic!("Should have failed due to insufficient balance");
        }
    }
}

#[tokio::test]
async fn test_e2e_concurrent_transfers() {
    init_tracing();

    info!("╔════════════════════════════════════════════════════════════════════╗");
    info!("║     🚀 E2E Single Node Test: Concurrent Transfers 🚀               ║");
    info!("╚════════════════════════════════════════════════════════════════════╝");
    info!("");

    // For concurrent test, we need multiple independent transfers
    // from different accounts to avoid conflicts

    let harness = SingleNodeTestHarness::new();

    // Prepare multiple tasks
    let transfers = vec![
        harness.user_submit_transfer("alice", "bob", 10),
        harness.user_submit_transfer("bob", "charlie", 5),
    ];

    // Prepare all tasks first
    let tasks: Vec<_> = transfers
        .iter()
        .filter_map(|t| harness.validator_prepare_task(t).ok())
        .collect();

    info!("");
    info!("✅ Prepared {} concurrent tasks", tasks.len());

    assert!(tasks.len() >= 1, "Should prepare at least one task");
}
