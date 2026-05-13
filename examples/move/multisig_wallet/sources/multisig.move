// N-of-M Ed25519 multisig wallet — B3 acceptance dapp.
//
// Demonstrates `crypto::ed25519_verify` + `bcs::to_bytes` working together:
//   - Each authorized signer holds a 32-byte Ed25519 public key.
//   - Spending requires `threshold` distinct signatures over a canonically
//     BCS-encoded `Intent` payload (so signers commit to *exactly* the
//     destination + amount + nonce, not just an opaque hash).
//   - Domain separation: the message actually verified is
//     `sha2_256("setu-multisig-v1" || bcs::to_bytes(intent))` to prevent
//     cross-protocol signature reuse (the same key can't be tricked into
//     authorizing a transfer by signing a chat message in another app).
//
// Determinism: every byte in the verified message is produced by the
// stdlib (BCS + sha2_256), so the same intent on every validator hashes
// to the same 32-byte pre-image.
module examples::multisig {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::TxContext;
    use setu::bcs;
    use setu::hash;
    use setu::crypto;
    use setu::vector;

    // ── Errors ──────────────────────────────────────────────────────────
    const E_BAD_THRESHOLD:    u64 = 1;
    const E_DUP_SIGNER:       u64 = 2;
    const E_LEN_MISMATCH:     u64 = 3;
    const E_NOT_ENOUGH_SIGS:  u64 = 4;
    const E_BAD_NONCE:        u64 = 5;

    // ── Types ───────────────────────────────────────────────────────────

    /// Shared multisig wallet state.
    struct Wallet has key, store {
        id: UID,
        /// Public keys of authorized signers (each 32 bytes).
        signers: vector<vector<u8>>,
        /// Minimum signatures required to authorize a `Spend`.
        threshold: u64,
        /// Monotonic nonce — every approved spend bumps this by 1, so a
        /// signature for nonce N can never be replayed once nonce moves on.
        nonce: u64,
    }

    /// What signers actually sign over.
    struct Intent has copy, drop, store {
        wallet_id: address,
        recipient: address,
        amount: u64,
        nonce: u64,
    }

    // ── Constructor ─────────────────────────────────────────────────────

    public entry fun create(
        signers: vector<vector<u8>>,
        threshold: u64,
        ctx: &mut TxContext,
    ) {
        let n = vector::length(&signers);
        // Threshold must satisfy 1 <= t <= N
        assert!(threshold >= 1 && threshold <= n, E_BAD_THRESHOLD);
        transfer::share_object(Wallet {
            id: object::new(ctx),
            signers,
            threshold,
            nonce: 0,
        });
    }

    // ── Authorization ───────────────────────────────────────────────────

    /// Verify `signatures[i]` was produced by `signer_indices[i]`-th signer
    /// over the canonical BCS-encoded `Intent` (with domain separation).
    /// Returns `true` and bumps the nonce if `>= threshold` signatures are
    /// distinct and valid; aborts otherwise.
    ///
    /// `signer_indices` MUST be strictly increasing so duplicates can't be
    /// counted twice (the trivial replay attack).
    public entry fun approve_spend(
        wallet: &mut Wallet,
        recipient: address,
        amount: u64,
        signer_indices: vector<u64>,
        signatures: vector<vector<u8>>,
        intent_nonce: u64,
    ) {
        // Replay protection.
        assert!(intent_nonce == wallet.nonce, E_BAD_NONCE);
        assert!(
            vector::length(&signer_indices) == vector::length(&signatures),
            E_LEN_MISMATCH,
        );

        // Build the message that signers committed to.
        let intent = Intent {
            wallet_id: object::uid_to_address(&wallet.id),
            recipient,
            amount,
            nonce: intent_nonce,
        };
        let msg = build_signing_message(&intent);

        // Walk the (signer_index, signature) pairs.
        let valid: u64 = 0;
        let last_idx: u64 = 0;
        let seen_first = false;
        let i = 0;
        let len = vector::length(&signer_indices);
        while (i < len) {
            let idx = *vector::borrow(&signer_indices, i);
            // Strictly increasing → no duplicates.
            if (seen_first) {
                assert!(idx > last_idx, E_DUP_SIGNER);
            };
            last_idx = idx;
            seen_first = true;

            let pubkey = *vector::borrow(&wallet.signers, idx);
            let sig = *vector::borrow(&signatures, i);
            if (crypto::ed25519_verify(sig, pubkey, msg)) {
                valid = valid + 1;
            };
            i = i + 1;
        };

        assert!(valid >= wallet.threshold, E_NOT_ENOUGH_SIGS);

        // Mark the nonce consumed.
        wallet.nonce = wallet.nonce + 1;
    }

    // ── Helpers (public for tests) ──────────────────────────────────────

    /// Canonical signing message — what every signer's wallet hashes
    /// before signing. Prefix byte string is the domain-separation tag.
    public fun build_signing_message(intent: &Intent): vector<u8> {
        let buf = b"setu-multisig-v1";
        let body = bcs::to_bytes(intent);
        vector::append(&mut buf, body);
        hash::sha2_256(buf)
    }
}
