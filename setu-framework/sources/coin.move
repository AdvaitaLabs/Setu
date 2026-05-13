// ===== setu-framework/sources/coin.move =====
// Coin object — unified fungible token representation.
// T is the token type (e.g., setu::setu::SETU)
module setu::coin {
    use std::string;
    use std::option;
    use std::vector;
    use setu::object::{Self, UID};
    use setu::balance::{Self, Balance};
    use setu::transfer;
    use setu::tx_context::TxContext;

    /// Coin object — unified fungible token
    struct Coin<phantom T> has key, store {
        id: UID,
        balance: Balance<T>,
    }

    /// Minting authority (globally unique per token type)
    struct TreasuryCap<phantom T> has key, store {
        id: UID,
        total_supply: u64,
    }

    /// Token metadata — created once per token type, frozen as immutable
    struct CoinMetadata<phantom T> has key, store {
        id: UID,
        /// Decimal places (e.g., 9 for SETU, 6 for USDC)
        decimals: u8,
        /// Token full name (e.g., "Setu Token")
        name: string::String,
        /// Token symbol (e.g., "SETU")
        symbol: string::String,
        /// Token description
        description: string::String,
        /// Optional icon URL
        icon_url: option::Option<string::String>,
    }

    // ── Queries ──

    /// Get Coin balance
    public fun value<T>(coin: &Coin<T>): u64 {
        balance::value(&coin.balance)
    }

    /// Get mutable reference to Coin's balance (for internal operations)
    public fun balance_mut<T>(coin: &mut Coin<T>): &mut Balance<T> {
        &mut coin.balance
    }

    // ── Operations ──

    /// Create Coin from Balance
    public fun from_balance<T>(balance: Balance<T>, ctx: &mut TxContext): Coin<T> {
        Coin {
            id: object::new(ctx),
            balance,
        }
    }

    /// Split: extract `amount` from self into a new Coin
    public fun split<T>(self: &mut Coin<T>, amount: u64, ctx: &mut TxContext): Coin<T> {
        let split_balance = balance::split(&mut self.balance, amount);
        from_balance(split_balance, ctx)
    }

    /// Join: merge `other` into `self`
    public entry fun join<T>(self: &mut Coin<T>, other: Coin<T>) {
        let Coin { id, balance } = other;
        object::delete(id);
        balance::join(&mut self.balance, balance);
    }

    /// Destroy zero-value Coin
    public fun destroy_zero<T>(coin: Coin<T>) {
        let Coin { id, balance } = coin;
        object::delete(id);
        balance::destroy_zero(balance);
    }

    /// Unwrap Coin into Balance (destroys the Coin, returns inner Balance)
    public fun into_balance<T>(coin: Coin<T>): Balance<T> {
        let Coin { id, balance } = coin;
        object::delete(id);
        balance
    }

    /// Put Balance into Coin (joins Balance into Coin's internal balance)
    public fun put<T>(coin: &mut Coin<T>, balance: Balance<T>) {
        balance::join(&mut coin.balance, balance);
    }

    /// Take Balance from Coin (splits amount from Coin's internal balance)
    public fun take<T>(coin: &mut Coin<T>, amount: u64): Balance<T> {
        balance::split(&mut coin.balance, amount)
    }

    /// Transfer: send Coin to recipient
    public entry fun transfer<T>(coin: Coin<T>, recipient: address) {
        transfer::transfer(coin, recipient);
    }

    // ── TreasuryCap operations (mint/burn) ──

    /// Mint new Coins
    public fun mint<T>(cap: &mut TreasuryCap<T>, amount: u64, ctx: &mut TxContext): Coin<T> {
        cap.total_supply = cap.total_supply + amount;
        let balance = balance::create_for_testing<T>(amount);
        from_balance(balance, ctx)
    }

    /// Burn Coins (return value to TreasuryCap)
    public fun burn<T>(cap: &mut TreasuryCap<T>, coin: Coin<T>): u64 {
        let Coin { id, balance } = coin;
        object::delete(id);
        let value = balance::destroy_and_return_value(balance);
        cap.total_supply = cap.total_supply - value;
        value
    }

    /// Get total supply
    public fun total_supply<T>(cap: &TreasuryCap<T>): u64 {
        cap.total_supply
    }

    /// Create TreasuryCap (package-scoped — for setu.move init or testing)
    public fun create_treasury_cap<T>(ctx: &mut TxContext): TreasuryCap<T> {
        TreasuryCap {
            id: object::new(ctx),
            total_supply: 0,
        }
    }

    // ── CoinMetadata operations ──

    /// Create CoinMetadata (requires TreasuryCap as authority proof).
    /// Typically called within the token module's init() function, then frozen.
    public fun create_coin_metadata<T>(
        _cap: &TreasuryCap<T>,
        decimals: u8,
        name: vector<u8>,
        symbol: vector<u8>,
        description: vector<u8>,
        icon_url: vector<u8>,
        ctx: &mut TxContext,
    ): CoinMetadata<T> {
        let icon = if (vector::is_empty(&icon_url)) {
            option::none<string::String>()
        } else {
            option::some(string::utf8(icon_url))
        };
        CoinMetadata {
            id: object::new(ctx),
            decimals,
            name: string::utf8(name),
            symbol: string::utf8(symbol),
            description: string::utf8(description),
            icon_url: icon,
        }
    }

    /// Get token decimal places
    public fun get_decimals<T>(metadata: &CoinMetadata<T>): u8 {
        metadata.decimals
    }

    /// Get token name
    public fun get_name<T>(metadata: &CoinMetadata<T>): string::String {
        metadata.name
    }

    /// Get token symbol
    public fun get_symbol<T>(metadata: &CoinMetadata<T>): string::String {
        metadata.symbol
    }

    /// Get token description
    public fun get_description<T>(metadata: &CoinMetadata<T>): string::String {
        metadata.description
    }

    /// Get token icon URL
    public fun get_icon_url<T>(metadata: &CoinMetadata<T>): option::Option<string::String> {
        metadata.icon_url
    }
}
