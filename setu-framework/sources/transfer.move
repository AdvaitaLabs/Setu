// ===== setu-framework/sources/transfer.move =====
// Ownership transfer operations. All functions delegate to native implementations.
// Native functions implemented in setu-move-vm/src/natives.rs
module setu::transfer {
    /// Transfer an owned object to a new owner
    public fun transfer<T: key>(obj: T, recipient: address) {
        transfer_internal(obj, recipient);
    }

    /// Make an object publicly shared (PWOO Phase 1+).
    /// The object becomes `Ownership::Shared { initial_shared_version }` at the
    /// storage layer. Subsequent MoveCalls must reference it via the
    /// `shared_object_ids` list so the validator can track concurrent-swap
    /// conflicts at the byte-level envelope boundary.
    public fun share_object<T: key>(obj: T) {
        share_internal(obj);
    }

    /// Freeze an object as immutable
    /// After freezing, everyone can read but no one can write
    public fun freeze_object<T: key>(obj: T) {
        freeze_internal(obj);
    }

    // ── Native functions (implemented in natives.rs §9) ──
    native fun transfer_internal<T: key>(obj: T, recipient: address);
    native fun share_internal<T: key>(obj: T);
    native fun freeze_internal<T: key>(obj: T);
}
