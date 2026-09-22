use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const PREFIX: &str = "attune_irh_";
const AUDIENCE: u8 = 1;
const VERSION: u8 = 1;
const NONCE_SIZE: usize = 12;
const PAYLOAD_SIZE: usize = 12;
pub const MAX_HANDLE_LENGTH: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InquiryResponseSelection {
    pub inquiry_id: i64,
    pub option_index: u16,
}

fn cipher(encryption_key: &str) -> Result<Aes256Gcm> {
    if encryption_key.len() < 32 {
        return Err(Error::encryption(
            "Encryption key must be at least 32 characters",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"attune:inquiry-response-handle:v1:");
    hasher.update(encryption_key.as_bytes());
    Aes256Gcm::new_from_slice(&hasher.finalize())
        .map_err(|error| Error::encryption(format!("Invalid key length: {error}")))
}

pub fn issue_inquiry_response_handle(
    inquiry_id: i64,
    option_index: u16,
    encryption_key: &str,
) -> Result<String> {
    if inquiry_id <= 0 {
        return Err(Error::Validation(
            "inquiry response handles require a positive inquiry ID".to_string(),
        ));
    }

    let mut payload = [0u8; PAYLOAD_SIZE];
    payload[0] = VERSION;
    payload[1] = AUDIENCE;
    payload[2..10].copy_from_slice(&inquiry_id.to_be_bytes());
    payload[10..12].copy_from_slice(&option_index.to_be_bytes());

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher(encryption_key)?
        .encrypt(&Nonce::from(nonce_bytes), payload.as_slice())
        .map_err(|error| Error::encryption(format!("Encryption failed: {error}")))?;
    let mut encoded = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    let handle = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(encoded));
    if handle.len() > MAX_HANDLE_LENGTH {
        return Err(Error::Validation(
            "Inquiry response handle exceeds the provider limit".to_string(),
        ));
    }
    Ok(handle)
}

pub fn resolve_inquiry_response_handle(
    handle: &str,
    encryption_key: &str,
) -> Result<InquiryResponseSelection> {
    if handle.len() <= PREFIX.len()
        || handle.len() > MAX_HANDLE_LENGTH
        || handle.trim() != handle
        || !handle.starts_with(PREFIX)
    {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    let encoded = URL_SAFE_NO_PAD
        .decode(&handle[PREFIX.len()..])
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    if encoded.len() <= NONCE_SIZE {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    let (nonce_bytes, ciphertext) = encoded.split_at(NONCE_SIZE);
    let nonce_bytes: [u8; NONCE_SIZE] = nonce_bytes
        .try_into()
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    let payload = cipher(encryption_key)?
        .decrypt(&Nonce::from(nonce_bytes), ciphertext)
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    let payload: [u8; PAYLOAD_SIZE] = payload
        .try_into()
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    if payload[0] != VERSION || payload[1] != AUDIENCE {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    let inquiry_id = i64::from_be_bytes(payload[2..10].try_into().expect("fixed payload slice"));
    if inquiry_id <= 0 {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    Ok(InquiryResponseSelection {
        inquiry_id,
        option_index: u16::from_be_bytes(payload[10..12].try_into().expect("fixed payload slice")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "test-encryption-key-that-is-long-enough";

    #[test]
    fn round_trips_option_selection_in_compact_url_safe_form() {
        let handle = issue_inquiry_response_handle(918, 7, KEY).unwrap();
        assert!(handle.starts_with(PREFIX));
        assert!(!handle.contains("918"));
        assert!(handle.len() <= 96);
        assert!(!handle.contains(['+', '/', '=']));
        assert_eq!(
            resolve_inquiry_response_handle(&handle, KEY).unwrap(),
            InquiryResponseSelection {
                inquiry_id: 918,
                option_index: 7,
            }
        );
    }

    #[test]
    fn repeated_issuance_produces_distinct_handles_for_the_same_inquiry() {
        let first = issue_inquiry_response_handle(918, 1, KEY).unwrap();
        let second = issue_inquiry_response_handle(918, 1, KEY).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            resolve_inquiry_response_handle(&first, KEY).unwrap(),
            InquiryResponseSelection {
                inquiry_id: 918,
                option_index: 1,
            }
        );
        assert_eq!(
            resolve_inquiry_response_handle(&second, KEY).unwrap(),
            InquiryResponseSelection {
                inquiry_id: 918,
                option_index: 1,
            }
        );
    }

    #[test]
    fn rejects_tampering_wrong_keys_and_malformed_values() {
        let handle = issue_inquiry_response_handle(918, 0, KEY).unwrap();
        let mut tampered = handle.clone();
        tampered.push('x');
        assert!(resolve_inquiry_response_handle(&tampered, KEY).is_err());
        assert!(resolve_inquiry_response_handle(
            &handle,
            "different-encryption-key-that-is-long-enough"
        )
        .is_err());
        assert!(resolve_inquiry_response_handle("918", KEY).is_err());
        assert!(resolve_inquiry_response_handle(&format!(" {handle}"), KEY).is_err());
        assert!(
            resolve_inquiry_response_handle(&format!("{PREFIX}{}", "a".repeat(96)), KEY).is_err()
        );
        assert!(issue_inquiry_response_handle(0, 0, KEY).is_err());
    }
}
