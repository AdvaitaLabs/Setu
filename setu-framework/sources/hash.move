// ===== setu-framework/sources/hash.move =====
// Cryptographic hash primitives. All return 32-byte digests.
//
// Determinism: backed by pure-Rust crates `sha2 = "0.10"`, `sha3 = "0.10"`,
// `blake3` (workspace-pinned), `tiny-keccak = "2"`. No platform-dependent
// SIMD intrinsics that affect output bytes.
module setu::hash {
    /// SHA-2 256 (FIPS 180-4).
    public fun sha2_256(data: vector<u8>): vector<u8> {
        sha2_256_internal(data)
    }

    /// SHA-3 256 (FIPS 202, Keccak with NIST padding).
    public fun sha3_256(data: vector<u8>): vector<u8> {
        sha3_256_internal(data)
    }

    /// BLAKE3 (default 32-byte output). Used by Setu's Merkle tree.
    public fun blake3(data: vector<u8>): vector<u8> {
        blake3_internal(data)
    }

    /// Ethereum-flavoured Keccak-256 (pre-FIPS-202 padding).
    /// Use this when interoperating with EVM signatures or Ethereum addresses.
    public fun keccak256(data: vector<u8>): vector<u8> {
        keccak256_internal(data)
    }

    native fun sha2_256_internal(data: vector<u8>): vector<u8>;
    native fun sha3_256_internal(data: vector<u8>): vector<u8>;
    native fun blake3_internal(data: vector<u8>): vector<u8>;
    native fun keccak256_internal(data: vector<u8>): vector<u8>;
}
