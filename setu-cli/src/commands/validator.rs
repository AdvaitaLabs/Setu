//! Validator command handlers

use crate::config::Config;
use crate::commands::keygen::{
    generate_keypair, save_keypair, load_keypair, 
    recover_from_mnemonic, display_keypair, export_keypair,
};
use colored::Colorize;
use anyhow::Result;
use setu_rpc::{
    RegisterValidatorRequest,
    HttpRegistrationClient,
};

pub async fn handle(action: crate::ValidatorAction, _config: &Config) -> Result<()> {
    match action {
        crate::ValidatorAction::Keygen { output, id, stake, commission } => {
            // Create metadata
            let metadata = serde_json::json!({
                "stake_amount": stake,
                "commission_rate": commission,
            });
            
            // Generate keypair
            let keypair = generate_keypair("validator", id, metadata)?;
            
            // Save to file
            save_keypair(&keypair, &output)?;
            
            // Display information
            display_keypair(&keypair, true);
            
            println!("\n{} Key file saved to: {}", 
                "✓".green().bold(), 
                output.cyan()
            );
            
            Ok(())
        }
        
        crate::ValidatorAction::ExportKey { key_file, format } => {
            println!("{} Loading keypair from: {}", 
                "→".cyan().bold(), 
                key_file.cyan()
            );
            
            let keypair = load_keypair(&key_file)?;
            export_keypair(&keypair, &format)?;
            
            Ok(())
        }
        
        crate::ValidatorAction::Recover { mnemonic, output, id } => {
            // Create default metadata
            let metadata = serde_json::json!({
                "stake_amount": 10000,
                "commission_rate": 10,
            });
            
            // Recover keypair
            let keypair = recover_from_mnemonic(&mnemonic, "validator", id, metadata)?;
            
            // Save to file
            save_keypair(&keypair, &output)?;
            
            // Display information
            display_keypair(&keypair, true);
            
            println!("\n{} Key recovered and saved to: {}", 
                "✓".green().bold(), 
                output.cyan()
            );
            
            Ok(())
        }
        
        crate::ValidatorAction::Register { key_file, network_address, network_port, router } => {
            println!("{} Registering validator...", "→".cyan().bold());
            
            // Load keypair
            let keypair = load_keypair(&key_file)?;
            
            println!("  Validator ID:     {}", keypair.node_id.cyan());
            println!("  Account Address:  {}", keypair.account_address.cyan());
            println!("  Network Address:  {}:{}", network_address.cyan(), network_port.to_string().cyan());
            println!("  Router:           {}", router.cyan());
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Get stake and commission from metadata
            let stake_amount = keypair.metadata.get("stake_amount")
                .and_then(|v| v.as_u64())
                .unwrap_or(10000);
            let commission_rate = keypair.metadata.get("commission_rate")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as u8;
            
            // Decode public key
            let public_key_bytes = hex::decode(&keypair.public_key)?;
            
            // Create registration message to sign
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let message = format!(
                "Register Validator: {}:{}:{}",
                keypair.node_id,
                keypair.account_address,
                timestamp
            );
            
            // TODO: Sign the message with private key
            // For now, use empty signature
            let signature = vec![];
            
            // Create registration request
            let request = RegisterValidatorRequest {
                validator_id: keypair.node_id.clone(),
                address: network_address.clone(),
                port: network_port,
                account_address: keypair.account_address.clone(),
                public_key: public_key_bytes,
                signature,
                stake_amount,
                commission_rate,
            };
            
            // Send registration request
            match client.register_validator(request).await {
                Ok(response) => {
                    if response.success {
                        println!("{} Validator registered successfully!", "✓".green().bold());
                        println!("  Message: {}", response.message.green());
                        println!("\n{} Validator Details:", "→".cyan().bold());
                        println!("  Validator ID:     {}", keypair.node_id.cyan());
                        println!("  Account Address:  {}", keypair.account_address.cyan());
                        println!("  Network Address:  {}:{}", network_address, network_port);
                        println!("  Stake Amount:     {} FLUX", stake_amount);
                        println!("  Commission Rate:  {}%", commission_rate);
                    } else {
                        println!("{} Registration failed: {}", "✗".red().bold(), response.message.red());
                    }
                }
                Err(e) => {
                    println!("{} Failed to connect to router: {}", "✗".red().bold(), e.to_string().red());
                    println!("{} Make sure the validator service is running at {}", 
                        "→".dimmed(), 
                        router.dimmed()
                    );
                }
            }
            
            Ok(())
        }
        
        crate::ValidatorAction::Status { id } => {
            println!("{} Querying validator status...", "→".cyan().bold());
            println!("  Validator ID: {}", id.cyan());
            
            // TODO: Implement status query via HTTP
            println!("{} Status query requires validator address", 
                "!".yellow().bold()
            );
            
            Ok(())
        }
        
        crate::ValidatorAction::List { router } => {
            println!("{} Listing validators from: {}", 
                "→".cyan().bold(), 
                router.cyan()
            );
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Get validator list
            match client.get_validator_list().await {
                Ok(response) => {
                    if response.validators.is_empty() {
                        println!("{} No validators registered", "→".dimmed());
                    } else {
                        println!("{} Found {} validator(s):", "✓".green().bold(), response.validators.len());
                        println!();
                        println!("  {:<20} {:<42} {:<20} {:<10}", 
                            "ID".bold(), 
                            "ACCOUNT ADDRESS".bold(),
                            "NETWORK".bold(), 
                            "STATUS".bold()
                        );
                        println!("  {}", "-".repeat(100));
                        
                        for v in response.validators {
                            let status_colored = match v.status.as_str() {
                                "online" => v.status.green(),
                                "offline" => v.status.red(),
                                _ => v.status.yellow(),
                            };
                            let network = format!("{}:{}", v.address, v.port);
                            println!("  {:<20} {:<42} {:<20} {:<10}", 
                                v.validator_id.cyan(),
                                v.account_address.unwrap_or_else(|| "N/A".to_string()),
                                network,
                                status_colored
                            );
                        }
                    }
                }
                Err(e) => {
                    println!("{} Failed to get validator list: {}", "✗".red().bold(), e.to_string().red());
                }
            }
            
            Ok(())
        }
    }
}

/// Parse address string into host and port
fn parse_address(addr: &str) -> Result<(String, u16)> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid address format. Expected host:port");
    }
    
    let host = parts[0].to_string();
    let port: u16 = parts[1].parse()
        .map_err(|_| anyhow::anyhow!("Invalid port number"))?;
    
    Ok((host, port))
}
