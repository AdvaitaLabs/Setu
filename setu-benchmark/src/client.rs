//! HTTP Client for benchmark
//!
//! Wraps reqwest client with retry logic and metrics collection.

use crate::metrics::RequestMetrics;
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Transfer request for benchmark
#[derive(Debug, Clone, Serialize)]
pub struct BenchTransferRequest {
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub transfer_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_solver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subnet_id: Option<String>,
    #[serde(default)]
    pub resources: Vec<String>,
}

/// Transfer response
#[derive(Debug, Clone, Deserialize)]
pub struct BenchTransferResponse {
    pub success: bool,
    pub message: String,
    pub transfer_id: Option<String>,
    #[allow(dead_code)]
    pub solver_id: Option<String>,
}

/// Benchmark HTTP client
pub struct BenchClient {
    client: Client,
    base_url: String,
    timeout: Duration,
}

impl BenchClient {
    /// Create a new benchmark client
    pub fn new(base_url: String, timeout_secs: u64, keep_alive: bool) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .pool_max_idle_per_host(100)
            .pool_idle_timeout(Duration::from_secs(30));

        if !keep_alive {
            builder = builder.pool_max_idle_per_host(0);
        }

        let client = builder.build()?;

        Ok(Self {
            client,
            base_url,
            timeout: Duration::from_secs(timeout_secs),
        })
    }

    /// Submit a transfer and measure latency
    pub async fn submit_transfer(&self, request: BenchTransferRequest) -> RequestMetrics {
        let url = format!("{}/api/v1/transfer", self.base_url);
        let start = Instant::now();

        let result = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await;

        let latency = start.elapsed();

        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    match response.json::<BenchTransferResponse>().await {
                        Ok(resp) => {
                            if resp.success {
                                debug!(
                                    transfer_id = ?resp.transfer_id,
                                    latency_ms = latency.as_millis(),
                                    "Transfer succeeded"
                                );
                                RequestMetrics::success(latency)
                            } else {
                                warn!(message = %resp.message, "Transfer failed (application error)");
                                RequestMetrics::failure(latency, resp.message)
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to parse response");
                            RequestMetrics::failure(latency, format!("Parse error: {}", e))
                        }
                    }
                } else {
                    let body = response.text().await.unwrap_or_default();
                    warn!(status = %status, body = %body, "HTTP error");
                    RequestMetrics::failure(latency, format!("HTTP {}: {}", status, body))
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    warn!(timeout_ms = self.timeout.as_millis(), "Request timeout");
                    RequestMetrics::timeout(latency)
                } else if e.is_connect() {
                    warn!(error = %e, "Connection error");
                    RequestMetrics::failure(latency, format!("Connection error: {}", e))
                } else {
                    warn!(error = %e, "Request error");
                    RequestMetrics::failure(latency, format!("Request error: {}", e))
                }
            }
        }
    }

    /// Health check
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/v1/status", self.base_url);
        let response = self.client.get(&url).send().await?;
        Ok(response.status().is_success())
    }
}

/// Generate a random transfer request
/// 
/// NOTE: Each request gets a unique resource key based on seq to ensure
/// consistent hash routing distributes requests evenly across solvers.
pub fn generate_transfer(
    sender_prefix: &str,
    receiver_prefix: &str,
    amount: u64,
    seq: u64,
) -> BenchTransferRequest {
    BenchTransferRequest {
        from: format!("{}_{}", sender_prefix, seq % 1000),
        to: format!("{}_{}", receiver_prefix, (seq + 500) % 1000),
        amount,
        transfer_type: "flux".to_string(),
        preferred_solver: None,
        shard_id: None,
        subnet_id: None,
        // Use unique resource key per request to ensure even distribution
        // across solvers via consistent hash routing
        resources: vec![format!("bench_resource_{}", seq)],
    }
}

/// Generate a transfer using pre-initialized test accounts (20 accounts)
/// 
/// Accounts: alice, bob, charlie, user_01 to user_17
/// This distributes load across many accounts to avoid state lock contention.
/// 
/// NOTE: Each request gets a unique resource key based on seq to ensure
/// consistent hash routing distributes requests evenly across solvers.
/// Without this, all requests from the same 'from' address would route
/// to the same solver, causing severe load imbalance in multi-solver tests.
pub fn generate_transfer_with_test_accounts(amount: u64, seq: u64) -> BenchTransferRequest {
    // 20 test accounts for better load distribution
    const ACCOUNTS: &[&str] = &[
        "alice", "bob", "charlie",
        "user_01", "user_02", "user_03", "user_04", "user_05",
        "user_06", "user_07", "user_08", "user_09", "user_10",
        "user_11", "user_12", "user_13", "user_14", "user_15",
        "user_16", "user_17",
    ];
    
    // Use different indices for sender and receiver to ensure they're different
    let sender_idx = seq as usize % ACCOUNTS.len();
    let receiver_idx = (seq as usize + 1 + (seq as usize / ACCOUNTS.len())) % ACCOUNTS.len();
    
    // Ensure sender != receiver
    let receiver_idx = if receiver_idx == sender_idx {
        (receiver_idx + 1) % ACCOUNTS.len()
    } else {
        receiver_idx
    };
    
    BenchTransferRequest {
        from: ACCOUNTS[sender_idx].to_string(),
        to: ACCOUNTS[receiver_idx].to_string(),
        amount,
        transfer_type: "flux".to_string(),
        preferred_solver: None,
        shard_id: None,
        subnet_id: None,
        // Use unique resource key per request to ensure even distribution
        // across solvers via consistent hash routing
        resources: vec![format!("bench_resource_{}", seq)],
    }
}
