// ===== setu-framework/sources/address.move =====
// Helpers for converting between `address` and 32-byte vectors.
module setu::address {
    /// Bytes provided to `from_bytes` were not exactly 32 bytes long.
    const EBadLen: u64 = 110;

    /// Build an `address` from a 32-byte vector. Aborts with `EBadLen`
    /// (110) if the input length is not 32.
    public fun from_bytes(bytes: vector<u8>): address {
        from_bytes_internal(bytes)
    }

    /// Serialize an `address` to its canonical 32-byte representation.
    public fun to_bytes(addr: address): vector<u8> {
        to_bytes_internal(addr)
    }

    native fun from_bytes_internal(bytes: vector<u8>): address;
    native fun to_bytes_internal(addr: address): vector<u8>;
}
