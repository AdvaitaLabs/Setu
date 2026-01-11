//! Transfer command handlers

use crate::config::Config;
use colored::Colorize;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Request to submit a transfer (matches Validator API)
#[derive(Debug, Clone, Serialize)]
struct SubmitTransferRequest {
    pub from: String,
    pub to: String,
    pub amount: i128,
    pub transfer_type: String,
    pub preferred_solver: Option<String>,
    pub shard_id: Option<String>,
    pub subnet_id: Option<String>,
    pub resources: Vec<String>,
}

/// Processing step in the transfer pipeline
#[derive(Debug, Clone, Deserialize)]
struct ProcessingStep {
    pub step: String,
    pub status: String,
    pub details: Option<String>,
    pub timestamp: u64,
}

/// Response from transfer submission
#[derive(Debug, Clone, Deserialize)]
struct SubmitTransferResponse {
    pub success: bool,
    pub message: String,
    pub transfer_id: Option<String>,
    pub solver_id: Option<String>,
    pub processing_steps: Vec<ProcessingStep>,
}

/// Request to get transfer status
#[derive(Debug, Clone, Serialize)]
struct GetTransferStatusRequest {
    pub transfer_id: String,
}

/// Response with transfer status
#[derive(Debug, Clone, Deserialize)]
struct GetTransferStatusResponse {
    pub found: bool,
    pub transfer_id: String,
    pub status: Option<String>,
    pub solver_id: Option<String>,
    pub event_id: Option<String>,
    pub processing_steps: Vec<ProcessingStep>,
}

pub async fn handle(action: crate::TransferAction, _config: &Config) -> Result<()> {
    match action {
        crate::TransferAction::Submit { 
            from, 
            to, 
            amount, 
            transfer_type, 
            solver, 
            shard, 
            router 
        } => {
            println!();
            println!("{}", "╔════════════════════════════════════════════════════════════╗".cyan());
            println!("{}", "║              Submitting Transfer to Validator              ║".cyan());
            println!("{}", "╚════════════════════════════════════════════════════════════╝".cyan());
            println!();
            
            println!("  {} {}", "From:".dimmed(), from.cyan().bold());
            println!("  {} {}", "To:".dimmed(), to.cyan().bold());
            println!("  {} {}", "Amount:".dimmed(), amount.to_string().green().bold());
            println!("  {} {}", "Type:".dimmed(), transfer_type.yellow());
            if let Some(ref solver_id) = solver {
                println!("  {} {}", "Solver:".dimmed(), solver_id.cyan());
            }
            if let Some(ref shard_id) = shard {
                println!("  {} {}", "Shard:".dimmed(), shard_id.cyan());
            }
            println!("  {} {}", "Router:".dimmed(), router.dimmed());
            println!();
            
            // Build request
            let request = SubmitTransferRequest {
                from: from.clone(),
                to: to.clone(),
                amount,
                transfer_type: transfer_type.clone(),
                preferred_solver: solver,
                shard_id: shard,
                subnet_id: None, // TODO: Add subnet support to CLI
                resources: vec![
                    format!("account:{}", from),
                    format!("account:{}", to),
                ],
            };
            
            // Send to validator
            let url = format!("http://{}/api/v1/transfer", router);
            println!("{} Sending to {}...", "→".cyan().bold(), url.dimmed());
            println!();
            
            let client = reqwest::Client::new();
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await;
            
            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let result: SubmitTransferResponse = resp.json().await?;
                        
                        if result.success {
                            println!("{}", "╔════════════════════════════════════════════════════════════╗".green());
                            println!("{}", "║              ✓ Transfer Submitted Successfully             ║".green());
                            println!("{}", "╠════════════════════════════════════════════════════════════╣".green());
                            
                            if let Some(ref tid) = result.transfer_id {
                                println!("{}  Transfer ID: {:<45} {}", "║".green(), tid.cyan().bold(), "║".green());
                            }
                            if let Some(ref sid) = result.solver_id {
                                println!("{}  Solver:      {:<45} {}", "║".green(), sid.yellow(), "║".green());
                            }
                            println!("{}  Message:     {:<45} {}", "║".green(), &result.message[..45.min(result.message.len())], "║".green());
                            
                            println!("{}", "╚════════════════════════════════════════════════════════════╝".green());
                            
                            // Show processing steps
                            if !result.processing_steps.is_empty() {
                                println!();
                                println!("{}", "Processing Pipeline:".bold());
                                println!("{}", "─".repeat(60).dimmed());
                                
                                for (i, step) in result.processing_steps.iter().enumerate() {
                                    let status_icon = match step.status.as_str() {
                                        "completed" => "✓".green(),
                                        "failed" => "✗".red(),
                                        _ => "○".yellow(),
                                    };
                                    
                                    let step_name = format_step_name(&step.step);
                                    println!(
                                        "  {} {} {}",
                                        status_icon,
                                        format!("[{}]", i + 1).dimmed(),
                                        step_name.bold()
                                    );
                                    
                                    if let Some(ref details) = step.details {
                                        println!("      └─ {}", details.dimmed());
                                    }
                                }
                                println!("{}", "─".repeat(60).dimmed());
                            }
                        } else {
                            println!("{}", "╔════════════════════════════════════════════════════════════╗".red());
                            println!("{}", "║              ✗ Transfer Submission Failed                  ║".red());
                            println!("{}", "╠════════════════════════════════════════════════════════════╣".red());
                            println!("{}  Error: {:<48} {}", "║".red(), &result.message[..48.min(result.message.len())], "║".red());
                            println!("{}", "╚════════════════════════════════════════════════════════════╝".red());
                        }
                    } else {
                        println!("{} HTTP Error: {}", "✗".red().bold(), resp.status());
                    }
                }
                Err(e) => {
                    println!("{}", "╔════════════════════════════════════════════════════════════╗".red());
                    println!("{}", "║              ✗ Connection Failed                           ║".red());
                    println!("{}", "╠════════════════════════════════════════════════════════════╣".red());
                    println!("{}  Could not connect to Validator at {}                 {}", "║".red(), router, "║".red());
                    println!("{}  Error: {:<48} {}", "║".red(), &e.to_string()[..48.min(e.to_string().len())], "║".red());
                    println!("{}", "╠════════════════════════════════════════════════════════════╣".red());
                    println!("{}  Make sure the Validator is running:                         {}", "║".red(), "║".red());
                    println!("{}    $ cargo run -p setu-validator                              {}", "║".red(), "║".red());
                    println!("{}", "╚════════════════════════════════════════════════════════════╝".red());
                }
            }
            
            println!();
            Ok(())
        }
        
        crate::TransferAction::Status { id, router } => {
            println!();
            println!("{}", "╔════════════════════════════════════════════════════════════╗".cyan());
            println!("{}", "║              Querying Transfer Status                       ║".cyan());
            println!("{}", "╚════════════════════════════════════════════════════════════╝".cyan());
            println!();
            
            println!("  {} {}", "Transfer ID:".dimmed(), id.cyan().bold());
            println!("  {} {}", "Router:".dimmed(), router.dimmed());
            println!();
            
            // Build request
            let request = GetTransferStatusRequest {
                transfer_id: id.clone(),
            };
            
            // Send to validator
            let url = format!("http://{}/api/v1/transfer/status", router);
            println!("{} Querying {}...", "→".cyan().bold(), url.dimmed());
            println!();
            
            let client = reqwest::Client::new();
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await;
            
            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let result: GetTransferStatusResponse = resp.json().await?;
                        
                        if result.found {
                            println!("{}", "╔════════════════════════════════════════════════════════════╗".green());
                            println!("{}", "║              Transfer Status                                ║".green());
                            println!("{}", "╠════════════════════════════════════════════════════════════╣".green());
                            
                            println!("{}  Transfer ID: {:<44} {}", "║".green(), result.transfer_id.cyan(), "║".green());
                            
                            if let Some(ref status) = result.status {
                                let status_colored = match status.as_str() {
                                    "pending_execution" => status.yellow(),
                                    "completed" => status.green(),
                                    "failed" => status.red(),
                                    _ => status.normal(),
                                };
                                println!("{}  Status:      {:<44} {}", "║".green(), status_colored, "║".green());
                            }
                            
                            if let Some(ref sid) = result.solver_id {
                                println!("{}  Solver:      {:<44} {}", "║".green(), sid.cyan(), "║".green());
                            }
                            
                            if let Some(ref eid) = result.event_id {
                                println!("{}  Event ID:    {:<44} {}", "║".green(), eid.cyan(), "║".green());
                            }
                            
                            println!("{}", "╚════════════════════════════════════════════════════════════╝".green());
                            
                            // Show processing steps
                            if !result.processing_steps.is_empty() {
                                println!();
                                println!("{}", "Processing History:".bold());
                                println!("{}", "─".repeat(60).dimmed());
                                
                                for (i, step) in result.processing_steps.iter().enumerate() {
                                    let status_icon = match step.status.as_str() {
                                        "completed" => "✓".green(),
                                        "failed" => "✗".red(),
                                        _ => "○".yellow(),
                                    };
                                    
                                    let step_name = format_step_name(&step.step);
                                    println!(
                                        "  {} {} {}",
                                        status_icon,
                                        format!("[{}]", i + 1).dimmed(),
                                        step_name.bold()
                                    );
                                    
                                    if let Some(ref details) = step.details {
                                        println!("      └─ {}", details.dimmed());
                                    }
                                }
                                println!("{}", "─".repeat(60).dimmed());
                            }
                        } else {
                            println!("{}", "╔════════════════════════════════════════════════════════════╗".yellow());
                            println!("{}", "║              Transfer Not Found                             ║".yellow());
                            println!("{}", "╠════════════════════════════════════════════════════════════╣".yellow());
                            println!("{}  Transfer ID {} not found                    {}", "║".yellow(), id, "║".yellow());
                            println!("{}", "╚════════════════════════════════════════════════════════════╝".yellow());
                        }
                    } else {
                        println!("{} HTTP Error: {}", "✗".red().bold(), resp.status());
                    }
                }
                Err(e) => {
                    println!("{}", "╔════════════════════════════════════════════════════════════╗".red());
                    println!("{}", "║              ✗ Connection Failed                           ║".red());
                    println!("{}", "╠════════════════════════════════════════════════════════════╣".red());
                    println!("{}  Could not connect to Validator at {}                 {}", "║".red(), router, "║".red());
                    println!("{}  Error: {:<48} {}", "║".red(), &e.to_string()[..48.min(e.to_string().len())], "║".red());
                    println!("{}", "╚════════════════════════════════════════════════════════════╝".red());
                }
            }
            
            println!();
            Ok(())
        }
    }
}

/// Format step name for display
fn format_step_name(step: &str) -> String {
    match step {
        "receive" => "Receive & Validate".to_string(),
        "vlc_assign" => "VLC Assignment".to_string(),
        "dag_resolve" => "DAG Parent Resolution".to_string(),
        "route" => "Router Selection".to_string(),
        "dispatch" => "Dispatch to Solver".to_string(),
        "consensus_prepare" => "Consensus Preparation".to_string(),
        "foldgraph_update" => "FoldGraph Update".to_string(),
        _ => step.replace('_', " ").to_string(),
    }
}
