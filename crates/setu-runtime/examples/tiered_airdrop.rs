#[path = "support/sui_example_utils.rs"]
mod sui_example_utils;

use anyhow::{Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, InMemoryStateStore, RuntimeExecutor, SuiVmArg,
};
use setu_types::Address;
use sui_example_utils::{
    create_temp_package_with_contract, execute_program_calls, expect_coin_balance, ProgramCall,
};

const CONTRACT: &str = r#"module tiered_airdrop::tiered_airdrop {
    use sui::coin::{Self, TreasuryCap};
    use sui::tx_context::{Self, TxContext};
    use sui::transfer;
    use std::option;

    public struct TIERED_AIRDROP has drop {}

    fun init(witness: TIERED_AIRDROP, ctx: &mut TxContext) {
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,
            b"AIR",
            b"Tiered Airdrop",
            b"Campaign rewards",
            option::none(),
            ctx,
        );

        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        transfer::public_freeze_object(metadata);
    }

    fun mint_to(
        treasury_cap: &mut TreasuryCap<TIERED_AIRDROP>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(treasury_cap, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }

    fun base_reward(
        treasury_cap: &mut TreasuryCap<TIERED_AIRDROP>,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        mint_to(treasury_cap, 50, recipient, ctx);
    }

    fun vip_bonus(
        treasury_cap: &mut TreasuryCap<TIERED_AIRDROP>,
        recipient: address,
        is_vip: bool,
        ctx: &mut TxContext,
    ) {
        if (is_vip) {
            mint_to(treasury_cap, 25, recipient, ctx);
        };
    }

    fun streak_bonus(
        treasury_cap: &mut TreasuryCap<TIERED_AIRDROP>,
        recipient: address,
        has_streak: bool,
        ctx: &mut TxContext,
    ) {
        if (has_streak) {
            mint_to(treasury_cap, 10, recipient, ctx);
        };
    }

    public entry fun distribute_campaign_rewards(
        treasury_cap: &mut TreasuryCap<TIERED_AIRDROP>,
        recipient: address,
        is_vip: bool,
        has_streak: bool,
        ctx: &mut TxContext,
    ) {
        base_reward(treasury_cap, recipient, ctx);
        vip_bonus(treasury_cap, recipient, is_vip, ctx);
        streak_bonus(treasury_cap, recipient, has_streak, ctx);
    }
}"#;

struct TieredAirdropExample {
    executor: RuntimeExecutor<InMemoryStateStore>,
    sender: Address,
    bob: Address,
    carol: Address,
    dave: Address,
    disassembly: String,
}

fn setup_state() -> Result<TieredAirdropExample> {
    let pkg = create_temp_package_with_contract("tiered_airdrop", "tiered_airdrop.move", CONTRACT)?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "tiered_airdrop")
        .context("Failed to compile tiered_airdrop package")?;
    println!("Compiled + disassembled module: tiered_airdrop");

    Ok(TieredAirdropExample {
        executor: RuntimeExecutor::new(InMemoryStateStore::new()),
        sender: Address::from_str_id("campaign_owner"),
        bob: Address::from_str_id("bob"),
        carol: Address::from_str_id("carol"),
        dave: Address::from_str_id("dave"),
        disassembly,
    })
}

fn execute_scenario(example: &mut TieredAirdropExample) -> Result<()> {
    let calls = [
        ProgramCall {
            function_name: "distribute_campaign_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::Address(example.bob),
                SuiVmArg::Bool(true),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
            timestamp: 1,
        },
        ProgramCall {
            function_name: "distribute_campaign_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::Address(example.carol),
                SuiVmArg::Bool(false),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
            timestamp: 2,
        },
        ProgramCall {
            function_name: "distribute_campaign_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::Address(example.dave),
                SuiVmArg::Bool(false),
                SuiVmArg::Bool(false),
                SuiVmArg::Opaque,
            ],
            timestamp: 3,
        },
        ProgramCall {
            function_name: "distribute_campaign_rewards",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::Address(example.bob),
                SuiVmArg::Bool(false),
                SuiVmArg::Bool(true),
                SuiVmArg::Opaque,
            ],
            timestamp: 4,
        },
    ];

    execute_program_calls(
        &mut example.executor,
        &example.sender,
        &example.disassembly,
        "tiered_airdrop",
        &calls,
    )
}

fn assert_state(example: &TieredAirdropExample) -> Result<()> {
    let coin_type = "TIERED_AIRDROP";
    let bob_coin = expect_coin_balance(
        example.executor.state(),
        &example.bob,
        coin_type,
        145,
        "bob reward",
    )?;
    let carol_coin = expect_coin_balance(
        example.executor.state(),
        &example.carol,
        coin_type,
        60,
        "carol reward",
    )?;
    let dave_coin = expect_coin_balance(
        example.executor.state(),
        &example.dave,
        coin_type,
        50,
        "dave reward",
    )?;

    println!("Bob reward total   = {}", bob_coin.data.balance.value());
    println!("Carol reward total = {}", carol_coin.data.balance.value());
    println!("Dave reward total  = {}", dave_coin.data.balance.value());
    println!("\nTiered airdrop example completed.");
    Ok(())
}

fn main() -> Result<()> {
    let mut example = setup_state()?;
    execute_scenario(&mut example)?;
    assert_state(&example)
}
