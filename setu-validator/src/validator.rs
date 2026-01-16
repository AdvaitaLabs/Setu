use setu_types::{
    ConsensusConfig, ConsensusFrame, Event, EventId, EventStatus,
    NodeInfo, NodeStatus, ValidatorInfo, Vote, VLC, 
    SetuError, SetuResult,
};
use setu_consensus::{ConsensusEngine, ConsensusMessage, ValidatorSet};
use setu_network::{NetworkConfig, NetworkService, NetworkEvent, NetworkClient, PeerRole};
use setu_storage::{StateStore, EventStore, AnchorStore, CFStore, RocksDBMerkleStore};
use setu_storage::subnet_state::GlobalStateManager;
use setu_merkle::MerkleStore;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::verifier::{EventVerifier, CFVerifier};

#[derive(Clone)]
pub struct ValidatorConfig {
    pub consensus: ConsensusConfig,
    pub network: NetworkConfig,
}

impl Default for ValidatorConfig {
    fn default() -> Self {
        Self {
            consensus: ConsensusConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

pub struct Validator {
    config: ValidatorConfig,
    node_info: NodeInfo,
    validator_info: Arc<RwLock<ValidatorInfo>>,
    consensus_engine: Arc<ConsensusEngine>,
    network: Option<Arc<NetworkService>>,
    state_store: Arc<StateStore>,
    event_store: Arc<EventStore>,
    anchor_store: Arc<AnchorStore>,
    cf_store: Arc<CFStore>,
    event_verifier: Arc<EventVerifier>,
    cf_verifier: Arc<CFVerifier>,
    running: Arc<RwLock<bool>>,
}

impl Validator {
    pub fn new(node_info: NodeInfo, is_leader: bool) -> Self {
        Self::with_config(node_info, is_leader, ValidatorConfig::default())
    }

    pub fn with_config(node_info: NodeInfo, is_leader: bool, config: ValidatorConfig) -> Self {
        let validator_info = ValidatorInfo::new(node_info.clone(), is_leader);
        
        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(validator_info.clone());

        let consensus_engine = Arc::new(ConsensusEngine::new(
            config.consensus.clone(),
            node_info.id.clone(),
            validator_set,
        ));

        Self {
            config: config.clone(),
            node_info: node_info.clone(),
            validator_info: Arc::new(RwLock::new(validator_info)),
            consensus_engine,
            network: None,
            state_store: Arc::new(StateStore::new()),
            event_store: Arc::new(EventStore::new()),
            anchor_store: Arc::new(AnchorStore::new()),
            cf_store: Arc::new(CFStore::new()),
            event_verifier: Arc::new(EventVerifier::new(node_info.id.clone())),
            cf_verifier: Arc::new(CFVerifier::new(
                node_info.id.clone(),
                config.consensus.validator_count,
            )),
            running: Arc::new(RwLock::new(false)),
        }
    }
    
    /// Create a validator with persistent Merkle store for state management
    /// 
    /// This constructor initializes the validator with a RocksDB-backed
    /// MerkleStore for persisting subnet state roots and global state roots.
    pub fn with_merkle_store(
        node_info: NodeInfo, 
        is_leader: bool, 
        config: ValidatorConfig,
        merkle_store: Arc<dyn MerkleStore>,
    ) -> Self {
        let validator_info = ValidatorInfo::new(node_info.clone(), is_leader);
        
        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(validator_info.clone());

        // Create GlobalStateManager with persistent store
        let state_manager = GlobalStateManager::with_store(merkle_store);
        
        // Create ConsensusEngine with state manager
        let consensus_engine = Arc::new(ConsensusEngine::with_state_manager(
            config.consensus.clone(),
            node_info.id.clone(),
            validator_set,
            state_manager,
        ));

        Self {
            config: config.clone(),
            node_info: node_info.clone(),
            validator_info: Arc::new(RwLock::new(validator_info)),
            consensus_engine,
            network: None,
            state_store: Arc::new(StateStore::new()),
            event_store: Arc::new(EventStore::new()),
            anchor_store: Arc::new(AnchorStore::new()),
            cf_store: Arc::new(CFStore::new()),
            event_verifier: Arc::new(EventVerifier::new(node_info.id.clone())),
            cf_verifier: Arc::new(CFVerifier::new(
                node_info.id.clone(),
                config.consensus.validator_count,
            )),
            running: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn start(&self) -> SetuResult<()> {
        *self.running.write().await = true;

        let (event_tx, mut event_rx) = mpsc::channel::<NetworkEvent>(1000);
        let network = Arc::new(NetworkService::new(
            self.config.network.clone(),
            self.node_info.clone(),
            event_tx,
        ));

        network.start().await?;

        {
            let mut validator_info = self.validator_info.write().await;
            validator_info.node.status = NodeStatus::Active;
        }

        self.start_event_handler(event_rx).await;
        self.start_consensus_loop().await;

        tracing::info!("Validator {} started", self.node_info.id);
        Ok(())
    }

    async fn start_event_handler(&self, mut event_rx: mpsc::Receiver<NetworkEvent>) {
        let running = Arc::clone(&self.running);
        let consensus_engine = Arc::clone(&self.consensus_engine);
        let event_store = Arc::clone(&self.event_store);
        let cf_store = Arc::clone(&self.cf_store);
        let anchor_store = Arc::clone(&self.anchor_store);
        let event_verifier = Arc::clone(&self.event_verifier);
        let cf_verifier = Arc::clone(&self.cf_verifier);

        tokio::spawn(async move {
            while *running.read().await {
                if let Some(net_event) = event_rx.recv().await {
                    match net_event {
                        NetworkEvent::EventReceived { event, .. } => {
                            if event_verifier.verify_event(&event).is_ok() {
                                let _ = consensus_engine.add_event(event.clone()).await;
                                let _ = event_store.store(event).await;
                            }
                        }
                        NetworkEvent::CFProposed { cf, .. } => {
                            if cf_verifier.verify_cf(&cf).is_ok() {
                                let _ = consensus_engine.receive_cf(cf.clone()).await;
                                let _ = cf_store.store(cf).await;
                            }
                        }
                        NetworkEvent::VoteReceived { vote, .. } => {
                            // receive_vote now returns (finalized, Option<Anchor>)
                            if let Ok((finalized, anchor)) = consensus_engine.receive_vote(vote).await {
                                if finalized {
                                    // Store the finalized anchor
                                    if let Some(ref anchor) = anchor {
                                        match anchor_store.store(anchor.clone()).await {
                                            Ok(_) => {
                                                // Mark as persisted for safe GC
                                                consensus_engine.mark_anchor_persisted(&anchor.id).await;
                                            }
                                            Err(e) => {
                                                tracing::error!("Failed to store finalized anchor: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        NetworkEvent::CFFinalized { cf, .. } => {
                            cf_store.mark_finalized(&cf.id).await;
                            // Also store the anchor from the finalized CF
                            match anchor_store.store(cf.anchor.clone()).await {
                                Ok(_) => {
                                    // Mark as persisted for safe GC
                                    consensus_engine.mark_anchor_persisted(&cf.anchor.id).await;
                                }
                                Err(e) => {
                                    tracing::error!("Failed to store anchor from finalized CF: {}", e);
                                }
                            }
                            tracing::info!("CF finalized: {}", cf.id);
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    async fn start_consensus_loop(&self) {
        let running = Arc::clone(&self.running);
        let validator_info = Arc::clone(&self.validator_info);
        let consensus_engine = Arc::clone(&self.consensus_engine);
        let cf_store = Arc::clone(&self.cf_store);
        let anchor_store = Arc::clone(&self.anchor_store);

        tokio::spawn(async move {
            while *running.read().await {
                let info = validator_info.read().await;
                let is_leader = info.is_leader;
                drop(info);

                if is_leader {
                    let vlc = consensus_engine.get_vlc_snapshot().await;
                    // Check if we should propose a new CF
                    // This is handled internally by the consensus engine
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });
    }

    pub async fn add_peer_validator(&self, node_info: NodeInfo) -> SetuResult<()> {
        if let Some(ref network) = self.network {
            network.connect_to_peer(node_info, PeerRole::Validator).await?;
        }
        Ok(())
    }

    pub async fn stop(&self) {
        *self.running.write().await = false;

        let mut validator_info = self.validator_info.write().await;
        validator_info.node.status = NodeStatus::Inactive;

        tracing::info!("Validator {} stopped", self.node_info.id);
    }

    pub async fn get_stats(&self) -> ValidatorStats {
        let validator_info = self.validator_info.read().await;
        let dag_stats = self.consensus_engine.get_dag_stats().await;
        let finalized_cfs = self.cf_store.finalized_count().await;
        let pending_cfs = self.cf_store.pending_count().await;

        ValidatorStats {
            node_id: self.node_info.id.clone(),
            is_leader: validator_info.is_leader,
            status: validator_info.node.status,
            dag_node_count: dag_stats.node_count,
            dag_max_depth: dag_stats.max_depth,
            finalized_cfs,
            pending_cfs,
            event_count: self.event_store.count().await,
        }
    }

    pub async fn submit_event(&self, event: Event) -> SetuResult<EventId> {
        self.event_verifier.verify_event(&event)?;
        
        let event_id = self.consensus_engine.add_event(event.clone()).await?;
        self.event_store.store(event).await?;

        Ok(event_id)
    }
}

#[derive(Debug, Clone)]
pub struct ValidatorStats {
    pub node_id: String,
    pub is_leader: bool,
    pub status: NodeStatus,
    pub dag_node_count: usize,
    pub dag_max_depth: u64,
    pub finalized_cfs: usize,
    pub pending_cfs: usize,
    pub event_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{EventType, VectorClock, VLCSnapshot};

    #[tokio::test]
    async fn test_validator_creation() {
        let node = NodeInfo::new_validator("v1".to_string(), "127.0.0.1".to_string(), 8000);
        let validator = Validator::new(node, true);

        let stats = validator.get_stats().await;
        assert_eq!(stats.node_id, "v1");
        assert!(stats.is_leader);
    }

    #[tokio::test]
    async fn test_submit_event() {
        let node = NodeInfo::new_validator("v1".to_string(), "127.0.0.1".to_string(), 8000);
        let validator = Validator::new(node, true);

        let event = Event::genesis(
            "v1".to_string(),
            VLCSnapshot {
                vector_clock: VectorClock::new(),
                logical_time: 0,
                physical_time: 0,
            },
        );

        let event_id = validator.submit_event(event).await.unwrap();
        assert!(!event_id.is_empty());

        let stats = validator.get_stats().await;
        assert_eq!(stats.dag_node_count, 1);
    }
}
