#[path = "support/sui_example_utils.rs"]
mod sui_example_utils;

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, InMemoryStateStore, RuntimeExecutor, StateStore, SuiVmArg,
};
use setu_types::deterministic_coin_id;
use sui_example_utils::{
    create_temp_package_with_contract, execute_published_contract_scenario,
    publish_contract as publish_contract_tx, ExampleState, PublishedProgramCallSpec,
};

const MODULE_NAME: &str = "published_coin";
const EXECUTOR_ID: &str = "sui_contract_publish_e2e";

const CONTRACT: &str = r#"module published_coin::published_coin {
    use sui::coin::{Self, Coin, TreasuryCap};
    use sui::tx_context::{Self, TxContext};
    use sui::transfer;
    use std::option;

    public struct PUBLISHED_COIN has drop {}

    fun init(witness: PUBLISHED_COIN, ctx: &mut TxContext) {
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,
            b"PUB",
            b"Published Coin",
            b"Published through Setu runtime",
            option::none(),
            ctx,
        );

        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        transfer::public_freeze_object(metadata);
    }

    fun mint_to(
        treasury_cap: &mut TreasuryCap<PUBLISHED_COIN>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(treasury_cap, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }

    public entry fun issue_to(
        treasury_cap: &mut TreasuryCap<PUBLISHED_COIN>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        mint_to(treasury_cap, amount, recipient, ctx);
    }

    fun burn_inner(
        treasury_cap: &mut TreasuryCap<PUBLISHED_COIN>,
        coin: Coin<PUBLISHED_COIN>,
    ) {
        coin::burn(treasury_cap, coin);
    }

    public entry fun burn_from(
        treasury_cap: &mut TreasuryCap<PUBLISHED_COIN>,
        coin: Coin<PUBLISHED_COIN>,
    ) {
        burn_inner(treasury_cap, coin);
    }
}"#;

fn prepare_initial_state() -> Result<(ExampleState<InMemoryStateStore>, String)> {
    let pkg = create_temp_package_with_contract("published_coin", "published_coin.move", CONTRACT)?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, MODULE_NAME)
        .context("Failed to compile and disassemble published_coin package")?;
    println!("Compiled + disassembled module: {}", MODULE_NAME);

    Ok((
        ExampleState::new(RuntimeExecutor::new(InMemoryStateStore::new())),
        disassembly,
    ))
}

fn publish_contract(
    state: ExampleState<InMemoryStateStore>,
    disassembly: String,
) -> Result<ExampleState<InMemoryStateStore>> {
    let publisher = setu_types::Address::from_str_id("publisher");
    let state = publish_contract_tx(state, publisher, MODULE_NAME, disassembly, 1, EXECUTOR_ID)?;

    let published = state
        .executor
        .state()
        .get_published_contract(MODULE_NAME)?
        .context("published contract missing from runtime state")?;
    println!(
        "Published {} with {} instruction lines",
        published.module_name,
        published.disassembly.lines().count()
    );

    Ok(state)
}

fn prepare_contract_calls() -> Vec<PublishedProgramCallSpec> {
    let publisher = setu_types::Address::from_str_id("publisher");
    let alice = setu_types::Address::from_str_id("alice");
    let bob = setu_types::Address::from_str_id("bob");

    vec![
        PublishedProgramCallSpec {
            sender: publisher,
            module_name: MODULE_NAME.to_string(),
            function_name: "issue_to".to_string(),
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(77),
                SuiVmArg::Address(alice),
                SuiVmArg::Opaque,
            ],
            timestamp: 2,
            executor_id: EXECUTOR_ID.to_string(),
        },
        PublishedProgramCallSpec {
            sender: publisher,
            module_name: MODULE_NAME.to_string(),
            function_name: "issue_to".to_string(),
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(23),
                SuiVmArg::Address(bob),
                SuiVmArg::Opaque,
            ],
            timestamp: 3,
            executor_id: EXECUTOR_ID.to_string(),
        },
        PublishedProgramCallSpec {
            sender: publisher,
            module_name: MODULE_NAME.to_string(),
            function_name: "burn_from".to_string(),
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::ObjectId(deterministic_coin_id(&bob, "PUBLISHED_COIN")),
            ],
            timestamp: 4,
            executor_id: EXECUTOR_ID.to_string(),
        },
    ]
}

fn execute_contract(
    state: ExampleState<InMemoryStateStore>,
) -> Result<ExampleState<InMemoryStateStore>> {
    let calls = prepare_contract_calls();
    execute_published_contract_scenario(state, &calls)
}

fn assert_state(state: &ExampleState<InMemoryStateStore>) -> Result<()> {
    let coin_type = "PUBLISHED_COIN";
    let alice_coin = state
        .executor
        .state()
        .get_object(&deterministic_coin_id(
            &setu_types::Address::from_str_id("alice"),
            coin_type,
        ))?
        .context("alice coin missing after published contract execution")?;
    if alice_coin.data.balance.value() != 77 {
        bail!(
            "expected alice balance 77, got {}",
            alice_coin.data.balance.value()
        );
    }

    let bob_coin_id = deterministic_coin_id(&setu_types::Address::from_str_id("bob"), coin_type);
    if state.executor.state().get_object(&bob_coin_id)?.is_some() {
        bail!("bob coin should have been burned by published contract execution");
    }

    println!(
        "Final published-contract state: alice = {}, bob coin burned",
        alice_coin.data.balance.value()
    );
    println!("\nSui publish -> stored instructions -> RuntimeExecutor execution completed.");
    Ok(())
}

fn main() -> Result<()> {
    let (state, disassembly) = prepare_initial_state()?;
    let state = publish_contract(state, disassembly)?;
    let state = execute_contract(state)?;
    assert_state(&state)
}
