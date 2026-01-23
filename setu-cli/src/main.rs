//! Setu CLI - Command-line tool for managing Setu nodes
//!
//! Usage:
//!   setu validator register --id validator-1 --address 127.0.0.1 --port 9000
//!   setu solver register --id solver-1 --address 127.0.0.1 --port 8001 --shard shard-1
//!   setu router status
//!   setu transfer submit --from alice --to bob --amount 1000

mod commands;
mod config;
mod client;

use clap::{Parser, Subcommand};
use tracing::Level;

#[derive(Parser)]
#[command(name = "setu")]
#[command(about = "Setu CLI - Manage Setu nodes and submit transfers", long_about = None)]
#[command(version)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
    
    /// Config file path
    #[arg(short, long, global = true)]
    config: Option<String>,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validator node management
    Validator {
        #[command(subcommand)]
        action: ValidatorAction,
    },
    
    /// Solver node management
    Solver {
        #[command(subcommand)]
        action: SolverAction,
    },
    
    /// Router management
    Router {
        #[command(subcommand)]
        action: RouterAction,
    },
    
    /// Transfer operations
    Transfer {
        #[command(subcommand)]
        action: TransferAction,
    },
    
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ValidatorAction {
    /// Generate a new validator keypair
    Keygen {
        /// Output file path for the keypair
        #[arg(long, default_value = "validator-key.json")]
        output: String,
        
        /// Validator ID (optional, will be derived from address if not provided)
        #[arg(long)]
        id: Option<String>,
        
        /// Stake amount in Flux
        #[arg(long, default_value = "10000")]
        stake: u64,
        
        /// Commission rate (0-100)
        #[arg(long, default_value = "10")]
        commission: u8,
    },
    
    /// Export private key from keypair file
    ExportKey {
        /// Keypair file path
        #[arg(long)]
        key_file: String,
        
        /// Export format: json, mnemonic, or private-key
        #[arg(long, default_value = "json")]
        format: String,
    },
    
    /// Recover keypair from mnemonic phrase
    Recover {
        /// Mnemonic phrase (12 or 24 words)
        #[arg(long)]
        mnemonic: String,
        
        /// Output file path
        #[arg(long, default_value = "validator-key-recovered.json")]
        output: String,
        
        /// Validator ID
        #[arg(long)]
        id: Option<String>,
    },
    
    /// Register a validator node
    Register {
        /// Keypair file path
        #[arg(long)]
        key_file: String,
        
        /// Network address (IP or hostname)
        #[arg(long)]
        network_address: String,
        
        /// Network port
        #[arg(long)]
        network_port: u16,
        
        /// Router address to register with
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
    
    /// Get validator status
    Status {
        /// Validator ID
        #[arg(long)]
        id: String,
    },
    
    /// List all validators
    List {
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
}

#[derive(Subcommand)]
enum SolverAction {
    /// Generate a new solver keypair
    Keygen {
        /// Output file path for the keypair
        #[arg(long, default_value = "solver-key.json")]
        output: String,
        
        /// Solver ID (optional, will be derived from address if not provided)
        #[arg(long)]
        id: Option<String>,
    },
    
    /// Export private key from keypair file
    ExportKey {
        /// Keypair file path
        #[arg(long)]
        key_file: String,
        
        /// Export format: json, mnemonic, or private-key
        #[arg(long, default_value = "json")]
        format: String,
    },
    
    /// Recover keypair from mnemonic phrase
    Recover {
        /// Mnemonic phrase (12 or 24 words)
        #[arg(long)]
        mnemonic: String,
        
        /// Output file path
        #[arg(long, default_value = "solver-key-recovered.json")]
        output: String,
        
        /// Solver ID
        #[arg(long)]
        id: Option<String>,
    },
    
    /// Register a solver node
    Register {
        /// Keypair file path
        #[arg(long)]
        key_file: String,
        
        /// Network address (IP or hostname)
        #[arg(long)]
        network_address: String,
        
        /// Network port
        #[arg(long)]
        network_port: u16,
        
        /// Solver capacity (transfers per second)
        #[arg(long, default_value = "100")]
        capacity: u32,
        
        /// Shard ID
        #[arg(long)]
        shard: Option<String>,
        
        /// Resources this solver handles (comma-separated)
        #[arg(long, value_delimiter = ',')]
        resources: Vec<String>,
        
        /// Router address to register with
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
    
    /// Get solver status
    Status {
        /// Solver ID
        #[arg(long)]
        id: String,
    },
    
    /// List all solvers
    List {
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
    
    /// Send heartbeat
    Heartbeat {
        /// Solver ID
        #[arg(long)]
        id: String,
        
        /// Current load
        #[arg(long)]
        load: u32,
        
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
}

#[derive(Subcommand)]
enum RouterAction {
    /// Get router status
    Status {
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        address: String,
    },
    
    /// List registered solvers
    Solvers {
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        address: String,
    },
}

#[derive(Subcommand)]
enum TransferAction {
    /// Submit a transfer
    Submit {
        /// Sender address
        #[arg(long)]
        from: String,
        
        /// Recipient address
        #[arg(long)]
        to: String,
        
        /// Transfer amount
        #[arg(long)]
        amount: i128,
        
        /// Transfer type (flux/power/task)
        #[arg(long, default_value = "flux")]
        transfer_type: String,
        
        /// Preferred solver (optional)
        #[arg(long)]
        solver: Option<String>,
        
        /// Shard ID (optional)
        #[arg(long)]
        shard: Option<String>,
        
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
    
    /// Query transfer status
    Status {
        /// Transfer ID
        #[arg(long)]
        id: String,
        
        /// Router address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Initialize configuration
    Init {
        /// Config file path
        #[arg(long)]
        path: Option<String>,
    },
    
    /// Show current configuration
    Show,
    
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        
        /// Configuration value
        value: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    // Initialize logging
    let log_level = if cli.verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();
    
    // Load config
    let config = if let Some(config_path) = &cli.config {
        config::Config::load(config_path)?
    } else {
        config::Config::default()
    };
    
    // Execute command
    match cli.command {
        Commands::Validator { action } => {
            commands::validator::handle(action, &config).await?;
        }
        Commands::Solver { action } => {
            commands::solver::handle(action, &config).await?;
        }
        Commands::Router { action } => {
            commands::router::handle(action, &config).await?;
        }
        Commands::Transfer { action } => {
            commands::transfer::handle(action, &config).await?;
        }
        Commands::Config { action } => {
            commands::config::handle(action).await?;
        }
    }
    
    Ok(())
}

