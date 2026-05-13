// Commit-reveal vote — B3 acceptance dapp.
//
// Standard cryptographic commit-reveal pattern using `hash::sha2_256` and
// `bcs::to_bytes` to build a binding, hiding commitment:
//
//   1. Commit phase: each voter publishes
//        commit_i = sha2_256( bcs::to_bytes(Reveal { choice, salt }) )
//      where `salt` is a 32-byte secret. The on-chain state stores ONLY
//      the 32-byte digest, so observers can't tally early.
//   2. Reveal phase: each voter publishes their `(choice, salt)`. The
//      contract recomputes `commit_i` and aborts if it doesn't match the
//      previously stored digest, then increments the matching tally.
//
// Determinism: BCS encoding of `Reveal { choice: u8, salt: vector<u8> }`
// is byte-canonical, so every validator computes the same commit hash.
module examples::commit_reveal {
    use setu::object::{Self, UID};
    use setu::transfer;
    use setu::tx_context::TxContext;
    use setu::bcs;
    use setu::hash;
    use setu::dynamic_field as df;
    use setu::vector;

    // ── Errors ──────────────────────────────────────────────────────────
    const E_ALREADY_COMMITTED: u64 = 1;
    const E_NOT_COMMITTED:     u64 = 2;
    const E_BAD_REVEAL:        u64 = 3;
    const E_BAD_CHOICE:        u64 = 4;
    const E_ALREADY_REVEALED:  u64 = 5;

    // ── Types ───────────────────────────────────────────────────────────

    struct Ballot has key, store {
        id: UID,
        /// Number of choices (0..choices_count exclusive).
        choices_count: u8,
        /// Tally per choice — index = choice id.
        tallies: vector<u64>,
    }

    /// Per-voter state, stored as a DF on `Ballot.id` keyed by voter address.
    /// Two-stage: first holds the 32-byte commitment; after reveal, holds
    /// the revealed choice and a "consumed" sentinel.
    struct Slot has store, drop {
        commit: vector<u8>,
        revealed: bool,
    }

    /// Pre-image of a commitment.
    struct Reveal has copy, drop, store {
        choice: u8,
        salt: vector<u8>,
    }

    // ── Constructor ─────────────────────────────────────────────────────

    public entry fun create(choices_count: u8, ctx: &mut TxContext) {
        assert!(choices_count >= 1, E_BAD_CHOICE);
        let tallies = vector::empty<u64>();
        let i: u8 = 0;
        while (i < choices_count) {
            vector::push_back(&mut tallies, 0);
            i = i + 1;
        };
        transfer::share_object(Ballot {
            id: object::new(ctx),
            choices_count,
            tallies,
        });
    }

    // ── Phases ──────────────────────────────────────────────────────────

    /// Voter publishes their 32-byte commitment. One commit per voter.
    public entry fun commit(
        ballot: &mut Ballot,
        voter: address,
        commit_digest: vector<u8>,
    ) {
        assert!(!df::exists_<address>(&ballot.id, voter), E_ALREADY_COMMITTED);
        df::add<address, Slot>(
            &mut ballot.id,
            voter,
            Slot { commit: commit_digest, revealed: false },
        );
    }

    /// Voter reveals `(choice, salt)`. Aborts if recomputed digest differs
    /// from the committed one or if the slot was already revealed.
    public entry fun reveal(
        ballot: &mut Ballot,
        voter: address,
        choice: u8,
        salt: vector<u8>,
    ) {
        assert!(df::exists_<address>(&ballot.id, voter), E_NOT_COMMITTED);
        assert!(choice < ballot.choices_count, E_BAD_CHOICE);

        let slot = df::remove<address, Slot>(&mut ballot.id, voter);
        let Slot { commit, revealed } = slot;
        assert!(!revealed, E_ALREADY_REVEALED);

        // Recompute commit and compare byte-for-byte.
        let pre = Reveal { choice, salt };
        let recomputed = hash::sha2_256(bcs::to_bytes(&pre));
        assert!(commit == recomputed, E_BAD_REVEAL);

        // Tally and re-insert the consumed slot so the voter cannot
        // commit again under the same address.
        let cell = vector::borrow_mut(&mut ballot.tallies, (choice as u64));
        *cell = *cell + 1;

        df::add<address, Slot>(
            &mut ballot.id,
            voter,
            Slot { commit, revealed: true },
        );
    }
}
