/// Test helper: mint SETU coins as Move objects for testing.
/// This creates a TreasuryCap<SETU> and mints coins, bypassing the native coin system.
module lightning::test_mint {
    use setu::coin;
    use setu::setu::SETU;
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    /// Create a TreasuryCap<SETU> and transfer to sender.
    public entry fun create_cap(ctx: &mut TxContext) {
        let cap = coin::create_treasury_cap<SETU>(ctx);
        transfer::transfer(cap, tx_context::sender(ctx));
    }

    /// Mint SETU coins and transfer to recipient.
    public entry fun mint_to(
        cap: &mut coin::TreasuryCap<SETU>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext,
    ) {
        let minted = coin::mint(cap, amount, ctx);
        transfer::transfer(minted, recipient);
    }
}
