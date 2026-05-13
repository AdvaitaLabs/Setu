// ===== setu-framework/sources/balance.move =====
// Balance value — phantom T for type safety (different Coin types cannot be mixed).
// Pure Move implementation, no native functions.
module setu::balance {
    friend setu::coin;  // coin.move can call public(friend) functions

    /// Balance value — phantom T ensures type safety
    struct Balance<phantom T> has store {
        value: u64,
    }

    /// Get balance value
    public fun value<T>(balance: &Balance<T>): u64 {
        balance.value
    }

    /// Create zero balance
    public fun zero<T>(): Balance<T> {
        Balance { value: 0 }
    }

    /// Split: extract `amount` from balance
    /// Aborts if balance.value < amount
    public fun split<T>(balance: &mut Balance<T>, amount: u64): Balance<T> {
        assert!(balance.value >= amount, 0); // E_INSUFFICIENT_BALANCE
        balance.value = balance.value - amount;
        Balance { value: amount }
    }

    /// Join: merge `other` into `self`
    public fun join<T>(self: &mut Balance<T>, other: Balance<T>) {
        let Balance { value } = other;
        self.value = self.value + value;
    }

    /// Destroy zero balance
    /// Aborts if balance.value != 0
    public fun destroy_zero<T>(balance: Balance<T>) {
        let Balance { value } = balance;
        assert!(value == 0, 1); // E_NON_ZERO_BALANCE
    }

    /// Destroy balance and return its value (friend modules only, e.g. coin::burn)
    public(friend) fun destroy_and_return_value<T>(balance: Balance<T>): u64 {
        let Balance { value } = balance;
        value
    }

    /// Create a balance with a specified value (friend modules only)
    /// Used by coin::mint and other privileged operations
    public(friend) fun create_for_testing<T>(value: u64): Balance<T> {
        Balance { value }
    }
}
