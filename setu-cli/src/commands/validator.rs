//! Validator command handlers

use crate::config::Config;
use colored::Colorize;
use anyhow::Result;
use setu_rpc::{
    RegisterValidatorRequest,
    HttpRegistrationClient,
};

pub async fn handle(action: crate::ValidatorAction, _config: &Config) -> Result<()> {
    match action {
        crate::ValidatorAction::Register { id, address, port, router } => {
            println!("{} Registering validator...", "→".cyan().bold());
            println!("  Validator ID: {}", id.cyan());
            println!("  Address:      {}:{}", address.cyan(), port.to_string().cyan());
            println!("  Router:       {}", router.cyan());
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Create registration request
            let request = RegisterValidatorRequest {
                validator_id: id.clone(),
                address: address.clone(),
                port,
                public_key: None,
                stake: None,
            };
            
            // Send registration request
            match client.register_validator(request).await {
                Ok(response) => {
                    if response.success {
                        println!("{} Validator registered successfully!", "✓".green().bold());
                        println!("  Message: {}", response.message.green());
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
                        println!("  {:<20} {:<20} {:<10} {:<10}", 
                            "ID".bold(), 
                            "ADDRESS".bold(), 
                            "PORT".bold(),
                            "STATUS".bold()
                        );
                        println!("  {}", "-".repeat(60));
                        
                        for v in response.validators {
                            let status_colored = match v.status.as_str() {
                                "online" => v.status.green(),
                                "offline" => v.status.red(),
                                _ => v.status.yellow(),
                            };
                            println!("  {:<20} {:<20} {:<10} {:<10}", 
                                v.validator_id.cyan(),
                                v.address,
                                v.port,
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
