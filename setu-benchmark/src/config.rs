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
    /// Validator HTTP API URL (single validator mode)
    #[arg(short = 'u', long, default_value = "http://127.0.0.1:8080")]
    pub validator_url: String,

    /// Comma-separated list of validator URLs for multi-validator load distribution.
    /// When set, transactions are distributed round-robin across these validators.
    /// Overrides --validator-url.
    #[arg(long, default_value = "")]
    pub validator_urls: String,

    /// Comma-separated list of subnet IDs to assign to transactions.
    /// Transactions cycle through these subnets in round-robin fashion.
    /// Example: "subnet-app-1,subnet-app-2,subnet-app-3"
    #[arg(long, default_value = "")]
    pub subnets: String,

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

    /// Number of test accounts to initialize before benchmark
    /// 
    /// When > 0, transfers funds from seed accounts (alice, bob, charlie) to
    /// create user_001, user_002, ... user_N test accounts.
    /// Each test account receives `init_account_balance` tokens.
    /// 
    /// Seed accounts are pre-sharded at genesis with multiple coins
    /// (default 5 per genesis.json `coins_per_account`), so up to
    /// 3 seeds × 5 coins = 15 accounts can be initialized in parallel
    /// per round. More seed coins → faster init.
    /// 
    /// This enables high-concurrency testing without requiring Validator
    /// to pre-initialize all test accounts.
    #[arg(long, default_value = "0")]
    pub init_accounts: u64,

    /// Balance to transfer to each initialized test account
    #[arg(long, default_value = "100000")]
    pub init_account_balance: u64,

    /// Number of coin objects to create per account (for concurrency)
    /// 
    /// Setu uses a multi-coin object model where each account can own
    /// multiple coin objects of the same type. Each coin can be reserved
    /// independently, enabling parallel transfers from the same sender.
    /// 
    /// During account initialization, the balance is split into N coin
    /// objects. More coins = higher per-account concurrency.
    /// 
    /// Recommended: set to concurrency / init_accounts * 2 or higher.
    #[arg(long, default_value = "5")]
    pub coins_per_account: u64,

    /// Path to genesis.json file (used to read seed account addresses)
    #[arg(long, default_value = "genesis.json")]
    pub genesis_file: String,

    /// Use batch API instead of single transfer API
    #[arg(long, default_value = "false")]
    pub use_batch: bool,

    /// Batch size (number of transfers per batch request)
    #[arg(long, default_value = "50")]
    pub batch_size: u64,
}

impl BenchmarkConfig {
    /// Parse validator URLs: if --validator-urls is set, split by comma;
    /// otherwise use the single --validator-url.
    pub fn get_validator_urls(&self) -> Vec<String> {
        if !self.validator_urls.is_empty() {
            self.validator_urls.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![self.validator_url.clone()]
        }
    }

    /// Parse subnet IDs from --subnets option.
    pub fn get_subnet_ids(&self) -> Vec<String> {
        if !self.subnets.is_empty() {
            self.subnets.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![]
        }
    }

    pub fn print_config(&self) {
        info!("Configuration:");
        let urls = self.get_validator_urls();
        if urls.len() > 1 {
            info!("  Validator URLs:   {} validators", urls.len());
            for (i, url) in urls.iter().enumerate() {
                info!("    [{}] {}", i + 1, url);
            }
        } else {
            info!("  Validator URL:    {}", self.validator_url);
        }
        let subnet_ids = self.get_subnet_ids();
        if !subnet_ids.is_empty() {
            info!("  Subnets:          {:?}", subnet_ids);
        }
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
        if self.init_accounts > 0 {
            info!("  Init Accounts:    {} (balance: {} each, {} coins/account)", self.init_accounts, self.init_account_balance, self.coins_per_account);
        }
        if self.use_batch {
            info!("  Batch Mode:       ENABLED");
            info!("  Batch Size:       {}", self.batch_size);
        } else {
            info!("  Batch Mode:       disabled (single transfer API)");
        }
        info!("");
    }
}
