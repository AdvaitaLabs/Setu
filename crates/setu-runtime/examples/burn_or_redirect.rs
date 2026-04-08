#[path = "support/sui_example_utils.rs"]
mod sui_example_utils;

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, InMemoryStateStore, RuntimeExecutor, StateStore, SuiVmArg,
};
use setu_types::{deterministic_coin_id, Address};
use sui_example_utils::{create_temp_package_with_contract, execute_program_calls, ProgramCall};

const CONTRACT: &str = r#"module burn_or_redirect::burn_or_redirect {
    use sui::coin::{Self, Coin, TreasuryCap};
    use sui::tx_context::{Self, TxContext};
    use sui::transfer;
    use std::option;

    public struct BURN_OR_REDIRECT has drop {}

    fun init(witness: BURN_OR_REDIRECT, ctx: &mut TxContext) {
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,
            b"CLM",
            b"Claim Token",
            b"Claim resolution flow",
            option::none(),
            ctx,
        );

        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        transfer::public_freeze_object(metadata);
    }

    fun mint_to(
        treasury_cap: &mut TreasuryCap<BURN_OR_REDIRECT>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(treasury_cap, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }

    public entry fun issue_claimable(
        treasury_cap: &mut TreasuryCap<BURN_OR_REDIRECT>,
        amount: u64,
        claimant: address,
        ctx: &mut TxContext,
    ) {
        mint_to(treasury_cap, amount, claimant, ctx);
    }

    fun redirect_coin(
        coin: Coin<BURN_OR_REDIRECT>,
        fallback_recipient: address,
    ) {
        transfer::public_transfer(coin, fallback_recipient);
    }

    fun destroy_coin(
        treasury_cap: &mut TreasuryCap<BURN_OR_REDIRECT>,
        coin: Coin<BURN_OR_REDIRECT>,
    ) {
        coin::burn(treasury_cap, coin);
    }

    fun resolve_exit(
        treasury_cap: &mut TreasuryCap<BURN_OR_REDIRECT>,
        coin: Coin<BURN_OR_REDIRECT>,
        fallback_recipient: address,
        should_burn: bool,
    ) {
        if (should_burn) {
            destroy_coin(treasury_cap, coin);
        } else {
            redirect_coin(coin, fallback_recipient);
        };
    }

    public entry fun resolve_failed_claim(
        treasury_cap: &mut TreasuryCap<BURN_OR_REDIRECT>,
        coin: Coin<BURN_OR_REDIRECT>,
        fallback_recipient: address,
        should_burn: bool,
    ) {
        resolve_exit(treasury_cap, coin, fallback_recipient, should_burn);
    }
}"#;

struct BurnOrRedirectExample {
    executor: RuntimeExecutor<InMemoryStateStore>,
    sender: Address,
    bob: Address,
    carol: Address,
    dave: Address,
    eve: Address,
    disassembly: String,
}

fn setup_state() -> Result<BurnOrRedirectExample> {
    let pkg =
        create_temp_package_with_contract("burn_or_redirect", "burn_or_redirect.move", CONTRACT)?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "burn_or_redirect")
        .context("Failed to compile burn_or_redirect package")?;
    println!("Compiled + disassembled module: burn_or_redirect");

    Ok(BurnOrRedirectExample {
        executor: RuntimeExecutor::new(InMemoryStateStore::new()),
        sender: Address::from_str_id("claims_operator"),
        bob: Address::from_str_id("bob"),
        carol: Address::from_str_id("carol"),
        dave: Address::from_str_id("dave"),
        eve: Address::from_str_id("eve"),
        disassembly,
    })
}

fn execute_scenario(example: &mut BurnOrRedirectExample) -> Result<()> {
    let coin_type = "BURN_OR_REDIRECT";
    let bob_coin_id = deterministic_coin_id(&example.bob, coin_type);
    let carol_coin_id = deterministic_coin_id(&example.carol, coin_type);

    let calls = [
        ProgramCall {
            function_name: "issue_claimable",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(30),
                SuiVmArg::Address(example.bob),
                SuiVmArg::Opaque,
            ],
            timestamp: 1,
        },
        ProgramCall {
            function_name: "issue_claimable",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::U64(12),
                SuiVmArg::Address(example.carol),
                SuiVmArg::Opaque,
            ],
            timestamp: 2,
        },
        ProgramCall {
            function_name: "resolve_failed_claim",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::ObjectId(bob_coin_id),
                SuiVmArg::Address(example.dave),
                SuiVmArg::Bool(false),
            ],
            timestamp: 3,
        },
        ProgramCall {
            function_name: "resolve_failed_claim",
            args: vec![
                SuiVmArg::Opaque,
                SuiVmArg::ObjectId(carol_coin_id),
                SuiVmArg::Address(example.eve),
                SuiVmArg::Bool(true),
            ],
            timestamp: 4,
        },
    ];

    execute_program_calls(
        &mut example.executor,
        &example.sender,
        &example.disassembly,
        "burn_or_redirect",
        &calls,
    )
}

fn assert_state(example: &BurnOrRedirectExample) -> Result<()> {
    let coin_type = "BURN_OR_REDIRECT";
    let bob_coin_id = deterministic_coin_id(&example.bob, coin_type);
    let carol_coin_id = deterministic_coin_id(&example.carol, coin_type);

    if example.executor.state().get_object(&bob_coin_id)?.is_some() {
        bail!("bob claim coin should have been redirected away");
    }
    if example.executor.state().get_object(&carol_coin_id)?.is_some() {
        bail!("carol claim coin should have been burned");
    }

    let redirected_coin = example
        .executor
        .state()
        .get_object(&deterministic_coin_id(&example.dave, coin_type))?
        .context("redirected coin missing for dave")?;
    if redirected_coin.data.balance.value() != 30 {
        bail!(
            "expected redirected coin balance 30, got {}",
            redirected_coin.data.balance.value()
        );
    }

    if example
        .executor
        .state()
        .get_object(&deterministic_coin_id(&example.eve, coin_type))?
        .is_some()
    {
        bail!("eve should not receive a coin when the claim is burned");
    }

    println!(
        "Redirected claim balance for dave = {}",
        redirected_coin.data.balance.value()
    );
    println!("Carol claim coin was burned and eve received nothing");
    println!("\nBurn-or-redirect example completed.");
    Ok(())
}

fn main() -> Result<()> {
    let mut example = setup_state()?;
    execute_scenario(&mut example)?;
    assert_state(&example)
}
