//! Key generation and management utilities

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use setu_keys::{
    SignatureScheme, SetuKeyPair,
    generate_new_key,
    key_derive::derive_key_pair_from_mnemonic,
};
use std::fs;
use std::path::Path;
use colored::Colorize;

/// Keypair data stored in JSON file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeypairData {
    /// Node ID (validator-xxx or solver-xxx)
    pub node_id: String,
    /// Node type (validator or solver)
    pub node_type: String,
    /// Setu account address (0x + 64 hex chars, 32 bytes from Blake2b-256)
    pub account_address: String,
    /// Public key (hex encoded, 32 bytes for ed25519)
    pub public_key: String,
    /// Private key (hex encoded, 32 bytes) - SENSITIVE!
    pub private_key: String,
    /// BIP39 mnemonic phrase (12 or 24 words) - SENSITIVE!
    pub mnemonic: String,
    /// Timestamp when created
    pub created_at: u64,
    /// Additional metadata (for validators: stake, commission)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Generate a new keypair for validator or solver
///
/// Uses ed25519 scheme — compatible with Vote signing/verification which
/// requires 32-byte ed25519 public keys.
pub fn generate_keypair(
    node_type: &str,
    node_id: Option<String>,
    metadata: serde_json::Value,
) -> Result<KeypairData> {
    println!("{} Generating {} keypair (ed25519)...", "🔑".cyan(), node_type);
    
    // Generate ed25519 keypair with mnemonic
    let (address, keypair, _scheme, mnemonic) = generate_new_key(
        SignatureScheme::ED25519,
        None,
        None,
    ).context("Failed to generate keypair")?;
    
    // ed25519 public key is 32 bytes
    let public_key_bytes = keypair.public().as_bytes();
    
    // Use standard SetuAddress (Blake2b-256) returned by generate_new_key
    let account_address = address.to_hex();
    
    // Generate node_id if not provided
    let node_id = node_id.unwrap_or_else(|| {
        format!("{}-{}", node_type, &account_address[2..10])
    });
    
    // Get private key bytes (skip scheme flag byte)
    let private_key_hex = keypair.encode_base64();
    let private_key_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &private_key_hex
    )?;
    let private_key_only = hex::encode(&private_key_bytes[1..]); // Skip scheme flag
    
    let keypair_data = KeypairData {
        node_id: node_id.clone(),
        node_type: node_type.to_string(),
        account_address,
        public_key: hex::encode(&public_key_bytes),
        private_key: private_key_only,
        mnemonic: mnemonic.clone(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        metadata,
    };
    
    Ok(keypair_data)
}

/// Save keypair to file
pub fn save_keypair(keypair: &KeypairData, output_path: &str) -> Result<()> {
    let json = serde_json::to_string_pretty(keypair)?;
    fs::write(output_path, json)
        .context(format!("Failed to write keypair to {}", output_path))?;
    Ok(())
}

/// Load keypair from file
pub fn load_keypair(key_file: &str) -> Result<KeypairData> {
    let json = fs::read_to_string(key_file)
        .context(format!("Failed to read keypair from {}", key_file))?;
    let keypair: KeypairData = serde_json::from_str(&json)
        .context("Failed to parse keypair JSON")?;
    Ok(keypair)
}

/// Recover keypair from mnemonic
pub fn recover_from_mnemonic(
    mnemonic: &str,
    node_type: &str,
    node_id: Option<String>,
    metadata: serde_json::Value,
) -> Result<KeypairData> {
    println!("{} Recovering {} keypair from mnemonic (ed25519)...", "🔄".cyan(), node_type);
    
    // Derive ed25519 keypair from mnemonic
    let (address, keypair) = derive_key_pair_from_mnemonic(
        mnemonic,
        &SignatureScheme::ED25519,
        None,
    ).context("Failed to derive keypair from mnemonic")?;
    
    // ed25519 public key is 32 bytes
    let public_key_bytes = keypair.public().as_bytes();
    
    // Use standard SetuAddress (Blake2b-256)
    let account_address = address.to_hex();
    
    // Generate node_id if not provided
    let node_id = node_id.unwrap_or_else(|| {
        format!("{}-{}", node_type, &account_address[2..10])
    });
    
    // Get private key bytes (skip scheme flag byte)
    let private_key_hex = keypair.encode_base64();
    let private_key_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &private_key_hex
    )?;
    let private_key_only = hex::encode(&private_key_bytes[1..]);
    
    let keypair_data = KeypairData {
        node_id: node_id.clone(),
        node_type: node_type.to_string(),
        account_address,
        public_key: hex::encode(&public_key_bytes),
        private_key: private_key_only,
        mnemonic: mnemonic.to_string(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        metadata,
    };
    
    Ok(keypair_data)
}

/// Display keypair information (with warnings for sensitive data)
pub fn display_keypair(keypair: &KeypairData, show_sensitive: bool) {
    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║        {} Key Generated Successfully                    ║", 
        keypair.node_type.to_uppercase().cyan().bold());
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║  Node ID:       {:<44} ║", keypair.node_id.cyan());
    println!("║  Node Type:     {:<44} ║", keypair.node_type);
    println!("║  Address:       {:<44} ║", &keypair.account_address[..22]);
    println!("║                 {:<44} ║", &keypair.account_address[22..]);
    
    if show_sensitive {
        println!("║                                                            ║");
        println!("║  {}                            ║", "⚠️  SENSITIVE INFORMATION BELOW".yellow().bold());
        println!("║                                                            ║");
        println!("║  Private Key:   {:<44} ║", &keypair.private_key[..44]);
        if keypair.private_key.len() > 44 {
            println!("║                 {:<44} ║", &keypair.private_key[44..]);
        }
        println!("║                                                            ║");
        println!("║  Mnemonic:      {:<44} ║", &keypair.mnemonic[..44.min(keypair.mnemonic.len())]);
        if keypair.mnemonic.len() > 44 {
            let remaining = &keypair.mnemonic[44..];
            for chunk in remaining.as_bytes().chunks(44) {
                let chunk_str = std::str::from_utf8(chunk).unwrap_or("");
                println!("║                 {:<44} ║", chunk_str);
            }
        }
    }
    
    println!("╚════════════════════════════════════════════════════════════╝");
    
    if show_sensitive {
        println!("\n{} {}", 
            "⚠️".yellow().bold(),
            "IMPORTANT: Save your private key and mnemonic securely!".yellow().bold()
        );
        println!("   {} Never share them with anyone!", "•".yellow());
        println!("   {} Store them in a secure location (password manager, hardware wallet, etc.)", "•".yellow());
        println!("   {} You can recover your key using the mnemonic phrase.", "•".yellow());
    }
}

/// Export keypair in different formats
pub fn export_keypair(keypair: &KeypairData, format: &str) -> Result<()> {
    println!("\n{} {}", 
        "⚠️".yellow().bold(),
        "WARNING: You are about to export sensitive information!".yellow().bold()
    );
    println!("\nAnyone with access to this information can:");
    println!("  {} Control your {}", "•".red(), keypair.node_type);
    println!("  {} Access your staked funds (for validators)", "•".red());
    println!("  {} Sign transactions on your behalf", "•".red());
    println!("\n{}", "Are you sure you want to continue? (yes/no): ".bold());
    
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    
    if input.trim().to_lowercase() != "yes" {
        println!("{} Export cancelled.", "✗".red().bold());
        return Ok(());
    }
    
    println!();
    match format.to_lowercase().as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(keypair)?);
        }
        "mnemonic" => {
            println!("Mnemonic: {}", keypair.mnemonic.green());
            println!("\n{} Keep this mnemonic safe! It can recover your private key.", 
                "⚠️".yellow().bold()
            );
        }
        "private-key" | "privatekey" => {
            println!("Private Key: 0x{}", keypair.private_key.green());
            println!("\n{} Never share your private key!", 
                "⚠️".yellow().bold()
            );
        }
        _ => {
            anyhow::bail!("Unknown export format: {}. Use 'json', 'mnemonic', or 'private-key'", format);
        }
    }
    
    Ok(())
}

