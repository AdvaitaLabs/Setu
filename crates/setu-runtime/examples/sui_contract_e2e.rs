use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, execute_sui_entry_from_disassembly, InMemoryStateStore,
    StateStore, SuiVmArg,
};
use setu_types::{deterministic_coin_id, Address};

const CONTRACT: &str = r#"module my_coin_pkg::my_coin {
    use sui::coin::{Self, Coin, TreasuryCap};
    use sui::tx_context::{Self, TxContext};
    use sui::transfer;
    use std::option;

    public struct MY_COIN has drop {}

    fun init(witness: MY_COIN, ctx: &mut TxContext) {
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,
            b"MYC",
            b"My Coin",
            b"An example Sui coin",
            option::none(),
            ctx,
        );

        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        transfer::public_freeze_object(metadata);
    }

    public entry fun mint(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(treasury_cap, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }

    public entry fun conditional_transfer(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        amount: u64,
        recipient: address,
        should_transfer: bool,
        ctx: &mut TxContext,
    ) {
        if (should_transfer) {
            let coin = coin::mint(treasury_cap, amount, ctx);
            transfer::public_transfer(coin, recipient);
        };
    }

    public entry fun burn(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        coin: Coin<MY_COIN>,
    ) {
        coin::burn(treasury_cap, coin);
    }
}"#;

fn main() -> Result<()> {
    let pkg = create_temp_package_with_contract()?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "my_coin")
        .context("Failed to compile and disassemble Sui contract")?;
    println!("Compiled + disassembled module: my_coin");

    let mut state = InMemoryStateStore::new();
    let sender = Address::from_str_id("alice");
    let recipient = Address::from_str_id("bob");
    let coin_type = "MY_COIN";

    // 1) Execute `mint` directly from Sui disassembly subset VM
    execute_sui_entry_from_disassembly(
        &mut state,
        &sender,
        &disassembly,
        "mint",
        &[
            SuiVmArg::Opaque,
            SuiVmArg::U64(100),
            SuiVmArg::Address(sender),
            SuiVmArg::Opaque,
        ],
    )?;

    let alice_coin_id = deterministic_coin_id(&sender, coin_type);
    let alice_coin = state
        .get_object(&alice_coin_id)?
        .context("alice coin missing after mint")?;
    println!(
        "After mint, alice balance = {}",
        alice_coin.data.balance.value()
    );

    // 2) Execute conditional transfer with should_transfer=true
    execute_sui_entry_from_disassembly(
        &mut state,
        &sender,
        &disassembly,
        "conditional_transfer",
        &[
            SuiVmArg::Opaque,
            SuiVmArg::U64(40),
            SuiVmArg::Address(recipient),
            SuiVmArg::Bool(true),
            SuiVmArg::Opaque,
        ],
    )?;

    let bob_coin_id = deterministic_coin_id(&recipient, coin_type);
    let bob_coin = state
        .get_object(&bob_coin_id)?
        .context("bob coin missing after conditional transfer=true")?;
    println!(
        "After conditional_transfer(true), bob balance = {}",
        bob_coin.data.balance.value()
    );

    // 3) Execute conditional transfer with should_transfer=false (no-op)
    execute_sui_entry_from_disassembly(
        &mut state,
        &sender,
        &disassembly,
        "conditional_transfer",
        &[
            SuiVmArg::Opaque,
            SuiVmArg::U64(55),
            SuiVmArg::Address(recipient),
            SuiVmArg::Bool(false),
            SuiVmArg::Opaque,
        ],
    )?;

    let bob_after_false = state
        .get_object(&bob_coin_id)?
        .context("bob coin missing after conditional transfer=false")?;
    if bob_after_false.data.balance.value() != 40 {
        bail!(
            "conditional_transfer(false) should be no-op, got bob balance {}",
            bob_after_false.data.balance.value()
        );
    }
    println!(
        "After conditional_transfer(false), bob balance remains {}",
        bob_after_false.data.balance.value()
    );

    // 4) Execute burn directly from Sui disassembly subset VM
    execute_sui_entry_from_disassembly(
        &mut state,
        &sender,
        &disassembly,
        "burn",
        &[SuiVmArg::Opaque, SuiVmArg::ObjectId(bob_coin_id)],
    )?;

    let post_burn = state.get_object(&bob_coin_id)?;
    if post_burn.is_some() {
        bail!("Burn failed: bob coin still exists");
    }
    println!("After burn, bob coin deleted");

    println!("\nE2E compile -> disassemble -> direct Sui subset VM execution completed.");
    Ok(())
}

fn create_temp_package_with_contract() -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let root = std::env::temp_dir().join(format!("setu_sui_e2e_{}", ts));
    fs::create_dir_all(&root)?;

    let status = Command::new("sui")
        .arg("move")
        .arg("new")
        .arg("my_coin_pkg")
        .current_dir(&root)
        .status()
        .context("Failed to execute `sui move new`")?;
    if !status.success() {
        bail!("`sui move new` failed with status {}", status);
    }

    let pkg = root.join("my_coin_pkg");
    let src = pkg.join("sources");
    let default_module = src.join("my_coin_pkg.move");
    if default_module.exists() {
        fs::remove_file(default_module)?;
    }
    fs::write(src.join("my_coin.move"), CONTRACT)?;

    Ok(pkg)
}
