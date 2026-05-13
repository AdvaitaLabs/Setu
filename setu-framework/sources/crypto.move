// ===== setu-framework/sources/crypto.move =====
// Public-key signature verification primitives.
//
// All functions return `bool` (no aborts on invalid input) so callers can
// branch. Encoding errors (wrong length pubkey/signature) yield `false`.
//
// Determinism / security:
// - `ed25519::verify` — RFC 8032 Ed25519. Strict mode (no malleable
//   non-canonical signatures, enforced by `ed25519-dalek 2.x`).
// - `ecdsa_k1::verify` — secp256k1 ECDSA with **low-s canonicalization
//   enforced at the native level**. High-s signatures return `false` even
//   if mathematically valid (blocks signature malleability — OWASP A02).
//   `msg_hash` MUST be a 32-byte digest already (caller hashes via
//   `hash::sha2_256` or `hash::keccak256`).
module setu::crypto {
    /// Verify an Ed25519 signature.
    /// `sig` MUST be 64 bytes, `pubkey` MUST be 32 bytes.
    /// Returns `false` on length mismatch or invalid signature.
    public fun ed25519_verify(
        sig: vector<u8>,
        pubkey: vector<u8>,
        msg: vector<u8>,
    ): bool {
        ed25519_verify_internal(sig, pubkey, msg)
    }

    /// Verify a secp256k1 ECDSA signature with low-s enforcement.
    /// `sig` MUST be 64 bytes (compact r||s); `pubkey` may be SEC1
    /// compressed (33 bytes) or uncompressed (65 bytes); `msg_hash`
    /// MUST be exactly 32 bytes (pre-hashed by the caller).
    /// Returns `false` on length mismatch, high-s, or invalid signature.
    public fun ecdsa_k1_verify(
        sig: vector<u8>,
        pubkey: vector<u8>,
        msg_hash: vector<u8>,
    ): bool {
        ecdsa_k1_verify_internal(sig, pubkey, msg_hash)
    }

    native fun ed25519_verify_internal(
        sig: vector<u8>,
        pubkey: vector<u8>,
        msg: vector<u8>,
    ): bool;

    native fun ecdsa_k1_verify_internal(
        sig: vector<u8>,
        pubkey: vector<u8>,
        msg_hash: vector<u8>,
    ): bool;
}
