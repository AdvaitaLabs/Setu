//! Config command handlers

use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub async fn handle(action: crate::ConfigAction) -> Result<()> {
    match action {
        crate::ConfigAction::Init { path } => {
            let config_path = if let Some(p) = path {
                let path = std::path::PathBuf::from(p);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let config = Config::default();
                config.save(path.to_str().unwrap())?;
                path
            } else {
                Config::init()?
            };

            println!(
                "{} Configuration initialized at: {}",
                "✓".green().bold(),
                config_path.display().to_string().cyan()
            );
            Ok(())
        }

        crate::ConfigAction::Show => {
            let config_path = Config::default_path();

            if !config_path.exists() {
                println!(
                    "{} No configuration file found. Run 'setu config init' first.",
                    "✗".red().bold()
                );
                return Ok(());
            }

            let config = Config::load(config_path.to_str().unwrap())?;

            println!("{}", "Configuration:".bold());
            println!("  Router Address: {}", config.router_address.cyan());
            println!(
                "  Router Port:    {}",
                config.router_port.to_string().cyan()
            );
            println!("  Client ID:      {}", config.client_id.cyan());
            println!();
            println!(
                "Config file: {}",
                config_path.display().to_string().dimmed()
            );

            Ok(())
        }

        crate::ConfigAction::Set { key, value } => {
            let config_path = Config::default_path();

            if !config_path.exists() {
                Config::init()?;
            }

            let mut config = Config::load(config_path.to_str().unwrap())?;

            match key.as_str() {
                "router_address" => {
                    config.router_address = value.clone();
                    println!(
                        "{} Set router_address = {}",
                        "✓".green().bold(),
                        value.cyan()
                    );
                }
                "router_port" => {
                    config.router_port = value.parse()?;
                    println!("{} Set router_port = {}", "✓".green().bold(), value.cyan());
                }
                "client_id" => {
                    config.client_id = value.clone();
                    println!("{} Set client_id = {}", "✓".green().bold(), value.cyan());
                }
                _ => {
                    println!("{} Unknown config key: {}", "✗".red().bold(), key.red());
                    println!("Available keys: router_address, router_port, client_id");
                    return Ok(());
                }
            }

            config.save(config_path.to_str().unwrap())?;

            Ok(())
        }
    }
}
