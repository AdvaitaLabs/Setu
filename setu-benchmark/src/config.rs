//! Benchmark configuration

use clap::{Parser, ValueEnum};
use tracing::info;

/// Benchmark mode
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum BenchmarkMode {
    /// Burst mode: send all transactions as fast as possible
    #[default]
    Burst,
    /// Sustained mode: maintain a target TPS for a duration
    Sustained,
    /// Ramp mode: gradually increase load to find max TPS
    Ramp,
}

/// Setu TPS Benchmark Configuration
#[derive(Parser, Debug, Clone)]
#[command(name = "setu-benchmark")]
#[command(about = "TPS Benchmark tool for Setu network", long_about = None)]
pub struct BenchmarkConfig {
    /// Validator HTTP API URL
    #[arg(short = 'u', long, default_value = "http://127.0.0.1:8080")]
    pub validator_url: String,

    /// Benchmark mode
    #[arg(short = 'm', long, value_enum, default_value = "burst")]
    pub mode: BenchmarkMode,

    /// Total number of transactions to send (burst mode)
    #[arg(short = 't', long, default_value = "1000")]
    pub total: u64,

    /// Number of concurrent workers
    #[arg(short = 'c', long, default_value = "10")]
    pub concurrency: u64,

    /// Duration in seconds (sustained/ramp mode)
    #[arg(short = 'd', long, default_value = "60")]
    pub duration: u64,

    /// Target TPS (sustained mode)
    #[arg(long, default_value = "100")]
    pub target_tps: u64,

    /// Starting TPS for ramp mode
    #[arg(long, default_value = "10")]
    pub ramp_start: u64,

    /// TPS increment per step in ramp mode
    #[arg(long, default_value = "10")]
    pub ramp_step: u64,

    /// Duration of each step in ramp mode (seconds)
    #[arg(long, default_value = "10")]
    pub ramp_step_duration: u64,

    /// Number of warmup transactions before measurement
    #[arg(long, default_value = "100")]
    pub warmup: u64,

    /// Enable detailed per-request logging
    #[arg(long, default_value = "false")]
    pub verbose: bool,

    /// Output format (text, json, csv)
    #[arg(long, default_value = "text")]
    pub output: String,

    /// Sender address prefix (will be randomized)
    #[arg(long, default_value = "bench_sender")]
    pub sender_prefix: String,

    /// Receiver address prefix (will be randomized)
    #[arg(long, default_value = "bench_receiver")]
    pub receiver_prefix: String,

    /// Transfer amount per transaction
    #[arg(long, default_value = "100")]
    pub amount: u64,

    /// HTTP request timeout in seconds
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Enable keep-alive connections
    #[arg(long, default_value = "true")]
    pub keep_alive: bool,

    /// Use pre-initialized test accounts (alice, bob, charlie) instead of random addresses
    #[arg(long, default_value = "false")]
    pub use_test_accounts: bool,
}

impl BenchmarkConfig {
    pub fn print_config(&self) {
        info!("Configuration:");
        info!("  Validator URL:    {}", self.validator_url);
        info!("  Mode:             {:?}", self.mode);
        info!("  Concurrency:      {}", self.concurrency);
        
        match self.mode {
            BenchmarkMode::Burst => {
                info!("  Total Txns:       {}", self.total);
            }
            BenchmarkMode::Sustained => {
                info!("  Duration:         {}s", self.duration);
                info!("  Target TPS:       {}", self.target_tps);
            }
            BenchmarkMode::Ramp => {
                info!("  Duration:         {}s", self.duration);
                info!("  Ramp Start:       {} TPS", self.ramp_start);
                info!("  Ramp Step:        {} TPS", self.ramp_step);
                info!("  Step Duration:    {}s", self.ramp_step_duration);
            }
        }
        
        info!("  Warmup Txns:      {}", self.warmup);
        info!("  Timeout:          {}s", self.timeout);
        info!("");
    }
}
