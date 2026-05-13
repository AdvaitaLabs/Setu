//! GovernanceService — manages async Agent subnet communication and pending proposals.
//!
//! Tracks pending proposals in DashMap (G10: recovered via DAG replay).
//! Dispatches proposals to Agent subnet via HTTP.
//! Detects orphaned proposals via subscribe_finalization() + timeout.

use dashmap::DashMap;
use setu_types::governance::{GovernanceDecision, ProposalContent};
use setu_types::genesis::GenesisValidator;
use setu_types::SubnetId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};

// ========== System Subnet Registry ==========

/// Source of a system subnet endpoint configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// From environment variable at startup (bootstrap only)
    Bootstrap,
    /// From on-chain RegisterSystemSubnet event
    OnChain,
}

/// Configuration for a single system subnet's Agent endpoint.
#[derive(Debug, Clone)]
pub struct SystemSubnetConfig {
    pub agent_endpoint: String,
    pub callback_addr: Option<String>,
    pub timeout: Duration,
    pub source: ConfigSource,
}

/// In-memory registry of system subnet Agent endpoints.
/// G10: recovered from DAG replay of GovernanceAction::RegisterSystemSubnet events.
pub struct SystemSubnetRegistry {
    endpoints: DashMap<SubnetId, SystemSubnetConfig>,
}

impl SystemSubnetRegistry {
    pub fn new() -> Self {
        Self {
            endpoints: DashMap::new(),
        }
    }

    /// Register or update an endpoint for a system subnet.
    pub fn register(&self, subnet_id: SubnetId, config: SystemSubnetConfig) {
        self.endpoints.insert(subnet_id, config);
    }

    /// Look up endpoint configuration for a system subnet.
    pub fn resolve(&self, subnet_id: &SubnetId) -> Option<SystemSubnetConfig> {
        self.endpoints.get(subnet_id).map(|v| v.clone())
    }

    /// Number of registered endpoints.
    pub fn len(&self) -> usize {
        self.endpoints.len()
    }
}

/// In-memory state for a pending proposal (G10: needs replay).
#[derive(Debug, Clone)]
pub struct PendingProposal {
    pub proposal_id: [u8; 32],
    pub content: ProposalContent,
    pub submitted_at: Instant,
    /// One-time callback token for Agent authentication (R1-ISSUE-8).
    pub callback_token: [u8; 32],
    /// Original event timestamp (millis). Used for fallback proposal construction
    /// so that resolve_proposal uses the true created_at instead of current time.
    pub created_at: u64,
}

/// GovernanceService configuration.
#[derive(Debug, Clone)]
pub struct GovernanceServiceConfig {
    /// Validator's externally-reachable callback address for Agent → Validator callbacks.
    /// e.g. "10.0.1.5:8080". Agent uses this to POST decisions back.
    pub callback_addr: String,
    /// Proposal timeout (default: 300s)
    pub timeout: Duration,
    /// Max HTTP retries for Agent dispatch (default: 3, exponential backoff 1s/4s/16s)
    pub max_retries: u32,
    /// Takeover multiplier: takeover after `created_at + takeover_factor * timeout` (default: 2)
    pub takeover_factor: u64,
}

impl Default for GovernanceServiceConfig {
    fn default() -> Self {
        Self {
            callback_addr: "127.0.0.1:8080".to_string(),
            timeout: Duration::from_secs(300),
            max_retries: 3,
            takeover_factor: 2,
        }
    }
}

/// Manages async Agent subnet communication and pending proposals.
pub struct GovernanceService {
    /// Configuration
    config: GovernanceServiceConfig,
    /// HTTP client (reqwest)
    client: reqwest::Client,
    /// Pending proposals awaiting Agent decision (initiator only)
    pending_governance: DashMap<[u8; 32], PendingProposal>,
    /// All-Validator proposal tracker: proposal_id → created_at timestamp (R6-ISSUE-2)
    /// Populated by listening to finalized CFs via subscribe_finalization().
    /// Used for takeover discovery when initiator crashes.
    /// G10: recovered from DAG replay on restart.
    proposal_tracker: DashMap<[u8; 32], u64>,
    /// Monotonic nonce for deterministic proposal_id generation
    nonce_counter: AtomicU64,
    /// Dynamic system subnet endpoint registry (G10: recovered from DAG replay).
    registry: SystemSubnetRegistry,
    /// Genesis validators for registration authorization.
    genesis_validators: Vec<GenesisValidator>,
    /// Decided proposals cache: proposal_id → decision.
    /// Filled when poll/callback consumes a pending proposal (before SMT finalization).
    decided_proposals: DashMap<[u8; 32], GovernanceDecision>,
}

impl GovernanceService {
    pub fn new(config: GovernanceServiceConfig) -> Self {
        Self::with_genesis_validators(config, Vec::new())
    }

    /// Create a new GovernanceService with genesis validator list for registration auth.
    pub fn with_genesis_validators(
        config: GovernanceServiceConfig,
        genesis_validators: Vec<GenesisValidator>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            config,
            client,
            pending_governance: DashMap::new(),
            proposal_tracker: DashMap::new(),
            nonce_counter: AtomicU64::new(0),
            registry: SystemSubnetRegistry::new(),
            genesis_validators,
            decided_proposals: DashMap::new(),
        }
    }

    /// Get the genesis validators list (for registration authorization).
    pub fn genesis_validators(&self) -> &[GenesisValidator] {
        &self.genesis_validators
    }

    /// Generate a deterministic proposal_id.
    /// `blake3("SETU_PROPOSAL:" + proposer + nonce + timestamp)`
    pub fn generate_proposal_id(&self, proposer: &str, timestamp: u64) -> [u8; 32] {
        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        let input = format!("SETU_PROPOSAL:{}:{}:{}", proposer, nonce, timestamp);
        *blake3::hash(input.as_bytes()).as_bytes()
    }

    /// Generate a cryptographically random callback token for Agent authentication.
    pub fn generate_callback_token() -> [u8; 32] {
        let mut token = [0u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut token);
        token
    }

    /// Insert a pending proposal (called after Propose event is submitted to DAG).
    pub fn insert_pending(&self, proposal: PendingProposal) {
        let id = proposal.proposal_id;
        self.pending_governance.insert(id, proposal);
    }

    /// Remove a pending proposal (called after Execute event is submitted to DAG).
    pub fn remove_pending(&self, proposal_id: &[u8; 32]) -> Option<PendingProposal> {
        self.pending_governance.remove(proposal_id).map(|(_, v)| v)
    }

    /// Look up pending proposal by proposal_id.
    pub fn get_pending(&self, proposal_id: &[u8; 32]) -> Option<PendingProposal> {
        self.pending_governance.get(proposal_id).map(|v| v.clone())
    }

    /// Record a decided (approved/rejected/timed-out) proposal for status queries.
    pub fn record_decided(&self, proposal_id: [u8; 32], decision: GovernanceDecision) {
        self.decided_proposals.insert(proposal_id, decision);
    }

    /// Look up a decided proposal by proposal_id.
    pub fn get_decided(&self, proposal_id: &[u8; 32]) -> Option<GovernanceDecision> {
        self.decided_proposals.get(proposal_id).map(|v| v.clone())
    }

    /// Validate a callback token against a pending proposal.
    /// Returns true if the token matches the expected callback_token.
    pub fn validate_callback_token(
        &self,
        proposal_id: &[u8; 32],
        token: &[u8; 32],
    ) -> bool {
        self.pending_governance
            .get(proposal_id)
            .map(|entry| constant_time_eq(&entry.callback_token, token))
            .unwrap_or(false)
    }

    /// Track a proposal in the all-validator tracker (from finalized CF).
    pub fn track_proposal(&self, proposal_id: [u8; 32], created_at: u64) {
        self.proposal_tracker.insert(proposal_id, created_at);
    }

    /// Untrack a proposal (Execute event finalized).
    pub fn untrack_proposal(&self, proposal_id: &[u8; 32]) {
        self.proposal_tracker.remove(proposal_id);
    }

    /// Check for timed-out proposals in the local pending_governance.
    pub fn check_timeouts(&self) -> Vec<[u8; 32]> {
        let now = Instant::now();
        self.pending_governance
            .iter()
            .filter(|entry| now.duration_since(entry.submitted_at) > self.config.timeout)
            .map(|entry| *entry.key())
            .collect()
    }

    /// Check for takeover candidates in the all-validator proposal_tracker.
    /// Returns proposal_ids where `created_at + takeover_factor * timeout < now`.
    /// NOTE: `created_at` and `now_timestamp` are both in milliseconds (from Event::new()).
    pub fn check_takeover_candidates(&self, now_timestamp: u64) -> Vec<[u8; 32]> {
        let takeover_threshold_ms = self.config.takeover_factor * self.config.timeout.as_millis() as u64;
        self.proposal_tracker
            .iter()
            .filter(|entry| now_timestamp > *entry.value() + takeover_threshold_ms)
            .map(|entry| *entry.key())
            .collect()
    }

    /// Dispatch a proposal to the Agent subnet for evaluation.
    /// Retries with exponential backoff (1s, 4s, 16s).
    ///
    /// `subnet_id`: target system subnet (Phase 1: always SubnetId::GOVERNANCE).
    /// Endpoint is resolved from the SystemSubnetRegistry.
    pub async fn dispatch_to_agent(
        &self,
        subnet_id: &SubnetId,
        proposal_id: [u8; 32],
        content: &ProposalContent,
        callback_token: [u8; 32],
        system_context: serde_json::Value,
    ) -> Result<(), GovernanceServiceError> {
        let config = self.resolve_endpoint(subnet_id).ok_or_else(|| {
            GovernanceServiceError::AgentError(format!(
                "No endpoint registered for subnet {}", subnet_id
            ))
        })?;
        let callback_addr = config.callback_addr
            .as_deref()
            .unwrap_or(&self.config.callback_addr);
        let url = format!("{}/evaluate", config.agent_endpoint);
        let body = serde_json::json!({
            "task_id": hex::encode(proposal_id),
            "callback_token": hex::encode(callback_token),
            "callback_url": format!("http://{}/api/v1/governance/callback",
                                     callback_addr),
            "proposal": content,
            "system_context": system_context
        });

        let mut last_err = None;
        let mut delay = Duration::from_secs(1);

        for attempt in 0..self.config.max_retries {
            match self.client.post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(
                        proposal_id = %hex::encode(proposal_id),
                        attempt = attempt + 1,
                        "Dispatched proposal to Agent subnet"
                    );
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    warn!(
                        proposal_id = %hex::encode(proposal_id),
                        attempt = attempt + 1,
                        status = %status,
                        "Agent subnet returned non-success status"
                    );
                    last_err = Some(GovernanceServiceError::AgentError(format!(
                        "HTTP {}", status
                    )));
                }
                Err(e) => {
                    warn!(
                        proposal_id = %hex::encode(proposal_id),
                        attempt = attempt + 1,
                        error = %e,
                        "Failed to dispatch to Agent subnet"
                    );
                    last_err = Some(GovernanceServiceError::NetworkError(e.to_string()));
                }
            }

            if attempt + 1 < self.config.max_retries {
                tokio::time::sleep(delay).await;
                delay *= 4; // exponential backoff: 1s, 4s, 16s
            }
        }

        // All retries exhausted — leave in pending for timeout handler
        Err(last_err.unwrap_or(GovernanceServiceError::AgentError(
            "All retries exhausted".to_string(),
        )))
    }

    /// Restore pending proposals from DAG replay stats.
    /// Called during startup after replay_all() completes.
    pub fn restore_from_replay(&self, pending: Vec<([u8; 32], ProposalContent, u64)>) {
        for (proposal_id, content, created_at) in pending {
            let callback_token = Self::generate_callback_token();
            self.insert_pending(PendingProposal {
                proposal_id,
                content,
                submitted_at: Instant::now(),
                callback_token,
                created_at,
            });
            info!(
                proposal_id = %hex::encode(proposal_id),
                "Restored pending proposal from DAG replay (new callback_token)"
            );
        }
    }

    // ========== System Subnet Registry Methods ==========

    /// Register or update a system subnet endpoint in the registry.
    pub fn register_system_endpoint(&self, subnet_id: SubnetId, config: SystemSubnetConfig) {
        let source_str = match &config.source {
            ConfigSource::Bootstrap => "bootstrap",
            ConfigSource::OnChain => "on-chain",
        };
        info!(
            subnet_id = %subnet_id,
            endpoint = %config.agent_endpoint,
            source = source_str,
            "Registered system subnet endpoint"
        );
        self.registry.register(subnet_id, config);
    }

    /// Look up endpoint configuration for a system subnet.
    pub fn resolve_endpoint(&self, subnet_id: &SubnetId) -> Option<SystemSubnetConfig> {
        self.registry.resolve(subnet_id)
    }

    /// Number of registered system subnet endpoints.
    pub fn registry_count(&self) -> usize {
        self.registry.len()
    }

    /// Get the configuration.
    pub fn config(&self) -> &GovernanceServiceConfig {
        &self.config
    }

    /// Get number of pending proposals.
    pub fn pending_count(&self) -> usize {
        self.pending_governance.len()
    }

    /// Get number of tracked proposals.
    pub fn tracked_count(&self) -> usize {
        self.proposal_tracker.len()
    }

    /// Return a snapshot of all pending proposals.
    pub fn all_pending(&self) -> Vec<PendingProposal> {
        self.pending_governance
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Poll Agent subnet for a single proposal's result.
    /// Returns `Ok(Some(decision))` if completed, `Ok(None)` if still pending.
    pub async fn poll_agent_result(
        &self,
        subnet_id: &SubnetId,
        proposal_id: [u8; 32],
    ) -> Result<Option<GovernanceDecision>, GovernanceServiceError> {
        let config = self.resolve_endpoint(subnet_id).ok_or_else(|| {
            GovernanceServiceError::AgentError(format!(
                "No endpoint registered for subnet {}", subnet_id
            ))
        })?;
        let url = format!(
            "{}/result/{}",
            config.agent_endpoint,
            hex::encode(proposal_id)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| GovernanceServiceError::NetworkError(e.to_string()))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GovernanceServiceError::NetworkError(e.to_string()))?;
        if body.get("status").and_then(|s| s.as_str()) == Some("completed") {
            let decision: GovernanceDecision = serde_json::from_value(
                body["decision"].clone(),
            )
            .map_err(|e| GovernanceServiceError::AgentError(e.to_string()))?;
            Ok(Some(decision))
        } else {
            Ok(None)
        }
    }
}

/// Constant-time comparison to prevent timing attacks on callback tokens.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[derive(Debug, thiserror::Error)]
pub enum GovernanceServiceError {
    #[error("Agent subnet error: {0}")]
    AgentError(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Proposal not found: {0}")]
    ProposalNotFound(String),
    #[error("Invalid callback token")]
    InvalidCallbackToken,
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::governance::{ProposalEffect, ProposalType};

    fn sample_config() -> GovernanceServiceConfig {
        GovernanceServiceConfig {
            callback_addr: "127.0.0.1:8080".to_string(),
            timeout: Duration::from_millis(100), // short for tests
            max_retries: 2,
            takeover_factor: 2,
        }
    }

    fn sample_content() -> ProposalContent {
        ProposalContent {
            proposer: "alice".to_string(),
            proposal_type: ProposalType::ParameterChange,
            title: "Test".to_string(),
            description: "desc".to_string(),
            action: ProposalEffect::UpdateParameter {
                key: "k".to_string(),
                value: vec![1],
            },
        }
    }

    // ---- GovernanceService tests ----

    #[test]
    fn test_generate_proposal_id_deterministic() {
        let svc = GovernanceService::new(sample_config());
        let id1 = svc.generate_proposal_id("alice", 1000);
        // Second call with same params gets different id (nonce increments)
        let id2 = svc.generate_proposal_id("alice", 1000);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_generate_proposal_id_different_proposers() {
        let svc = GovernanceService::new(sample_config());
        let id1 = svc.generate_proposal_id("alice", 1000);
        let _skip = svc.generate_proposal_id("_skip", 0); // consume nonce=1
        // Reset nonce to match: not possible, so just verify different proposers give different ids
        // even at different nonce values
        let id3 = svc.generate_proposal_id("bob", 1000);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_insert_remove_pending() {
        let svc = GovernanceService::new(sample_config());
        let proposal = PendingProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(),
            callback_token: [2u8; 32],
            created_at: 1000,
        };
        svc.insert_pending(proposal.clone());
        assert_eq!(svc.pending_count(), 1);

        let removed = svc.remove_pending(&[1u8; 32]);
        assert!(removed.is_some());
        assert_eq!(svc.pending_count(), 0);
    }

    #[test]
    fn test_validate_callback_token_ok() {
        let svc = GovernanceService::new(sample_config());
        let token = [42u8; 32];
        svc.insert_pending(PendingProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(),
            callback_token: token,
            created_at: 1000,
        });
        assert!(svc.validate_callback_token(&[1u8; 32], &token));
    }

    #[test]
    fn test_validate_callback_token_wrong() {
        let svc = GovernanceService::new(sample_config());
        svc.insert_pending(PendingProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(),
            callback_token: [42u8; 32],
            created_at: 1000,
        });
        assert!(!svc.validate_callback_token(&[1u8; 32], &[99u8; 32]));
    }

    #[test]
    fn test_validate_callback_token_not_found() {
        let svc = GovernanceService::new(sample_config());
        assert!(!svc.validate_callback_token(&[1u8; 32], &[42u8; 32]));
    }

    #[test]
    fn test_check_timeouts() {
        let svc = GovernanceService::new(sample_config());
        // Insert a proposal with submitted_at in the past
        svc.insert_pending(PendingProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            submitted_at: Instant::now() - Duration::from_millis(200), // past timeout
            callback_token: [2u8; 32],
            created_at: 1000,
        });
        svc.insert_pending(PendingProposal {
            proposal_id: [2u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(), // not timed out
            callback_token: [3u8; 32],
            created_at: 2000,
        });

        let timed_out = svc.check_timeouts();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], [1u8; 32]);
    }

    #[test]
    fn test_track_untrack_proposal() {
        let svc = GovernanceService::new(sample_config());
        svc.track_proposal([1u8; 32], 1000);
        assert_eq!(svc.tracked_count(), 1);

        svc.untrack_proposal(&[1u8; 32]);
        assert_eq!(svc.tracked_count(), 0);
    }

    #[test]
    fn test_check_takeover_candidates() {
        let svc = GovernanceService::new(GovernanceServiceConfig {
            timeout: Duration::from_secs(10),
            takeover_factor: 2,
            ..sample_config()
        });
        // created_at values in milliseconds (as stored from event.timestamp)
        svc.track_proposal([1u8; 32], 1_000_000); // created_at = 1_000_000 ms
        svc.track_proposal([2u8; 32], 5_000_000); // created_at = 5_000_000 ms

        // takeover_threshold_ms = 2 * 10_000 = 20_000 ms
        // At time 1_030_000 ms, proposal 1 should be a takeover candidate (1_000_000 + 20_000 = 1_020_000 < 1_030_000)
        let candidates = svc.check_takeover_candidates(1_030_000);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], [1u8; 32]);

        // At time 5_030_000 ms, both should be takeover candidates
        let candidates = svc.check_takeover_candidates(5_030_000);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn test_restore_from_replay() {
        let svc = GovernanceService::new(sample_config());
        let pending = vec![
            ([1u8; 32], sample_content(), 1000u64),
            ([2u8; 32], sample_content(), 2000u64),
        ];
        svc.restore_from_replay(pending);
        assert_eq!(svc.pending_count(), 2);
        // Restored proposals have new callback tokens
        let p1 = svc.get_pending(&[1u8; 32]).unwrap();
        let p2 = svc.get_pending(&[2u8; 32]).unwrap();
        assert_ne!(p1.callback_token, p2.callback_token);
        // Verify created_at is preserved
        assert_eq!(p1.created_at, 1000);
        assert_eq!(p2.created_at, 2000);
    }

    #[tokio::test]
    async fn test_dispatch_to_agent_no_server() {
        // Dispatch to a non-existent server — should fail after retries
        let svc = GovernanceService::new(GovernanceServiceConfig {
            timeout: Duration::from_secs(1),
            max_retries: 1,
            ..sample_config()
        });
        // Register the GOVERNANCE endpoint so dispatch can resolve it
        svc.register_system_endpoint(
            setu_types::SubnetId::GOVERNANCE,
            SystemSubnetConfig {
                agent_endpoint: "http://127.0.0.1:1".to_string(),
                callback_addr: None,
                timeout: Duration::from_secs(1),
                source: ConfigSource::Bootstrap,
            },
        );
        let result = svc
            .dispatch_to_agent(&setu_types::SubnetId::GOVERNANCE, [1u8; 32], &sample_content(), [2u8; 32], serde_json::json!({"validator_id": "test"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_all_pending() {
        let svc = GovernanceService::new(sample_config());
        svc.insert_pending(PendingProposal {
            proposal_id: [1u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(),
            callback_token: [10u8; 32],
            created_at: 1000,
        });
        svc.insert_pending(PendingProposal {
            proposal_id: [2u8; 32],
            content: sample_content(),
            submitted_at: Instant::now(),
            callback_token: [20u8; 32],
            created_at: 2000,
        });
        let all = svc.all_pending();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_poll_agent_result_no_server() {
        let svc = GovernanceService::new(sample_config());
        // Register a non-reachable endpoint
        svc.register_system_endpoint(
            setu_types::SubnetId::GOVERNANCE,
            SystemSubnetConfig {
                agent_endpoint: "http://127.0.0.1:1".to_string(),
                callback_addr: None,
                timeout: Duration::from_secs(1),
                source: ConfigSource::OnChain,
            },
        );
        let result = svc.poll_agent_result(&setu_types::SubnetId::GOVERNANCE, [1u8; 32]).await;
        // No server → either connection error (Err) or non-200 response (Ok(None))
        match result {
            Err(_) => {} // connection refused — expected
            Ok(None) => {} // non-success HTTP status — also acceptable
            Ok(Some(_)) => panic!("Should not get a decision from non-existent server"),
        }
    }

    #[test]
    fn test_config_callback_addr_default() {
        let config = GovernanceServiceConfig::default();
        assert!(!config.callback_addr.is_empty());
        assert!(config.callback_addr.contains(':'));
    }

    // ---- SystemSubnetRegistry tests ----

    #[test]
    fn test_register_system_endpoint_basic() {
        let svc = GovernanceService::new(sample_config());
        let subnet = setu_types::SubnetId::new_system(0x20);
        svc.register_system_endpoint(
            subnet,
            SystemSubnetConfig {
                agent_endpoint: "http://oracle:8091".to_string(),
                callback_addr: Some("10.0.1.5:8080".to_string()),
                timeout: Duration::from_secs(60),
                source: ConfigSource::OnChain,
            },
        );
        let resolved = svc.resolve_endpoint(&subnet);
        assert!(resolved.is_some());
        let config = resolved.unwrap();
        assert_eq!(config.agent_endpoint, "http://oracle:8091");
        assert_eq!(config.callback_addr.as_deref(), Some("10.0.1.5:8080"));
        assert_eq!(config.source, ConfigSource::OnChain);
        assert_eq!(svc.registry_count(), 1);
    }

    #[test]
    fn test_register_system_endpoint_overwrite() {
        let svc = GovernanceService::new(sample_config());
        let subnet = setu_types::SubnetId::GOVERNANCE;
        // Bootstrap config
        svc.register_system_endpoint(
            subnet,
            SystemSubnetConfig {
                agent_endpoint: "http://localhost:8090".to_string(),
                callback_addr: None,
                timeout: Duration::from_secs(300),
                source: ConfigSource::Bootstrap,
            },
        );
        assert_eq!(svc.resolve_endpoint(&subnet).unwrap().source, ConfigSource::Bootstrap);

        // OnChain overwrite
        svc.register_system_endpoint(
            subnet,
            SystemSubnetConfig {
                agent_endpoint: "http://new-agent:8090".to_string(),
                callback_addr: Some("10.0.2.1:8080".to_string()),
                timeout: Duration::from_secs(120),
                source: ConfigSource::OnChain,
            },
        );
        let resolved = svc.resolve_endpoint(&subnet).unwrap();
        assert_eq!(resolved.agent_endpoint, "http://new-agent:8090");
        assert_eq!(resolved.source, ConfigSource::OnChain);
        assert_eq!(svc.registry_count(), 1); // no duplicate
    }

    #[test]
    fn test_resolve_endpoint_miss() {
        let svc = GovernanceService::new(sample_config());
        let result = svc.resolve_endpoint(&setu_types::SubnetId::new_system(0x99));
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_dispatch_uses_registry() {
        let svc = GovernanceService::new(GovernanceServiceConfig {
            max_retries: 1,
            ..sample_config()
        });
        // No endpoint registered → dispatch should fail with "No endpoint registered"
        let result = svc
            .dispatch_to_agent(
                &setu_types::SubnetId::GOVERNANCE,
                [1u8; 32],
                &sample_content(),
                [2u8; 32],
                serde_json::json!({}),
            )
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No endpoint registered"), "Error was: {}", err_msg);
    }
}
