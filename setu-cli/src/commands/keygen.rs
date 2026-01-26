//! Key generation and management utilities

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use setu_keys::{
    SignatureScheme, SetuKeyPair, EthereumAddress,
    derive_ethereum_address_from_secp256k1,
    generate_new_key,
    key_derive::derive_key_pair_from_mnemonic,
};
use k256::elliptic_curve::sec1::ToEncodedPoint;
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
    /// Ethereum-style account address (0x...)
    pub account_address: String,
    /// Public key (hex encoded, 65 bytes uncompressed for secp256k1)
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
pub fn generate_keypair(
    node_type: &str,
    node_id: Option<String>,
    metadata: serde_json::Value,
) -> Result<KeypairData> {
    println!("{} Generating {} keypair...", "🔑".cyan(), node_type);
    
    // Generate secp256k1 keypair with mnemonic
    let (address, keypair, scheme, mnemonic) = generate_new_key(
        SignatureScheme::Secp256k1,
        None,
        None,
    ).context("Failed to generate keypair")?;
    
    // Get public key bytes (uncompressed, 65 bytes)
    let public_key = keypair.public();
    
    // For secp256k1, we need to get the uncompressed format (65 bytes)
    let public_key_bytes = match &public_key {
        setu_keys::PublicKey::Secp256k1(vk) => {
            // Get uncompressed point (65 bytes: 0x04 || x || y)
            use k256::elliptic_curve::sec1::ToEncodedPoint;
            let point = vk.to_encoded_point(false); // false = uncompressed
            point.as_bytes().to_vec()
        }
        _ => public_key.as_bytes(),
    };
    
    // Derive Ethereum-style address
    let eth_address = derive_ethereum_address_from_secp256k1(&public_key_bytes)
        .context("Failed to derive Ethereum address")?;
    
    // Generate node_id if not provided
    let node_id = node_id.unwrap_or_else(|| {
        format!("{}-{}", node_type, &eth_address.to_hex()[2..10])
    });
    
    // Get private key bytes
    let private_key_hex = keypair.encode_base64();
    // Extract just the private key part (remove scheme flag)
    let private_key_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &private_key_hex
    )?;
    let private_key_only = hex::encode(&private_key_bytes[1..]); // Skip scheme flag
    
    let keypair_data = KeypairData {
        node_id: node_id.clone(),
        node_type: node_type.to_string(),
        account_address: eth_address.to_hex(),
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
    println!("{} Recovering {} keypair from mnemonic...", "🔄".cyan(), node_type);
    
    // Derive keypair from mnemonic
    let (address, keypair) = derive_key_pair_from_mnemonic(
        mnemonic,
        &SignatureScheme::Secp256k1,
        None,
    ).context("Failed to derive keypair from mnemonic")?;
    
    // Get public key bytes
    let public_key = keypair.public();
    
    // For secp256k1, we need to get the uncompressed format (65 bytes)
    let public_key_bytes = match &public_key {
        setu_keys::PublicKey::Secp256k1(vk) => {
            // Get uncompressed point (65 bytes: 0x04 || x || y)
            use k256::elliptic_curve::sec1::ToEncodedPoint;
            let point = vk.to_encoded_point(false); // false = uncompressed
            point.as_bytes().to_vec()
        }
        _ => public_key.as_bytes(),
    };
    
    // Derive Ethereum-style address
    let eth_address = derive_ethereum_address_from_secp256k1(&public_key_bytes)
        .context("Failed to derive Ethereum address")?;
    
    // Generate node_id if not provided
    let node_id = node_id.unwrap_or_else(|| {
        format!("{}-{}", node_type, &eth_address.to_hex()[2..10])
    });
    
    // Get private key bytes
    let private_key_hex = keypair.encode_base64();
    let private_key_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &private_key_hex
    )?;
    let private_key_only = hex::encode(&private_key_bytes[1..]);
    
    let keypair_data = KeypairData {
        node_id: node_id.clone(),
        node_type: node_type.to_string(),
        account_address: eth_address.to_hex(),
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

