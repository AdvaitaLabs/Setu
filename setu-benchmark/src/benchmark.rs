//! Benchmark runner implementation

use crate::client::{generate_transfer, generate_transfer_with_n_accounts, BenchClient, BenchTransferRequest};
use crate::config::{BenchmarkConfig, BenchmarkMode};
use crate::metrics::{BenchmarkSummary, MetricsCollector};
use anyhow::{bail, Result};
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, warn};

/// Generate a batch of transfer requests
fn generate_transfer_batch(
    config: &BenchmarkConfig,
    start_seq: u64,
    batch_size: u64,
) -> Vec<BenchTransferRequest> {
    (0..batch_size)
        .map(|i| {
            let seq = start_seq + i;
            generate_single_transfer(config, seq)
        })
        .collect()
}

/// Generate a single transfer request based on config
fn generate_single_transfer(config: &BenchmarkConfig, seq: u64) -> BenchTransferRequest {
    if config.use_test_accounts {
        // Use init_accounts if specified, otherwise fall back to seed accounts
        let num_accounts = if config.init_accounts > 0 {
            Some(config.init_accounts)
        } else {
            None
        };
        generate_transfer_with_n_accounts(config.amount, seq, num_accounts)
    } else {
        generate_transfer(
            &config.sender_prefix,
            &config.receiver_prefix,
            config.amount,
            seq,
        )
    }
}

/// Benchmark runner
pub struct BenchmarkRunner {
    config: BenchmarkConfig,
}

impl BenchmarkRunner {
    pub fn new(config: BenchmarkConfig) -> Self {
        Self { config }
    }

    /// Run the benchmark
    pub async fn run(&self) -> Result<BenchmarkSummary> {
        // Create client
        let client = Arc::new(BenchClient::new(
            self.config.validator_url.clone(),
            self.config.timeout,
            self.config.keep_alive,
        )?);

        // Health check
        info!("Checking validator connectivity...");
        match client.health_check().await {
            Ok(true) => info!("✓ Validator is reachable"),
            Ok(false) => {
                warn!("⚠ Validator returned non-success status, continuing anyway...");
            }
            Err(e) => {
                error!("✗ Failed to connect to validator: {}", e);
                bail!("Cannot connect to validator at {}", self.config.validator_url);
            }
        }

        // Initialize test accounts if requested
        if self.config.init_accounts > 0 {
            info!("");
            info!("Initializing {} test accounts...", self.config.init_accounts);
            self.init_test_accounts(&client).await?;
            info!("✓ Test accounts initialized");
            sleep(Duration::from_millis(500)).await;
        }

        // Run warmup
        if self.config.warmup > 0 {
            info!("Running warmup ({} transactions)...", self.config.warmup);
            self.run_warmup(&client).await?;
            info!("✓ Warmup complete");
            sleep(Duration::from_millis(500)).await;
        }

        // Run actual benchmark
        info!("");
        info!("Starting benchmark...");
        if self.config.use_batch {
            info!("Mode: BATCH (batch_size={})", self.config.batch_size);
        }
        info!("═══════════════════════════════════════════════════════════");

        let result = if self.config.use_batch {
            // Batch mode
            match self.config.mode {
                BenchmarkMode::Burst => self.run_burst_batch_mode(&client).await,
                BenchmarkMode::Sustained => self.run_sustained_batch_mode(&client).await,
                BenchmarkMode::Ramp => self.run_ramp_batch_mode(&client).await,
            }
        } else {
            // Single request mode
            match self.config.mode {
                BenchmarkMode::Burst => self.run_burst_mode(&client).await,
                BenchmarkMode::Sustained => self.run_sustained_mode(&client).await,
                BenchmarkMode::Ramp => self.run_ramp_mode(&client).await,
            }
        }?;

        info!("═══════════════════════════════════════════════════════════");
        info!("Benchmark complete!");
        info!("");

        Ok(result)
    }

    /// Run warmup transactions (with retry for coin reservation conflicts)
    async fn run_warmup(&self, client: &Arc<BenchClient>) -> Result<()> {
        let warmup_count = self.config.warmup;
        let concurrency = (self.config.concurrency as usize).min(warmup_count as usize);
        let semaphore = Arc::new(Semaphore::new(concurrency));

        let tasks: Vec<_> = (0..warmup_count)
            .map(|i| {
                let client = client.clone();
                let sem = semaphore.clone();
                let config = self.config.clone();
                async move {
                    let _permit = sem.acquire().await.unwrap();
                    
                    // Retry logic for coin reservation conflicts during warmup
                    let max_retries = 20u32;
                    let base_delay_ms = 3u64;
                    let total_accounts = config.init_accounts.max(3) as u64;
                    
                    for attempt in 0..=max_retries {
                        // On retry, offset the seq to try a different sender
                        let effective_seq = if attempt == 0 {
                            i
                        } else {
                            i.wrapping_add(attempt as u64 * 7)
                        };
                        let request = generate_single_transfer(&config, effective_seq);
                        let result = client.submit_transfer(request).await;
                        
                        if result.success {
                            break;
                        }
                        
                        let is_reservation_error = result.error_message
                            .as_ref()
                            .map(|msg| msg.contains("reserved") || msg.contains("Reserved"))
                            .unwrap_or(false);
                        
                        if !is_reservation_error || attempt == max_retries {
                            break;
                        }
                        
                        let delay = (base_delay_ms * (attempt as u64 + 1)).min(30);
                        let jitter = (i % total_accounts) as u64 % 5;
                        tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
                    }
                }
            })
            .collect();

        stream::iter(tasks)
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        Ok(())
    }

    /// Initialize test accounts by transferring from seed accounts
    /// 
    /// This creates user_001, user_002, ... user_N by transferring funds from
    /// seed accounts (alice, bob, charlie). Each account receives multiple coin
    /// objects (controlled by `coins_per_account`) to support concurrent transactions.
    /// 
    /// ## Coin Pre-sharding
    /// 
    /// Instead of 1 transfer of 100000, we do N transfers of 100000/N each.
    /// Each transfer creates a new coin object via split, so the user ends up with
    /// N coin objects that can be reserved independently for parallel transfers.
    /// 
    /// Example (coins_per_account=5, balance=100000):
    ///   seed → user_001: 20000  →  coin_1 (20000)
    ///   seed → user_001: 20000  →  coin_2 (20000)
    ///   seed → user_001: 20000  →  coin_3 (20000)
    ///   seed → user_001: 20000  →  coin_4 (20000)
    ///   seed → user_001: 20000  →  coin_5 (20000)
    ///   → user_001 has 5 coins, supports 5 concurrent sends
    /// 
    /// Transfers are distributed across 3 seed accounts using round-robin.
    async fn init_test_accounts(&self, client: &Arc<BenchClient>) -> Result<()> {
        let num_accounts = self.config.init_accounts;
        let total_balance = self.config.init_account_balance;
        let coins_per_account = std::cmp::max(1, self.config.coins_per_account);
        
        // Each coin gets an equal share of the total balance
        let balance_per_coin = total_balance / coins_per_account;
        // Last coin gets the remainder to avoid rounding loss
        let last_coin_balance = total_balance - balance_per_coin * (coins_per_account - 1);
        
        // Seed accounts to transfer from
        const SEED_ACCOUNTS: &[&str] = &["alice", "bob", "charlie"];
        const MAX_RETRIES: u32 = 20;
        const RETRY_DELAY_MS: u64 = 200;
        const BATCH_WAIT_MS: u64 = 500;
        
        let total_transfers = num_accounts * coins_per_account;
        info!("  Creating {} test accounts with {} balance each ({} coins/account, {} per coin)...",
            num_accounts, total_balance, coins_per_account, balance_per_coin);
        info!("  Total init transfers: {} (using seed accounts: {:?})", total_transfers, SEED_ACCOUNTS);
        
        // Track success/failure
        let mut success_count = 0u64;
        let mut fail_count = 0u64;
        
        // Process in batches of 3 (one per seed account) for parallelism
        let mut i = 0u64;
        while i < num_accounts {
            let batch_size = std::cmp::min(SEED_ACCOUNTS.len() as u64, num_accounts - i);
            
            // For each account in this batch, send coins_per_account transfers
            for coin_idx in 0..coins_per_account {
                let mut handles = Vec::new();
                let amount = if coin_idx == coins_per_account - 1 {
                    last_coin_balance
                } else {
                    balance_per_coin
                };
                
                for j in 0..batch_size {
                    let account_idx = i + j;
                    let account_name = format!("user_{:03}", account_idx + 1);
                    let account_name_for_task = account_name.clone();
                    let seed_account = SEED_ACCOUNTS[j as usize].to_string();
                    let client = Arc::clone(client);
                    
                    let handle = tokio::spawn(async move {
                        Self::init_single_account_with_retry(
                            &client,
                            &seed_account,
                            &account_name_for_task,
                            amount,
                            MAX_RETRIES,
                            RETRY_DELAY_MS,
                        ).await
                    });
                    handles.push((account_name, handle));
                }
                
                // Wait for all parallel transfers to complete
                for (account_name, handle) in handles {
                    match handle.await {
                        Ok(true) => success_count += 1,
                        Ok(false) => {
                            fail_count += 1;
                            warn!("  Failed to init coin {} for {}", coin_idx + 1, account_name);
                        }
                        Err(e) => {
                            fail_count += 1;
                            warn!("  Task error for {} coin {}: {:?}", account_name, coin_idx + 1, e);
                        }
                    }
                }
                
                // Wait between coin rounds for TEE to complete and release reservations
                if coin_idx < coins_per_account - 1 {
                    tokio::time::sleep(Duration::from_millis(BATCH_WAIT_MS)).await;
                }
            }
            
            i += batch_size;
            
            // Wait between account batches
            if i < num_accounts {
                tokio::time::sleep(Duration::from_millis(BATCH_WAIT_MS)).await;
            }
            
            // Progress update
            let expected = i * coins_per_account;
            if i % 10 < batch_size || i >= num_accounts {
                info!("  Progress: {}/{} accounts ({}/{} transfers, {} success, {} failed)",
                    i, num_accounts, expected, total_transfers, success_count, fail_count);
            }
        }
        
        if fail_count > 0 {
            warn!("  ⚠ {} accounts failed to initialize", fail_count);
        }
        
        // Wait for consensus to apply state changes
        // Setu uses eventual consistency - state is only written after anchor creation
        // We poll for the first account's balance to confirm state has been applied
        info!("  Waiting for consensus to apply state changes...");
        let poll_account = "user_001";
        let max_wait_secs = 30;
        let poll_interval_ms = 500;
        let mut waited_ms = 0u64;
        
        loop {
            if let Some(balance) = client.get_balance(poll_account).await {
                info!("  ✓ State applied! {} balance: {}", poll_account, balance);
                break;
            }
            
            if waited_ms >= max_wait_secs * 1000 {
                warn!("  ⚠ Timeout waiting for state to be applied ({}s)", max_wait_secs);
                warn!("    This may indicate consensus is not running or vlc_delta_threshold not reached");
                break;
            }
            
            tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;
            waited_ms += poll_interval_ms;
            
            if waited_ms % 5000 == 0 {
                info!("  Still waiting for consensus... ({}s elapsed)", waited_ms / 1000);
            }
        }
        
        info!("  ✓ Account initialization complete: {} success, {} failed", success_count, fail_count);
        
        Ok(())
    }
    
    /// Initialize a single account with retry logic
    /// 
    /// Retries if the seed account's coin is currently reserved (1:1 coin model constraint)
    async fn init_single_account_with_retry(
        client: &BenchClient,
        seed_account: &str,
        target_account: &str,
        balance: u64,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> bool {
        for attempt in 0..=max_retries {
            let request = BenchTransferRequest {
                from: seed_account.to_string(),
                to: target_account.to_string(),
                amount: balance,
                transfer_type: "flux".to_string(),
                preferred_solver: None,
                shard_id: None,
                subnet_id: None,
                resources: vec![format!("init_{}", target_account)],
            };
            
            let result = client.submit_transfer(request).await;
            
            if result.success {
                debug!("  ✓ {} initialized via {} (attempt {})", target_account, seed_account, attempt + 1);
                return true;
            }
            
            // Check if it's a coin reservation error (retryable)
            let is_reservation_error = result.error_message
                .as_ref()
                .map(|msg| msg.contains("reserved") || msg.contains("Reserved"))
                .unwrap_or(false);
            
            if !is_reservation_error {
                // Non-retryable error
                warn!("  Non-retryable error for {}: {:?}", target_account, result.error_message);
                return false;
            }
            
            if attempt < max_retries {
                // Wait and retry - coin is reserved, TEE might still be executing
                let wait_ms = retry_delay_ms * (attempt as u64 + 1);
                debug!("  {} coin reserved, retry {}/{} in {}ms", seed_account, attempt + 1, max_retries, wait_ms);
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            }
        }
        
        warn!("  Failed to initialize {} after {} retries (coin reservation timeout)", target_account, max_retries);
        false
    }

    /// Burst mode: send all transactions as fast as possible
    /// 
    /// Includes retry logic for coin reservation conflicts:
    /// In 1:1 coin model, concurrent transfers from the same sender will
    /// conflict on coin reservation. We retry with backoff to handle this.
    async fn run_burst_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let total = self.config.total;
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        // Progress reporting
        let progress_counter = counter.clone();
        let progress_metrics = metrics.clone();
        let total_for_progress = total;
        let progress_handle = tokio::spawn(async move {
            let mut last_count = 0u64;
            let mut ticker = interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let current = progress_counter.load(Ordering::Relaxed);
                let success = progress_metrics.success_count.load(Ordering::Relaxed);
                let tps = current - last_count;
                last_count = current;
                
                if current >= total_for_progress {
                    break;
                }
                
                info!(
                    "Progress: {}/{} ({:.1}%) | TPS: {} | Success: {}",
                    current,
                    total_for_progress,
                    (current as f64 / total_for_progress as f64) * 100.0,
                    tps,
                    success
                );
            }
        });

        metrics.mark_start();

        let tasks: Vec<_> = (0..total)
            .map(|i| {
                let client = client.clone();
                let sem = semaphore.clone();
                let metrics = metrics.clone();
                let config = self.config.clone();
                let counter = counter.clone();
                async move {
                    let _permit = sem.acquire().await.unwrap();
                    
                    // Retry logic for coin reservation conflicts
                    // With 50 concurrency and 100 accounts, expect ~50% collision rate
                    // TEE execution takes ~5-30ms, so we need retries spanning that window
                    // On retry, we use a different seq to select a different sender
                    let max_retries = 20u32;
                    let base_delay_ms = 3u64;
                    let mut last_result = None;
                    let total_accounts = config.init_accounts.max(3) as u64;
                    
                    for attempt in 0..=max_retries {
                        // On retry, offset the seq to try a different sender
                        let effective_seq = if attempt == 0 {
                            i
                        } else {
                            i.wrapping_add(attempt as u64 * 7) // prime multiplier for spread
                        };
                        let request = generate_single_transfer(&config, effective_seq);
                        let result = client.submit_transfer(request).await;
                        
                        if result.success {
                            last_result = Some(result);
                            break;
                        }
                        
                        // Check if it's a coin reservation error (retryable)
                        let is_reservation_error = result.error_message
                            .as_ref()
                            .map(|msg| msg.contains("reserved") || msg.contains("Reserved"))
                            .unwrap_or(false);
                        
                        if !is_reservation_error || attempt == max_retries {
                            // Non-retryable error or exhausted retries
                            last_result = Some(result);
                            break;
                        }
                        
                        // Short delay with jitter (3ms, 6ms, 9ms, ... capped at 30ms)
                        let delay = (base_delay_ms * (attempt as u64 + 1)).min(30);
                        let jitter = (i % total_accounts) as u64 % 5; // spread jitter
                        tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
                    }
                    
                    if let Some(result) = last_result {
                        metrics.record(result).await;
                    }
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
            .collect();

        stream::iter(tasks)
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        metrics.mark_end();
        progress_handle.abort();

        Ok(metrics.summary().await)
    }

    /// Sustained mode: maintain target TPS for duration
    async fn run_sustained_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let target_tps = self.config.target_tps;
        let duration = Duration::from_secs(self.config.duration);
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        // Calculate interval between requests to achieve target TPS
        let interval_us = if target_tps > 0 {
            1_000_000 / target_tps
        } else {
            1000
        };

        info!("Target TPS: {}, interval: {}μs", target_tps, interval_us);

        metrics.mark_start();
        let start = Instant::now();

        // Progress reporting task
        let progress_counter = counter.clone();
        let progress_metrics = metrics.clone();
        let progress_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                let current = progress_counter.load(Ordering::Relaxed);
                let success = progress_metrics.success_count.load(Ordering::Relaxed);
                let failures = progress_metrics.failure_count.load(Ordering::Relaxed);
                let elapsed = progress_metrics.elapsed_ms();
                let actual_tps = if elapsed > 0 {
                    (success as f64 / (elapsed as f64 / 1000.0)) as u64
                } else {
                    0
                };
                
                info!(
                    "Elapsed: {:.1}s | Sent: {} | Success: {} | Fail: {} | Actual TPS: {}",
                    elapsed as f64 / 1000.0,
                    current,
                    success,
                    failures,
                    actual_tps
                );
            }
        });

        // Request dispatcher
        let mut seq = 0u64;
        let mut request_interval = tokio::time::interval(Duration::from_micros(interval_us));

        while start.elapsed() < duration {
            request_interval.tick().await;

            // Capture seq BEFORE incrementing to ensure unique values per spawn
            let current_seq = seq;
            seq += 1;

            let client = client.clone();
            let sem = semaphore.clone();
            let metrics_clone = metrics.clone();
            let config = self.config.clone();
            let counter_clone = counter.clone();

            tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                
                // Retry logic for coin reservation conflicts (same as burst mode)
                let max_retries = 20u32;
                let base_delay_ms = 3u64;
                let total_accounts = config.init_accounts.max(3) as u64;
                let mut last_result = None;
                
                for attempt in 0..=max_retries {
                    let effective_seq = if attempt == 0 {
                        current_seq
                    } else {
                        current_seq.wrapping_add(attempt as u64 * 7)
                    };
                    let request = generate_single_transfer(&config, effective_seq);
                    let result = client.submit_transfer(request).await;
                    
                    if result.success {
                        last_result = Some(result);
                        break;
                    }
                    
                    let is_reservation_error = result.error_message
                        .as_ref()
                        .map(|msg| msg.contains("reserved") || msg.contains("Reserved"))
                        .unwrap_or(false);
                    
                    if !is_reservation_error || attempt == max_retries {
                        last_result = Some(result);
                        break;
                    }
                    
                    let delay = (base_delay_ms * (attempt as u64 + 1)).min(30);
                    let jitter = (current_seq % total_accounts) as u64 % 5;
                    tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
                }
                
                if let Some(result) = last_result {
                    metrics_clone.record(result).await;
                }
                counter_clone.fetch_add(1, Ordering::Relaxed);
            });
        }

        // Wait for in-flight requests
        sleep(Duration::from_secs(2)).await;

        metrics.mark_end();
        progress_handle.abort();

        Ok(metrics.summary().await)
    }

    /// Ramp mode: gradually increase load
    async fn run_ramp_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let mut current_tps = self.config.ramp_start;
        let step = self.config.ramp_step;
        let step_duration = Duration::from_secs(self.config.ramp_step_duration);
        let total_duration = Duration::from_secs(self.config.duration);
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        metrics.mark_start();
        let start = Instant::now();
        let mut seq = 0u64;

        while start.elapsed() < total_duration {
            let step_start = Instant::now();
            info!("Ramp step: {} TPS", current_tps);

            let interval_us = if current_tps > 0 {
                1_000_000 / current_tps
            } else {
                1_000_000
            };
            let mut request_interval = tokio::time::interval(Duration::from_micros(interval_us));

            while step_start.elapsed() < step_duration && start.elapsed() < total_duration {
                request_interval.tick().await;

                let client = client.clone();
                let sem = semaphore.clone();
                let metrics_clone = metrics.clone();
                let config = self.config.clone();
                let counter_clone = counter.clone();
                let s = seq;

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    
                    // Retry logic for coin reservation conflicts
                    let max_retries = 20u32;
                    let base_delay_ms = 3u64;
                    let total_accounts = config.init_accounts.max(3) as u64;
                    let mut last_result = None;
                    
                    for attempt in 0..=max_retries {
                        let effective_seq = if attempt == 0 {
                            s
                        } else {
                            s.wrapping_add(attempt as u64 * 7)
                        };
                        let request = generate_single_transfer(&config, effective_seq);
                        let result = client.submit_transfer(request).await;
                        
                        if result.success {
                            last_result = Some(result);
                            break;
                        }
                        
                        let is_reservation_error = result.error_message
                            .as_ref()
                            .map(|msg| msg.contains("reserved") || msg.contains("Reserved"))
                            .unwrap_or(false);
                        
                        if !is_reservation_error || attempt == max_retries {
                            last_result = Some(result);
                            break;
                        }
                        
                        let delay = (base_delay_ms * (attempt as u64 + 1)).min(30);
                        let jitter = (s % total_accounts) as u64 % 5;
                        tokio::time::sleep(Duration::from_millis(delay + jitter)).await;
                    }
                    
                    if let Some(result) = last_result {
                        metrics_clone.record(result).await;
                    }
                    counter_clone.fetch_add(1, Ordering::Relaxed);
                });

                seq += 1;
            }

            // Log step results
            let success = metrics.success_count.load(Ordering::Relaxed);
            let failures = metrics.failure_count.load(Ordering::Relaxed);
            info!(
                "Step complete: {} TPS target | {} success | {} failures",
                current_tps, success, failures
            );

            current_tps += step;
        }

        // Wait for in-flight requests
        sleep(Duration::from_secs(2)).await;

        metrics.mark_end();

        Ok(metrics.summary().await)
    }

    // =========================================================================
    // Batch Mode Implementations
    // =========================================================================

    /// Burst batch mode: send all transactions in batches as fast as possible
    async fn run_burst_batch_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let total = self.config.total;
        let batch_size = self.config.batch_size;
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        // Calculate number of batches
        let num_batches = (total + batch_size - 1) / batch_size;
        info!(
            "Burst batch mode: {} total requests in {} batches of {} (concurrency: {})",
            total, num_batches, batch_size, concurrency
        );

        // Progress reporting
        let progress_counter = counter.clone();
        let progress_metrics = metrics.clone();
        let total_for_progress = total;
        let progress_handle = tokio::spawn(async move {
            let mut last_count = 0u64;
            let mut ticker = interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let current = progress_counter.load(Ordering::Relaxed);
                let success = progress_metrics.success_count.load(Ordering::Relaxed);
                let tps = current - last_count;
                last_count = current;

                if current >= total_for_progress {
                    break;
                }

                info!(
                    "Progress: {}/{} ({:.1}%) | TPS: {} | Success: {}",
                    current,
                    total_for_progress,
                    (current as f64 / total_for_progress as f64) * 100.0,
                    tps,
                    success
                );
            }
        });

        metrics.mark_start();

        let tasks: Vec<_> = (0..num_batches)
            .map(|batch_idx| {
                let client = client.clone();
                let sem = semaphore.clone();
                let metrics = metrics.clone();
                let config = self.config.clone();
                let counter = counter.clone();
                let start_seq = batch_idx * batch_size;
                // Handle last batch which may be smaller
                let actual_batch_size = (total - start_seq).min(batch_size);

                async move {
                    let _permit = sem.acquire().await.unwrap();
                    let requests = generate_transfer_batch(&config, start_seq, actual_batch_size);
                    let results = client.submit_transfers_batch(requests).await;
                    for result in results {
                        metrics.record(result).await;
                    }
                    counter.fetch_add(actual_batch_size, Ordering::Relaxed);
                }
            })
            .collect();

        stream::iter(tasks)
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        metrics.mark_end();
        progress_handle.abort();

        Ok(metrics.summary().await)
    }

    /// Sustained batch mode: maintain target TPS using batches
    async fn run_sustained_batch_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let target_tps = self.config.target_tps;
        let batch_size = self.config.batch_size;
        let duration = Duration::from_secs(self.config.duration);
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        // Calculate batch interval to achieve target TPS
        // If target is 1000 TPS and batch_size is 50, we need 20 batches/sec
        let batches_per_second = if batch_size > 0 {
            (target_tps + batch_size - 1) / batch_size
        } else {
            target_tps
        };
        let interval_us = if batches_per_second > 0 {
            1_000_000 / batches_per_second
        } else {
            1_000_000
        };

        info!(
            "Target TPS: {}, batch size: {}, batches/sec: {}, interval: {}μs",
            target_tps, batch_size, batches_per_second, interval_us
        );

        metrics.mark_start();
        let start = Instant::now();

        // Progress reporting task
        let progress_counter = counter.clone();
        let progress_metrics = metrics.clone();
        let progress_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                let current = progress_counter.load(Ordering::Relaxed);
                let success = progress_metrics.success_count.load(Ordering::Relaxed);
                let failures = progress_metrics.failure_count.load(Ordering::Relaxed);
                let elapsed = progress_metrics.elapsed_ms();
                let actual_tps = if elapsed > 0 {
                    (success as f64 / (elapsed as f64 / 1000.0)) as u64
                } else {
                    0
                };

                info!(
                    "Elapsed: {:.1}s | Sent: {} | Success: {} | Fail: {} | Actual TPS: {}",
                    elapsed as f64 / 1000.0,
                    current,
                    success,
                    failures,
                    actual_tps
                );
            }
        });

        // Batch dispatcher
        let mut seq = 0u64;
        let mut request_interval = tokio::time::interval(Duration::from_micros(interval_us));

        while start.elapsed() < duration {
            request_interval.tick().await;

            let current_seq = seq;
            seq += batch_size;

            let client = client.clone();
            let sem = semaphore.clone();
            let metrics_clone = metrics.clone();
            let config = self.config.clone();
            let counter_clone = counter.clone();
            let bs = batch_size;

            tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let requests = generate_transfer_batch(&config, current_seq, bs);
                let results = client.submit_transfers_batch(requests).await;
                for result in results {
                    metrics_clone.record(result).await;
                }
                counter_clone.fetch_add(bs, Ordering::Relaxed);
            });
        }

        // Wait for in-flight requests
        sleep(Duration::from_secs(2)).await;

        metrics.mark_end();
        progress_handle.abort();

        Ok(metrics.summary().await)
    }

    /// Ramp batch mode: gradually increase load using batches
    async fn run_ramp_batch_mode(&self, client: &Arc<BenchClient>) -> Result<BenchmarkSummary> {
        let mut current_tps = self.config.ramp_start;
        let step = self.config.ramp_step;
        let batch_size = self.config.batch_size;
        let step_duration = Duration::from_secs(self.config.ramp_step_duration);
        let total_duration = Duration::from_secs(self.config.duration);
        let concurrency = self.config.concurrency as usize;
        let metrics = MetricsCollector::new();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let counter = Arc::new(AtomicU64::new(0));

        metrics.mark_start();
        let start = Instant::now();
        let mut seq = 0u64;

        while start.elapsed() < total_duration {
            let step_start = Instant::now();

            // Calculate batches per second for current TPS target
            let batches_per_second = if batch_size > 0 {
                (current_tps + batch_size - 1) / batch_size
            } else {
                current_tps
            };
            let interval_us = if batches_per_second > 0 {
                1_000_000 / batches_per_second
            } else {
                1_000_000
            };

            info!(
                "Ramp step: {} TPS (batch_size={}, batches/sec={})",
                current_tps, batch_size, batches_per_second
            );

            let mut request_interval = tokio::time::interval(Duration::from_micros(interval_us));

            while step_start.elapsed() < step_duration && start.elapsed() < total_duration {
                request_interval.tick().await;

                let client = client.clone();
                let sem = semaphore.clone();
                let metrics_clone = metrics.clone();
                let config = self.config.clone();
                let counter_clone = counter.clone();
                let current_seq = seq;
                let bs = batch_size;

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    let requests = generate_transfer_batch(&config, current_seq, bs);
                    let results = client.submit_transfers_batch(requests).await;
                    for result in results {
                        metrics_clone.record(result).await;
                    }
                    counter_clone.fetch_add(bs, Ordering::Relaxed);
                });

                seq += batch_size;
            }

            // Log step results
            let success = metrics.success_count.load(Ordering::Relaxed);
            let failures = metrics.failure_count.load(Ordering::Relaxed);
            info!(
                "Step complete: {} TPS target | {} success | {} failures",
                current_tps, success, failures
            );

            current_tps += step;
        }

        // Wait for in-flight requests
        sleep(Duration::from_secs(2)).await;

        metrics.mark_end();

        Ok(metrics.summary().await)
    }
}
