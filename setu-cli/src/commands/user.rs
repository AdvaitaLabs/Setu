//! User profile and subnet membership commands

use crate::config::Config;
use crate::UserAction;
use anyhow::{Context, Result};
use setu_keys::keypair_file::read_keypair_from_file;
use setu_rpc::{
    HttpUserClient, UpdateProfileRequest, JoinSubnetRequest, LeaveSubnetRequest,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn handle(action: UserAction, _config: &Config) -> Result<()> {
    match action {
        UserAction::Profile { action } => match action {
            crate::ProfileAction::Update {
                address,
                display_name,
                avatar_url,
                bio,
                key_file,
                validator,
            } => {
                update_profile(&validator, &address, display_name, avatar_url, bio, &key_file)
                    .await?;
            }
            crate::ProfileAction::Get { address, validator } => {
                get_profile(&validator, &address).await?;
            }
        },
        UserAction::Subnet { action } => match action {
            crate::UserSubnetAction::Join {
                address,
                subnet_id,
                key_file,
                validator,
            } => {
                join_subnet(&validator, &address, &subnet_id, &key_file).await?;
            }
            crate::UserSubnetAction::Leave {
                address,
                subnet_id,
                key_file,
                validator,
            } => {
                leave_subnet(&validator, &address, &subnet_id, &key_file).await?;
            }
            crate::UserSubnetAction::Check {
                address,
                subnet_id,
                validator,
            } => {
                check_membership(&validator, &address, &subnet_id).await?;
            }
            crate::UserSubnetAction::List { address, validator } => {
                list_subnets(&validator, &address).await?;
            }
        },
    }
    Ok(())
}

fn sign_message(key_file: &str, message: &str) -> Result<(Vec<u8>, String)> {
    let keypair = read_keypair_from_file(key_file)
        .context(format!("Cannot read key file: {}", key_file))?;

    let public_key = keypair.public().encode_base64();
    let signature = keypair.sign(message.as_bytes());

    Ok((signature.as_bytes(), public_key))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn update_profile(
    validator: &str,
    address: &str,
    display_name: Option<String>,
    avatar_url: Option<String>,
    bio: Option<String>,
    key_file: &str,
) -> Result<()> {
    let timestamp = now_millis();
    let message = format!("update_profile:{}:{}", address, timestamp);
    let (signature, public_key) = sign_message(key_file, &message)?;

    let client = HttpUserClient::new(validator);
    let resp = client
        .update_profile(UpdateProfileRequest {
            address: address.to_string(),
            display_name,
            avatar_url,
            bio,
            attributes: None,
            signature,
            message,
            timestamp,
            public_key: Some(public_key),
            nostr_pubkey: None,
        })
        .await?;

    if resp.success {
        println!("✓ Profile updated");
        if let Some(eid) = resp.event_id {
            println!("  Event ID: {}", eid);
        }
    } else {
        println!("✗ Failed: {}", resp.message);
    }
    Ok(())
}

async fn get_profile(validator: &str, address: &str) -> Result<()> {
    let client = HttpUserClient::new(validator);
    let resp = client.get_profile(address).await?;

    if resp.found {
        println!("Profile for {}", resp.address);
        if let Some(p) = resp.profile {
            println!("  Display Name: {}", p.display_name.as_deref().unwrap_or("-"));
            println!("  Avatar URL:   {}", p.avatar_url.as_deref().unwrap_or("-"));
            println!("  Bio:          {}", p.bio.as_deref().unwrap_or("-"));
            println!("  Created At:   {}", p.created_at);
        }
    } else {
        println!("No profile found for {}", address);
    }
    Ok(())
}

async fn join_subnet(
    validator: &str,
    address: &str,
    subnet_id: &str,
    key_file: &str,
) -> Result<()> {
    let timestamp = now_millis();
    let message = format!("join_subnet:{}:{}:{}", address, subnet_id, timestamp);
    let (signature, public_key) = sign_message(key_file, &message)?;

    let client = HttpUserClient::new(validator);
    let resp = client
        .join_subnet(JoinSubnetRequest {
            address: address.to_string(),
            subnet_id: subnet_id.to_string(),
            signature,
            message,
            timestamp,
            public_key: Some(public_key),
            nostr_pubkey: None,
        })
        .await?;

    if resp.success {
        println!("✓ Joined subnet '{}'", subnet_id);
        if let Some(eid) = resp.event_id {
            println!("  Event ID: {}", eid);
        }
    } else {
        println!("✗ Failed: {}", resp.message);
    }
    Ok(())
}

async fn leave_subnet(
    validator: &str,
    address: &str,
    subnet_id: &str,
    key_file: &str,
) -> Result<()> {
    let timestamp = now_millis();
    let message = format!("leave_subnet:{}:{}:{}", address, subnet_id, timestamp);
    let (signature, public_key) = sign_message(key_file, &message)?;

    let client = HttpUserClient::new(validator);
    let resp = client
        .leave_subnet(LeaveSubnetRequest {
            address: address.to_string(),
            subnet_id: subnet_id.to_string(),
            signature,
            message,
            timestamp,
            public_key: Some(public_key),
            nostr_pubkey: None,
        })
        .await?;

    if resp.success {
        println!("✓ Left subnet '{}'", subnet_id);
        if let Some(eid) = resp.event_id {
            println!("  Event ID: {}", eid);
        }
    } else {
        println!("✗ Failed: {}", resp.message);
    }
    Ok(())
}

async fn check_membership(validator: &str, address: &str, subnet_id: &str) -> Result<()> {
    let client = HttpUserClient::new(validator);
    let resp = client.check_membership(address, subnet_id).await?;

    if resp.is_member {
        println!("✓ {} is a member of '{}'", address, subnet_id);
        if let Some(ts) = resp.joined_at {
            println!("  Joined At: {}", ts);
        }
    } else {
        println!("✗ {} is NOT a member of '{}'", address, subnet_id);
    }
    Ok(())
}

async fn list_subnets(validator: &str, address: &str) -> Result<()> {
    let client = HttpUserClient::new(validator);
    let resp = client.get_user_subnets(address).await?;

    if resp.subnets.is_empty() {
        println!("No subnet memberships for {}", address);
    } else {
        println!("Subnets for {} ({}):", address, resp.subnets.len());
        for sid in &resp.subnets {
            println!("  - {}", sid);
        }
    }
    Ok(())
}
