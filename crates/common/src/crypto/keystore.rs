//! Key-record encryption. Record IDs, not mutable names, bind derivation and AAD.
//! Other encrypted entity classes intentionally use the separate shared helpers.
use super::{generate_nonce, BASE64, NONCE_SIZE};
use crate::{Error, Result};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SALT: &[u8] = b"attune:keystore:hkdf-sha256\0";
const KEY_INFO: &[u8] = b"attune:keystore:aes-256-gcm\0";
const VALUE_AAD: &[u8] = b"attune:keystore:value\0";
const FINGERPRINT: &[u8] = b"attune:keystore:key-fingerprint\0";

fn context(label: &[u8], id: i64) -> Result<Vec<u8>> {
    if id <= 0 {
        return Err(Error::encryption(
            "Key encryption requires a positive record ID",
        ));
    }
    Ok([label, &id.to_be_bytes()].concat())
}

fn derive_key(master: &str, id: i64) -> Result<[u8; 32]> {
    if master.len() < 32 {
        return Err(Error::encryption(
            "Encryption key must be at least 32 characters",
        ));
    }
    let info = context(KEY_INFO, id)?;
    let pieces = [info.as_slice()];
    let prk = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, SALT).extract(master.as_bytes());
    let output = prk
        .expand(&pieces, ring::hkdf::HKDF_SHA256)
        .map_err(|_| Error::encryption("Key derivation failed"))?;
    let mut key = [0; 32];
    output
        .fill(&mut key)
        .map_err(|_| Error::encryption("Key derivation failed"))?;
    Ok(key)
}

/// Non-secret verifier. Never persist the HKDF output or extraction key.
pub fn fingerprint(master: &str, id: i64) -> Result<String> {
    let key = derive_key(master, id)?;
    let mut hash = Sha256::new();
    hash.update(FINGERPRINT);
    hash.update(key);
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Encode one Base64 string containing nonce, ciphertext and authentication tag.
pub fn encrypt_json(value: &Value, master: &str, id: i64) -> Result<Value> {
    seal(value, master, id, generate_nonce())
}

fn seal(value: &Value, master: &str, id: i64, nonce: [u8; NONCE_SIZE]) -> Result<Value> {
    let key = derive_key(master, id)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| Error::encryption("Invalid Key encryption key"))?;
    let aad = context(VALUE_AAD, id)?;
    let plaintext = serde_json::to_vec(value)?;
    let encrypted = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| Error::encryption("Key encryption failed"))?;
    Ok(Value::String(
        BASE64.encode([nonce.as_slice(), encrypted.as_slice()].concat()),
    ))
}

/// The caller must supply the trusted loaded row ID, not an ID from ciphertext.
pub fn decrypt_json(value: &Value, master: &str, id: i64) -> Result<Value> {
    let key = derive_key(master, id)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| Error::encryption("Invalid Key encryption key"))?;
    let encoded = value
        .as_str()
        .ok_or_else(|| Error::encryption("Encrypted Key must be a string"))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| Error::encryption("Invalid Key ciphertext encoding"))?;
    if bytes.len() < NONCE_SIZE + 16 {
        return Err(Error::encryption("Invalid Key ciphertext length"));
    }
    let nonce: [u8; NONCE_SIZE] = bytes[..NONCE_SIZE]
        .try_into()
        .map_err(|_| Error::encryption("Invalid Key nonce"))?;
    let aad = context(VALUE_AAD, id)?;
    let plaintext = cipher
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &bytes[NONCE_SIZE..],
                aad: &aad,
            },
        )
        .map_err(|_| Error::encryption("Key authentication failed"))?;
    serde_json::from_slice(&plaintext).map_err(|_| Error::encryption("Invalid decrypted Key JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MASTER: &str = "synthetic-keystore-master-secret-32-bytes";

    #[test]
    fn records_have_distinct_keys_even_with_identical_values_and_nonces() {
        let a = derive_key(MASTER, 1).unwrap();
        let b = derive_key(MASTER, 2).unwrap();
        assert!(a != b, "Record keys must differ");
        let value = serde_json::json!({"credential": "synthetic-only"});
        let first = seal(&value, MASTER, 1, [7; NONCE_SIZE]).unwrap();
        let second = seal(&value, MASTER, 2, [7; NONCE_SIZE]).unwrap();
        assert_ne!(first, second);
        assert_eq!(decrypt_json(&first, MASTER, 1).unwrap(), value);
        assert_eq!(decrypt_json(&second, MASTER, 2).unwrap(), value);
        assert!(decrypt_json(&first, MASTER, 2).is_err());
        assert!(decrypt_json(&first, "different-synthetic-master-secret-32", 1).is_err());
        assert_ne!(
            fingerprint(MASTER, 1).unwrap(),
            fingerprint(MASTER, 2).unwrap()
        );
        let fp = fingerprint(MASTER, 1).unwrap();
        let fp: Vec<u8> = (0..fp.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fp[i..i + 2], 16).unwrap())
            .collect();
        assert!(
            fp.as_slice() != a,
            "Fingerprint must not be a usable AES key"
        );
        let bytes = BASE64.decode(first.as_str().unwrap()).unwrap();
        let nonce: [u8; NONCE_SIZE] = bytes[..NONCE_SIZE].try_into().unwrap();
        let aad = context(VALUE_AAD, 1).unwrap();
        assert!(Aes256Gcm::new_from_slice(&fp)
            .unwrap()
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &bytes[NONCE_SIZE..],
                    aad: &aad
                }
            )
            .is_err());
        // A record key alone is insufficient when the authenticated context changes.
        let cipher = Aes256Gcm::new_from_slice(&a).unwrap();
        for wrong_aad in [Vec::new(), context(VALUE_AAD, 2).unwrap()] {
            assert!(cipher
                .decrypt(
                    &Nonce::from(nonce),
                    Payload {
                        msg: &bytes[NONCE_SIZE..],
                        aad: &wrong_aad
                    }
                )
                .is_err());
        }
        for position in [0, NONCE_SIZE, bytes.len() - 1] {
            let mut tampered = bytes.clone();
            tampered[position] ^= 1;
            assert!(decrypt_json(&Value::String(BASE64.encode(tampered)), MASTER, 1).is_err());
        }
        assert!(super::super::decrypt_json(&first, MASTER).is_err());
    }

    #[test]
    fn rejects_invalid_identity_and_master_and_legacy_ciphertext() {
        for id in [0, -1, i64::MIN] {
            assert!(encrypt_json(&Value::Null, MASTER, id).is_err());
            assert!(fingerprint(MASTER, id).is_err());
        }
        assert!(encrypt_json(&Value::Null, "short", 1).is_err());
        let legacy = super::super::encrypt_json(&Value::Null, MASTER).unwrap();
        assert!(decrypt_json(&legacy, MASTER, 1).is_err());
    }
}
