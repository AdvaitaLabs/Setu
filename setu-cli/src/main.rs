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
    
    /// Subnet management
    Subnet {
        #[command(subcommand)]
        action: SubnetAction,
    },
    
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate, recover, inspect, and export keys
    #[command(name = "gen-key")]
    GenKey {
        #[command(subcommand)]
        action: GenKeyAction,
    },

    /// User profile and subnet membership
    User {
        #[command(subcommand)]
        action: UserAction,
    },
}

#[derive(Subcommand)]
pub enum GenKeyAction {
    /// Generate a new keypair (random mnemonic)
    Generate {
        /// Signature scheme: ed25519, secp256k1, secp256r1
        #[arg(long, short, default_value = "ed25519")]
        scheme: String,

        /// Mnemonic word count: 12, 15, 18, 21, 24
        #[arg(long, short, default_value_t = 12)]
        words: u8,

        /// Output key file path (base64-encoded keypair)
        #[arg(long, short)]
        output: Option<String>,

        /// Print output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Recover keypair from a BIP-39 mnemonic
    Recover {
        /// BIP-39 mnemonic phrase
        #[arg(long, short)]
        mnemonic: String,

        /// Signature scheme
        #[arg(long, short, default_value = "ed25519")]
        scheme: String,

        /// Output key file path
        #[arg(long, short)]
        output: Option<String>,

        /// Print output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect an existing key file (public info only)
    Inspect {
        /// Key file path
        #[arg(long, short = 'f')]
        file: String,
    },

    /// Export key material from a key file
    Export {
        /// Key file path
        #[arg(long, short = 'f')]
        file: String,

        /// Format: base64, hex, public
        #[arg(long, default_value = "base64")]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum UserAction {
    /// Profile management
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Subnet membership management
    Subnet {
        #[command(subcommand)]
        action: UserSubnetAction,
    },
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// Update user profile
    Update {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Display name
        #[arg(long)]
        display_name: Option<String>,
        /// Avatar URL
        #[arg(long)]
        avatar_url: Option<String>,
        /// Bio text
        #[arg(long)]
        bio: Option<String>,
        /// Key file for signing
        #[arg(long)]
        key_file: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
    },
    /// Get user profile
    Get {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
    },
}

#[derive(Subcommand)]
pub enum UserSubnetAction {
    /// Join a subnet
    Join {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Subnet ID to join
        #[arg(long)]
        subnet_id: String,
        /// Key file for signing
        #[arg(long)]
        key_file: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
    },
    /// Leave a subnet
    Leave {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Subnet ID to leave
        #[arg(long)]
        subnet_id: String,
        /// Key file for signing
        #[arg(long)]
        key_file: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
    },
    /// Check membership in a subnet
    Check {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Subnet ID to check
        #[arg(long)]
        subnet_id: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
    },
    /// List user's subnet memberships
    List {
        /// User address (0x...)
        #[arg(long)]
        address: String,
        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        validator: String,
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
        #[arg(long, default_value = "setu")]
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
pub enum SubnetAction {
    /// Register a new subnet
    Register {
        /// Unique subnet identifier
        #[arg(long)]
        subnet_id: String,

        /// Human-readable name
        #[arg(long)]
        name: String,

        /// Owner address (0x...)
        #[arg(long)]
        owner: String,

        /// Token symbol (required, e.g., "GAME")
        #[arg(long)]
        token_symbol: String,

        /// Subnet description
        #[arg(long)]
        description: Option<String>,

        /// Subnet type: app, organization, personal
        #[arg(long)]
        subnet_type: Option<String>,

        /// Parent subnet ID
        #[arg(long)]
        parent: Option<String>,

        /// Maximum number of users
        #[arg(long)]
        max_users: Option<u64>,

        /// Maximum TPS
        #[arg(long)]
        max_tps: Option<u64>,

        /// Maximum storage bytes
        #[arg(long)]
        max_storage: Option<u64>,

        /// Initial token supply minted to owner
        #[arg(long)]
        initial_supply: Option<u64>,

        /// Token decimals (default: 8)
        #[arg(long)]
        token_decimals: Option<u8>,

        /// Max token supply cap
        #[arg(long)]
        token_max_supply: Option<u64>,

        /// Whether token is mintable after creation (default: false)
        #[arg(long)]
        token_mintable: Option<bool>,

        /// Whether token is burnable (default: true)
        #[arg(long)]
        token_burnable: Option<bool>,

        /// Airdrop amount for new users
        #[arg(long)]
        user_airdrop: Option<u64>,

        /// Solver IDs to assign (comma-separated)
        #[arg(long, value_delimiter = ',')]
        solvers: Vec<String>,

        /// Validator address
        #[arg(long, default_value = "127.0.0.1:8080")]
        router: String,
    },

    /// List registered subnets
    List {
        /// Validator address
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
        Commands::Subnet { action } => {
            commands::subnet::handle(action, &config).await?;
        }
        Commands::Config { action } => {
            commands::config::handle(action).await?;
        }
        Commands::GenKey { action } => {
            commands::gen_key::handle(action).await?;
        }
        Commands::User { action } => {
            commands::user::handle(action, &config).await?;
        }
    }
    
    Ok(())
}

