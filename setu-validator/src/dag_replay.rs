//! DAG event replay — rebuilds in-memory registries from persisted events.
//!
//! On restart, `ValidatorNetworkService` loses all in-memory state (subnets,
//! validators, solvers). This module reads persisted DAG events from RocksDB
//! in topological (depth) order and re-applies registration side effects.

use crate::ValidatorNetworkService;
use setu_storage::EventStoreBackend;
use setu_types::governance::ProposalContent;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Actions that can result from replaying a single event.
#[derive(Debug)]
pub enum ReplayAction {
    Applied(ReplayKind),
    Skipped,
    Error(String),
}

/// The kind of registration side-effect that was applied.
#[derive(Debug, Clone)]
pub enum ReplayKind {
    SubnetRegister,
    ValidatorRegister,
    ValidatorUnregister,
    SolverRegister,
    SolverUnregister,
    GovernancePropose([u8; 32], ProposalContent, u64),
    GovernanceExecute([u8; 32]),
    /// System subnet endpoint registered directly into GovernanceService registry.
    SystemSubnetRegister,
}

/// Statistics collected during a replay run.
#[derive(Debug, Default)]
pub struct ReplayStats {
    pub total_events: usize,
    pub replayed_events: usize,
    pub skipped_events: usize,
    pub subnets_registered: usize,
    pub validators_registered: usize,
    pub validators_unregistered: usize,
    pub solvers_registered: usize,
    pub solvers_unregistered: usize,
    pub governance_proposals: usize,
    pub governance_executions: usize,
    /// System subnet registrations replayed.
    pub system_subnet_registers: usize,
    /// Unmatched Propose events (no Execute) — pending governance proposals to re-dispatch.
    pub pending_governance: Vec<([u8; 32], ProposalContent, u64)>,
    pub errors: usize,
    pub duration_ms: u64,
}

/// Errors that can occur during replay.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("Storage error: {0}")]
    StorageError(String),
}

/// DAG event replay manager.
///
/// Reads all persisted events in depth order and re-applies registration
/// side effects to rebuild `ValidatorNetworkService` in-memory state.
pub struct DagReplayManager {
    event_store: Arc<dyn EventStoreBackend>,
}

impl DagReplayManager {
    pub fn new(event_store: Arc<dyn EventStoreBackend>) -> Self {
        Self { event_store }
    }

    /// Replay all DAG events and rebuild in-memory registries.
    ///
    /// Events are read in depth-ascending (topological) order so that
    /// Register events are always processed before their corresponding
    /// Unregister events.
    ///
    /// Events are fetched in batches of `BATCH_DEPTH_SIZE` depths to avoid
    /// loading the entire DAG into memory at once.
    ///
    /// Only registration/unregistration events produce side effects;
    /// Transfer, CoinMerge, etc. are skipped.
    const BATCH_DEPTH_SIZE: u64 = 1000;

    pub async fn replay_all(
        &self,
        network_service: &ValidatorNetworkService,
    ) -> Result<ReplayStats, ReplayError> {
        let start = Instant::now();
        let mut stats = ReplayStats::default();
        // Track Propose→Execute matching for pending governance discovery
        let mut governance_pending: HashMap<[u8; 32], (ProposalContent, u64)> = HashMap::new();

        // 1. Determine the depth range
        let max_depth = match self.event_store.get_max_depth().await {
            Some(d) => d,
            None => {
                info!("No events in storage, replay skipped");
                return Ok(stats);
            }
        };

        info!(max_depth = max_depth, "Starting DAG replay");

        // 2. Process events in depth-range batches to limit memory usage
        let mut current_min: u64 = 0;
        while current_min <= max_depth {
            let current_max = (current_min + Self::BATCH_DEPTH_SIZE - 1).min(max_depth);

            let events = self
                .event_store
                .get_events_by_depth_range(current_min, current_max)
                .await
                .map_err(|e| ReplayError::StorageError(e.to_string()))?;

            stats.total_events += events.len();

            // 3. Apply side effects in depth order
            for (event, _depth) in events {
                match network_service.apply_replay_event(&event) {
                    ReplayAction::Applied(kind) => {
                        stats.replayed_events += 1;
                        match kind {
                            ReplayKind::SubnetRegister => stats.subnets_registered += 1,
                            ReplayKind::ValidatorRegister => stats.validators_registered += 1,
                            ReplayKind::ValidatorUnregister => stats.validators_unregistered += 1,
                            ReplayKind::SolverRegister => stats.solvers_registered += 1,
                            ReplayKind::SolverUnregister => stats.solvers_unregistered += 1,
                            ReplayKind::GovernancePropose(id, content, timestamp) => {
                                stats.governance_proposals += 1;
                                governance_pending.insert(id, (content, timestamp));
                            }
                            ReplayKind::GovernanceExecute(id) => {
                                stats.governance_executions += 1;
                                governance_pending.remove(&id);
                            }
                            ReplayKind::SystemSubnetRegister => {
                                stats.system_subnet_registers += 1;
                            }
                        }
                    }
                    ReplayAction::Skipped => {
                        stats.skipped_events += 1;
                    }
                    ReplayAction::Error(e) => {
                        warn!(event_id = %event.id, error = %e, "Replay error (non-fatal)");
                        stats.errors += 1;
                    }
                }
            }

            current_min = current_max + 1;
        }

        // Unmatched proposals → pending governance (need re-dispatch)
        stats.pending_governance = governance_pending.into_iter()
            .map(|(id, (content, ts))| (id, content, ts))
            .collect();

        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{SolverInfo, SubnetInfo, ValidatorInfo};
    use setu_storage::memory::EventStore;
    use setu_types::{
        registration::{SolverRegistration, SubnetRegistration, Unregistration},
        Event, EventPayload, EventType, VLCSnapshot,
    };

    /// Helper to create a minimal event with the given payload and depth.
    fn make_event(event_type: EventType, payload: EventPayload, id_suffix: &str) -> Event {
        let mut event = Event::new(
            event_type,
            vec![],
            VLCSnapshot::default(),
            format!("test-creator-{}", id_suffix),
        );
        event.payload = payload;
        event.timestamp = 1700000000_000; // fixed timestamp ms
        event.recompute_id();
        event
    }

    /// Build a minimal ValidatorNetworkService for testing.
    /// We only need the struct to exist — no real network.
    fn make_test_service() -> ValidatorNetworkService {
        let router_manager = Arc::new(crate::RouterManager::new());
        let task_preparer = Arc::new(crate::TaskPreparer::new_for_testing(
            "test-validator".to_string(),
        ));
        let batch_task_preparer = Arc::new(crate::BatchTaskPreparer::new_for_testing(
            "test-validator".to_string(),
        ));
        let config = crate::NetworkServiceConfig::default();
        ValidatorNetworkService::new(
            "test-validator".to_string(),
            router_manager,
            task_preparer,
            batch_task_preparer,
            config,
        )
    }

    #[tokio::test]
    async fn tc001_empty_database_replay() {
        let store: Arc<dyn EventStoreBackend> = Arc::new(EventStore::new());
        let replay = DagReplayManager::new(store);
        let service = make_test_service();

        let stats = replay.replay_all(&service).await.unwrap();
        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.replayed_events, 0);
    }

    #[tokio::test]
    async fn tc002_subnet_register_replay() {
        let store = Arc::new(EventStore::new());
        let event = make_event(
            EventType::SubnetRegister,
            EventPayload::SubnetRegister(SubnetRegistration::new(
                "subnet-1", "Test Subnet", "owner-1", "TST",
            )),
            "subnet1",
        );
        store.store_with_depth(event, 1).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.subnets_registered, 1);
        assert_eq!(stats.total_events, 1);
        assert!(service.get_all_subnets().iter().any(|s| s.subnet_id == "subnet-1"));
    }

    #[tokio::test]
    async fn tc004_solver_register_replay() {
        let store = Arc::new(EventStore::new());
        let event = make_event(
            EventType::SolverRegister,
            EventPayload::SolverRegister(SolverRegistration::new(
                "solver-1", "127.0.0.1", 9001, "0xacc", vec![1], vec![2],
            )),
            "solver1",
        );
        store.store_with_depth(event, 1).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.solvers_registered, 1);
        assert!(service.get_all_solvers().iter().any(|s| s.solver_id == "solver-1"));
    }

    #[tokio::test]
    async fn tc005_register_then_unregister_validator() {
        let store = Arc::new(EventStore::new());

        // Register at depth 1
        let reg_event = make_event(
            EventType::ValidatorRegister,
            EventPayload::ValidatorRegister(setu_types::registration::ValidatorRegistration::new(
                "v1", "10.0.0.1", 9000, "0xacc", vec![1], vec![2], 1000,
            )),
            "vreg",
        );
        store.store_with_depth(reg_event, 1).await.unwrap();

        // Unregister at depth 2
        let unreg_event = make_event(
            EventType::ValidatorUnregister,
            EventPayload::ValidatorUnregister(Unregistration::validator("v1")),
            "vunreg",
        );
        store.store_with_depth(unreg_event, 2).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.validators_registered, 1);
        assert_eq!(stats.validators_unregistered, 1);
        assert!(service.get_all_validators().is_empty());
    }

    #[tokio::test]
    async fn tc007_mixed_events_skip_transfers() {
        let store = Arc::new(EventStore::new());

        // Genesis at depth 0
        let genesis = make_event(EventType::Genesis, EventPayload::None, "gen");
        store.store_with_depth(genesis, 0).await.unwrap();

        // SubnetRegister at depth 1
        let subnet = make_event(
            EventType::SubnetRegister,
            EventPayload::SubnetRegister(SubnetRegistration::new(
                "s1", "S1", "owner", "TKN",
            )),
            "sub",
        );
        store.store_with_depth(subnet, 1).await.unwrap();

        // Two transfers at depth 2,3
        let t1 = make_event(EventType::Transfer, EventPayload::None, "t1");
        store.store_with_depth(t1, 2).await.unwrap();
        let t2 = make_event(EventType::Transfer, EventPayload::None, "t2");
        store.store_with_depth(t2, 3).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.total_events, 4);
        assert_eq!(stats.replayed_events, 1); // only SubnetRegister
        assert_eq!(stats.skipped_events, 3);  // Genesis + 2 Transfers
    }

    #[tokio::test]
    async fn tc008_multi_depth_mixed() {
        let store = Arc::new(EventStore::new());

        // depth 0: genesis
        store.store_with_depth(
            make_event(EventType::Genesis, EventPayload::None, "gen"),
            0,
        ).await.unwrap();

        // depth 1: subnet-a + validator v1
        store.store_with_depth(
            make_event(
                EventType::SubnetRegister,
                EventPayload::SubnetRegister(SubnetRegistration::new("sa", "A", "o", "A")),
                "sa",
            ),
            1,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::ValidatorRegister,
                EventPayload::ValidatorRegister(
                    setu_types::registration::ValidatorRegistration::new(
                        "v1", "10.0.0.1", 9000, "0x1", vec![1], vec![2], 1000,
                    ),
                ),
                "v1",
            ),
            1,
        ).await.unwrap();

        // depth 2: solver s1 + subnet-b
        store.store_with_depth(
            make_event(
                EventType::SolverRegister,
                EventPayload::SolverRegister(SolverRegistration::new(
                    "s1", "127.0.0.1", 9001, "0x2", vec![1], vec![2],
                )),
                "s1",
            ),
            2,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::SubnetRegister,
                EventPayload::SubnetRegister(SubnetRegistration::new("sb", "B", "o", "B")),
                "sb",
            ),
            2,
        ).await.unwrap();

        // depth 3: unregister v1
        store.store_with_depth(
            make_event(
                EventType::ValidatorUnregister,
                EventPayload::ValidatorUnregister(Unregistration::validator("v1")),
                "uv1",
            ),
            3,
        ).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        // 2 subnets
        assert_eq!(service.get_all_subnets().len(), 2);
        // 0 validators (registered then unregistered)
        assert!(service.get_all_validators().is_empty());
        // 1 solver
        assert_eq!(service.get_all_solvers().len(), 1);

        assert_eq!(stats.subnets_registered, 2);
        assert_eq!(stats.validators_registered, 1);
        assert_eq!(stats.validators_unregistered, 1);
        assert_eq!(stats.solvers_registered, 1);
    }

    #[tokio::test]
    async fn tc006_solver_register_then_unregister() {
        let store = Arc::new(EventStore::new());

        // Register solver at depth 1
        let reg = make_event(
            EventType::SolverRegister,
            EventPayload::SolverRegister(SolverRegistration::new(
                "solver-x", "10.0.0.5", 9005, "0xacc", vec![1], vec![2],
            )),
            "sreg",
        );
        store.store_with_depth(reg, 1).await.unwrap();

        // Unregister solver at depth 2
        let unreg = make_event(
            EventType::SolverUnregister,
            EventPayload::SolverUnregister(Unregistration::solver("solver-x")),
            "sunreg",
        );
        store.store_with_depth(unreg, 2).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.solvers_registered, 1);
        assert_eq!(stats.solvers_unregistered, 1);
        assert!(service.get_all_solvers().is_empty(), "Solver should be removed after unregister");
    }

    #[tokio::test]
    async fn tc009_depth_ordering_guarantee() {
        // Events stored in reverse depth order should still replay correctly
        let store = Arc::new(EventStore::new());

        // Store unregister FIRST (depth 2) — before its register
        let unreg = make_event(
            EventType::ValidatorUnregister,
            EventPayload::ValidatorUnregister(Unregistration::validator("v-order")),
            "unreg-order",
        );
        store.store_with_depth(unreg, 2).await.unwrap();

        // Store register SECOND (depth 1) — after its unregister in insertion order
        let reg = make_event(
            EventType::ValidatorRegister,
            EventPayload::ValidatorRegister(
                setu_types::registration::ValidatorRegistration::new(
                    "v-order", "10.0.0.9", 9009, "0xord", vec![1], vec![2], 500,
                ),
            ),
            "reg-order",
        );
        store.store_with_depth(reg, 1).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        // Despite insertion order being reversed, replay must process depth 1
        // (register) before depth 2 (unregister) → result is empty
        assert_eq!(stats.validators_registered, 1);
        assert_eq!(stats.validators_unregistered, 1);
        assert!(
            service.get_all_validators().is_empty(),
            "Depth ordering must ensure register before unregister"
        );
    }

    #[tokio::test]
    async fn tc010_idempotent_subnet_overwrite() {
        // Two SubnetRegister events with the same subnet_id at different depths;
        // the later one (higher depth) should overwrite the earlier one.
        let store = Arc::new(EventStore::new());

        let reg1 = make_event(
            EventType::SubnetRegister,
            EventPayload::SubnetRegister(SubnetRegistration::new(
                "dup-subnet", "Name V1", "owner-old", "TK1",
            )),
            "dup1",
        );
        store.store_with_depth(reg1, 1).await.unwrap();

        let reg2 = make_event(
            EventType::SubnetRegister,
            EventPayload::SubnetRegister(SubnetRegistration::new(
                "dup-subnet", "Name V2", "owner-new", "TK2",
            )),
            "dup2",
        );
        store.store_with_depth(reg2, 2).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.subnets_registered, 2);
        let subnets = service.get_all_subnets();
        assert_eq!(subnets.len(), 1, "Same subnet_id should result in one entry");
        assert_eq!(subnets[0].name, "Name V2", "Later registration should overwrite");
        assert_eq!(subnets[0].owner, "owner-new");
    }

    #[tokio::test]
    async fn tc003_single_validator_register_replay() {
        let store = Arc::new(EventStore::new());
        let event = make_event(
            EventType::ValidatorRegister,
            EventPayload::ValidatorRegister(
                setu_types::registration::ValidatorRegistration::new(
                    "val-1", "10.0.0.1", 9000, "0xacc1", vec![1], vec![2], 1000,
                ),
            ),
            "val1",
        );
        store.store_with_depth(event, 1).await.unwrap();

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.validators_registered, 1);
        assert_eq!(stats.total_events, 1);
        let validators = service.get_all_validators();
        assert_eq!(validators.len(), 1);
        assert_eq!(validators[0].validator_id, "val-1");
    }

    #[tokio::test]
    async fn tc011_replay_stats_correctness() {
        let store = Arc::new(EventStore::new());

        // 5 registration/unregistration events
        store.store_with_depth(
            make_event(
                EventType::SubnetRegister,
                EventPayload::SubnetRegister(SubnetRegistration::new("s1", "S1", "o", "T1")),
                "s1",
            ), 1,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::ValidatorRegister,
                EventPayload::ValidatorRegister(
                    setu_types::registration::ValidatorRegistration::new(
                        "v1", "10.0.0.1", 9000, "0x1", vec![1], vec![2], 1000,
                    ),
                ),
                "v1",
            ), 2,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::SolverRegister,
                EventPayload::SolverRegister(SolverRegistration::new(
                    "sol1", "10.0.0.2", 9001, "0x2", vec![1], vec![2],
                )),
                "sol1",
            ), 3,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::ValidatorUnregister,
                EventPayload::ValidatorUnregister(Unregistration::validator("v1")),
                "uv1",
            ), 4,
        ).await.unwrap();
        store.store_with_depth(
            make_event(
                EventType::SolverUnregister,
                EventPayload::SolverUnregister(Unregistration::solver("sol1")),
                "usol1",
            ), 5,
        ).await.unwrap();

        // 10 Transfer events (skipped by replay)
        for i in 0..10 {
            store.store_with_depth(
                make_event(EventType::Transfer, EventPayload::None, &format!("t{}", i)),
                (6 + i) as u64,
            ).await.unwrap();
        }

        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.total_events, 15, "total = 5 reg/unreg + 10 transfers");
        assert_eq!(stats.replayed_events, 5, "only reg/unreg events are replayed");
        assert_eq!(stats.skipped_events, 10, "10 transfers skipped");
        assert_eq!(stats.subnets_registered, 1);
        assert_eq!(stats.validators_registered, 1);
        assert_eq!(stats.validators_unregistered, 1);
        assert_eq!(stats.solvers_registered, 1);
        assert_eq!(stats.solvers_unregistered, 1);
        assert_eq!(stats.errors, 0);
        assert!(stats.duration_ms < 5000, "replay should complete quickly");
    }

    /// TC-012: Regression test for R4-11 — sync-before-finalize scenario.
    /// An event stored without depth (via store()) and then re-stored with depth
    /// (via store_batch_with_depth()) must be visible to replay.
    #[tokio::test]
    async fn tc012_sync_before_finalize_depth_upsert() {
        let store = Arc::new(EventStore::new());

        // Step 1: Simulate P2P sync — store event WITHOUT depth
        let event = make_event(
            EventType::SubnetRegister,
            EventPayload::SubnetRegister(SubnetRegistration::new(
                "synced-subnet", "Synced", "owner-sync", "SYN",
            )),
            "sync1",
        );
        store.store(event.clone()).await.unwrap();

        // Verify: event exists but has no depth
        assert!(store.exists(&event.id).await, "event should exist after store()");
        assert!(store.get_depth(&event.id).await.is_none(), "no depth after store()");

        // Step 2: Simulate anchor finalization — store_batch_with_depth
        let result = store.store_batch_with_depth(vec![(event.clone(), 1)]).await;
        assert_eq!(result.skipped, 1, "event should be considered skipped (pre-existing)");

        // Step 3: Verify depth was upserted despite being "skipped"
        assert_eq!(store.get_depth(&event.id).await, Some(1), "depth must be upserted for pre-existing event");

        // Step 4: Verify event is visible to replay via get_events_by_depth_range (trait method)
        let store_dyn: Arc<dyn EventStoreBackend> = store.clone();
        let events = store_dyn.get_events_by_depth_range(0, 10).await.unwrap();
        assert_eq!(events.len(), 1, "event must be visible to depth range query");
        assert_eq!(events[0].0.id, event.id);
        assert_eq!(events[0].1, 1);

        // Step 5: Full replay should find the event
        let replay = DagReplayManager::new(store as Arc<dyn EventStoreBackend>);
        let service = make_test_service();
        let stats = replay.replay_all(&service).await.unwrap();

        assert_eq!(stats.subnets_registered, 1);
        assert!(
            service.get_all_subnets().iter().any(|s| s.subnet_id == "synced-subnet"),
            "synced-subnet should be recovered after replay"
        );
    }
}
