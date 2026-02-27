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
};
use setu_storage::{
    SetuDB, RocksDBEventStore, RocksDBCFStore, RocksDBAnchorStore, RocksDBMerkleStore,
    GlobalStateManager, EventStoreBackend, CFStoreBackend, AnchorStoreBackend, B4StoreExt,
    MerkleStateProvider,
};
use setu_types::{
    NodeInfo, ConsensusConfig,
    GenesisConfig, Event, EventPayload, ExecutionResult, StateChange,
    CoinState, Address, VLCSnapshot,
};
use setu_keys::{load_keypair};
use std::sync::{Arc, RwLock};
use std::net::SocketAddr;
use tracing::{info, error, warn, Level};
use tracing_subscriber;

/// Validator configuration from environment
#[derive(Debug, Clone)]
struct ValidatorConfig {
    /// Node configuration
    node_config: NodeConfig,
    /// HTTP API listen address
    http_addr: SocketAddr,
    /// P2P listen address (for future use)
    p2p_addr: SocketAddr,
    /// Key file path (optional)
    key_file: Option<String>,
    /// Database path for RocksDB persistence (optional)
    /// If not set, runs in pure memory mode
    db_path: Option<String>,
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
        
        Self {
            node_config,
            http_addr: format!("{}:{}", listen_addr, http_port).parse().unwrap(),
            p2p_addr: format!("{}:{}", listen_addr, p2p_port).parse().unwrap(),
            key_file,
            db_path,
        }
    }
}

/// Load keypair from file and extract registration info
fn load_key_info(key_file: &str) -> anyhow::Result<(String, Vec<u8>, Vec<u8>)> {
    info!("Loading keypair from: {}", key_file);
    
    let keypair = load_keypair(key_file)?;
    let account_address = keypair.address().to_string();
    let public_key = keypair.public().as_bytes();
    
    // Create registration message to sign
    let message = format!("Register Validator: {}", account_address);
    let signature = keypair.sign(message.as_bytes()).as_bytes();
    
    info!("Keypair loaded successfully");
    info!("  Account Address: {}", account_address);
    info!("  Public Key: {}", hex::encode(&public_key));
    
    Ok((account_address, public_key, signature))
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

    // Load key info if key file is provided
    let _key_info = if let Some(ref key_file) = config.key_file {
        match load_key_info(key_file) {
            Ok(info) => {
                info!("✓ Validator keypair loaded successfully");
                Some(info)
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

    // Create router manager (shared between NetworkService components)
    let router_manager = Arc::new(RouterManager::new());
    
    // Create ConsensusValidator for DAG + VLC + Consensus
    let node_info = NodeInfo::new_validator(
        config.node_config.node_id.clone(),
        config.http_addr.ip().to_string(),
        config.http_addr.port(),
    );
    
    // Single node mode: set validator_count = 1 for quorum to work
    // quorum = (1 * 2) / 3 + 1 = 1, so single validator can finalize
    let mut consensus = ConsensusConfig::default();
    consensus.validator_count = 1;
    // Lower vlc_delta_threshold for testing - allows faster CF creation
    // Default is 10, which requires 10 VLC ticks to create a CF
    // For low-throughput scenarios (like account initialization), this causes delays
    consensus.vlc_delta_threshold = 3;
    
    let consensus_config = ConsensusValidatorConfig {
        node_info,
        consensus,
        is_leader: true, // Single node mode: always leader
        ..Default::default()
    };
    
    // Create SHARED GlobalStateManager (used by both TaskPreparer and ConsensusValidator)
    // This is the key fix: both components now share the same state!
    let shared_state_manager: Arc<RwLock<GlobalStateManager>> = if let Some(ref db_path) = config.db_path {
        // RocksDB persistence mode
        info!("Opening RocksDB at: {}", db_path);
        let db = match SetuDB::open_default(db_path) {
            Ok(db) => Arc::new(db),
            Err(e) => {
                error!("Failed to open RocksDB at {}: {}", db_path, e);
                return Err(anyhow::anyhow!("Database open failed: {}", e));
            }
        };
        
        let merkle_store: Arc<dyn B4StoreExt> = Arc::new(RocksDBMerkleStore::from_shared(db.clone()));
        Arc::new(RwLock::new(GlobalStateManager::with_store(merkle_store)))
    } else {
        // Memory mode
        Arc::new(RwLock::new(GlobalStateManager::new()))
    };
    
    // Create task preparer with the SHARED state manager
    let task_preparer = Arc::new(setu_validator::TaskPreparer::new_with_state_manager(
        config.node_config.node_id.clone(),
        Arc::clone(&shared_state_manager),
    ));
    info!("✓ TaskPreparer initialized with shared state manager");
    
    // Create ConsensusValidator with appropriate storage backend
    let consensus_validator = if let Some(ref db_path) = config.db_path {
        // RocksDB persistence mode - open database and create all backends
        let db = match SetuDB::open_default(db_path) {
            Ok(db) => Arc::new(db),
            Err(e) => {
                error!("Failed to open RocksDB at {}: {}", db_path, e);
                return Err(anyhow::anyhow!("Database open failed: {}", e));
            }
        };
        
        // Create all RocksDB-backed stores from shared database
        let event_store: Arc<dyn EventStoreBackend> = Arc::new(RocksDBEventStore::from_shared(db.clone()));
        let cf_store: Arc<dyn CFStoreBackend> = Arc::new(RocksDBCFStore::from_shared(db.clone()));
        let anchor_store: Arc<dyn AnchorStoreBackend> = Arc::new(RocksDBAnchorStore::from_shared(db.clone()));
        
        info!("✓ RocksDB backends initialized (Events, CF, Anchors, Merkle)");
        
        // Use the SHARED state manager
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
    
    info!("✓ ConsensusValidator initialized with shared state manager (single node mode)");
    
    // Attempt to recover state from storage (if any)
    // This is safe to call even with empty storage (fresh start)
    if let Err(e) = consensus_validator.recover_from_storage().await {
        warn!("Recovery from storage failed: {}, starting fresh", e);
    }

    // ========================================
    // Genesis Event: Initialize seed accounts
    // ========================================
    // Load genesis.json and create a proper Genesis Event that flows through
    // the DAG → CF → commit pipeline, replacing the old direct init_coin hack.
    {
        let genesis_path = std::env::var("GENESIS_FILE")
            .unwrap_or_else(|_| "genesis.json".to_string());

        match GenesisConfig::load(&genesis_path) {
            Ok(genesis_config) => {
                info!(
                    chain_id = %genesis_config.chain_id,
                    accounts = genesis_config.accounts.len(),
                    subnet = %genesis_config.subnet_id,
                    "Loaded genesis config from {}",
                    genesis_path
                );

                // Build state changes for each genesis account
                let mut state_changes = Vec::new();
                for account in &genesis_config.accounts {
                    let owner_hex = Address::from(account.name.as_str()).to_string();
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
                        name = %account.name,
                        owner = %owner_hex,
                        balance = account.balance,
                        object_id = %hex::encode(object_id_bytes),
                        "Genesis account prepared"
                    );
                }

                // Build genesis event with pre-computed execution result
                let vlc_snapshot = VLCSnapshot::default();
                let mut genesis_event = Event::genesis(
                    config.node_config.node_id.clone(),
                    vlc_snapshot,
                );
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
                    let mut gsm = shared_state_manager.write()
                        .expect("Failed to acquire write lock on GlobalStateManager");
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
    info!("│             - Mode: Single validator (leader)              │");
    info!("│             - CF creation and finalization enabled         │");
    
    // AnchorBuilder
    info!("│ [ANCHOR]    AnchorBuilder initialized                      │");
    info!("│             - Merkle tree computation enabled              │");
    info!("│             - State persistence ready                      │");
    
    info!("└─────────────────────────────────────────────────────────────┘");

    // Start background reservation cleanup task (prevents memory accumulation)
    let _cleanup_handle = network_service.start_reservation_cleanup_task();
    info!("Background reservation cleanup task started (60s interval)");

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
            info!("Received Ctrl+C, shutting down...");
        }
    }

    info!("Validator shutdown complete");
    Ok(())
}