//! Router command handlers

use crate::config::Config;
use colored::Colorize;
use anyhow::Result;
use setu_rpc::HttpRegistrationClient;

pub async fn handle(action: crate::RouterAction, _config: &Config) -> Result<()> {
    match action {
        crate::RouterAction::Status { address } => {
            println!("{} Querying router status...", "→".cyan().bold());
            println!("  Router: {}", address.cyan());
            
            // Parse address
            let (host, port) = parse_address(&address)?;
            
            // Query health endpoint
            let url = format!("http://{}:{}/api/v1/health", host, port);
            
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let health: serde_json::Value = response.json().await?;
                        
                        println!("{} Router is healthy!", "✓".green().bold());
                        println!();
                        println!("  {:<20} {}", "Status:".bold(), 
                            health["status"].as_str().unwrap_or("unknown").green()
                        );
                        println!("  {:<20} {}", "Validator ID:".bold(), 
                            health["validator_id"].as_str().unwrap_or("unknown").cyan()
                        );
                        println!("  {:<20} {} seconds", "Uptime:".bold(), 
                            health["uptime_seconds"].as_u64().unwrap_or(0)
                        );
                        println!("  {:<20} {}", "Solver Count:".bold(), 
                            health["solver_count"].as_u64().unwrap_or(0)
                        );
                        println!("  {:<20} {}", "Validator Count:".bold(), 
                            health["validator_count"].as_u64().unwrap_or(0)
                        );
                    } else {
                        println!("{} Router returned error: HTTP {}", 
                            "✗".red().bold(), 
                            response.status()
                        );
                    }
                }
                Err(e) => {
                    println!("{} Failed to connect to router: {}", 
                        "✗".red().bold(), 
                        e.to_string().red()
                    );
                    println!("{} Make sure the validator service is running at {}", 
                        "→".dimmed(), 
                        address.dimmed()
                    );
                }
            }
            
            Ok(())
        }
        
        crate::RouterAction::Solvers { address } => {
            println!("{} Listing registered solvers...", "→".cyan().bold());
            println!("  Router: {}", address.cyan());
            
            // Parse address
            let (host, port) = parse_address(&address)?;
            
            // Create HTTP client
            let client = HttpRegistrationClient::new(&host, port);
            
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
