// ===== examples/move/lightning/sources/channel.move =====
// Simplified Lightning-style Payment Channel for Setu.
//
// Adapted from a Sui-based Lightning Network channel contract.
// Rewritten to work within Setu's Phase 0-4 constraints:
//
//   ┌─────────────────────────────────────────────────────────────────┐
//   │ Sui feature              │ Setu adaptation                     │
//   ├──────────────────────────┼─────────────────────────────────────┤
//   │ share_object (Channel)   │ Owned + pass_to_counterparty()     │
//   │ sui::table::Table        │ Parallel vectors                   │
//   │ sui::clock::Clock        │ Removed (no on-chain time)         │
//   │ sui::ecdsa_k1            │ Removed (DAG layer authentication) │
//   │ std::hash::sha2_256      │ Removed (no on-chain hash)         │
//   │ sui::event::emit         │ Removed (use DAG events)           │
//   │ std::option::Option      │ Sentinel values                    │
//   │ SUI token type           │ SETU token type                    │
//   │ coin::take / into_balance│ balance::split + coin::from_balance│
//   │ public_transfer          │ transfer::transfer                 │
//   └─────────────────────────────────────────────────────────────────┘
//
// Design: Turn-based owned-object protocol
//   The Channel object is transferred between party_a and party_b.
//   Only the current holder (owner) can operate on the channel.
//   Off-chain negotiations happen between on-chain calls.
//
// Limitations vs full Lightning Network:
//   - No hash-locked conditional payments (HTLCs are commitment-based)
//   - No on-chain dispute period or punishment mechanism
//   - No on-chain signature verification (DAG layer provides authentication)
//   - No timelock (no Clock module available)
//
// Flow:
//   1. A opens channel:       open_channel(coin, amount, party_b)
//   2. A passes to B:         pass_to_counterparty(channel)
//   3. B funds channel:       fund_channel(channel, coin, amount)
//   4. Channel is now OPEN — parties interact via pass/update/htlc
//   5. Either party closes:   close_cooperative(channel) or force_close(channel)
//
// API examples:
//
//   # 1. Open channel (Alice funds 1000, Bob is counterparty)
//   POST /api/v1/move/call {
//     "sender": "alice", "package": "0xcafe", "module": "channel",
//     "function": "open_channel",
//     "input_object_ids": ["<alice_coin_id>"],
//     "mutable_indices": [0],
//     "args": ["e803000000000000", "<bob_address_64hex>"],
//     "needs_tx_context": true
//   }
//
//   # 2. Alice passes channel to Bob
//   POST /api/v1/move/call {
//     "sender": "alice", "package": "0xcafe", "module": "channel",
//     "function": "pass_to_counterparty",
//     "input_object_ids": ["<channel_id>"],
//     "consumed_indices": [0],
//     "needs_tx_context": true
//   }
//
//   # 3. Bob funds the channel (adds 500)
//   POST /api/v1/move/call {
//     "sender": "bob", "package": "0xcafe", "module": "channel",
//     "function": "fund_channel",
//     "input_object_ids": ["<channel_id>", "<bob_coin_id>"],
//     "mutable_indices": [0, 1],
//     "args": ["f401000000000000"],
//     "needs_tx_context": true
//   }
//
//   # 4. Update state (new balances after off-chain payments)
//   POST /api/v1/move/call {
//     "sender": "bob", "package": "0xcafe", "module": "channel",
//     "function": "update_state",
//     "input_object_ids": ["<channel_id>"],
//     "mutable_indices": [0],
//     "args": ["c800000000000000", "9606000000000000", "0100000000000000"],
//     "needs_tx_context": false
//   }
//
//   # 5. Cooperative close
//   POST /api/v1/move/call {
//     "sender": "bob", "package": "0xcafe", "module": "channel",
//     "function": "close_cooperative",
//     "input_object_ids": ["<channel_id>"],
//     "consumed_indices": [0],
//     "needs_tx_context": true
//   }

module lightning::channel {
    use std::vector;
    use setu::object::{Self, UID};
    use setu::coin::{Self, Coin};
    use setu::balance::{Self, Balance};
    use setu::setu::SETU;
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};

    // ── Errors ──
    const ENotAuthorized: u64 = 0;
    const EChannelNotOpen: u64 = 1;
    const EInvalidStatus: u64 = 2;
    const EInsufficientBalance: u64 = 3;
    const EInvalidHtlcIndex: u64 = 4;
    const EHtlcNotPending: u64 = 5;
    const EBalanceMismatch: u64 = 6;
    const EInvalidStateNum: u64 = 7;

    // ── Channel object ──

    /// Payment channel between two parties.
    ///
    /// Owned object — the current holder controls the channel.
    /// Use `pass_to_counterparty()` to hand off control.
    ///
    /// Status codes: 0 = FUNDED_A (waiting for B), 1 = OPEN, 2 = CLOSED
    struct Channel has key, store {
        id: UID,
        party_a: address,
        party_b: address,
        balance_a: u64,
        balance_b: u64,
        funding: Balance<SETU>,
        status: u8,
        state_num: u64,
        // HTLC tracking via parallel vectors (replaces sui::table::Table)
        // direction: 0 = A→B, 1 = B→A
        // htlc_status: 0 = PENDING, 1 = SETTLED, 2 = REFUNDED
        htlc_amounts: vector<u64>,
        htlc_directions: vector<u8>,
        htlc_statuses: vector<u8>,
    }

    // ═══════════════════════════════════════════════════════════════
    // Channel lifecycle
    // ═══════════════════════════════════════════════════════════════

    /// Open a new payment channel.
    /// Party A provides initial funding. The channel is owned by A.
    /// Next: A calls `pass_to_counterparty` → B calls `fund_channel`.
    public entry fun open_channel(
        funding_coin: &mut Coin<SETU>,
        amount: u64,
        party_b: address,
        ctx: &mut TxContext,
    ) {
        let party_a = tx_context::sender(ctx);
        let deposited = balance::split(coin::balance_mut(funding_coin), amount);

        let channel = Channel {
            id: object::new(ctx),
            party_a,
            party_b,
            balance_a: amount,
            balance_b: 0,
            funding: deposited,
            status: 0,
            state_num: 0,
            htlc_amounts: vector::empty<u64>(),
            htlc_directions: vector::empty<u8>(),
            htlc_statuses: vector::empty<u8>(),
        };

        transfer::transfer(channel, party_a);
    }

    /// Party B deposits funds and activates the channel.
    /// Requires: status == FUNDED_A, caller == party_b.
    public entry fun fund_channel(
        channel: &mut Channel,
        funding_coin: &mut Coin<SETU>,
        amount: u64,
        ctx: &mut TxContext,
    ) {
        assert!(channel.status == 0, EInvalidStatus);
        assert!(tx_context::sender(ctx) == channel.party_b, ENotAuthorized);

        let deposited = balance::split(coin::balance_mut(funding_coin), amount);
        balance::join(&mut channel.funding, deposited);
        channel.balance_b = amount;
        channel.status = 1; // OPEN
    }

    /// Reclaim funds from a channel that B never funded.
    /// Only valid when status == FUNDED_A.
    public entry fun withdraw_unfunded(
        channel: Channel,
        ctx: &mut TxContext,
    ) {
        let Channel {
            id,
            party_a,
            party_b: _,
            balance_a,
            balance_b: _,
            funding,
            status,
            state_num: _,
            htlc_amounts: _,
            htlc_directions: _,
            htlc_statuses: _,
        } = channel;

        assert!(status == 0, EInvalidStatus);
        object::delete(id);

        if (balance_a > 0) {
            let coin_a = coin::from_balance(funding, ctx);
            transfer::transfer(coin_a, party_a);
        } else {
            balance::destroy_zero(funding);
        };
    }

    /// Transfer channel to the counterparty (turn-based handoff).
    /// Replaces shared-object concurrency with explicit ownership passing.
    /// Only party_a or party_b may call this.
    public entry fun pass_to_counterparty(
        channel: Channel,
        ctx: &mut TxContext,
    ) {
        let sender = tx_context::sender(ctx);
        let a = channel.party_a;
        let b = channel.party_b;

        if (sender == a) {
            transfer::transfer(channel, b);
        } else {
            assert!(sender == b, ENotAuthorized);
            transfer::transfer(channel, a);
        };
    }

    // ═══════════════════════════════════════════════════════════════
    // State updates
    // ═══════════════════════════════════════════════════════════════

    /// Commit a new balance distribution agreed off-chain.
    /// state_num must be strictly increasing to prevent replay.
    /// Invariant: new_balance_a + new_balance_b + pending_htlcs == total funding.
    public entry fun update_state(
        channel: &mut Channel,
        new_balance_a: u64,
        new_balance_b: u64,
        new_state_num: u64,
    ) {
        assert!(channel.status == 1, EChannelNotOpen);
        assert!(new_state_num > channel.state_num, EInvalidStateNum);

        let pending = sum_all_pending(&channel.htlc_amounts, &channel.htlc_statuses);
        assert!(
            new_balance_a + new_balance_b + pending == balance::value(&channel.funding),
            EBalanceMismatch,
        );

        channel.balance_a = new_balance_a;
        channel.balance_b = new_balance_b;
        channel.state_num = new_state_num;
    }

    // ═══════════════════════════════════════════════════════════════
    // HTLCs (simplified — commitment-based, no hash-lock)
    //
    // In a full Lightning Network, HTLCs use SHA256 hash-locks:
    //   the payee reveals a preimage to claim funds.
    // Setu has no on-chain hash primitives, so HTLCs here are
    //   commitment-based: the channel holder authorizes claim/refund.
    // ═══════════════════════════════════════════════════════════════

    /// Add a pending conditional payment.
    /// Locks `amount` from the payer's balance until settled or refunded.
    /// direction: 0 = A pays B, 1 = B pays A.
    public entry fun add_htlc(
        channel: &mut Channel,
        amount: u64,
        direction: u8,
    ) {
        assert!(channel.status == 1, EChannelNotOpen);

        if (direction == 0) {
            assert!(channel.balance_a >= amount, EInsufficientBalance);
            channel.balance_a = channel.balance_a - amount;
        } else {
            assert!(channel.balance_b >= amount, EInsufficientBalance);
            channel.balance_b = channel.balance_b - amount;
        };

        vector::push_back(&mut channel.htlc_amounts, amount);
        vector::push_back(&mut channel.htlc_directions, direction);
        vector::push_back(&mut channel.htlc_statuses, 0); // PENDING
    }

    /// Claim an HTLC — transfers locked funds to the payee.
    /// In full Lightning this would require a hash preimage;
    /// here the channel holder authorizes the claim.
    public entry fun claim_htlc(
        channel: &mut Channel,
        htlc_index: u64,
    ) {
        assert!(channel.status == 1, EChannelNotOpen);
        let len = vector::length(&channel.htlc_amounts);
        assert!(htlc_index < len, EInvalidHtlcIndex);
        assert!(
            *vector::borrow(&channel.htlc_statuses, htlc_index) == 0,
            EHtlcNotPending,
        );

        let amount = *vector::borrow(&channel.htlc_amounts, htlc_index);
        let direction = *vector::borrow(&channel.htlc_directions, htlc_index);

        if (direction == 0) {
            channel.balance_b = channel.balance_b + amount;
        } else {
            channel.balance_a = channel.balance_a + amount;
        };

        *vector::borrow_mut(&mut channel.htlc_statuses, htlc_index) = 1; // SETTLED
    }

    /// Refund an HTLC — returns locked funds to the payer.
    public entry fun refund_htlc(
        channel: &mut Channel,
        htlc_index: u64,
    ) {
        assert!(channel.status == 1, EChannelNotOpen);
        let len = vector::length(&channel.htlc_amounts);
        assert!(htlc_index < len, EInvalidHtlcIndex);
        assert!(
            *vector::borrow(&channel.htlc_statuses, htlc_index) == 0,
            EHtlcNotPending,
        );

        let amount = *vector::borrow(&channel.htlc_amounts, htlc_index);
        let direction = *vector::borrow(&channel.htlc_directions, htlc_index);

        if (direction == 0) {
            channel.balance_a = channel.balance_a + amount;
        } else {
            channel.balance_b = channel.balance_b + amount;
        };

        *vector::borrow_mut(&mut channel.htlc_statuses, htlc_index) = 2; // REFUNDED
    }

    // ═══════════════════════════════════════════════════════════════
    // Channel close
    // ═══════════════════════════════════════════════════════════════

    /// Cooperative close — both parties agreed off-chain.
    /// The current holder executes; channel is destroyed and funds distributed.
    /// Any pending HTLCs are automatically refunded to payers.
    public entry fun close_cooperative(
        channel: Channel,
        ctx: &mut TxContext,
    ) {
        let Channel {
            id,
            party_a,
            party_b,
            balance_a,
            balance_b,
            funding,
            status,
            state_num: _,
            htlc_amounts,
            htlc_directions,
            htlc_statuses,
        } = channel;

        assert!(status == 1, EChannelNotOpen);

        let refunded_a = sum_pending_refunds(&htlc_amounts, &htlc_directions, &htlc_statuses, 0);
        let refunded_b = sum_pending_refunds(&htlc_amounts, &htlc_directions, &htlc_statuses, 1);
        let final_a = balance_a + refunded_a;
        let final_b = balance_b + refunded_b;

        object::delete(id);
        distribute_and_destroy(funding, final_a, final_b, party_a, party_b, ctx);
    }

    /// Unilateral (force) close using the last committed on-chain state.
    /// Without on-chain timelocks, there is no dispute period.
    /// Unlike cooperative close, pending HTLCs are settled in favor of
    /// the payee (not refunded to payer), since the counterparty may
    /// have already revealed preimages off-chain.
    public entry fun force_close(
        channel: Channel,
        ctx: &mut TxContext,
    ) {
        let Channel {
            id,
            party_a,
            party_b,
            balance_a,
            balance_b,
            funding,
            status,
            state_num: _,
            htlc_amounts,
            htlc_directions,
            htlc_statuses,
        } = channel;

        assert!(status == 1, EChannelNotOpen);

        // Force close: settle all pending HTLCs in favor of payees
        // (A→B htlcs go to B, B→A htlcs go to A)
        let settled_to_b = sum_pending_refunds(&htlc_amounts, &htlc_directions, &htlc_statuses, 0);
        let settled_to_a = sum_pending_refunds(&htlc_amounts, &htlc_directions, &htlc_statuses, 1);
        let final_a = balance_a + settled_to_a;
        let final_b = balance_b + settled_to_b;

        object::delete(id);
        distribute_and_destroy(funding, final_a, final_b, party_a, party_b, ctx);
    }

    // ═══════════════════════════════════════════════════════════════
    // Read-only accessors
    // ═══════════════════════════════════════════════════════════════

    public fun get_balance_a(ch: &Channel): u64 { ch.balance_a }
    public fun get_balance_b(ch: &Channel): u64 { ch.balance_b }
    public fun get_state_num(ch: &Channel): u64 { ch.state_num }
    public fun get_status(ch: &Channel): u8 { ch.status }
    public fun get_total_funding(ch: &Channel): u64 { balance::value(&ch.funding) }
    public fun get_htlc_count(ch: &Channel): u64 { vector::length(&ch.htlc_amounts) }

    // ═══════════════════════════════════════════════════════════════
    // Internal helpers
    // ═══════════════════════════════════════════════════════════════

    /// Sum ALL pending HTLC amounts (regardless of direction).
    fun sum_all_pending(
        amounts: &vector<u64>,
        statuses: &vector<u8>,
    ): u64 {
        let total = 0;
        let i = 0;
        let len = vector::length(amounts);
        while (i < len) {
            if (*vector::borrow(statuses, i) == 0) {
                total = total + *vector::borrow(amounts, i);
            };
            i = i + 1;
        };
        total
    }

    /// Sum pending HTLC amounts that should be refunded to a given party.
    /// `refund_to_direction`: 0 = refund to A (A→B htlcs), 1 = refund to B (B→A htlcs).
    fun sum_pending_refunds(
        amounts: &vector<u64>,
        directions: &vector<u8>,
        statuses: &vector<u8>,
        refund_to_direction: u8,
    ): u64 {
        let total = 0;
        let i = 0;
        let len = vector::length(amounts);
        while (i < len) {
            if (*vector::borrow(statuses, i) == 0) {
                if (*vector::borrow(directions, i) == refund_to_direction) {
                    total = total + *vector::borrow(amounts, i);
                };
            };
            i = i + 1;
        };
        total
    }

    /// Distribute funding balance to both parties as Coins, then destroy the remainder.
    /// Invariant: amount_a + amount_b == balance::value(&funding).
    fun distribute_and_destroy(
        funding: Balance<SETU>,
        amount_a: u64,
        amount_b: u64,
        party_a: address,
        party_b: address,
        ctx: &mut TxContext,
    ) {
        if (amount_a > 0 && amount_b > 0) {
            let bal_a = balance::split(&mut funding, amount_a);
            let coin_a = coin::from_balance(bal_a, ctx);
            transfer::transfer(coin_a, party_a);

            let coin_b = coin::from_balance(funding, ctx);
            transfer::transfer(coin_b, party_b);
        } else if (amount_a > 0) {
            let coin_a = coin::from_balance(funding, ctx);
            transfer::transfer(coin_a, party_a);
        } else if (amount_b > 0) {
            let coin_b = coin::from_balance(funding, ctx);
            transfer::transfer(coin_b, party_b);
        } else {
            balance::destroy_zero(funding);
        };
    }
}
