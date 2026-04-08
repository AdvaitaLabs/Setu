#[path = "support/sui_example_utils.rs"]
mod sui_example_utils;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, InMemoryStateStore, RuntimeExecutor, StateStore, SuiVmArg,
};
use setu_types::{deterministic_coin_id, Address};
use sui_example_utils::{execute_program_calls, ProgramCall};

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

    fun mint_to(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(treasury_cap, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }

    fun maybe_mint_to(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        amount: u64,
        recipient: address,
        should_transfer: bool,
        ctx: &mut TxContext,
    ) {
        if (should_transfer) {
            mint_to(treasury_cap, amount, recipient, ctx);
        };
    }

    fun reward_pair(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        primary_amount: u64,
        primary_recipient: address,
        secondary_amount: u64,
        secondary_recipient: address,
        mint_secondary: bool,
        ctx: &mut TxContext,
    ) {
        mint_to(treasury_cap, primary_amount, primary_recipient, ctx);
        maybe_mint_to(
            treasury_cap,
            secondary_amount,
            secondary_recipient,
            mint_secondary,
            ctx,
        );
    }

    public entry fun orchestrate_rewards(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        primary_amount: u64,
        primary_recipient: address,
        secondary_amount: u64,
        secondary_recipient: address,
        mint_secondary: bool,
        ctx: &mut TxContext,
    ) {
        reward_pair(
            treasury_cap,
            primary_amount,
            primary_recipient,
            secondary_amount,
            secondary_recipient,
            mint_secondary,
            ctx,
        );
    }

    fun burn_inner(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        coin: Coin<MY_COIN>,
    ) {
        coin::burn(treasury_cap, coin);
    }

    public entry fun burn_via_helper(
        treasury_cap: &mut TreasuryCap<MY_COIN>,
        coin: Coin<MY_COIN>,
    ) {
        burn_inner(treasury_cap, coin);
    }
}"#;

struct SuiContractExample {
    executor: RuntimeExecutor<InMemoryStateStore>,
    sender: Address,
    recipient: Address,
    third_recipient: Address,
    disassembly: String,
}

fn setup_state() -> Result<SuiContractExample> {
    let pkg = create_temp_package_with_contract()?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "my_coin")
        .context("Failed to compile and disassemble Sui contract")?;
    println!("Compiled + disassembled module: my_coin");

    Ok(SuiContractExample {
        executor: RuntimeExecutor::new(InMemoryStateStore::new()),
        sender: Address::from_str_id("alice"),
        recipient: Address::from_str_id("bob"),
        third_recipient: Address::from_str_id("carol"),
        disassembly,
    })
}

fn execute_scenario(example: &mut SuiContractExample) -> Result<()> {
    let calls = [
        ProgramCall {
            function_name: "orchestrate_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(100),
                SuiVmArg::Address(example.sender),
                SuiVmArg::U64(40),
                SuiVmArg::Address(example.recipient),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
            timestamp: 2,
        },
        ProgramCall {
            function_name: "orchestrate_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(10),
                SuiVmArg::Address(example.sender),
                SuiVmArg::U64(55),
                SuiVmArg::Address(example.recipient),
                SuiVmArg::Bool(false),
                SuiVmArg::Opaque,
            ],
            timestamp: 3,
        },
        ProgramCall {
            function_name: "orchestrate_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(15),
                SuiVmArg::Address(example.recipient),
                SuiVmArg::U64(5),
                SuiVmArg::Address(example.third_recipient),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
            timestamp: 4,
        },
        ProgramCall {
            function_name: "burn_via_helper",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::ObjectId(deterministic_coin_id(&example.recipient, "MY_COIN")),
            ],
            timestamp: 5,
        },
    ];

    execute_program_calls(
        &mut example.executor,
        &example.sender,
        &example.disassembly,
        "sui_contract_e2e",
        &calls,
    )
}

fn assert_state(example: &SuiContractExample) -> Result<()> {
    let coin_type = "MY_COIN";
    let alice_coin_id = deterministic_coin_id(&example.sender, coin_type);
    let alice_coin = example
        .executor
        .state()
        .get_object(&alice_coin_id)?
        .context("alice coin missing after orchestrate_rewards")?;
    if alice_coin.data.balance.value() != 110 {
        bail!(
            "expected alice balance 110 after helper flows, got {}",
            alice_coin.data.balance.value()
        );
    }

    let bob_coin_id = deterministic_coin_id(&example.recipient, coin_type);
    let bob_after_burn = example.executor.state().get_object(&bob_coin_id)?;
    if bob_after_burn.is_some() {
        bail!("Burn failed: bob coin still exists");
    }

    let carol_coin_id = deterministic_coin_id(&example.third_recipient, coin_type);
    let carol_coin = example
        .executor
        .state()
        .get_object(&carol_coin_id)?
        .context("carol coin missing after top-up flow")?;
    if carol_coin.data.balance.value() != 5 {
        bail!(
            "expected carol balance 5 after top-up flow, got {}",
            carol_coin.data.balance.value()
        );
    }

    println!(
        "Final balances: alice = {}, carol = {}, bob coin burned",
        alice_coin.data.balance.value(),
        carol_coin.data.balance.value()
    );
    println!("\nE2E compile -> disassemble -> RuntimeExecutor execution completed.");
    Ok(())
}

fn main() -> Result<()> {
    let mut example = setup_state()?;
    execute_scenario(&mut example)?;
    assert_state(&example)
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
