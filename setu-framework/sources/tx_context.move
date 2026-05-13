// ===== setu-framework/sources/tx_context.move =====
// Transaction context — injected by the framework as the last argument to entry functions.
//
// v3.5 R5-C2: TxContext is NOT injected by sui-adapter (Setu doesn't use it).
// Instead, SetuMoveEngine::execute() appends BCS(TxContext) to args (§6.4).
//
// v3.5 R5-C1: sender()/tx_hash()/epoch() are pure Move field accesses,
// no native functions needed. Address comes via BCS-injected TxContext struct.
module setu::tx_context {
    /// Transaction context — each transaction gets one injected as the last parameter
    struct TxContext has drop {
        /// Transaction sender address
        sender: address,
        /// Transaction hash (BLAKE3)
        tx_hash: vector<u8>,
        /// Transaction epoch
        epoch: u64,
        /// Number of new objects created (for deterministic ID generation)
        ids_created: u64,
    }

    /// Get the transaction sender address
    public fun sender(ctx: &TxContext): address {
        ctx.sender
    }

    /// Get the transaction hash
    public fun tx_hash(ctx: &TxContext): vector<u8> {
        ctx.tx_hash
    }

    /// Get the current epoch
    public fun epoch(ctx: &TxContext): u64 {
        ctx.epoch
    }

    /// Derive a deterministic ID (used internally by object::new)
    /// ID = BLAKE3("SETU_DERIVE_ID:" || tx_hash || creation_num)
    public fun derive_id(tx_hash: vector<u8>, creation_num: u64): address {
        derive_id_internal(tx_hash, creation_num)
    }

    // ── Native functions (only derive_id_internal needs native) ──
    native fun derive_id_internal(tx_hash: vector<u8>, ids_created: u64): address;
}
