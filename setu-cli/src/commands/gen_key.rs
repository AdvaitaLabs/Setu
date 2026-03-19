// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! `setu gen-key` — Unified key generation tool chain.
//!
//! Generates, recovers, inspects, and exports Setu keypairs for any role
//! (validator, solver, or generic account).

use anyhow::{Context, Result};
use colored::Colorize;
use setu_keys::{
    SignatureScheme, SetuKeyPair,
    generate_new_key,
    key_derive::{derive_key_pair_from_mnemonic, WordCount},
    keypair_file::{write_keypair_to_file, read_keypair_from_file},
};
use std::path::Path;

use crate::GenKeyAction;

// ─── handlers ───────────────────────────────────────────────────────────────

pub async fn handle(action: GenKeyAction) -> Result<()> {
    match action {
        GenKeyAction::Generate {
            scheme,
            words,
            output,
            json,
        } => handle_generate(&scheme, words, output.as_deref(), json),

        GenKeyAction::Recover {
            mnemonic,
            scheme,
            output,
            json,
        } => handle_recover(&mnemonic, &scheme, output.as_deref(), json),

        GenKeyAction::Inspect { file } => handle_inspect(&file),

        GenKeyAction::Export {
            file,
            format,
        } => handle_export(&file, &format),
    }
}

// ─── generate ───────────────────────────────────────────────────────────────

fn handle_generate(
    scheme_str: &str,
    words: u8,
    output: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let scheme: SignatureScheme = scheme_str
        .parse()
        .context("Invalid signature scheme. Use: ed25519, secp256k1, secp256r1")?;

    let word_count: WordCount = words
        .to_string()
        .parse()
        .context("Invalid word count. Use: 12, 15, 18, 21, 24")?;

    let (address, keypair, _scheme, mnemonic) =
        generate_new_key(scheme, None, Some(word_count))
            .context("Key generation failed")?;

    // Possibly write the binary (base64) key file
    let saved_path = if let Some(path) = output {
        write_keypair_to_file(&keypair, path)
            .context("Failed to write key file")?;
        Some(path.to_string())
    } else {
        None
    };

    if json_output {
        print_json(&keypair, &address, &mnemonic, saved_path.as_deref())?;
    } else {
        print_pretty(&keypair, &address, &mnemonic, saved_path.as_deref());
    }

    Ok(())
}

// ─── recover ────────────────────────────────────────────────────────────────

fn handle_recover(
    mnemonic: &str,
    scheme_str: &str,
    output: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let scheme: SignatureScheme = scheme_str
        .parse()
        .context("Invalid signature scheme")?;

    let (address, keypair) =
        derive_key_pair_from_mnemonic(mnemonic, &scheme, None)
            .context("Failed to derive keypair from mnemonic")?;

    let saved_path = if let Some(path) = output {
        write_keypair_to_file(&keypair, path)
            .context("Failed to write key file")?;
        Some(path.to_string())
    } else {
        None
    };

    if json_output {
        print_json(&keypair, &address, mnemonic, saved_path.as_deref())?;
    } else {
        print_pretty(&keypair, &address, mnemonic, saved_path.as_deref());
    }

    Ok(())
}

// ─── inspect ────────────────────────────────────────────────────────────────

fn handle_inspect(file: &str) -> Result<()> {
    let keypair = read_keypair_from_file(file)
        .context(format!("Cannot read key file: {}", file))?;
    let address = keypair.address();
    let public = keypair.public();

    println!("{}", "Key file info".bold().cyan());
    println!("  File:        {}", file);
    println!("  Scheme:      {}", keypair.scheme());
    println!("  Address:     {}", address);
    println!("  Public key:  {}", public.encode_base64());
    Ok(())
}

// ─── export ─────────────────────────────────────────────────────────────────

fn handle_export(file: &str, format: &str) -> Result<()> {
    let keypair = read_keypair_from_file(file)
        .context(format!("Cannot read key file: {}", file))?;

    match format {
        "base64" => {
            println!("{}", keypair.encode_base64());
        }
        "hex" => {
            println!("{}", hex::encode(keypair.secret_bytes()));
        }
        "public" => {
            println!("{}", keypair.public().encode_base64());
        }
        _ => {
            anyhow::bail!(
                "Unknown format '{}'. Use: base64, hex, public",
                format
            );
        }
    }
    Ok(())
}

// ─── pretty / json printers ────────────────────────────────────────────────

fn print_pretty(
    keypair: &SetuKeyPair,
    address: &setu_keys::SetuAddress,
    mnemonic: &str,
    saved: Option<&str>,
) {
    let public = keypair.public();
    println!();
    println!("{}", "=== Setu Key Generated ===".bold().green());
    println!("  Scheme:      {}", keypair.scheme());
    println!("  Address:     {}", address);
    println!("  Public key:  {}", public.encode_base64());
    println!();
    println!("{}", "  --- sensitive ---".yellow());
    println!("  Private key (base64): {}", keypair.encode_base64());
    println!("  Mnemonic:    {}", mnemonic);
    if let Some(p) = saved {
        println!();
        println!("  {} Saved to: {}", "✓".green().bold(), p.cyan());
    }
    println!();
    println!(
        "{}",
        "  ⚠  Back up your mnemonic — it is the only way to recover this key."
            .yellow()
            .bold()
    );
}

fn print_json(
    keypair: &SetuKeyPair,
    address: &setu_keys::SetuAddress,
    mnemonic: &str,
    saved: Option<&str>,
) -> Result<()> {
    let public = keypair.public();
    let obj = serde_json::json!({
        "scheme": keypair.scheme().to_string(),
        "address": address.to_hex(),
        "public_key": public.encode_base64(),
        "private_key": keypair.encode_base64(),
        "mnemonic": mnemonic,
        "key_file": saved,
    });
    println!("{}", serde_json::to_string_pretty(&obj)?);
    Ok(())
}
