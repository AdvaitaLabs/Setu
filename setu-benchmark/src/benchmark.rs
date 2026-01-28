//! Benchmark runner implementation

use crate::client::{generate_transfer, generate_transfer_with_test_accounts, BenchClient};
use crate::config::{BenchmarkConfig, BenchmarkMode};
use crate::metrics::{BenchmarkSummary, MetricsCollector};
use anyhow::{bail, Result};
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::{interval, sleep};
use tracing::{error, info, warn};

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
        info!("═══════════════════════════════════════════════════════════");

        let result = match self.config.mode {
            BenchmarkMode::Burst => self.run_burst_mode(&client).await,
            BenchmarkMode::Sustained => self.run_sustained_mode(&client).await,
            BenchmarkMode::Ramp => self.run_ramp_mode(&client).await,
        }?;

        info!("═══════════════════════════════════════════════════════════");
        info!("Benchmark complete!");
        info!("");

        Ok(result)
    }

    /// Run warmup transactions
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
                    let request = if config.use_test_accounts {
                        generate_transfer_with_test_accounts(config.amount, i)
                    } else {
                        generate_transfer(
                            &config.sender_prefix,
                            &config.receiver_prefix,
                            config.amount,
                            i,
                        )
                    };
                    let _ = client.submit_transfer(request).await;
                }
            })
            .collect();

        stream::iter(tasks)
            .buffer_unordered(concurrency)
            .for_each(|_| async {})
            .await;

        Ok(())
    }

    /// Burst mode: send all transactions as fast as possible
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
                    let request = if config.use_test_accounts {
                        generate_transfer_with_test_accounts(config.amount, i)
                    } else {
                        generate_transfer(
                            &config.sender_prefix,
                            &config.receiver_prefix,
                            config.amount,
                            i,
                        )
                    };
                    let result = client.submit_transfer(request).await;
                    metrics.record(result).await;
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
                let request = if config.use_test_accounts {
                    generate_transfer_with_test_accounts(config.amount, current_seq)
                } else {
                    generate_transfer(
                        &config.sender_prefix,
                        &config.receiver_prefix,
                        config.amount,
                        current_seq,
                    )
                };
                let result = client.submit_transfer(request).await;
                metrics_clone.record(result).await;
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
                    let request = if config.use_test_accounts {
                        generate_transfer_with_test_accounts(config.amount, s)
                    } else {
                        generate_transfer(
                            &config.sender_prefix,
                            &config.receiver_prefix,
                            config.amount,
                            s,
                        )
                    };
                    let result = client.submit_transfer(request).await;
                    metrics_clone.record(result).await;
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
}
