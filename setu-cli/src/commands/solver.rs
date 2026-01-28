//! Solver command handlers

use crate::config::Config;
use crate::commands::keygen::{
    generate_keypair, save_keypair, load_keypair, 
    recover_from_mnemonic, display_keypair, export_keypair,
};
use colored::Colorize;
use anyhow::Result;
use setu_rpc::{
    RegisterSolverRequest,
    HttpRegistrationClient,
};

pub async fn handle(action: crate::SolverAction, _config: &Config) -> Result<()> {
    match action {
        crate::SolverAction::Keygen { output, id } => {
            // Create metadata
            let metadata = serde_json::json!({});
            
            // Generate keypair
            let keypair = generate_keypair("solver", id, metadata)?;
            
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
        
        crate::SolverAction::ExportKey { key_file, format } => {
            println!("{} Loading keypair from: {}", 
                "→".cyan().bold(), 
                key_file.cyan()
            );
            
            let keypair = load_keypair(&key_file)?;
            export_keypair(&keypair, &format)?;
            
            Ok(())
        }
        
        crate::SolverAction::Recover { mnemonic, output, id } => {
            // Create default metadata
            let metadata = serde_json::json!({});
            
            // Recover keypair
            let keypair = recover_from_mnemonic(&mnemonic, "solver", id, metadata)?;
            
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
        
        crate::SolverAction::Register { 
            key_file, 
            network_address, 
            network_port, 
            capacity, 
            shard, 
            resources, 
            router 
        } => {
            println!("{} Registering solver...", "→".cyan().bold());
            
            // Load keypair
            let keypair = load_keypair(&key_file)?;
            
            println!("  Solver ID:        {}", keypair.node_id.cyan());
            println!("  Account Address:  {}", keypair.account_address.cyan());
            println!("  Network Address:  {}:{}", network_address.cyan(), network_port.to_string().cyan());
            println!("  Capacity:         {}", capacity);
            if let Some(ref s) = shard {
                println!("  Shard:            {}", s.cyan());
            }
            if !resources.is_empty() {
                println!("  Resources:        {}", resources.join(", ").cyan());
            }
            println!("  Router:           {}", router.cyan());
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Decode public key
            let public_key_bytes = hex::decode(&keypair.public_key)?;
            
            // Create registration message to sign
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let message = format!(
                "Register Solver: {}:{}:{}",
                keypair.node_id,
                keypair.account_address,
                timestamp
            );
            
            // TODO: Sign the message with private key
            // For now, use empty signature
            let signature = vec![];
            
            // Create registration request
            let request = RegisterSolverRequest {
                solver_id: keypair.node_id.clone(),
                address: network_address.clone(),
                port: network_port,
                account_address: keypair.account_address.clone(),
                public_key: public_key_bytes,
                signature,
                capacity,
                shard_id: shard.clone(),
                resources: resources.clone(),
            };
            
            // Send registration request
            match client.register_solver(request).await {
                Ok(response) => {
                    if response.success {
                        println!("{} Solver registered successfully!", "✓".green().bold());
                        println!("  Message: {}", response.message.green());
                        println!("\n{} Solver Details:", "→".cyan().bold());
                        println!("  Solver ID:        {}", keypair.node_id.cyan());
                        println!("  Account Address:  {}", keypair.account_address.cyan());
                        println!("  Network Address:  {}:{}", network_address, network_port);
                        println!("  Capacity:         {}", capacity);
                        if let Some(s) = shard {
                            println!("  Shard:            {}", s);
                        }
                        if !resources.is_empty() {
                            println!("  Resources:        {}", resources.join(", "));
                        }
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
        
        crate::SolverAction::Status { id } => {
            println!("{} Querying solver status...", "→".cyan().bold());
            println!("  Solver ID: {}", id.cyan());
            
            // TODO: Implement status query via HTTP
            println!("{} Status query not yet implemented", 
                "!".yellow().bold()
            );
            
            Ok(())
        }
        
        crate::SolverAction::List { router } => {
            println!("{} Listing solvers from: {}", 
                "→".cyan().bold(), 
                router.cyan()
            );
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Get solver list
            match client.get_solver_list().await {
                Ok(response) => {
                    if response.solvers.is_empty() {
                        println!("{} No solvers registered", "→".dimmed());
                    } else {
                        println!("{} Found {} solver(s):", "✓".green().bold(), response.solvers.len());
                        println!();
                        println!("  {:<20} {:<42} {:<20} {:<10} {:<10}", 
                            "ID".bold(), 
                            "ACCOUNT ADDRESS".bold(),
                            "NETWORK".bold(),
                            "CAPACITY".bold(),
                            "STATUS".bold()
                        );
                        println!("  {}", "-".repeat(110));
                        
                        for s in response.solvers {
                            let status_colored = match s.status.as_str() {
                                "online" => s.status.green(),
                                "offline" => s.status.red(),
                                _ => s.status.yellow(),
                            };
                            let network = format!("{}:{}", s.address, s.port);
                            println!("  {:<20} {:<42} {:<20} {:<10} {:<10}", 
                                s.solver_id.cyan(),
                                s.account_address.unwrap_or_else(|| "N/A".to_string()),
                                network,
                                s.capacity,
                                status_colored
                            );
                        }
                    }
                }
                Err(e) => {
                    println!("{} Failed to get solver list: {}", "✗".red().bold(), e.to_string().red());
                }
            }
            
            Ok(())
        }
        
        crate::SolverAction::Heartbeat { id, load, router } => {
            println!("{} Sending heartbeat...", "→".cyan().bold());
            println!("  Solver ID: {}", id.cyan());
            println!("  Load:      {}", load);
            println!("  Router:    {}", router.cyan());
            
            // TODO: Implement heartbeat via HTTP
            println!("{} Heartbeat not yet implemented", 
                "!".yellow().bold()
            );
            
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
