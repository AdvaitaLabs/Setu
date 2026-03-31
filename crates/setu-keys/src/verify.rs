// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Signature verification utilities for user registration.
//!
//! Three registration paths:
//! - **Setu native** (Ed25519 / Secp256k1 / Secp256r1): standard `PublicKey::verify()`
//! - **MetaMask**: secp256k1 ECDSA with Ethereum `personal_sign` prefix + recovery
//! - **Nostr**: Schnorr BIP-340 (x-only key)

use crate::crypto::{PublicKey, SetuAddress, Signature};
use crate::address_derive::{derive_address_from_secp256k1, derive_address_from_nostr_pubkey, address_from_hex};
use crate::error::KeyError;

/// Verify a Setu-native signature (Ed25519, Secp256k1, or Secp256r1).
///
/// 1. Decodes `public_key_b64` → [`PublicKey`] (flag ‖ pk_bytes, base64)
/// 2. Decodes `signature_b64`  → [`Signature`] (flag ‖ sig_bytes, base64)
/// 3. `pk.verify(message, sig)`
/// 4. Checks `address == SetuAddress::from(&pk).to_hex()`
pub fn verify_setu_native(
    address: &str,
    public_key_b64: &str,
    signature_b64: &str,
    message: &[u8],
) -> Result<(), KeyError> {
    let pk = PublicKey::decode_base64(public_key_b64)?;
    let sig = Signature::decode_base64(signature_b64)?;
    verify_setu_native_inner(address, &pk, &sig, message)
}

/// Same as [`verify_setu_native`] but takes raw flag-prefixed bytes
/// (`flag ‖ key_bytes` / `flag ‖ sig_bytes`) instead of base64 strings.
pub fn verify_setu_native_raw(
    address: &str,
    public_key_bytes: &[u8],
    signature_bytes: &[u8],
    message: &[u8],
) -> Result<(), KeyError> {
    if public_key_bytes.is_empty() {
        return Err(KeyError::Decoding("Empty public key bytes".to_string()));
    }
    if signature_bytes.is_empty() {
        return Err(KeyError::Decoding("Empty signature bytes".to_string()));
    }
    let scheme = crate::crypto::SignatureScheme::from_flag(public_key_bytes[0])?;
    let pk = PublicKey::from_bytes(scheme, &public_key_bytes[1..])?;

    let sig_scheme = crate::crypto::SignatureScheme::from_flag(signature_bytes[0])?;
    let sig = Signature::from_bytes(sig_scheme, &signature_bytes[1..])?;

    verify_setu_native_inner(address, &pk, &sig, message)
}

fn verify_setu_native_inner(
    address: &str,
    pk: &PublicKey,
    sig: &Signature,
    message: &[u8],
) -> Result<(), KeyError> {

    // Verify cryptographic signature
    pk.verify(message, sig)?;

    // Verify address matches public key (Blake2b-256 derivation)
    let expected = SetuAddress::from(pk).to_hex();
    if !address.eq_ignore_ascii_case(&expected) {
        return Err(KeyError::SignatureVerification(
            "Address does not match public key".to_string(),
        ));
    }

    Ok(())
}

/// Verify a MetaMask `personal_sign` signature.
///
/// Ethereum signs: `keccak256("\x19Ethereum Signed Message:\n" + len(msg) + msg)`
///
/// Signature is 65 bytes: `r(32) ‖ s(32) ‖ v(1)`.
/// Recovers the secp256k1 public key, derives the 32-byte Setu address
/// (Keccak-256 of uncompressed key), and compares with `address`.
pub fn verify_metamask_personal_sign(
    address: &str,
    signature: &[u8],
    message: &str,
) -> Result<(), KeyError> {
    if signature.len() != 65 {
        return Err(KeyError::SignatureVerification(format!(
            "MetaMask signature must be 65 bytes, got {}",
            signature.len()
        )));
    }

    // 1. Ethereum personal_sign hash  (Keccak-256, NOT SHA3-256)
    use sha3::{Keccak256, Digest};
    let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", message.len(), message);
    let hash = Keccak256::digest(prefixed.as_bytes());

    // 2. Recovery ID from v byte
    let v = signature[64];
    let is_y_odd = match v {
        27 | 0 => false,
        28 | 1 => true,
        _ => {
            return Err(KeyError::SignatureVerification(format!(
                "Invalid recovery ID v={}",
                v
            )))
        }
    };

    // 3. Recover public key
    let ecdsa_sig = k256::ecdsa::Signature::from_slice(&signature[..64])
        .map_err(|e| KeyError::SignatureVerification(e.to_string()))?;
    let recid = k256::ecdsa::RecoveryId::new(is_y_odd, false);
    let recovered_key =
        k256::ecdsa::VerifyingKey::recover_from_prehash(hash.as_slice(), &ecdsa_sig, recid)
            .map_err(|e| KeyError::SignatureVerification(e.to_string()))?;

    // 4. Derive Setu address (Keccak-256 of uncompressed pubkey — NOT Blake2b)
    use k256::elliptic_curve::sec1::ToEncodedPoint as _;
    let uncompressed = recovered_key.to_encoded_point(false);
    let derived = derive_address_from_secp256k1(uncompressed.as_bytes())?;
    let provided = address_from_hex(address)?;

    if derived != provided {
        return Err(KeyError::SignatureVerification(
            "Recovered address does not match provided address".to_string(),
        ));
    }

    Ok(())
}

/// Verify a Nostr Schnorr BIP-340 signature.
///
/// 1. Verifies `address == derive_address_from_nostr_pubkey(nostr_pubkey)`
/// 2. Constructs x-only [`k256::schnorr::VerifyingKey`]
/// 3. Verifies the Schnorr signature
pub fn verify_nostr_schnorr(
    address: &str,
    nostr_pubkey: &[u8],
    signature: &[u8],
    message: &[u8],
) -> Result<(), KeyError> {
    // 1. Address ↔ nostr_pubkey consistency
    let derived = derive_address_from_nostr_pubkey(nostr_pubkey)?;
    let provided = address_from_hex(address)?;
    if derived != provided {
        return Err(KeyError::SignatureVerification(
            "Address does not match Nostr public key".to_string(),
        ));
    }

    // 2. Build x-only verifying key
    let vk = k256::schnorr::VerifyingKey::from_bytes(nostr_pubkey)
        .map_err(|e| KeyError::SignatureVerification(e.to_string()))?;

    // 3. Parse signature (64 bytes)
    let sig = k256::schnorr::Signature::try_from(signature)
        .map_err(|e| KeyError::SignatureVerification(e.to_string()))?;

    // 4. Verify
    use signature::Verifier;
    vk.verify(message, &sig)
        .map_err(|e| KeyError::SignatureVerification(e.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{SetuKeyPair, SignatureScheme};
    use crate::address_derive::address_to_hex;

    // ── Setu native ─────────────────────────────────────────────

    #[test]
    fn test_verify_setu_native_ed25519() {
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        let address = kp.address().to_hex();
        let msg = b"Register to Setu: 1711234567890";
        let sig = kp.sign(msg);

        let pk_b64 = kp.public().encode_base64();
        let sig_b64 = sig.encode_base64();

        verify_setu_native(&address, &pk_b64, &sig_b64, msg).expect("Ed25519 verify should pass");
    }

    #[test]
    fn test_verify_setu_native_secp256k1() {
        let kp = SetuKeyPair::generate(SignatureScheme::Secp256k1);
        let address = kp.address().to_hex();
        let msg = b"Register to Setu: 1711234567890";
        let sig = kp.sign(msg);

        let pk_b64 = kp.public().encode_base64();
        let sig_b64 = sig.encode_base64();

        verify_setu_native(&address, &pk_b64, &sig_b64, msg)
            .expect("Secp256k1 verify should pass");
    }

    #[test]
    fn test_verify_setu_native_wrong_signature() {
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        let address = kp.address().to_hex();
        let msg = b"Register to Setu: 1711234567890";

        // Sign a different message
        let wrong_sig = kp.sign(b"wrong message");
        let pk_b64 = kp.public().encode_base64();
        let sig_b64 = wrong_sig.encode_base64();

        let result = verify_setu_native(&address, &pk_b64, &sig_b64, msg);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_setu_native_wrong_address() {
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        let msg = b"Register to Setu: 1711234567890";
        let sig = kp.sign(msg);

        let pk_b64 = kp.public().encode_base64();
        let sig_b64 = sig.encode_base64();

        // Use a different address (all zeros)
        let fake_addr = format!("0x{}", "00".repeat(32));
        let result = verify_setu_native(&fake_addr, &pk_b64, &sig_b64, msg);
        assert!(result.is_err());
    }

    // ── MetaMask personal_sign ──────────────────────────────────

    #[test]
    fn test_verify_metamask_personal_sign() {
        use k256::ecdsa::SigningKey;
        use k256::elliptic_curve::sec1::ToEncodedPoint as _;

        let sk = SigningKey::random(&mut rand::thread_rng());
        let vk = sk.verifying_key();

        // Derive Setu address (Keccak-256 of uncompressed key)
        let uncompressed = vk.to_encoded_point(false);
        let addr_bytes = derive_address_from_secp256k1(uncompressed.as_bytes()).unwrap();
        let address = address_to_hex(&addr_bytes);

        // Build personal_sign hash
        let message = "Register to Setu: 1711234567890";
        let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", message.len(), message);

        use sha3::{Keccak256, Digest};
        let hash = Keccak256::digest(prefixed.as_bytes());

        // Sign with recovery
        let (ecdsa_sig, recid) = sk
            .sign_prehash_recoverable(hash.as_slice())
            .expect("prehash sign");

        // Build 65-byte MetaMask signature: r(32) || s(32) || v(1)
        let mut sig_bytes = ecdsa_sig.to_bytes().to_vec(); // 64 bytes
        sig_bytes.push(recid.to_byte() + 27); // Ethereum convention

        verify_metamask_personal_sign(&address, &sig_bytes, message)
            .expect("MetaMask verify should pass");
    }

    #[test]
    fn test_verify_metamask_wrong_v() {
        let result = verify_metamask_personal_sign(
            &format!("0x{}", "ab".repeat(32)),
            &[0u8; 65],              // v = 0 at index 64 is valid
            "hello",
        );
        // This will fail at signature recovery, not at v-check (v=0 is valid).
        // To test invalid v, set byte 64 to 99:
        let mut bad_sig = [0u8; 65];
        bad_sig[64] = 99;
        let result2 = verify_metamask_personal_sign(
            &format!("0x{}", "ab".repeat(32)),
            &bad_sig,
            "hello",
        );
        assert!(result2.is_err());
        assert!(
            format!("{}", result2.unwrap_err()).contains("Invalid recovery ID"),
            "should mention invalid recovery ID"
        );
        // also accept generic error from first call
        let _ = result;
    }

    #[test]
    fn test_verify_metamask_wrong_address() {
        use k256::ecdsa::SigningKey;
        use k256::elliptic_curve::sec1::ToEncodedPoint as _;

        let sk = SigningKey::random(&mut rand::thread_rng());

        let message = "Register to Setu: 1711234567890";
        let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", message.len(), message);

        use sha3::{Keccak256, Digest};
        let hash = Keccak256::digest(prefixed.as_bytes());

        let (ecdsa_sig, recid) = sk
            .sign_prehash_recoverable(hash.as_slice())
            .expect("prehash sign");

        let mut sig_bytes = ecdsa_sig.to_bytes().to_vec();
        sig_bytes.push(recid.to_byte() + 27);

        // Use wrong address
        let fake_addr = format!("0x{}", "00".repeat(32));
        let result = verify_metamask_personal_sign(&fake_addr, &sig_bytes, message);
        assert!(result.is_err());
    }

    // ── Nostr Schnorr ───────────────────────────────────────────

    #[test]
    fn test_verify_nostr_schnorr() {
        use k256::schnorr::SigningKey;

        let sk = SigningKey::random(&mut rand::thread_rng());
        let vk = sk.verifying_key();
        let nostr_pubkey = vk.to_bytes();

        // Derive address
        let addr_bytes = derive_address_from_nostr_pubkey(&nostr_pubkey).unwrap();
        let address = address_to_hex(&addr_bytes);

        // Sign
        let msg = b"Register to Setu: 1711234567890";
        use signature::Signer;
        let sig: k256::schnorr::Signature = sk.sign(msg);

        verify_nostr_schnorr(&address, &nostr_pubkey, &sig.to_bytes(), msg)
            .expect("Nostr Schnorr verify should pass");
    }

    #[test]
    fn test_verify_nostr_wrong_signature() {
        use k256::schnorr::SigningKey;

        let sk = SigningKey::random(&mut rand::thread_rng());
        let vk = sk.verifying_key();
        let nostr_pubkey = vk.to_bytes();

        let addr_bytes = derive_address_from_nostr_pubkey(&nostr_pubkey).unwrap();
        let address = address_to_hex(&addr_bytes);

        // Sign wrong message
        use signature::Signer;
        let wrong_sig: k256::schnorr::Signature = sk.sign(b"wrong");

        let result =
            verify_nostr_schnorr(&address, &nostr_pubkey, &wrong_sig.to_bytes(), b"correct");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_nostr_address_mismatch() {
        use k256::schnorr::SigningKey;

        let sk = SigningKey::random(&mut rand::thread_rng());
        let vk = sk.verifying_key();
        let nostr_pubkey = vk.to_bytes();

        let msg = b"Register to Setu: 1711234567890";
        use signature::Signer;
        let sig: k256::schnorr::Signature = sk.sign(msg);

        // Wrong address
        let fake_addr = format!("0x{}", "00".repeat(32));
        let result = verify_nostr_schnorr(&fake_addr, &nostr_pubkey, &sig.to_bytes(), msg);
        assert!(result.is_err());
        assert!(
            format!("{}", result.unwrap_err()).contains("does not match"),
            "should mention address mismatch"
        );
    }
}
