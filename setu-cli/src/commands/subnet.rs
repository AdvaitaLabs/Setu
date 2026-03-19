//! Subnet management commands

use crate::config::Config;
use crate::SubnetAction;
use anyhow::Result;
use tracing::info;

pub async fn handle(action: SubnetAction, _config: &Config) -> Result<()> {
    match action {
        SubnetAction::Register {
            subnet_id,
            name,
            owner,
            token_symbol,
            description,
            subnet_type,
            parent,
            max_users,
            max_tps,
            max_storage,
            initial_supply,
            token_decimals,
            token_max_supply,
            token_mintable,
            token_burnable,
            user_airdrop,
            solvers,
            router,
        } => {
            register_subnet(
                &router,
                subnet_id,
                name,
                owner,
                token_symbol,
                description,
                subnet_type,
                parent,
                max_users,
                max_tps,
                max_storage,
                initial_supply,
                token_decimals,
                token_max_supply,
                token_mintable,
                token_burnable,
                user_airdrop,
                solvers,
            )
            .await?;
        }
        SubnetAction::List { router } => {
            list_subnets(&router).await?;
        }
    }
    Ok(())
}

async fn register_subnet(
    router: &str,
    subnet_id: String,
    name: String,
    owner: String,
    token_symbol: String,
    description: Option<String>,
    subnet_type: Option<String>,
    parent: Option<String>,
    max_users: Option<u64>,
    max_tps: Option<u64>,
    max_storage: Option<u64>,
    initial_supply: Option<u64>,
    token_decimals: Option<u8>,
    token_max_supply: Option<u64>,
    token_mintable: Option<bool>,
    token_burnable: Option<bool>,
    user_airdrop: Option<u64>,
    solvers: Vec<String>,
) -> Result<()> {
    let url = format!("http://{}/api/v1/register/subnet", router);

    let request = serde_json::json!({
        "subnet_id": subnet_id,
        "name": name,
        "owner": owner,
        "token_symbol": token_symbol,
        "description": description,
        "subnet_type": subnet_type,
        "parent_subnet_id": parent,
        "max_users": max_users,
        "max_tps": max_tps,
        "max_storage_bytes": max_storage,
        "initial_token_supply": initial_supply,
        "token_decimals": token_decimals,
        "token_max_supply": token_max_supply,
        "token_mintable": token_mintable,
        "token_burnable": token_burnable,
        "user_airdrop_amount": user_airdrop,
        "assigned_solvers": solvers,
    });

    info!("Registering subnet '{}' (token: {})...", subnet_id, token_symbol);

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await?;

    if response.status().is_success() {
        let body: serde_json::Value = response.json().await?;
        if body["success"].as_bool().unwrap_or(false) {
            println!("✓ Subnet registered successfully");
            if let Some(sid) = body["subnet_id"].as_str() {
                println!("  Subnet ID: {}", sid);
            }
            if let Some(eid) = body["event_id"].as_str() {
                println!("  Event ID:  {}", eid);
            }
        } else {
            let msg = body["message"].as_str().unwrap_or("Unknown error");
            println!("✗ Registration failed: {}", msg);
        }
    } else {
        println!("✗ HTTP error: {}", response.status());
    }

    Ok(())
}

async fn list_subnets(router: &str) -> Result<()> {
    let url = format!("http://{}/api/v1/subnets", router);

    let client = reqwest::Client::new();
    let response = client.get(&url).send().await?;

    if response.status().is_success() {
        let body: serde_json::Value = response.json().await?;
        let subnets = body["subnets"].as_array();

        match subnets {
            Some(list) if !list.is_empty() => {
                println!("Registered subnets ({}):", list.len());
                println!("{:<20} {:<20} {:<12} {:<10} {}", "SUBNET ID", "NAME", "TOKEN", "TYPE", "OWNER");
                println!("{}", "-".repeat(80));
                for s in list {
                    println!(
                        "{:<20} {:<20} {:<12} {:<10} {}",
                        s["subnet_id"].as_str().unwrap_or("-"),
                        s["name"].as_str().unwrap_or("-"),
                        s["token_symbol"].as_str().unwrap_or("-"),
                        s["subnet_type"].as_str().unwrap_or("-"),
                        s["owner"].as_str().unwrap_or("-"),
                    );
                }
            }
            _ => {
                println!("No subnets registered");
            }
        }
    } else {
        println!("✗ HTTP error: {}", response.status());
    }

    Ok(())
}
