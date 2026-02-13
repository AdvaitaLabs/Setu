// Copyright (c) Hetu Project
// SPDX-License-Identifier: Apache-2.0

//! Utilities for reading and writing keypairs to files.

use crate::crypto::SetuKeyPair;
use crate::error::KeyError;
use std::path::Path;

/// Write a keypair to a file as Base64 encoded `flag || privkey`.
pub fn write_keypair_to_file<P: AsRef<Path>>(keypair: &SetuKeyPair, path: P) -> Result<(), KeyError> {
    let contents = keypair.encode_base64();
    std::fs::write(&path, contents)?;
    
    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = path.as_ref();
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    
    Ok(())
}

/// Read a keypair from a file encoded as Base64 `flag || privkey`.
pub fn read_keypair_from_file<P: AsRef<Path>>(path: P) -> Result<SetuKeyPair, KeyError> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(KeyError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Key file not found: {:?}", path),
        )));
    }
    let contents = std::fs::read_to_string(path)?;
    SetuKeyPair::decode_base64(contents.trim())
}

/// Read a keypair from a file, supporting multiple formats:
/// - Base64 encoded `flag || privkey`
/// - Hex encoded private key (assumes Ed25519)
/// - JSON format (from CLI keygen)
pub fn read_key<P: AsRef<Path>>(path: P) -> Result<SetuKeyPair, KeyError> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(KeyError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Key file not found: {:?}", path),
        )));
    }
    
    let contents = std::fs::read_to_string(path)?;
    let contents = contents.trim();
    
    // Try hex encoded private key first (32 bytes, most common for CLI-generated keys)
    if let Ok(bytes) = hex::decode(contents) {
        if bytes.len() == 32 {
            // Raw 32-byte private key - try Secp256k1 first (validator keys)
            if let Ok(kp) = SetuKeyPair::from_bytes(crate::crypto::SignatureScheme::Secp256k1, &bytes) {
                return Ok(kp);
            }
            // Fallback to Ed25519
            return SetuKeyPair::from_bytes(crate::crypto::SignatureScheme::ED25519, &bytes);
        }
    }
    
    // Try Base64 encoded SetuKeyPair
    if let Ok(kp) = SetuKeyPair::decode_base64(contents) {
        return Ok(kp);
    }
    
    // Try JSON format (from CLI keygen)
    if contents.starts_with('{') {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(contents) {
            if let Some(private_key_str) = json.get("private_key").and_then(|v| v.as_str()) {
                if let Ok(bytes) = hex::decode(private_key_str) {
                    // Raw 32-byte private key from JSON - use Secp256k1 for validator keys
                    if bytes.len() == 32 {
                        return SetuKeyPair::from_bytes(crate::crypto::SignatureScheme::Secp256k1, &bytes);
                    }
                }
            }
        }
    }
    
    Err(KeyError::InvalidKeyFormat(
        "Could not parse key file in any supported format".to_string(),
    ))
}

/// Write a keypair to a file as hex-encoded private key (for interoperability).
pub fn write_keypair_hex<P: AsRef<Path>>(keypair: &SetuKeyPair, path: P) -> Result<(), KeyError> {
    // Get the raw secret bytes (without the flag)
    let encoded = keypair.encode_base64();
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
        .map_err(|e| KeyError::Decoding(e.to_string()))?;
    
    // Skip the first byte (flag)
    let hex_str = hex::encode(&bytes[1..]);
    std::fs::write(&path, hex_str)?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = path.as_ref();
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::SignatureScheme;
    use tempfile::tempdir;

    #[test]
    fn test_write_read_keypair() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("keypair.key");

        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        write_keypair_to_file(&kp, &path).unwrap();

        let loaded = read_keypair_from_file(&path).unwrap();
        assert_eq!(kp.address(), loaded.address());
    }

    #[test]
    fn test_read_key_formats() {
        let dir = tempdir().unwrap();

        // Test Base64 format
        let path = dir.path().join("base64.key");
        let kp = SetuKeyPair::generate(SignatureScheme::ED25519);
        write_keypair_to_file(&kp, &path).unwrap();
        let loaded = read_key(&path).unwrap();
        assert_eq!(kp.address(), loaded.address());

        // Test hex format
        let hex_path = dir.path().join("hex.key");
        write_keypair_hex(&kp, &hex_path).unwrap();
        let loaded_hex = read_key(&hex_path).unwrap();
        assert_eq!(kp.address(), loaded_hex.address());
    }

    #[test]
    fn test_file_not_found() {
        let result = read_keypair_from_file("/nonexistent/path/key.file");
        assert!(result.is_err());
    }
}
