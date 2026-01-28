//! Metrics collection and statistics

use hdrhistogram::Histogram;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Metrics for a single request
#[derive(Debug, Clone)]
pub struct RequestMetrics {
    pub latency: Duration,
    pub success: bool,
    pub timeout: bool,
    #[allow(dead_code)]
    pub error_message: Option<String>,
}

impl RequestMetrics {
    pub fn success(latency: Duration) -> Self {
        Self {
            latency,
            success: true,
            timeout: false,
            error_message: None,
        }
    }

    pub fn failure(latency: Duration, message: String) -> Self {
        Self {
            latency,
            success: false,
            timeout: false,
            error_message: Some(message),
        }
    }

    pub fn timeout(latency: Duration) -> Self {
        Self {
            latency,
            success: false,
            timeout: true,
            error_message: Some("Request timeout".to_string()),
        }
    }
}

/// Aggregated metrics collector
pub struct MetricsCollector {
    /// Total requests sent
    pub total_requests: AtomicU64,
    /// Successful requests
    pub success_count: AtomicU64,
    /// Failed requests
    pub failure_count: AtomicU64,
    /// Timeout requests
    pub timeout_count: AtomicU64,
    /// Latency histogram (microseconds)
    latency_histogram: RwLock<Histogram<u64>>,
    /// Start time (epoch millis)
    pub start_time_ms: AtomicU64,
    /// End time (epoch millis)
    pub end_time_ms: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            total_requests: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            timeout_count: AtomicU64::new(0),
            latency_histogram: RwLock::new(
                Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap()
            ),
            start_time_ms: AtomicU64::new(0),
            end_time_ms: AtomicU64::new(0),
        })
    }

    /// Mark benchmark start
    pub fn mark_start(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.start_time_ms.store(now, Ordering::SeqCst);
    }

    /// Mark benchmark end
    pub fn mark_end(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.end_time_ms.store(now, Ordering::SeqCst);
    }

    /// Record a request result
    pub async fn record(&self, metrics: RequestMetrics) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        if metrics.success {
            self.success_count.fetch_add(1, Ordering::Relaxed);
        } else if metrics.timeout {
            self.timeout_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failure_count.fetch_add(1, Ordering::Relaxed);
        }

        // Record latency in microseconds
        let latency_us = metrics.latency.as_micros() as u64;
        let mut hist = self.latency_histogram.write().await;
        let _ = hist.record(latency_us.min(60_000_000)); // Cap at 60s
    }

    /// Get total elapsed time in milliseconds
    /// During benchmark: returns time since start
    /// After benchmark: returns total duration
    pub fn elapsed_ms(&self) -> u64 {
        let start = self.start_time_ms.load(Ordering::SeqCst);
        if start == 0 {
            return 0; // Not started yet
        }
        
        let end = self.end_time_ms.load(Ordering::SeqCst);
        if end > start {
            // Benchmark completed
            end - start
        } else {
            // Benchmark in progress - use current time
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            now.saturating_sub(start)
        }
    }

    /// Calculate TPS
    pub fn tps(&self) -> f64 {
        let elapsed_ms = self.elapsed_ms();
        if elapsed_ms == 0 {
            return 0.0;
        }
        let success = self.success_count.load(Ordering::SeqCst) as f64;
        success / (elapsed_ms as f64 / 1000.0)
    }

    /// Get success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::SeqCst);
        if total == 0 {
            return 0.0;
        }
        let success = self.success_count.load(Ordering::SeqCst);
        (success as f64 / total as f64) * 100.0
    }

    /// Get latency percentiles
    pub async fn latency_percentiles(&self) -> LatencyStats {
        let hist = self.latency_histogram.read().await;
        LatencyStats {
            min_us: hist.min(),
            max_us: hist.max(),
            mean_us: hist.mean(),
            p50_us: hist.value_at_percentile(50.0),
            p90_us: hist.value_at_percentile(90.0),
            p95_us: hist.value_at_percentile(95.0),
            p99_us: hist.value_at_percentile(99.0),
            p999_us: hist.value_at_percentile(99.9),
        }
    }

    /// Generate summary
    pub async fn summary(&self) -> BenchmarkSummary {
        BenchmarkSummary {
            total_requests: self.total_requests.load(Ordering::SeqCst),
            success_count: self.success_count.load(Ordering::SeqCst),
            failure_count: self.failure_count.load(Ordering::SeqCst),
            timeout_count: self.timeout_count.load(Ordering::SeqCst),
            elapsed_ms: self.elapsed_ms(),
            tps: self.tps(),
            success_rate: self.success_rate(),
            latency: self.latency_percentiles().await,
        }
    }
}

/// Latency statistics
#[derive(Debug, Clone)]
pub struct LatencyStats {
    pub min_us: u64,
    pub max_us: u64,
    pub mean_us: f64,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
}

/// Complete benchmark summary
#[derive(Debug, Clone)]
pub struct BenchmarkSummary {
    pub total_requests: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub timeout_count: u64,
    pub elapsed_ms: u64,
    pub tps: f64,
    pub success_rate: f64,
    pub latency: LatencyStats,
}
