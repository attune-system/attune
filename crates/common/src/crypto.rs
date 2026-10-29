//! Cryptographic utilities for encrypting and decrypting sensitive data
//!
//! This module provides functions for encrypting and decrypting secret values
//! using AES-256-GCM encryption with randomly generated nonces.
//!
//! ## JSON value encryption
//!
//! [`encrypt_json`] / [`decrypt_json`] operate on [`serde_json::Value`] values.
//! The JSON value is serialised to its compact string form before encryption,
//! and the resulting ciphertext is stored as a JSON string (`Value::String`).
//! This means the JSONB column always holds a plain JSON string when encrypted,
//! and the original structured value is recovered after decryption.

use crate::{Error, Result};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

pub mod keystore;

/// Size of the nonce in bytes (96 bits for AES-GCM)
const NONCE_SIZE: usize = 12;

/// Encrypt a plaintext value using AES-256-GCM
///
/// The encryption key is derived from the provided key string using SHA-256.
/// A random nonce is generated for each encryption operation.
/// The returned ciphertext is base64-encoded and contains: nonce || encrypted_data || tag
///
/// # Arguments
/// * `plaintext` - The plaintext value to encrypt
/// * `encryption_key` - The encryption key (will be hashed with SHA-256)
///
/// # Returns
/// Base64-encoded encrypted value
pub fn encrypt(plaintext: &str, encryption_key: &str) -> Result<String> {
    if encryption_key.len() < 32 {
        return Err(Error::encryption(
            "Encryption key must be at least 32 characters",
        ));
    }

    // Derive a 256-bit key from the encryption key using SHA-256
    let key_bytes = derive_key(encryption_key);
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| Error::encryption(format!("Invalid key length: {e}")))?;

    // Generate a random nonce
    let nonce_bytes = generate_nonce();
    let nonce = Nonce::from(nonce_bytes);

    // Encrypt the plaintext
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| Error::encryption(format!("Encryption failed: {}", e)))?;

    // Combine nonce + ciphertext and encode as base64
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(BASE64.encode(&result))
}

/// Encrypt a [`JsonValue`] using AES-256-GCM.
///
/// The value is first serialised to its compact JSON string representation,
/// then encrypted with [`encrypt`]. The returned value is a
/// [`JsonValue::String`] containing the base64 ciphertext, suitable for
/// storage in a JSONB column.
pub fn encrypt_json(value: &JsonValue, encryption_key: &str) -> Result<JsonValue> {
    let plaintext = serde_json::to_string(value)
        .map_err(|e| Error::encryption(format!("Failed to serialise JSON for encryption: {e}")))?;
    let ciphertext = encrypt(&plaintext, encryption_key)?;
    Ok(JsonValue::String(ciphertext))
}

/// Decrypt a [`JsonValue`] that was previously encrypted with [`encrypt_json`].
///
/// The input must be a [`JsonValue::String`] containing a base64 ciphertext.
/// After decryption the JSON string is parsed back into the original
/// structured [`JsonValue`].
pub fn decrypt_json(value: &JsonValue, encryption_key: &str) -> Result<JsonValue> {
    let ciphertext = value
        .as_str()
        .ok_or_else(|| Error::encryption("Encrypted JSON value must be a string"))?;
    let plaintext = decrypt(ciphertext, encryption_key)?;
    serde_json::from_str(&plaintext)
        .map_err(|e| Error::encryption(format!("Failed to parse decrypted JSON: {e}")))
}

/// Decrypt a ciphertext value using AES-256-GCM
///
/// The ciphertext should be base64-encoded and contain: nonce || encrypted_data || tag
///
/// # Arguments
/// * `ciphertext` - Base64-encoded encrypted value
/// * `encryption_key` - The encryption key (will be hashed with SHA-256)
///
/// # Returns
/// Decrypted plaintext value
pub fn decrypt(ciphertext: &str, encryption_key: &str) -> Result<String> {
    if encryption_key.len() < 32 {
        return Err(Error::encryption(
            "Encryption key must be at least 32 characters",
        ));
    }

    // Decode base64
    let encrypted_data = BASE64
        .decode(ciphertext)
        .map_err(|e| Error::encryption(format!("Invalid base64: {}", e)))?;

    if encrypted_data.len() < NONCE_SIZE {
        return Err(Error::encryption("Invalid ciphertext: too short"));
    }

    // Split nonce and ciphertext
    let (nonce_bytes, ciphertext_bytes) = encrypted_data.split_at(NONCE_SIZE);
    let nonce_bytes: [u8; NONCE_SIZE] = nonce_bytes
        .try_into()
        .map_err(|_| Error::encryption("Invalid ciphertext: invalid nonce length"))?;
    let nonce = Nonce::from(nonce_bytes);

    // Derive the key
    let key_bytes = derive_key(encryption_key);
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| Error::encryption(format!("Invalid key length: {e}")))?;

    // Decrypt
    let plaintext_bytes = cipher
        .decrypt(&nonce, ciphertext_bytes)
        .map_err(|e| Error::encryption(format!("Decryption failed: {}", e)))?;

    String::from_utf8(plaintext_bytes)
        .map_err(|e| Error::encryption(format!("Invalid UTF-8 in decrypted data: {}", e)))
}

/// Derive a 256-bit key from the encryption key string using SHA-256
fn derive_key(encryption_key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(encryption_key.as_bytes());
    let result = hasher.finalize();
    result.into()
}

/// Generate a random 96-bit nonce for AES-GCM
fn generate_nonce() -> [u8; NONCE_SIZE] {
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Compute a one-way, domain-separated verifier for the configured encryption key.
///
/// This is used to verify that the correct encryption key is being used
/// without storing usable AES key material. Never persist `derive_key` directly:
/// those bytes decrypt every value encrypted with the same configured secret.
/// The configured secret must have at least 256 bits of random entropy; this
/// fast verifier is not a password-hardening function.
///
/// Older persisted verifiers require explicit data conversion. Do not accept
/// the old AES-key-as-verifier representation as a runtime fallback.
pub fn hash_encryption_key(encryption_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"attune:encryption-key-fingerprint\0");
    hasher.update(derive_key(encryption_key));
    let result = hasher.finalize();
    result.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = "this_is_a_test_key_that_is_32_chars_long!!!!";

    #[test]
    fn stored_fingerprint_cannot_decrypt_json() {
        let value = serde_json::json!({"credential": "synthetic-only"});
        let stored = encrypt_json(&value, TEST_KEY).unwrap();
        let fingerprint = hash_encryption_key(TEST_KEY);
        let bytes: Vec<u8> = (0..fingerprint.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fingerprint[i..i + 2], 16).unwrap())
            .collect();
        let encoded = BASE64.decode(stored.as_str().unwrap()).unwrap();
        let nonce: [u8; NONCE_SIZE] = encoded[..NONCE_SIZE].try_into().unwrap();
        let recovered = Aes256Gcm::new_from_slice(&bytes)
            .unwrap()
            .decrypt(&Nonce::from(nonce), &encoded[NONCE_SIZE..]);
        assert!(
            recovered.is_err(),
            "Persisted fingerprint permits raw AES-GCM decryption"
        );
    }

    #[test]
    fn fingerprint_is_domain_separated_from_aes_key() {
        let aes_key = Sha256::digest(TEST_KEY.as_bytes());
        let mut expected = Sha256::new();
        expected.update(b"attune:encryption-key-fingerprint\0");
        expected.update(aes_key);
        let fingerprint = hash_encryption_key(TEST_KEY);
        let expected_hex: String = expected
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let aes_hex: String = aes_key.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(fingerprint, expected_hex);
        assert!(
            fingerprint != aes_hex,
            "Fingerprint must not be AES key material"
        );
    }

    #[test]
    fn ciphertext_tampering_is_rejected() {
        let ciphertext = encrypt("synthetic-only", TEST_KEY).unwrap();
        let encoded = BASE64.decode(ciphertext).unwrap();
        for position in [0, NONCE_SIZE, encoded.len() - 1] {
            let mut tampered = encoded.clone();
            tampered[position] ^= 1;
            assert!(decrypt(&BASE64.encode(tampered), TEST_KEY).is_err());
        }
    }

    #[test]
    fn existing_ciphertext_key_derivation_and_encoding_are_preserved() {
        // Fixed synthetic nonce, only for a deterministic historical-format fixture.
        let nonce = [7u8; NONCE_SIZE];
        let aes_key = Sha256::digest(TEST_KEY.as_bytes());
        let value = serde_json::json!({"historical": "synthetic-only"});
        let ciphertext = Aes256Gcm::new_from_slice(&aes_key)
            .unwrap()
            .encrypt(&Nonce::from(nonce), value.to_string().as_bytes())
            .unwrap();
        let fixture = BASE64.encode([nonce.as_slice(), ciphertext.as_slice()].concat());
        assert_eq!(
            decrypt_json(&JsonValue::String(fixture), TEST_KEY).unwrap(),
            value
        );
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = "my_secret_password";
        let encrypted = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");
        let decrypted = decrypt(&encrypted, TEST_KEY).expect("Decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_encrypt_produces_different_output() {
        let plaintext = "my_secret_password";
        let encrypted1 = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");
        let encrypted2 = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");

        // Should produce different ciphertext due to random nonce
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same value
        let decrypted1 = decrypt(&encrypted1, TEST_KEY).expect("Decryption should succeed");
        let decrypted2 = decrypt(&encrypted2, TEST_KEY).expect("Decryption should succeed");
        assert_eq!(decrypted1, decrypted2);
        assert_eq!(plaintext, decrypted1);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let plaintext = "my_secret_password";
        let encrypted = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");

        let wrong_key = "wrong_key_that_is_also_32_chars_long!!!";
        let result = decrypt(&encrypted, wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_with_short_key_fails() {
        let plaintext = "my_secret_password";
        let short_key = "short";
        let result = encrypt(plaintext, short_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_invalid_base64_fails() {
        let result = decrypt("not valid base64!!!", TEST_KEY);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_too_short_fails() {
        let result = decrypt(&BASE64.encode(b"short"), TEST_KEY);
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_encryption_key() {
        let hash1 = hash_encryption_key(TEST_KEY);
        let hash2 = hash_encryption_key(TEST_KEY);

        // Same key should produce same hash
        assert_eq!(hash1, hash2);

        // Hash should be 64 hex characters (SHA-256)
        assert_eq!(hash1.len(), 64);

        // Different key should produce different hash
        let different_key = "different_key_that_is_32_chars_long!!";
        let hash3 = hash_encryption_key(different_key);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_encrypt_empty_string() {
        let plaintext = "";
        let encrypted = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");
        let decrypted = decrypt(&encrypted, TEST_KEY).expect("Decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_encrypt_unicode() {
        let plaintext = "🔐 Secret émojis and spëcial çhars! 日本語";
        let encrypted = encrypt(plaintext, TEST_KEY).expect("Encryption should succeed");
        let decrypted = decrypt(&encrypted, TEST_KEY).expect("Decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_derive_key_consistency() {
        let key1 = derive_key(TEST_KEY);
        let key2 = derive_key(TEST_KEY);
        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 32); // 256 bits
    }

    // ── JSON encryption tests ──────────────────────────────────────

    #[test]
    fn test_encrypt_decrypt_json_string() {
        let value = serde_json::json!("my_secret_token");
        let encrypted = encrypt_json(&value, TEST_KEY).expect("encrypt_json should succeed");
        assert!(encrypted.is_string(), "encrypted JSON should be a string");
        let decrypted = decrypt_json(&encrypted, TEST_KEY).expect("decrypt_json should succeed");
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_json_object() {
        let value = serde_json::json!({"user": "admin", "password": "s3cret", "port": 5432});
        let encrypted = encrypt_json(&value, TEST_KEY).expect("encrypt_json should succeed");
        let decrypted = decrypt_json(&encrypted, TEST_KEY).expect("decrypt_json should succeed");
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_json_array() {
        let value = serde_json::json!(["token1", "token2", 42, true, null]);
        let encrypted = encrypt_json(&value, TEST_KEY).expect("encrypt_json should succeed");
        let decrypted = decrypt_json(&encrypted, TEST_KEY).expect("decrypt_json should succeed");
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_json_number() {
        let value = serde_json::json!(42);
        let encrypted = encrypt_json(&value, TEST_KEY).unwrap();
        let decrypted = decrypt_json(&encrypted, TEST_KEY).unwrap();
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_json_bool() {
        let value = serde_json::json!(true);
        let encrypted = encrypt_json(&value, TEST_KEY).unwrap();
        let decrypted = decrypt_json(&encrypted, TEST_KEY).unwrap();
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_decrypt_json_wrong_key_fails() {
        let value = serde_json::json!({"secret": "data"});
        let encrypted = encrypt_json(&value, TEST_KEY).unwrap();
        let wrong = "wrong_key_that_is_also_32_chars_long!!!";
        assert!(decrypt_json(&encrypted, wrong).is_err());
    }

    #[test]
    fn test_decrypt_json_non_string_fails() {
        let not_encrypted = serde_json::json!(42);
        assert!(decrypt_json(&not_encrypted, TEST_KEY).is_err());
    }
}
