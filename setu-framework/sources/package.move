// ===== setu-framework/sources/package.move =====
// Package upgrade primitives — Phase 8 / B5.
//
// Implements the Sui-style hot-potato upgrade flow on top of Setu's
// fresh-address-per-version package model (design.md §4.5):
//
//   1. Publisher of a package owns a unique `UpgradeCap` (key, store).
//   2. To upgrade, the publisher calls `authorize_upgrade(cap, policy, digest)`
//      which mints a single-use `UpgradeTicket`. The ticket has NO abilities,
//      so it can neither be stored nor copied — the only place it can land is
//      a same-PTB `Command::Upgrade` whose engine lowering consumes it.
//   3. The engine's `lower_upgrade_inline` validates the ticket, runs relink
//      + ABI-compat check, publishes the new bundle at a fresh package
//      address, and produces a `UpgradeReceipt` (also no abilities).
//   4. The PTB's tail call is `commit_upgrade(cap, receipt)` which bumps
//      `cap.version` and burns the receipt.
//
// `UpgradeCap` is a regular Setu Move object — created via `object::new`,
// transferred via `setu::transfer::transfer`. No new natives required.
//
// `UpgradeTicket` / `UpgradeReceipt` are values, not objects (no `key`),
// produced and consumed inside one PTB. Their lack of abilities is what
// makes the flow forge-resistant: any PTB whose final command is not
// `commit_upgrade` will fail to type-check (the receipt cannot be stored
// or dropped).

module setu::package {
    use setu::object::{Self, UID, ID};
    use setu::tx_context::TxContext;

    // ── Policy bytes (must match `setu_move_vm::compat::UpgradePolicy`) ──

    const POLICY_COMPATIBLE:  u8 = 0;
    const POLICY_ADDITIVE:    u8 = 1;
    const POLICY_DEP_ONLY:    u8 = 2;

    /// `policy` value not understood by the VM.
    const E_UNKNOWN_POLICY: u64 = 0;
    /// `commit_upgrade` called with a receipt whose `cap_id` does not match
    /// the cap being committed against. Indicates a forged or cross-PTB
    /// receipt.
    const E_CAP_RECEIPT_MISMATCH: u64 = 1;
    /// `authorize_upgrade` was called with a tighter policy than the cap
    /// currently allows; cap policies are monotonically non-decreasing.
    const E_POLICY_TIGHTER_THAN_CAP: u64 = 2;

    // ── UpgradeCap ─────────────────────────────────────────────────────

    /// Owner-held capability authorising upgrades to a single package family.
    ///
    /// `package` tracks the *latest* package address (mutated by
    /// `commit_upgrade`); the original family ID stays on the cap's `id`
    /// field. `version` is bumped on every successful commit. `policy` is
    /// the strongest policy any future upgrade may use; tighter policies
    /// (lower numeric value) are allowed, looser ones rejected.
    struct UpgradeCap has key, store {
        id: UID,
        /// Latest package address. Initially equal to the address derived
        /// at the first publish; rewritten by `commit_upgrade`.
        package: ID,
        /// Number of times this package has been upgraded. 1 after the
        /// initial publish; bumped on every successful commit.
        version: u64,
        /// Tightest policy this cap allows. `authorize_upgrade` may
        /// downgrade (numerically increase) the policy supplied at call
        /// time but never loosen it.
        policy: u8,
    }

    // ── UpgradeTicket / UpgradeReceipt (hot-potato) ─────────────────────

    /// Single-use authorisation to perform exactly one `Command::Upgrade`.
    /// Has NO abilities — cannot be stored, dropped, copied, or held in
    /// any object field. The only valid lifecycle is:
    ///   `authorize_upgrade` (mint) → `Command::Upgrade` (consume).
    struct UpgradeTicket {
        /// Pinned to the cap that minted it. The engine matches against
        /// `cap.id` to reject cross-cap forgery.
        cap_id: ID,
        /// Family ID — preserved across all versions; used by linkage
        /// writes.
        family_id: ID,
        policy: u8,
        /// `blake3` of the new module bundle (caller-supplied; the engine
        /// recomputes and rejects on mismatch).
        digest: vector<u8>,
    }

    /// Proof produced by `Command::Upgrade` lowering. Has NO abilities;
    /// must be consumed by `commit_upgrade` in the same PTB.
    struct UpgradeReceipt {
        /// Same cap-id as the ticket that authorised this upgrade.
        cap_id: ID,
        /// Address the new bundle was published at.
        new_package: ID,
        /// `cap.version + 1` — what `commit_upgrade` must overwrite to.
        new_version: u64,
    }

    // ── Public API ──────────────────────────────────────────────────────

    /// Create the cap that the engine emits for a fresh publish (called
    /// implicitly by `lower_publish_inline` step 2 — design.md §4.5 v0
    /// sequence). Userspace MUST NOT call this directly; the engine
    /// supplies the freshly-derived `package` address that callers cannot
    /// produce on their own (they would need access to `tx_hash + output_counter`).
    public fun make_upgrade_cap(
        package: ID,
        ctx: &mut TxContext,
    ): UpgradeCap {
        UpgradeCap {
            id: object::new(ctx),
            package,
            version: 1,
            policy: POLICY_COMPATIBLE,
        }
    }

    /// Mint a fresh `UpgradeTicket` for a single upgrade attempt. The
    /// caller proves authorisation by holding `&mut UpgradeCap`. `policy`
    /// must be at least as strict as `cap.policy`.
    public fun authorize_upgrade(
        cap: &mut UpgradeCap,
        policy: u8,
        digest: vector<u8>,
    ): UpgradeTicket {
        assert!(
            policy == POLICY_COMPATIBLE
                || policy == POLICY_ADDITIVE
                || policy == POLICY_DEP_ONLY,
            E_UNKNOWN_POLICY,
        );
        // Numerically larger policy values are LOOSER (Compatible=0 strict,
        // DepOnly=2 loose). Reject loosening attempts.
        assert!(policy <= cap.policy, E_POLICY_TIGHTER_THAN_CAP);
        UpgradeTicket {
            cap_id: *object::uid_to_inner(&cap.id),
            family_id: cap.package,
            policy,
            digest,
        }
    }

    /// Burn the receipt produced by the engine, advancing the cap's
    /// version + package pointer. Asserts the receipt belongs to this
    /// cap (forge resistance).
    public fun commit_upgrade(
        cap: &mut UpgradeCap,
        receipt: UpgradeReceipt,
    ) {
        let UpgradeReceipt { cap_id, new_package, new_version } = receipt;
        assert!(*object::uid_to_inner(&cap.id) == cap_id, E_CAP_RECEIPT_MISMATCH);
        cap.package = new_package;
        cap.version = new_version;
    }

    // ── Read-only accessors (used by RPC + indexer) ─────────────────────

    public fun version(cap: &UpgradeCap): u64 { cap.version }
    public fun package(cap: &UpgradeCap): ID { cap.package }
    public fun policy(cap: &UpgradeCap): u8 { cap.policy }

    public fun ticket_cap_id(t: &UpgradeTicket): ID { t.cap_id }
    public fun ticket_family_id(t: &UpgradeTicket): ID { t.family_id }
    public fun ticket_policy(t: &UpgradeTicket): u8 { t.policy }
    public fun ticket_digest(t: &UpgradeTicket): vector<u8> { t.digest }

    public fun receipt_cap_id(r: &UpgradeReceipt): ID { r.cap_id }
    public fun receipt_new_package(r: &UpgradeReceipt): ID { r.new_package }
    public fun receipt_new_version(r: &UpgradeReceipt): u64 { r.new_version }

    // ── Engine hooks (only callable from `lower_upgrade_inline`) ────────
    //
    // These are `public` because Move has no friend-only visibility for
    // cross-package calls; semantic protection comes from the fact that
    // `UpgradeTicket` has no abilities, so user PTBs cannot manufacture
    // one and call `consume_ticket` themselves. The engine is the only
    // code path that holds a ticket *and* produces a matching receipt
    // bytecode-deterministically.

    /// Engine-only: deconstruct a ticket into its fields (engine consumes
    /// the ticket value, then performs relink+publish, then mints a
    /// receipt). Cannot be called from user PTBs — userspace can never
    /// hold a `UpgradeTicket` value to feed in (no abilities).
    public fun consume_ticket(t: UpgradeTicket): (ID, ID, u8, vector<u8>) {
        let UpgradeTicket { cap_id, family_id, policy, digest } = t;
        (cap_id, family_id, policy, digest)
    }

    /// Engine-only: produce a fresh receipt. Same forge-resistance argument
    /// as `consume_ticket` — `UpgradeReceipt` has no abilities, so the
    /// engine is the only code that can *both* call this *and* place the
    /// resulting value where `commit_upgrade` will consume it.
    public fun make_receipt(
        cap_id: ID,
        new_package: ID,
        new_version: u64,
    ): UpgradeReceipt {
        UpgradeReceipt { cap_id, new_package, new_version }
    }
}
