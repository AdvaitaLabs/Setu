//! B3 (Phase 6) natives: BCS / hash / address / crypto.
//!
//! Registered under `0x1` alongside the existing 14 natives in [`crate::natives`].
//! All natives ship with `InternalGas::zero()` per design §5.4 R1d-ISSUE-1
//! (Option A — gas table deferred to a separate FDP `move-vm-native-gas-table`).
//!
//! Determinism (G1):
//! - BCS uses the workspace-pinned `bcs = "=0.1.6"` crate (see root `Cargo.toml`).
//! - Hash crates are pure-Rust, no platform-dependent intrinsics that affect output.
//! - ECDSA enforces low-s canonicalization to block signature malleability (R1c-ISSUE-1).
//! - Bytes are not allowed to carry `f64`/`f32`; only integer/struct natives.

use std::collections::VecDeque;

use move_binary_format::errors::{PartialVMError, PartialVMResult};
use move_core_types::{
    account_address::AccountAddress, gas_algebra::InternalGas, vm_status::StatusCode,
};
use move_vm_runtime::native_functions::NativeContext;
use move_vm_types::{
    loaded_data::runtime_types::Type,
    natives::function::NativeResult,
    values::{Reference, VMValueCast, Value, Vector},
};
use smallvec::smallvec;

// ═══════════════════════════════════════════════════════════════
// Abort codes (mirrors setu-framework/sources/{bcs,hash,crypto}.move)
// ═══════════════════════════════════════════════════════════════

/// Bytes failed BCS deserialization (malformed / truncated / non-canonical).
pub const E_BCS_BAD_BYTES: u64 = 100;
/// Bytes provided to `address::from_bytes` were not exactly 32 bytes.
pub const E_ADDRESS_BAD_LEN: u64 = 110;

// Note: `crypto::ed25519_verify` and `crypto::ecdsa_k1_verify` return `false`
// on any decoding/verification failure (no abort), so no error codes for the
// signature path are needed at the native boundary.

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

fn pop<T>(args: &mut VecDeque<Value>) -> PartialVMResult<T>
where
    Value: VMValueCast<T>,
{
    let v = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    VMValueCast::cast(v)
}

/// Extract a `vector<u8>` argument (top of the arg stack).
fn pop_vec_u8(args: &mut VecDeque<Value>) -> PartialVMResult<Vec<u8>> {
    let v = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let vec_obj: Vector = VMValueCast::cast(v)?;
    vec_obj.to_vec_u8()
}

fn ok_bytes(out: Vec<u8>) -> PartialVMResult<NativeResult> {
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::vector_u8(out)],
    ))
}

fn ok_bool(b: bool) -> PartialVMResult<NativeResult> {
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::bool(b)],
    ))
}

fn err_abort(code: u64) -> PartialVMResult<NativeResult> {
    Ok(NativeResult::err(InternalGas::zero(), code))
}

// ═══════════════════════════════════════════════════════════════
// bcs::to_bytes<T> (Phase 6.1) — thin wrapper over typed_serialize
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun to_bytes<T>(v: &T): vector<u8>;`
///
/// `&T` arg means the VM pushes a [`Reference`] onto the args stack, not
/// the underlying value — the native MUST `read_ref()` before serializing.
/// Mirrors Sui upstream `move-stdlib-natives/src/bcs.rs::native_to_bytes`
/// and the `&UID` deref pattern at [`crate::natives`] (`StructRef::read_ref`).
pub(crate) fn native_bcs_to_bytes(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let ref_to_val: Reference = pop(&mut args)?;
    let ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let layout = context
        .type_to_type_layout(&ty)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let val = ref_to_val.read_ref()?;
    let bytes = val
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;
    ok_bytes(bytes)
}

// ═══════════════════════════════════════════════════════════════
// bcs::from_bytes<T> (Phase 6.2) — type-tag-driven deserialization
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun from_bytes<T: drop>(bytes: vector<u8>): T;`
///
/// Architecturally novel piece per design §5.1: walks `MoveTypeLayout`
/// recursively via `Value::simple_deserialize`. The depth bound is enforced
/// by the layout itself (Move's type system already disallows infinite
/// recursion). On any deserialization failure (bad leb128, truncated bytes,
/// non-canonical encoding), aborts with `E_BCS_BAD_BYTES`.
///
/// Caller responsibility (per R1a-Q4):
/// - `T: drop` constraint is enforced at Move-compile time, not here.
/// - Caller MUST audit which types they accept; this native does not
///   discriminate between "safe" and "capability" types.
pub(crate) fn native_bcs_from_bytes(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let bytes = pop_vec_u8(&mut args)?;
    let ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let layout = context
        .type_to_type_layout(&ty)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    match Value::simple_deserialize(&bytes, &layout) {
        Some(v) => Ok(NativeResult::ok(InternalGas::zero(), smallvec![v])),
        None => err_abort(E_BCS_BAD_BYTES),
    }
}

// ═══════════════════════════════════════════════════════════════
// address::{from_bytes, to_bytes} (Phase 6.1)
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun from_bytes(bytes: vector<u8>): address;`
pub(crate) fn native_address_from_bytes(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let bytes = pop_vec_u8(&mut args)?;
    if bytes.len() != AccountAddress::LENGTH {
        return err_abort(E_ADDRESS_BAD_LEN);
    }
    let mut buf = [0u8; AccountAddress::LENGTH];
    buf.copy_from_slice(&bytes);
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::address(AccountAddress::new(buf))],
    ))
}

/// Move: `native fun to_bytes(addr: address): vector<u8>;`
pub(crate) fn native_address_to_bytes(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let addr: AccountAddress = pop(&mut args)?;
    ok_bytes(addr.into_bytes().to_vec())
}

// ═══════════════════════════════════════════════════════════════
// hash:: natives (Phase 6.1)
// ═══════════════════════════════════════════════════════════════

pub(crate) fn native_hash_sha2_256(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use sha2::{Digest, Sha256};
    let data = pop_vec_u8(&mut args)?;
    ok_bytes(Sha256::digest(&data).to_vec())
}

pub(crate) fn native_hash_sha3_256(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use sha3::{Digest, Sha3_256};
    let data = pop_vec_u8(&mut args)?;
    ok_bytes(Sha3_256::digest(&data).to_vec())
}

pub(crate) fn native_hash_blake3(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let data = pop_vec_u8(&mut args)?;
    ok_bytes(blake3::hash(&data).as_bytes().to_vec())
}

pub(crate) fn native_hash_keccak256(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use tiny_keccak::{Hasher, Keccak};
    let data = pop_vec_u8(&mut args)?;
    let mut k = Keccak::v256();
    k.update(&data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    ok_bytes(out.to_vec())
}

// ═══════════════════════════════════════════════════════════════
// ed25519::verify (Phase 6.3)
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun verify(sig: vector<u8>, pubkey: vector<u8>, msg: vector<u8>): bool;`
///
/// Returns `false` on any verification or decoding failure (no abort) —
/// Move callers can branch on the bool. Per design §5.3 Q2: domain
/// separation is the caller's responsibility; `crypto::verify_with_domain`
/// stdlib helper wraps this for safer default.
pub(crate) fn native_ed25519_verify(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    // Move arg order: (sig, pubkey, msg) — popped RTL: msg, pubkey, sig
    let msg = pop_vec_u8(&mut args)?;
    let pubkey = pop_vec_u8(&mut args)?;
    let sig = pop_vec_u8(&mut args)?;

    if pubkey.len() != ed25519_dalek::PUBLIC_KEY_LENGTH {
        return ok_bool(false);
    }
    if sig.len() != ed25519_dalek::SIGNATURE_LENGTH {
        return ok_bool(false);
    }
    let mut pk_arr = [0u8; ed25519_dalek::PUBLIC_KEY_LENGTH];
    pk_arr.copy_from_slice(&pubkey);
    let mut sig_arr = [0u8; ed25519_dalek::SIGNATURE_LENGTH];
    sig_arr.copy_from_slice(&sig);

    let verifying_key = match VerifyingKey::from_bytes(&pk_arr) {
        Ok(vk) => vk,
        Err(_) => return ok_bool(false),
    };
    let signature = Signature::from_bytes(&sig_arr);
    ok_bool(verifying_key.verify(&msg, &signature).is_ok())
}

// ═══════════════════════════════════════════════════════════════
// ecdsa_k1::verify (Phase 6.3) — secp256k1 with low-s enforcement
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun verify(sig: vector<u8>, pubkey: vector<u8>, msg_hash: vector<u8>): bool;`
///
/// `sig` is 64 bytes (r||s, IEEE compact form).
/// `pubkey` accepts both compressed (33 bytes) and uncompressed (65 bytes) SEC1.
/// `msg_hash` MUST already be a 32-byte digest (callers should use `hash::sha2_256` or `keccak256`).
///
/// Per design §5.3 R1c-ISSUE-1: enforces **low-s canonicalization** at the
/// native level to block signature-malleability (OWASP A02). High-s signatures
/// return `false` even if mathematically valid.
pub(crate) fn native_ecdsa_k1_verify(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use k256::ecdsa::{signature::hazmat::PrehashVerifier, Signature, VerifyingKey};

    let msg_hash = pop_vec_u8(&mut args)?;
    let pubkey = pop_vec_u8(&mut args)?;
    let sig = pop_vec_u8(&mut args)?;

    if msg_hash.len() != 32 {
        return ok_bool(false);
    }
    if sig.len() != 64 {
        return ok_bool(false);
    }
    if pubkey.len() != 33 && pubkey.len() != 65 {
        return ok_bool(false);
    }

    let signature = match Signature::from_slice(&sig) {
        Ok(s) => s,
        Err(_) => return ok_bool(false),
    };
    // Low-s enforcement: reject high-s even though k256's Signature::from_slice
    // accepts both. `normalize_s()` returns `Some(_)` iff the input was high-s.
    if signature.normalize_s().is_some() {
        return ok_bool(false);
    }
    let verifying_key = match VerifyingKey::from_sec1_bytes(&pubkey) {
        Ok(vk) => vk,
        Err(_) => return ok_bool(false),
    };
    ok_bool(verifying_key.verify_prehash(&msg_hash, &signature).is_ok())
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    //! Pure-Rust unit tests for the underlying crypto/hash crates.
    //! End-to-end native invocation tests live in `engine.rs` integration
    //! tests and shell tests under `tests/move_overlay/mo_natives_*`.

    #[test]
    fn sha2_256_nist_vector() {
        use sha2::{Digest, Sha256};
        // NIST CAVP: empty input
        let h = Sha256::digest(b"");
        assert_eq!(
            hex::encode(h),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha3_256_nist_vector() {
        use sha3::{Digest, Sha3_256};
        // FIPS 202: empty input
        let h = Sha3_256::digest(b"");
        assert_eq!(
            hex::encode(h),
            "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
        );
    }

    #[test]
    fn keccak256_known_vector() {
        use tiny_keccak::{Hasher, Keccak};
        let mut k = Keccak::v256();
        k.update(b"");
        let mut out = [0u8; 32];
        k.finalize(&mut out);
        // Ethereum keccak256("") well-known constant
        assert_eq!(
            hex::encode(out),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn ed25519_rfc8032_vector() {
        use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
        // RFC 8032 §7.1 test 1
        let secret_hex = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
        let pubkey_hex = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let msg: &[u8] = &[];
        let sig_hex = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";

        let mut sk = [0u8; 32];
        hex::decode_to_slice(secret_hex, &mut sk).unwrap();
        let signing = SigningKey::from_bytes(&sk);
        let signature: Signature = signing.sign(msg);
        assert_eq!(hex::encode(signature.to_bytes()), sig_hex);
        let vk = signing.verifying_key();
        assert_eq!(hex::encode(vk.to_bytes()), pubkey_hex);
        assert!(vk.verify(msg, &signature).is_ok());
    }

    #[test]
    fn ecdsa_k1_low_s_enforced() {
        use k256::ecdsa::{signature::Signer, Signature, SigningKey};
        use k256::elliptic_curve::rand_core::OsRng;
        let sk = SigningKey::random(&mut OsRng);
        let msg = b"test-message-for-ecdsa";
        let sig: Signature = sk.sign(msg);
        // k256's `sign` already produces low-s; normalize_s should be None.
        assert!(sig.normalize_s().is_none());
    }
}
