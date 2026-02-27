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

/// Batch transfer request
#[derive(Debug, Clone, Serialize)]
pub struct BenchBatchRequest {
    pub transfers: Vec<BenchTransferRequest>,
}

/// Batch transfer result for a single transfer
#[derive(Debug, Clone, Deserialize)]
pub struct BenchBatchTransferResult {
    pub index: usize,
    pub success: bool,
    pub transfer_id: Option<String>,
    pub solver_id: Option<String>,
    pub error: Option<String>,
}

/// Batch transfer response
#[derive(Debug, Clone, Deserialize)]
pub struct BenchBatchResponse {
    pub success: bool,
    pub message: String,
    pub submitted: usize,
    pub failed: usize,
    pub results: Vec<BenchBatchTransferResult>,
    #[allow(dead_code)]
    pub stats: Option<BenchBatchStats>,
}

/// Batch preparation statistics
#[derive(Debug, Clone, Deserialize)]
pub struct BenchBatchStats {
    #[allow(dead_code)]
    pub total_transfers: usize,
    #[allow(dead_code)]
    pub unique_sender_subnet_pairs: usize,
    #[allow(dead_code)]
    pub coins_selected: usize,
    #[allow(dead_code)]
    pub same_sender_conflicts: usize,
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

    /// Submit a batch of transfers and measure latency
    /// 
    /// Returns a vector of RequestMetrics, one for each transfer in the batch.
    /// The latency is the total batch request latency divided by batch size.
    pub async fn submit_transfers_batch(&self, requests: Vec<BenchTransferRequest>) -> Vec<RequestMetrics> {
        let batch_size = requests.len();
        if batch_size == 0 {
            return vec![];
        }

        let url = format!("{}/api/v1/transfers/batch", self.base_url);
        let start = Instant::now();

        let batch_request = BenchBatchRequest { transfers: requests };

        let result = self
            .client
            .post(&url)
            .json(&batch_request)
            .send()
            .await;

        let total_latency = start.elapsed();
        // Approximate per-transfer latency (batch amortizes overhead)
        let per_transfer_latency = total_latency / batch_size as u32;

        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    match response.json::<BenchBatchResponse>().await {
                        Ok(resp) => {
                            debug!(
                                submitted = resp.submitted,
                                failed = resp.failed,
                                total_latency_ms = total_latency.as_millis(),
                                "Batch transfer completed"
                            );
                            
                            // Convert batch results to individual metrics
                            resp.results
                                .iter()
                                .map(|r| {
                                    if r.success {
                                        RequestMetrics::success(per_transfer_latency)
                                    } else {
                                        RequestMetrics::failure(
                                            per_transfer_latency,
                                            r.error.clone().unwrap_or_else(|| "Unknown error".to_string()),
                                        )
                                    }
                                })
                                .collect()
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to parse batch response");
                            vec![RequestMetrics::failure(total_latency, format!("Parse error: {}", e)); batch_size]
                        }
                    }
                } else {
                    let body = response.text().await.unwrap_or_default();
                    warn!(status = %status, body = %body, "HTTP error on batch request");
                    vec![RequestMetrics::failure(total_latency, format!("HTTP {}: {}", status, body)); batch_size]
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    warn!(timeout_ms = self.timeout.as_millis(), "Batch request timeout");
                    vec![RequestMetrics::timeout(total_latency); batch_size]
                } else {
                    warn!(error = %e, "Batch request error");
                    vec![RequestMetrics::failure(total_latency, format!("Request error: {}", e)); batch_size]
                }
            }
        }
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

/// Generate a transfer using test accounts
/// 
/// Test accounts are created in two tiers:
/// 1. Seed accounts: alice, bob, charlie (always available, high balance)
/// 2. User accounts: user_001, user_002, ... (created via --init-accounts)
/// 
/// If `num_test_accounts` is specified, uses that many user accounts.
/// Otherwise, falls back to using only the 3 seed accounts.
/// 
/// ## IMPORTANT: Setu's 1:1 Coin Binding
/// 
/// Setu uses 1:1 binding between (owner, subnet) and coin.
/// Each account has exactly ONE coin, so concurrent requests from the same
/// sender will cause reservation conflicts.
/// 
/// Success rate formula: min(num_accounts / concurrency, 1) * 100%
/// - 100 accounts, 100 concurrent → ~100% success
/// - 3 accounts, 100 concurrent → ~3% success
/// 
/// ## NOTE on Solver Distribution
/// 
/// Each request gets a unique resource key based on seq to ensure
/// consistent hash routing distributes requests evenly across solvers.
/// Without this, all requests from the same 'from' address would route
/// to the same solver, causing severe load imbalance in multi-solver tests.
#[allow(dead_code)]
pub fn generate_transfer_with_test_accounts(amount: u64, seq: u64) -> BenchTransferRequest {
    generate_transfer_with_n_accounts(amount, seq, None)
}

/// Generate a transfer using a specified number of test accounts
/// 
/// - If `num_accounts` is None or 0, uses only seed accounts (alice, bob, charlie)
/// - Otherwise, uses user_001 to user_N (must be initialized via --init-accounts first)
pub fn generate_transfer_with_n_accounts(amount: u64, seq: u64, num_accounts: Option<u64>) -> BenchTransferRequest {
    // Build account list based on configuration
    let accounts: Vec<String> = match num_accounts {
        Some(n) if n > 0 => {
            // Use initialized user accounts (user_001 to user_N)
            (1..=n).map(|i| format!("user_{:03}", i)).collect()
        }
        _ => {
            // Fall back to seed accounts only
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string(),
            ]
        }
    };
    
    let num = accounts.len();
    
    // Use different indices for sender and receiver to ensure they're different
    let sender_idx = seq as usize % num;
    let receiver_idx = (seq as usize + 1 + (seq as usize / num)) % num;
    
    // Ensure sender != receiver
    let receiver_idx = if receiver_idx == sender_idx {
        (receiver_idx + 1) % num
    } else {
        receiver_idx
    };
    
    BenchTransferRequest {
        from: accounts[sender_idx].clone(),
        to: accounts[receiver_idx].clone(),
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

/// Response for balance query
#[derive(Debug, Deserialize)]
pub struct GetBalanceResponse {
    pub account: String,
    pub balance: u128,
    pub exists: bool,
}

impl BenchClient {
    /// Query account balance
    /// 
    /// Returns Some(balance) if the account exists, None otherwise.
    /// This can be used to check if a transfer has been applied to state.
    pub async fn get_balance(&self, account: &str) -> Option<u64> {
        let url = format!("{}/api/v1/state/balance/{}", self.base_url, account);
        
        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                match response.json::<GetBalanceResponse>().await {
                    Ok(resp) if resp.exists => Some(resp.balance as u64),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
