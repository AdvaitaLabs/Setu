//! Solver command handlers

use crate::config::Config;
use colored::Colorize;
use anyhow::Result;
use setu_rpc::{
    RegisterSolverRequest,
    HttpRegistrationClient,
};

pub async fn handle(action: crate::SolverAction, _config: &Config) -> Result<()> {
    match action {
        crate::SolverAction::Register { 
            id, 
            address, 
            port, 
            capacity, 
            shard, 
            resources, 
            router 
        } => {
            println!("{} Registering solver...", "→".cyan().bold());
            println!("  Solver ID:  {}", id.cyan());
            println!("  Address:    {}:{}", address.cyan(), port.to_string().cyan());
            println!("  Capacity:   {}", capacity.to_string().cyan());
            if let Some(shard_id) = &shard {
                println!("  Shard:      {}", shard_id.cyan());
            }
            if !resources.is_empty() {
                println!("  Resources:  {}", resources.join(", ").cyan());
            }
            println!("  Router:     {}", router.cyan());
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&router_host, router_port);
            
            // Create registration request
            let request = RegisterSolverRequest {
                solver_id: id.clone(),
                address: address.clone(),
                port,
                capacity,
                shard_id: shard.clone(),
                resources: resources.clone(),
                public_key: None,
            };
            
            // Send registration request
            match client.register_solver(request).await {
                Ok(response) => {
                    if response.success {
                        println!("{} Solver registered successfully!", "✓".green().bold());
                        println!("  Message: {}", response.message.green());
                        if let Some(assigned_id) = response.assigned_id {
                            println!("  Assigned ID: {}", assigned_id.cyan());
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
            println!("{} Status query requires validator address", 
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
                        println!("  {:<15} {:<15} {:<8} {:<10} {:<10} {:<10} {:<10}", 
                            "ID".bold(), 
                            "ADDRESS".bold(), 
                            "PORT".bold(),
                            "CAPACITY".bold(),
                            "LOAD".bold(),
                            "STATUS".bold(),
                            "SHARD".bold()
                        );
                        println!("  {}", "-".repeat(80));
                        
                        for s in response.solvers {
                            let status_colored = match s.status.as_str() {
                                "Online" => s.status.green(),
                                "Offline" => s.status.red(),
                                _ => s.status.yellow(),
                            };
                            let shard_display = s.shard_id.unwrap_or_else(|| "-".to_string());
                            let load_pct = if s.capacity > 0 {
                                format!("{}%", (s.current_load * 100 / s.capacity))
                            } else {
                                "N/A".to_string()
                            };
                            
                            println!("  {:<15} {:<15} {:<8} {:<10} {:<10} {:<10} {:<10}", 
                                s.solver_id.cyan(),
                                s.address,
                                s.port,
                                s.capacity,
                                load_pct,
                                status_colored,
                                shard_display
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
            println!("  Load:      {}", load.to_string().cyan());
            println!("  Router:    {}", router.cyan());
            
            // Parse router address
            let (router_host, router_port) = parse_address(&router)?;
            
            // Send heartbeat via HTTP
            let url = format!("http://{}:{}/api/v1/heartbeat", router_host, router_port);
            
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            let request = setu_rpc::HeartbeatRequest {
                node_id: id.clone(),
                current_load: Some(load),
                timestamp,
            };
            
            let client = reqwest::Client::new();
            match client.post(&url).json(&request).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        println!("{} Heartbeat sent successfully!", "✓".green().bold());
                    } else {
                        println!("{} Heartbeat failed: HTTP {}", "✗".red().bold(), response.status());
                    }
                }
                Err(e) => {
                    println!("{} Failed to send heartbeat: {}", "✗".red().bold(), e.to_string().red());
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
