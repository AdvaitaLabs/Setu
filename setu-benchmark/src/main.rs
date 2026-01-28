//! Setu TPS Benchmark Tool
//!
//! A comprehensive benchmark tool to measure the TPS (Transactions Per Second)
//! of the Setu network.
//!
//! # Usage
//!
//! ```bash
//! # Run with default settings (100 transactions, 10 concurrent)
//! setu-benchmark --validator-url http://127.0.0.1:8080
//!
//! # Run with custom parameters
//! setu-benchmark --validator-url http://127.0.0.1:8080 \
//!     --total 1000 \
//!     --concurrency 50 \
//!     --duration 60
//!
//! # Run sustained load test
//! setu-benchmark --validator-url http://127.0.0.1:8080 \
//!     --mode sustained \
//!     --duration 300 \
//!     --target-tps 1000
//! ```

mod benchmark;
mod client;
mod config;
mod metrics;
mod report;

use clap::Parser;
use config::BenchmarkConfig;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    // Parse command line arguments
    let config = BenchmarkConfig::parse();

    info!("╔══════════════════════════════════════════════════════════╗");
    info!("║            Setu TPS Benchmark Tool v0.1.0                ║");
    info!("╚══════════════════════════════════════════════════════════╝");
    info!("");

    // Print configuration
    config.print_config();

    // Run benchmark
    let runner = benchmark::BenchmarkRunner::new(config);
    let result = runner.run().await?;

    // Print report
    report::print_report(&result);

    Ok(())
}
