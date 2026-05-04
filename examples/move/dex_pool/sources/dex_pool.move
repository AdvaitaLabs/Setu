// Dynamic Fields — PWOO hotspot sharding example.
//
// Demonstrates the canonical "shared parent + per-key DF slots" pattern
// from `docs/feat/dynamic-fields/design.md` §3.10:
//   - A single shared `Pool` is the parent object
//   - Each trading pair's liquidity is stored as a DF under `Pool.id`
//   - Two senders concurrently writing **different** pairs do NOT
//     stale_read on the pool itself, because §3.7a parent-bytes-preserved
//     rule skips the pool StateChange (no version bump) when only DF
//     slots changed.
//
// Pairs with PWOO's `counter` example and the `df_registry` M5 helper:
//   - pwoo_counter: single shared slot, hotspot that DOES contend
//   - df_registry:  DF API surface exercised end-to-end
//   - dex_pool:     DF as a hotspot mitigation technique (this file)
//
// Limitations:
//   * `set_liquidity` uses remove+add instead of `df::borrow_mut` because
//     true in-place `&mut V` writeback is still a VM-runtime placeholder
//     (tracked in `docs/bugs/20260420-df-borrow-mut-placeholder.md`). The
//     visible contract-level behaviour is identical.
//   * `Liquidity` is `has store, drop` only for demo simplicity — a real
//     DEX would never drop reserves silently; it would return the removed
//     value to the caller or assert a balance invariant.
module examples::dex_pool {
    use setu::object::{Self, UID};
    use setu::dynamic_field as df;
    use setu::transfer;
    use setu::tx_context::{Self, TxContext};
    use setu::bcs;
    use setu::hash;
    use setu::event;

    // ── Abort codes ──────────────────────────────────────────────────────
    const E_PAIR_EXISTS:  u64 = 1;
    const E_PAIR_MISSING: u64 = 2;

    // ── Parent + DF value types ─────────────────────────────────────────

    /// Shared pool: the parent object. `has key, store` is required so it
    /// can be `share_object`-ed; its `id: UID` is what DF entries hang off.
    struct Pool has key, store {
        id: UID,
    }

    /// DF key type. `copy + drop + store` satisfies the
    /// `dynamic_field::*<K: copy + drop + store, V: store>` bounds and makes
    /// the BCS-encoded key deterministic (used by `derive_df_oid`).
    struct Pair has copy, drop, store {
        token_a: address,
        token_b: address,
    }

    /// DF value type. Must have `store`; must NOT have `key` (enforced at
    /// native entry per §3.6 two-rail ability check).
    struct Liquidity has store, drop {
        reserve_a: u64,
        reserve_b: u64,
    }

    // ── Events ──────────────────────────────────────────────────────────

    /// Emitted whenever a pair's reserves change (created or replaced).
    /// `pair_bcs` is the BCS encoding of the `Pair` key — proves the
    /// `bcs::to_bytes` native is wired in (B3 acceptance §9 item 5).
    /// Off-chain indexers can decode it back into `(token_a, token_b)`.
    /// `copy + drop` are required by `event::emit<T: copy + drop>`.
    struct PriceObservation has copy, drop {
        pair_bcs: vector<u8>,
        reserve_a: u64,
        reserve_b: u64,
        epoch: u64,
    }

    // ── Constructors ─────────────────────────────────────────────────────

    /// One-step create + share. After this, anyone can call the entry
    /// functions below referencing the pool via `shared_object_ids`.
    public entry fun create_shared(ctx: &mut TxContext) {
        transfer::share_object(Pool { id: object::new(ctx) });
    }

    // ── Entry functions ─────────────────────────────────────────────────

    /// Create a new trading pair under this pool. Aborts if the pair
    /// already exists — use `set_liquidity` instead for existing pairs.
    public entry fun create_pair(
        pool: &mut Pool,
        token_a: address,
        token_b: address,
        reserve_a: u64,
        reserve_b: u64,
        ctx: &TxContext,
    ) {
        let pair = Pair { token_a, token_b };
        assert!(!df::exists_<Pair>(&pool.id, pair), E_PAIR_EXISTS);
        df::add<Pair, Liquidity>(
            &mut pool.id,
            pair,
            Liquidity { reserve_a, reserve_b },
        );
        event::emit(PriceObservation {
            pair_bcs: bcs::to_bytes(&pair),
            reserve_a,
            reserve_b,
            epoch: tx_context::epoch(ctx),
        });
    }

    /// Replace an existing pair's liquidity.
    ///
    /// Uses remove+add (see module-level comment) which at the DF-effect
    /// level collapses into a single `df_mutated` entry and is equivalent
    /// to a future native `borrow_mut` writeback.
    public entry fun set_liquidity(
        pool: &mut Pool,
        token_a: address,
        token_b: address,
        reserve_a: u64,
        reserve_b: u64,
        ctx: &TxContext,
    ) {
        let pair = Pair { token_a, token_b };
        assert!(df::exists_<Pair>(&pool.id, pair), E_PAIR_MISSING);
        let _old = df::remove<Pair, Liquidity>(&mut pool.id, pair);
        df::add<Pair, Liquidity>(
            &mut pool.id,
            pair,
            Liquidity { reserve_a, reserve_b },
        );
        event::emit(PriceObservation {
            pair_bcs: bcs::to_bytes(&pair),
            reserve_a,
            reserve_b,
            epoch: tx_context::epoch(ctx),
        });
    }

    /// Remove a pair entirely (listing delisted, etc).
    public entry fun remove_pair(
        pool: &mut Pool,
        token_a: address,
        token_b: address,
    ) {
        let pair = Pair { token_a, token_b };
        let _liq = df::remove<Pair, Liquidity>(&mut pool.id, pair);
    }

    // ── B3 acceptance: bcs + hash usage ─────────────────────────────────

    /// Deterministic identifier for a pair, derived as
    /// `sha2_256(bcs::to_bytes(Pair))`. Two validators executing this on
    /// the same `(token_a, token_b)` MUST produce byte-identical output.
    public fun pair_digest(token_a: address, token_b: address): vector<u8> {
        let pair = Pair { token_a, token_b };
        hash::sha2_256(bcs::to_bytes(&pair))
    }
}
