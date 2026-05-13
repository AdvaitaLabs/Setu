// ===== setu-framework/sources/object.move =====
// Object lifecycle: UID, ID, creation, deletion, address extraction.
// Native functions implemented in setu-move-vm/src/natives.rs
module setu::object {
    use setu::tx_context::TxContext;

    /// Unique identifier — every Setu object's first field must be UID
    struct UID has store {
        id: ID,
    }

    /// Object ID (32-byte address wrapper)
    struct ID has store, drop, copy {
        bytes: address,
    }

    /// Create a new UID (called only during object construction)
    /// Internally uses deterministic ID: BLAKE3("SETU_FRESH_ID:" || tx_hash || counter)
    public fun new(ctx: &mut TxContext): UID {
        UID { id: new_uid_internal(ctx) }
    }

    /// Destroy UID (must be called when an object is deconstructed/deleted)
    public fun delete(uid: UID) {
        let UID { id } = uid;
        delete_uid_internal(id);
    }

    /// Extract address from UID (immutable reference)
    public fun uid_to_address(uid: &UID): address {
        uid_to_address_internal(uid)
    }

    /// Extract address from ID
    public fun id_to_address(id: &ID): address {
        id.bytes
    }

    /// Get reference to ID from UID
    public fun uid_to_inner(uid: &UID): &ID {
        &uid.id
    }

    /// Get copy of ID from UID
    public fun uid_as_inner(uid: &UID): ID {
        uid.id
    }

    /// Create ID from address
    public fun id_from_address(addr: address): ID {
        ID { bytes: addr }
    }

    // ── Native functions (implemented in natives.rs §9) ──
    native fun new_uid_internal(ctx: &mut TxContext): ID;
    native fun delete_uid_internal(id: ID);
    native fun uid_to_address_internal(uid: &UID): address;
}
