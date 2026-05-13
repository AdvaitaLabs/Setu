// ===== setu-framework/sources/access_control.move =====
// Canonical AdminCap + shared-policy pattern for PWOO.
//
// Motivation
// ----------
// PWOO (Phase-1 Writable Owned Objects) lets Move contracts create objects
// whose `Ownership` is `Shared { initial_shared_version }`. Any sender can
// route such an object into a MoveCall via `shared_object_ids`, so access
// control must be enforced inside Move logic, not by object ownership.
//
// The canonical pattern is:
//   1. A module issues an `AdminCap` once at publish time and transfers it to
//      the deployer (an `AddressOwner` object, usable only by its holder).
//   2. Sensitive mutators on Shared policy objects take `&AdminCap` as a
//      parameter; the mere presence of the capability in the sender's owned
//      inputs proves authorization.
//   3. The `only_admin` assertion centralizes capability binding: callers pass
//      their `AdminCap` and the policy they are mutating, and we check that
//      the capability was issued for this exact policy id.
//
// This module intentionally stays minimal — it provides only the primitives
// that downstream apps compose, not a full RBAC framework.
module setu::access_control {
    use setu::object::{Self, UID, ID};
    use setu::tx_context::{Self, TxContext};
    use setu::transfer;

    /// Capability representing "admin of policy `policy_id`".
    /// Holding this cap authorizes calls that assert `only_admin`.
    struct AdminCap has key {
        id: UID,
        /// The ID of the shared policy object this cap governs.
        policy_id: ID,
    }

    /// Error: supplied AdminCap does not match the target policy.
    const E_NOT_ADMIN: u64 = 1;

    /// Issue an `AdminCap` for a freshly-created policy and transfer it to the
    /// publisher (`ctx.sender()`). Call this once, immediately after creating
    /// the shared policy object.
    public fun issue_admin_cap(policy_id: ID, ctx: &mut TxContext) {
        let cap = AdminCap { id: object::new(ctx), policy_id };
        transfer::transfer(cap, tx_context::sender(ctx));
    }

    /// Abort unless `cap` is the admin capability for `policy_id`.
    public fun only_admin(cap: &AdminCap, policy_id: ID) {
        assert!(cap.policy_id == policy_id, E_NOT_ADMIN);
    }

    /// Read the policy id this capability governs.
    public fun cap_policy_id(cap: &AdminCap): ID {
        cap.policy_id
    }
}
