//! Setu Validator - Main entry point
//!
//! The Validator node is responsible for:
//! - Receiving and routing transfers to Solvers
//! - Verifying events from Solvers
//! - Managing the DAG with consensus
//! - Coordinating consensus (leader election, CF voting)
//! - Providing HTTP API for registration and transfer submission

use setu_core::NodeConfig;
use setu_validator::{
    RouterManager, 
    ValidatorNetworkService, NetworkServiceConfig,
    ConsensusValidator, ConsensusValidatorConfig,
    AnemoConsensusBroadcaster,
    ConsensusEngineStore, SetuMessageHandler,
    NetworkEvent,
};
use setu_network_anemo::{
    AnemoNetworkService, NetworkConfig as AnemoNetworkConfig,
    AnemoConfig, NetworkNodeInfo,
};
use setu_storage::{
    SetuDB, RocksDBEventStore, RocksDBCFStore, RocksDBAnchorStore, RocksDBMerkleStore,
    GlobalStateManager, SharedStateManager, EventStoreBackend, CFStoreBackend, AnchorStoreBackend, B4StoreExt,
    MerkleStateProvider,
};
use setu_types::{
    NodeInfo, ConsensusConfig,
    GenesisConfig, Event, EventPayload, ExecutionResult, StateChange,
    CoinState, Address, VLCSnapshot,
};
use setu_keys::{load_keypair};
use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use tracing::{info, error, warn, Level};
use tracing_subscriber;

/// Validator configuration from environment
#[derive(Debug, Clone)]
struct ValidatorConfig {
    /// Node configuration
    node_config: NodeConfig,
    /// HTTP API listen address
    http_addr: SocketAddr,
    /// P2P listen address
    p2p_addr: SocketAddr,
    /// Key file path (optional)
    key_file: Option<String>,
    /// Database path for RocksDB persistence (optional)
    /// If not set, runs in pure memory mode
    db_path: Option<String>,
    /// Seed peer list (PEER_VALIDATORS env, format: "host1:port1,host2:port2")
    peer_validators: Vec<String>,
}

impl ValidatorConfig {
    fn from_env() -> Self {
        let node_config = NodeConfig::from_env();
        
        // HTTP API port (default: 8080)
        let http_port: u16 = std::env::var("VALIDATOR_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8080);
        
        // P2P port (default: 9000)
        let p2p_port: u16 = std::env::var("VALIDATOR_P2P_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9000);
        
        let listen_addr = std::env::var("VALIDATOR_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
        let key_file = std::env::var("VALIDATOR_KEY_FILE").ok();
        
        // Database path for persistence (optional)
        // If set, enables RocksDB persistence for Events and Anchors
        let db_path = std::env::var("VALIDATOR_DB_PATH").ok();
        
        // Seed peers for P2P connections
        let peer_validators = std::env::var("PEER_VALIDATORS")
            .ok()
            .map(|s| s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();
        
        Self {
            node_config,
            http_addr: format!("{}:{}", listen_addr, http_port).parse().unwrap(),
            p2p_addr: format!("{}:{}", listen_addr, p2p_port).parse().unwrap(),
            key_file,
            db_path,
            peer_validators,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with more detailed output
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_thread_ids(true)
        .init();

    // Load configuration
    let config = ValidatorConfig::from_env();
    
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Setu Validator Node Starting                  ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  Node ID:    {:^44} ║", config.node_config.node_id);
    info!("║  HTTP API:   {:^44} ║", config.http_addr);
    info!("║  P2P Addr:   {:^44} ║", config.p2p_addr);
    if let Some(ref db_path) = config.db_path {
        info!("║  DB Path:    {:^44} ║", db_path);
    } else {
        info!("║  DB Path:    {:^44} ║", "(memory mode)");
    }
    info!("╚════════════════════════════════════════════════════════════╝");

    // Log persistence mode
    if config.db_path.is_some() {
        info!("✓ Persistence mode: RocksDB enabled");
    } else {
        info!("⚠ Persistence mode: Memory only (data lost on restart)");
        info!("  Set VALIDATOR_DB_PATH to enable persistence");
    }

    // Load keypair once for both logging and private key injection (R4 fix: single load)
    let keypair = if let Some(ref key_file) = config.key_file {
        match load_keypair(key_file) {
            Ok(kp) => {
                info!("✓ Validator keypair loaded successfully");
                info!("  Account Address: {}", kp.address());
                info!("  Public Key: {}", hex::encode(kp.public().as_bytes()));
                Some(kp)
            }
            Err(e) => {
                warn!("Failed to load key file: {}", e);
                warn!("Validator will run without keypair (registration features limited)");
                None
            }
        }
    } else {
        info!("No key file provided (VALIDATOR_KEY_FILE not set)");
        info!("Validator will run without keypair (registration features limited)");
        None
    };

    // Load genesis config early to determine validator_count
    let genesis_path = std::env::var("GENESIS_FILE")
        .unwrap_or_else(|_| "genesis.json".to_string());
    let genesis_result = GenesisConfig::load(&genesis_path);

    // Determine genesis validator count for logging
    let genesis_validator_count = match &genesis_result {
        Ok(gc) if !gc.validators.is_empty() => gc.validators.len(),
        _ => 1,
    };

    // Create router manager (shared between NetworkService components)
    let router_manager = Arc::new(RouterManager::new());
    
    // Create ConsensusValidator for DAG + VLC + Consensus
    // N3 fix: Use P2P address/port (not HTTP) so all validators in ValidatorSet
    // share the same address semantics. HTTP addr is only used by the API server.
    let mut node_info = NodeInfo::new_validator(
        config.node_config.node_id.clone(),
        config.p2p_addr.ip().to_string(),
        config.p2p_addr.port(),
    );
    // R5 fix: Inject local validator's public key into NodeInfo so that
    // remote validators can verify our vote signatures.
    if let Some(ref kp) = keypair {
        node_info.public_key = kp.public().as_bytes().to_vec();
    }
    
    // Set validator_count: start with 1 (self only), registration loop will update
    // Note: Don't pre-set to genesis count here because add_consensus_validator()
    // updates validator_count = vs.count() which would go 3→2→3 causing confusion.
    let mut consensus = ConsensusConfig::default();
    consensus.validator_count = 1;
    // Higher vlc_delta_threshold reduces anchor commit frequency,
    // minimizing GlobalStateManager write-lock contention under high TPS.
    // Each anchor commit blocks ALL task preparation reads, so less frequent = higher throughput.
    // 3→5287; 10→5658; 20→5472; 50→5227; 100→4801; 200→4411. Optimal = 10.
    consensus.vlc_delta_threshold = std::env::var("VLC_DELTA_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    
    let consensus_config = ConsensusValidatorConfig {
        node_info,
        consensus,
        is_leader: false, // RotatingProposer determines leader; no hardcoded leader
        ..Default::default()
    };
    
    // R1 fix: Open RocksDB ONCE, share the single Arc<SetuDB> across all backends.
    // Previously opened twice (for SharedStateManager and ConsensusValidator) causing
    // LOCK file conflict crash in persistence mode.
    let db: Option<Arc<SetuDB>> = if let Some(ref db_path) = config.db_path {
        info!("Opening RocksDB at: {}", db_path);
        match SetuDB::open_default(db_path) {
            Ok(db) => {
                info!("✓ RocksDB opened successfully");
                Some(Arc::new(db))
            }
            Err(e) => {
                error!("Failed to open RocksDB at {}: {}", db_path, e);
                return Err(anyhow::anyhow!("Database open failed: {}", e));
            }
        }
    } else {
        None
    };

    // Create SHARED GlobalStateManager (used by both TaskPreparer and ConsensusValidator)
    let shared_state_manager: Arc<SharedStateManager> = if let Some(ref db) = db {
        let merkle_store: Arc<dyn B4StoreExt> = Arc::new(RocksDBMerkleStore::from_shared(db.clone()));
        let mut manager = GlobalStateManager::with_store(merkle_store);
        // Recover all subnet SMT trees from persisted state (B4 commit data)
        match manager.recover() {
            Ok(summary) => info!("✓ GSM recovered: {} subnets, {} leaves",
                summary.subnets_recovered, summary.total_leaves),
            Err(e) => warn!("GSM recovery failed: {}, starting fresh: {}", e, e),
        }
        Arc::new(SharedStateManager::new(manager))
    } else {
        Arc::new(SharedStateManager::new(GlobalStateManager::new()))
    };
    
    // Create task preparer with the SHARED state manager
    let task_preparer = Arc::new(setu_validator::TaskPreparer::new_with_state_manager(
        config.node_config.node_id.clone(),
        Arc::clone(&shared_state_manager),
    ));
    info!("✓ TaskPreparer initialized with shared state manager");

    // Create batch task preparer sharing the same state (production path)
    let batch_task_preparer = Arc::new(setu_validator::BatchTaskPreparer::new(
        config.node_config.node_id.clone(),
        Arc::new(setu_storage::MerkleStateProvider::new(Arc::clone(&shared_state_manager))),
    ));
    info!("✓ BatchTaskPreparer initialized with shared state manager");
    
    // Create ConsensusValidator with appropriate storage backend
    let consensus_validator = if let Some(ref db) = db {
        // RocksDB persistence mode - reuse the single DB handle
        let event_store: Arc<dyn EventStoreBackend> = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        let cf_store: Arc<dyn CFStoreBackend> = Arc::new(RocksDBCFStore::from_shared(db.clone()));
        let anchor_store: Arc<dyn AnchorStoreBackend> = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        info!("✓ RocksDB backends initialized (Events, CF, Anchors, Merkle)");
        
        Arc::new(ConsensusValidator::with_all_backends(
            consensus_config,
            Arc::clone(&shared_state_manager),
            event_store,
            cf_store,
            anchor_store,
        ))
    } else {
        // Memory mode - use shared state manager
        Arc::new(ConsensusValidator::with_shared_state_manager(
            consensus_config, 
            Arc::clone(&shared_state_manager),
        ))
    };
    
    info!("✓ ConsensusValidator initialized (genesis_validators={})", genesis_validator_count);
    
    // ========================================
    // Register all genesis validators into consensus layer
    // ========================================
    // R2 fix: Use add_peer_validator() instead of engine().add_consensus_validator()
    // to update BOTH ConsensusValidator.validator_set AND engine.validator_set atomically.
    if let Ok(ref genesis_config) = genesis_result {
        for gv in &genesis_config.validators {
            if gv.id == config.node_config.node_id {
                continue; // Skip self (already registered by constructor)
            }
            let mut peer_node_info = NodeInfo::new_validator(
                gv.id.clone(),
                gv.address.clone(),
                gv.p2p_port,
            );
            // Inject public key from genesis if present
            if let Some(ref pk_hex) = gv.public_key {
                if let Ok(pk_bytes) = hex::decode(pk_hex) {
                    peer_node_info.public_key = pk_bytes;
                }
            }
            consensus_validator.add_peer_validator(peer_node_info).await;
        }
        let engine = consensus_validator.engine();
        let vs = engine.validator_set_ref().read().await;
        info!(
            "✓ ValidatorSet initialized: {} validators, leader={:?}",
            vs.count(),
            vs.get_leader_id()
        );
        drop(vs);
    }

    // Inject private key for vote signing
    if let Some(ref kp) = keypair {
        let private_key_bytes = kp.secret_bytes().to_vec();
        consensus_validator.engine().set_private_key(private_key_bytes).await;
        info!("✓ Private key injected for vote signing");
    }

    // Attempt to recover state from storage (if any)
    // This is safe to call even with empty storage (fresh start)
    if let Err(e) = consensus_validator.recover_from_storage().await {
        warn!("Recovery from storage failed: {}, starting fresh", e);
    }

    // ========================================
    // Genesis Event: Initialize seed accounts
    // ========================================
    {
        match &genesis_result {
            Ok(genesis_config) => {
                info!(
                    chain_id = %genesis_config.chain_id,
                    accounts = genesis_config.accounts.len(),
                    subnet = %genesis_config.subnet_id,
                    "Loaded genesis config from {}",
                    genesis_path
                );

                // Build state changes for each genesis account
                // Supports multi-coin: when coins_per_account > 1, balance is split
                // across N coin objects for higher per-sender parallelism.
                let mut state_changes = Vec::new();
                for account in &genesis_config.accounts {
                    // Validate that the address is a proper hex address
                    let owner_addr = Address::from_hex(&account.address)
                        .expect("genesis account must have valid hex address");
                    let owner_hex = owner_addr.to_string();
                    let num_coins = account.coins_per_account.max(1) as u64;

                    if num_coins == 1 {
                        // Single coin (legacy path): use deterministic_coin_id
                        let object_id_bytes = MerkleStateProvider::coin_object_id_with_type(
                            &owner_hex,
                            &genesis_config.subnet_id,
                        );
                        let coin_state = CoinState::new_with_type(
                            owner_hex.clone(),
                            account.balance,
                            genesis_config.subnet_id.clone(),
                        );
                        let key = format!("oid:{}", hex::encode(object_id_bytes));
                        state_changes.push(StateChange {
                            key,
                            old_value: None,
                            new_value: Some(coin_state.to_bytes()),
                        });
                        info!(
                            name = ?account.name,
                            owner = %owner_hex,
                            balance = account.balance,
                            object_id = %hex::encode(object_id_bytes),
                            "Genesis account prepared (1 coin)"
                        );
                    } else {
                        // Multi-coin: split balance across N coins
                        let balance_per_coin = account.balance / num_coins;
                        let remainder = account.balance - balance_per_coin * (num_coins - 1);

                        for idx in 0..num_coins {
                            let coin_balance = if idx == num_coins - 1 {
                                remainder  // last coin absorbs rounding remainder
                            } else {
                                balance_per_coin
                            };

                            let object_id = if idx == 0 {
                                // Index 0: use legacy deterministic_coin_id for compatibility
                                setu_types::deterministic_coin_id_from_str(
                                    &owner_hex,
                                    &genesis_config.subnet_id,
                                )
                            } else {
                                setu_types::deterministic_genesis_coin_id(
                                    &owner_hex,
                                    &genesis_config.subnet_id,
                                    idx as u32,
                                )
                            };
                            let object_id_bytes = *object_id.as_bytes();

                            let coin_state = CoinState::new_with_type(
                                owner_hex.clone(),
                                coin_balance,
                                genesis_config.subnet_id.clone(),
                            );
                            let key = format!("oid:{}", hex::encode(object_id_bytes));
                            state_changes.push(StateChange {
                                key,
                                old_value: None,
                                new_value: Some(coin_state.to_bytes()),
                            });
                        }
                        info!(
                            name = ?account.name,
                            owner = %owner_hex,
                            total_balance = account.balance,
                            coins = num_coins,
                            balance_per_coin = balance_per_coin,
                            "Genesis account prepared ({} coins)",
                            num_coins
                        );
                    }
                }

                // Build genesis event with pre-computed execution result
                // Use deterministic fields so all validators produce the same ID
                let mut vlc_snapshot = VLCSnapshot::default();
                vlc_snapshot.physical_time = 0; // Eliminate SystemTime::now() non-determinism
                let mut genesis_event = Event::genesis(
                    "genesis".to_string(), // Fixed creator (not node_id) for cross-node consistency
                    vlc_snapshot,
                );
                genesis_event.timestamp = 0; // Fixed timestamp
                genesis_event.recompute_id(); // Recompute ID with deterministic fields
                genesis_event.payload = EventPayload::Genesis(genesis_config.clone());
                genesis_event.set_execution_result(ExecutionResult {
                    success: true,
                    message: Some(format!(
                        "Genesis: {} accounts initialized on {}",
                        genesis_config.accounts.len(),
                        genesis_config.chain_id
                    )),
                    state_changes: state_changes.clone(),
                });
                // Recompute ID after setting payload and execution_result
                // (verify_id checks against parent_ids, vlc, creator, timestamp)
                // The ID is computed from (parent_ids, vlc, creator, timestamp) so
                // payload/execution_result changes don't invalidate it.

                // Submit genesis event to the DAG
                match consensus_validator.submit_event(genesis_event.clone()).await {
                    Ok(event_id) => {
                        info!(
                            event_id = %event_id,
                            "Genesis event submitted to DAG"
                        );
                    }
                    Err(e) => {
                        error!("Failed to submit genesis event: {}", e);
                        return Err(anyhow::anyhow!("Genesis event submission failed: {}", e));
                    }
                }

                // Also apply state changes directly to GSM for immediate availability.
                // The genesis event is in the DAG but the CF won't form until
                // vlc_delta_threshold more events arrive. We need the coins to be
                // queryable right away for benchmarks/tests.
                // When the CF eventually forms and commits, apply_committed_events
                // will re-apply these changes (upsert is idempotent).
                {
                    let genesis_event_id = genesis_event.id.clone();
                    let mut gsm = shared_state_manager.lock_write();
                    for change in &state_changes {
                        gsm.apply_state_change(
                            setu_types::subnet::SubnetId::ROOT,
                            change,
                        );
                        // Record this genesis event as the last modifier of each coin object.
                        // This enables TaskPreparer.derive_dependencies() to set proper
                        // parent_ids on subsequent transfer events, establishing the causal
                        // chain: genesis_event → first_transfer → second_transfer → ...
                        let object_id_hex = change.key.strip_prefix("oid:").unwrap_or("");
                        if let Ok(bytes) = hex::decode(object_id_hex) {
                            if bytes.len() == 32 {
                                let mut arr = [0u8; 32];
                                arr.copy_from_slice(&bytes);
                                gsm.record_modification(&genesis_event_id, arr);
                            }
                        }
                    }
                    shared_state_manager.publish_snapshot(&gsm);
                }
                info!(
                    "✓ Genesis state applied: {} seed accounts initialized",
                    genesis_config.accounts.len()
                );
            }
            Err(e) => {
                warn!("No genesis config loaded ({}), starting with empty state", e);
            }
        }
    }
    
    // ========================================
    // Phase 2: P2P Network Startup
    // ========================================
    
    // 2.1 Create event channel for network → consensus flow
    let (network_event_tx, network_event_rx) = tokio::sync::mpsc::channel::<NetworkEvent>(1000);
    
    // 2.2 Create MessageHandlerStore (three-layer query, direct storage)
    let handler_store = Arc::new(ConsensusEngineStore::new(
        consensus_validator.engine(),
        consensus_validator.event_store(),
    ));
    
    // 2.3 Create SetuMessageHandler
    let setu_handler = Arc::new(SetuMessageHandler::new(
        handler_store,
        config.node_config.node_id.clone(),
        network_event_tx,
    ));
    
    // 2.4 Build Anemo network configuration
    let anemo_config = AnemoNetworkConfig {
        anemo: AnemoConfig {
            listen_addr: config.p2p_addr.to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    
    // 2.5 Build network-layer NodeInfo
    let anemo_node_info = NetworkNodeInfo::new_validator(
        config.node_config.node_id.clone(),
        config.p2p_addr.ip().to_string(),
        config.p2p_addr.port(),
    );
    
    // 2.6 Start Anemo P2P network
    let anemo_network = Arc::new(
        AnemoNetworkService::with_handler(anemo_config, anemo_node_info, setu_handler)
            .await
            .expect("Failed to start Anemo P2P network")
    );
    info!(
        listen_addr = %config.p2p_addr,
        "✓ Anemo P2P network started"
    );
    
    // 2.7 Create and inject broadcaster (P2P → consensus)
    let broadcaster = Arc::new(AnemoConsensusBroadcaster::new(
        Arc::clone(&anemo_network),
        config.node_config.node_id.clone(),
    ));
    consensus_validator.set_broadcaster(broadcaster).await;
    info!("✓ Consensus broadcaster connected");
    
    // 2.8 Start network event handler (network → consensus routing)
    let _network_handler = consensus_validator.start_network_event_handler(network_event_rx);
    info!("✓ Network event handler started");
    
    // ========================================
    // Phase 2.4: Connect to Seed Peers
    // ========================================
    for peer_addr in &config.peer_validators {
        let parts: Vec<&str> = peer_addr.split(':').collect();
        if parts.len() != 2 {
            warn!("Invalid peer address format: {} (expected host:port)", peer_addr);
            continue;
        }
        let host = parts[0];
        let port: u16 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => {
                warn!("Invalid port in peer address: {}", peer_addr);
                continue;
            }
        };
        
        // R9 fix: Look up genesis validator ID by address:port for consistent naming.
        // Falls back to "peer-<addr>" if no genesis match found.
        let peer_id_name = genesis_result.as_ref().ok()
            .and_then(|gc| gc.validators.iter().find(|v| v.address == host && v.p2p_port == port))
            .map(|v| v.id.clone())
            .unwrap_or_else(|| format!("peer-{}", peer_addr));
        let peer_info = NetworkNodeInfo::new_validator(
            peer_id_name,
            host.to_string(),
            port,
        );
        
        // Retry connection (peer may still be starting)
        for retry in 0..5u32 {
            match anemo_network.connect_to_peer(peer_info.clone()).await {
                Ok(peer_id) => {
                    info!(peer_id = %peer_id, addr = %peer_addr, "✓ Connected to peer");
                    break;
                }
                Err(e) => {
                    if retry < 4 {
                        warn!(retry = retry + 1, addr = %peer_addr, error = %e, "Retrying peer connection");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    } else {
                        error!(addr = %peer_addr, error = %e, "Failed to connect to peer after 5 retries");
                    }
                }
            }
        }
    }

    // Create network service configuration
    let network_config = NetworkServiceConfig {
        http_listen_addr: config.http_addr,
        p2p_listen_addr: config.p2p_addr,
    };
    
    // Create network service with consensus enabled
    let network_service = Arc::new(ValidatorNetworkService::with_consensus(
        config.node_config.node_id.clone(),
        router_manager.clone(),
        task_preparer.clone(),
        batch_task_preparer.clone(),
        consensus_validator.clone(),
        network_config,
    ));

    // ========================================
    // Components Status
    // ========================================
    
    info!("┌─────────────────────────────────────────────────────────────┐");
    info!("│                  Components Initialized                     │");
    info!("├─────────────────────────────────────────────────────────────┤");
    
    // VLC (Vector Logical Clock) - Real implementation
    info!("│ [VLC]       Vector Logical Clock initialized               │");
    info!("│             - Managed by ConsensusEngine                   │");
    info!("│             - Increments on each event                     │");
    
    // DAG Manager - Real implementation  
    info!("│ [DAG]       DAG Manager initialized                        │");
    info!("│             - Events stored in ConsensusEngine DAG         │");
    info!("│             - Parent tracking: enabled                     │");
    
    // Consensus - Real implementation
    info!("│ [CONSENSUS] Consensus module initialized                   │");
    if genesis_validator_count > 1 {
        info!("│             - Mode: Multi-validator ({} nodes)             │", genesis_validator_count);
        info!("│             - P2P network: Anemo (QUIC)                   │");
    } else {
        info!("│             - Mode: Single validator (leader)              │");
    }
    info!("│             - CF creation and finalization enabled         │");
    
    // AnchorBuilder
    info!("│ [ANCHOR]    AnchorBuilder initialized                      │");
    info!("│             - Merkle tree computation enabled              │");
    info!("│             - State persistence ready                      │");
    
    info!("└─────────────────────────────────────────────────────────────┘");

    // Start background reservation cleanup task (prevents memory accumulation)
    let _cleanup_handle = network_service.start_reservation_cleanup_task();
    info!("Background reservation cleanup task started (60s interval)");

    // ========================================
    // Phase 3: DAG Replay — rebuild in-memory registries from persisted events
    // ========================================
    {
        let event_store = consensus_validator.event_store();
        let replay_manager = setu_validator::dag_replay::DagReplayManager::new(event_store);
        match replay_manager.replay_all(&network_service).await {
            Ok(stats) => {
                if stats.total_events > 0 {
                    info!(
                        total = stats.total_events,
                        replayed = stats.replayed_events,
                        skipped = stats.skipped_events,
                        subnets = stats.subnets_registered,
                        validators = stats.validators_registered,
                        solvers = stats.solvers_registered,
                        errors = stats.errors,
                        duration_ms = stats.duration_ms,
                        "✓ DAG replay complete — in-memory registries rebuilt"
                    );
                } else {
                    info!("✓ DAG replay: no events to replay (fresh start)");
                }
            }
            Err(e) => {
                warn!("DAG replay failed (non-fatal, registries may be incomplete): {}", e);
            }
        }
    }

    // Spawn HTTP server
    let http_service = network_service.clone();
    let http_handle = tokio::spawn(async move {
        info!("Starting HTTP API server...");
        if let Err(e) = http_service.start_http_server().await {
            error!("HTTP server error: {}", e);
        }
    });

    // Log startup complete
    info!("╔════════════════════════════════════════════════════════════╗");
    info!("║              Validator Ready for Connections               ║");
    info!("╠════════════════════════════════════════════════════════════╣");
    info!("║  HTTP API Endpoints:                                       ║");
    info!("║    POST /api/v1/register/solver    - Register a solver     ║");
    info!("║    POST /api/v1/register/validator - Register a validator  ║");
    info!("║    GET  /api/v1/solvers            - List solvers          ║");
    info!("║    GET  /api/v1/validators         - List validators       ║");
    info!("║    GET  /api/v1/health             - Health check          ║");
    info!("║    POST /api/v1/transfer           - Submit transfer       ║");
    info!("║    POST /api/v1/event              - Submit event (Solver) ║");
    info!("║    GET  /api/v1/events             - List events           ║");
    info!("╚════════════════════════════════════════════════════════════╝");

    // Wait for shutdown signal
    tokio::select! {
        _ = http_handle => {
            info!("HTTP server stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, initiating graceful shutdown...");
        }
    }

    // ── Graceful shutdown sequence ──
    // Step 1: Stop batch collector (drain pending entries)
    info!("Step 1: Stopping batch collector...");
    network_service.shutdown_batch_collector().await;

    // Step 2: Wait for pending post-execution tasks (consensus + storage)
    info!("Step 2: Waiting for pending tasks...");
    match network_service.wait_for_pending_tee_tasks(Duration::from_secs(10)).await {
        Ok(()) => info!("All pending tasks completed"),
        Err(e) => warn!("Shutdown timeout: {}", e),
    }

    info!("Validator shutdown complete");
    Ok(())
}