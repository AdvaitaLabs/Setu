// ===== examples/move/custom_token/sources/custom_token.move =====
// Custom fungible token — demonstrates how to create and manage
// a new token type on Setu using the Coin framework.
//
// Demonstrates:
//   - Defining a new token type (GOLD)
//   - Creating a TreasuryCap (mint authority)
//   - Minting, transferring, splitting, and joining coins
//   - Burning coins
//
// Deploy:
//   POST /api/v1/move/publish { "sender": "alice", "modules": ["<hex bytecode>"] }
//
// Usage:
//   1. Initialize: call `init` to create the TreasuryCap
//   2. Mint:       call `mint` with the TreasuryCap object
//   3. Transfer:   call `send` to transfer GOLD coins
module examples::custom_token {
    use setu::object::UID;
    use setu::coin::{Self, Coin, TreasuryCap};
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// One-Time Witness for the GOLD token type.
    /// ALL-CAPS + only `drop` ability = Sui OTW convention.
    struct GOLD has drop {}

    /// Wrapper for TreasuryCap to make it a transferable object
    struct MintCap has key, store {
        id: UID,
        cap: TreasuryCap<GOLD>,
    }

    /// Initialize: create the TreasuryCap and transfer to sender.
    /// Should be called once after contract deployment.
    public entry fun init(ctx: &mut TxContext) {
        let cap = coin::create_treasury_cap<GOLD>(ctx);
        let mint_cap = MintCap {
            id: setu::object::new(ctx),
            cap,
        };
        transfer::transfer(mint_cap, tx_context::sender(ctx));
    }

    /// Mint `amount` of GOLD tokens to the `recipient`.
    /// Requires the MintCap (only the holder can mint).
    public entry fun mint(
        mint_cap: &mut MintCap,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let coin = coin::mint(&mut mint_cap.cap, amount, ctx);
        transfer::transfer(coin, recipient);
    }

    /// Transfer GOLD coins to a recipient (entry function for API calls)
    public entry fun send(coin: Coin<GOLD>, recipient: address) {
        coin::transfer(coin, recipient);
    }

    /// Burn GOLD coins, reducing total supply.
    /// Returns the amount burned.
    public fun burn(mint_cap: &mut MintCap, coin: Coin<GOLD>): u64 {
        coin::burn(&mut mint_cap.cap, coin)
    }

    /// Get total supply of GOLD tokens
    public fun total_supply(mint_cap: &MintCap): u64 {
        coin::total_supply(&mint_cap.cap)
    }
}
