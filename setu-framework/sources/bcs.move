// ===== setu-framework/sources/bcs.move =====
// BCS (Binary Canonical Serialization) helpers.
//
// `to_bytes<T>` is the canonical serialization used everywhere in Setu
// (events, state, hashing). Wraps the same internal path that
// `event::emit_internal` uses, so emitted events and `bcs::to_bytes` of the
// same struct produce byte-identical output.
//
// `from_bytes<T>` enforces `T: drop` at the Move-compile boundary so that
// callers cannot mint capability/resource types out of thin air. Aborts with
// `EBadBytes (100)` on malformed input.
module setu::bcs {
    /// Bytes failed BCS deserialization (truncated / non-canonical / type mismatch).
    const EBadBytes: u64 = 100;

    /// Canonically serialize any value to bytes.
    public fun to_bytes<T>(v: &T): vector<u8> {
        to_bytes_internal<T>(v)
    }

    /// Canonically deserialize bytes back to a value.
    /// Aborts with `EBadBytes` if the input is malformed for `T`'s layout.
    /// `T: drop` is enforced so capability types cannot be forged.
    public fun from_bytes<T: drop>(bytes: vector<u8>): T {
        from_bytes_internal<T>(bytes)
    }

    native fun to_bytes_internal<T>(v: &T): vector<u8>;
    native fun from_bytes_internal<T: drop>(bytes: vector<u8>): T;
}
