use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, execute_sui_entry_from_disassembly, InMemoryStateStore,
    StateStore, SuiVmArg,
};
use setu_types::{deterministic_coin_id, Address, Object, ObjectId};

// Lightning contract + a VM-subset-compatible entry used for direct Sui subset execution.
const CONTRACT: &str = r#"module lightning_pkg::lightning {
    use sui::object::{Self, ID, UID};
    use sui::transfer;
    use sui::tx_context::{Self, TxContext};
    use sui::coin::{Self, Coin};
    use sui::balance::Balance;
    use sui::sui::SUI;
    use sui::event;
    use std::option::{Self, Option};
    use sui::table::{Self, Table};
    use sui::ecdsa_k1;
    use sui::bcs;
    use std::hash;
    use std::vector;

    const EInvalidSignature: u64 = 0;
    const EInvalidStateNum: u64 = 1;
    const EChannelNotOpen: u64 = 2;
    const EInsufficientBalance: u64 = 3;
    const EInvalidPreimage: u64 = 4;
    const ENotExpired: u64 = 5;
    const EInvalidHash: u64 = 6;

    public struct Channel has key {
        id: UID,
        party_a: address,
        party_b: address,
        balance_a: u64,
        balance_b: u64,
        funding_balance: Balance<SUI>,
        pubkey_a: vector<u8>,
        pubkey_b: vector<u8>,
        status: u8,
        state_num: u64,
        to_self_delay: u64,
        close_epoch: u64,
        htlcs: Table<u64, HTLC>,
        revocation_key: Option<vector<u8>>,
        revocation_hash: vector<u8>,
    }

    public struct HTLC has store, drop {
        htlc_id: u64,
        amount: u64,
        payment_hash: vector<u8>,
        expiry: u64,
        direction: u8,
        status: u8,
    }

    public struct ChannelOpenEvent has copy, drop {
        channel_id: ID,
        party_a: address,
        party_b: address,
        capacity: u64,
    }

    public struct ChannelSpendEvent has copy, drop {
        channel_id: ID,
        htlc_id: u64,
        spend_type: u8,
    }

    public fun open_channel(
        funding_coin: &mut Coin<SUI>,
        amount: u64,
        pubkey_a: vector<u8>,
        pubkey_b: vector<u8>,
        party_b: address,
        to_self_delay: u64,
        ctx: &mut TxContext
    ) {
        let party_a = tx_context::sender(ctx);
        let split_coin = coin::split(funding_coin, amount, ctx);
        let capacity = amount;

        let channel = Channel {
            id: object::new(ctx),
            party_a,
            party_b,
            balance_a: capacity,
            balance_b: 0,
            funding_balance: coin::into_balance(split_coin),
            pubkey_a,
            pubkey_b,
            status: 0,
            state_num: 0,
            to_self_delay,
            close_epoch: 0,
            htlcs: table::new(ctx),
            revocation_key: option::none(),
            revocation_hash: vector::empty<u8>(),
        };

        event::emit(ChannelOpenEvent {
            channel_id: object::id(&channel),
            party_a,
            party_b,
            capacity,
        });

        transfer::share_object(channel);
    }

    public fun close_channel(
        channel: &mut Channel,
        state_num: u64,
        balance_a: u64,
        balance_b: u64,
        _sig_a: vector<u8>,
        _sig_b: vector<u8>,
        _ctx: &mut TxContext
    ) {
        assert!(channel.status == 0, EChannelNotOpen);
        channel.balance_a = balance_a;
        channel.balance_b = balance_b;
        channel.state_num = state_num;
        channel.status = 2;

        event::emit(ChannelSpendEvent {
            channel_id: object::id(channel),
            htlc_id: 0,
            spend_type: 0,
        });
    }

    public fun force_close(
        channel: &mut Channel,
        state_num: u64,
        revocation_hash: vector<u8>,
        commitment_sig: vector<u8>,
        ctx: &mut TxContext
    ) {
        assert!(channel.status == 0, EChannelNotOpen);
        assert!(state_num >= channel.state_num, EInvalidStateNum);

        let payload = object::id_to_bytes(&object::id(channel));
        let num_bytes = bcs::to_bytes(&state_num);
        let mut mut_payload = payload;
        vector::append(&mut mut_payload, num_bytes);
        vector::append(&mut mut_payload, revocation_hash);

        assert!(ecdsa_k1::secp256k1_verify(&commitment_sig, &channel.pubkey_b, &mut_payload, 1), EInvalidSignature);

        channel.status = 1;
        channel.close_epoch = tx_context::epoch(ctx);
        channel.revocation_hash = revocation_hash;

        event::emit(ChannelSpendEvent {
            channel_id: object::id(channel),
            htlc_id: 0,
            spend_type: 1,
        });
    }

    public fun htlc_claim(
        channel: &mut Channel,
        htlc_id: u64,
        preimage: vector<u8>,
        _ctx: &mut TxContext
    ) {
        let htlc = table::borrow_mut(&mut channel.htlcs, htlc_id);
        assert!(htlc.status == 0, 0);

        let hash = hash::sha2_256(preimage);
        assert!(hash == htlc.payment_hash, EInvalidPreimage);

        htlc.status = 1;

        if (htlc.direction == 0) {
            channel.balance_a = channel.balance_a - htlc.amount;
            channel.balance_b = channel.balance_b + htlc.amount;
        } else {
            channel.balance_b = channel.balance_b - htlc.amount;
            channel.balance_a = channel.balance_a + htlc.amount;
        };

        event::emit(ChannelSpendEvent {
            channel_id: object::id(channel),
            htlc_id,
            spend_type: 2,
        });
    }

    public fun htlc_timeout(
        channel: &mut Channel,
        htlc_id: u64,
        ctx: &mut TxContext
    ) {
        let htlc = table::borrow_mut(&mut channel.htlcs, htlc_id);
        assert!(htlc.status == 0, 0);
        assert!(tx_context::epoch(ctx) >= htlc.expiry, ENotExpired);

        htlc.status = 2;

        event::emit(ChannelSpendEvent {
            channel_id: object::id(channel),
            htlc_id,
            spend_type: 3,
        });
    }

    public fun penalize(
        channel: &mut Channel,
        revocation_secret: vector<u8>,
        _ctx: &mut TxContext
    ) {
        let actual_hash = hash::sha2_256(revocation_secret);
        assert!(actual_hash == channel.revocation_hash, EInvalidHash);

        channel.balance_a = 0;
        channel.balance_b = channel.balance_a + channel.balance_b;
        channel.status = 2;

        event::emit(ChannelSpendEvent {
            channel_id: object::id(channel),
            htlc_id: 0,
            spend_type: 4,
        });
    }

    // New-paradigm compatibility entry:
    // uses only the currently implemented direct Sui subset VM opcodes/calls.
    public entry fun vm_subset_branch_transfer(
        coin: Coin<SUI>,
        recipient_true: address,
        recipient_false: address,
        should_transfer: bool,
    ) {
        if (should_transfer) {
            transfer::public_transfer(coin, recipient_true);
        } else {
            transfer::public_transfer(coin, recipient_false);
        };
    }
}"#;

fn main() -> Result<()> {
    let pkg = create_temp_package_with_contract()?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "lightning")
        .context("Failed to compile/disassemble lightning contract")?;
    println!("Compiled + disassembled module: lightning");

    // Validate target lightning functions are present.
    require_contains(&disassembly, "public open_channel(")?;
    require_contains(&disassembly, "public close_channel(")?;
    require_contains(&disassembly, "public force_close(")?;
    require_contains(&disassembly, "public htlc_claim(")?;
    require_contains(&disassembly, "public htlc_timeout(")?;
    require_contains(&disassembly, "public penalize(")?;
    require_contains(&disassembly, "entry public vm_subset_branch_transfer(")?;
    println!("Detected lightning functions and subset-compatible entry in disassembly");

    let mut state = InMemoryStateStore::new();
    let alice = Address::from_str_id("alice");
    let bob = Address::from_str_id("bob");
    let carol = Address::from_str_id("carol");

    // Case 1: should_transfer=true -> transfer to bob
    let coin_true_id = deterministic_coin_id(&alice, "SUI");
    let coin_true = Object::new_owned(
        coin_true_id,
        alice,
        setu_types::CoinData {
            coin_type: setu_types::CoinType::new("SUI"),
            balance: setu_types::Balance::new(300),
        },
    );
    state.set_object(coin_true_id, coin_true)?;

    execute_sui_entry_from_disassembly(
        &mut state,
        &alice,
        &disassembly,
        "vm_subset_branch_transfer",
        &[
            SuiVmArg::ObjectId(coin_true_id),
            SuiVmArg::Address(bob),
            SuiVmArg::Address(carol),
            SuiVmArg::Bool(true),
        ],
    )?;

    let bob_coin_id = deterministic_coin_id(&bob, "SUI");
    let bob_coin = state
        .get_object(&bob_coin_id)?
        .context("bob coin missing after true branch")?;
    if bob_coin.data.balance.value() != 300 {
        bail!(
            "expected bob balance 300 after true branch, got {}",
            bob_coin.data.balance.value()
        );
    }
    if state.get_object(&coin_true_id)?.is_some() {
        bail!("source coin was not consumed in true branch");
    }

    // Case 2: should_transfer=false -> transfer to carol
    let coin_false_id = ObjectId::new([0x22; 32]);
    let coin_false = Object::new_owned(
        coin_false_id,
        alice,
        setu_types::CoinData {
            coin_type: setu_types::CoinType::new("SUI"),
            balance: setu_types::Balance::new(180),
        },
    );
    state.set_object(coin_false_id, coin_false)?;

    execute_sui_entry_from_disassembly(
        &mut state,
        &alice,
        &disassembly,
        "vm_subset_branch_transfer",
        &[
            SuiVmArg::ObjectId(coin_false_id),
            SuiVmArg::Address(bob),
            SuiVmArg::Address(carol),
            SuiVmArg::Bool(false),
        ],
    )?;

    let carol_coin_id = deterministic_coin_id(&carol, "SUI");
    let carol_coin = state
        .get_object(&carol_coin_id)?
        .context("carol coin missing after false branch")?;
    if carol_coin.data.balance.value() != 180 {
        bail!(
            "expected carol balance 180 after false branch, got {}",
            carol_coin.data.balance.value()
        );
    }
    if state.get_object(&coin_false_id)?.is_some() {
        bail!("source coin was not consumed in false branch");
    }

    println!(
        "Direct Sui subset VM on lightning module succeeded: bob={}, carol={}",
        bob_coin.data.balance.value(),
        carol_coin.data.balance.value()
    );
    println!("\nLightning E2E (compile + direct Sui subset VM execution) completed.");

    Ok(())
}

fn create_temp_package_with_contract() -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let root = std::env::temp_dir().join(format!("setu_sui_lightning_e2e_{}", ts));
    fs::create_dir_all(&root)?;

    let status = Command::new("sui")
        .arg("move")
        .arg("new")
        .arg("lightning_pkg")
        .current_dir(&root)
        .status()
        .context("Failed to execute `sui move new`")?;
    if !status.success() {
        bail!("`sui move new` failed with status {}", status);
    }

    let pkg = root.join("lightning_pkg");
    let src = pkg.join("sources");
    let default_module = src.join("lightning_pkg.move");
    if default_module.exists() {
        fs::remove_file(default_module)?;
    }
    fs::write(src.join("lightning.move"), CONTRACT)?;

    Ok(pkg)
}

fn require_contains(text: &str, needle: &str) -> Result<()> {
    if !text.contains(needle) {
        bail!("Disassembly missing expected pattern: {}", needle);
    }
    Ok(())
}
