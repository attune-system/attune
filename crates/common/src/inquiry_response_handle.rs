use serde::{Deserialize, Serialize};

use crate::{crypto, Error, Result};

const PREFIX: &str = "attune_irh_";
const AUDIENCE: &str = "inquiry_response";
const VERSION: u8 = 1;
const MAX_HANDLE_LENGTH: usize = 1024;

#[derive(Debug, Serialize, Deserialize)]
struct InquiryResponseHandleClaims {
    version: u8,
    audience: String,
    inquiry_id: i64,
}

fn domain_key(encryption_key: &str) -> String {
    format!("attune:inquiry-response-handle:v1:{encryption_key}")
}

pub fn issue_inquiry_response_handle(inquiry_id: i64, encryption_key: &str) -> Result<String> {
    if inquiry_id <= 0 {
        return Err(Error::Validation(
            "inquiry response handles require a positive inquiry ID".to_string(),
        ));
    }
    let claims = InquiryResponseHandleClaims {
        version: VERSION,
        audience: AUDIENCE.to_string(),
        inquiry_id,
    };
    let plaintext = serde_json::to_string(&claims)
        .map_err(|error| Error::encryption(format!("Failed to encode response handle: {error}")))?;
    let ciphertext = crypto::encrypt(&plaintext, &domain_key(encryption_key))?;
    Ok(format!("{PREFIX}{ciphertext}"))
}

pub fn resolve_inquiry_response_handle(handle: &str, encryption_key: &str) -> Result<i64> {
    if handle.len() <= PREFIX.len()
        || handle.len() > MAX_HANDLE_LENGTH
        || handle.trim() != handle
        || !handle.starts_with(PREFIX)
    {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    let plaintext = crypto::decrypt(&handle[PREFIX.len()..], &domain_key(encryption_key))
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    let claims: InquiryResponseHandleClaims = serde_json::from_str(&plaintext)
        .map_err(|_| Error::Validation("Invalid inquiry response handle".to_string()))?;
    if claims.version != VERSION || claims.audience != AUDIENCE || claims.inquiry_id <= 0 {
        return Err(Error::Validation(
            "Invalid inquiry response handle".to_string(),
        ));
    }
    Ok(claims.inquiry_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "test-encryption-key-that-is-long-enough";

    #[test]
    fn round_trips_without_exposing_the_inquiry_id() {
        let handle = issue_inquiry_response_handle(918, KEY).unwrap();
        assert!(handle.starts_with(PREFIX));
        assert!(!handle.contains("918"));
        assert_eq!(resolve_inquiry_response_handle(&handle, KEY).unwrap(), 918);
    }

    #[test]
    fn repeated_issuance_produces_distinct_handles_for_the_same_inquiry() {
        let first = issue_inquiry_response_handle(918, KEY).unwrap();
        let second = issue_inquiry_response_handle(918, KEY).unwrap();
        assert_ne!(first, second);
        assert_eq!(resolve_inquiry_response_handle(&first, KEY).unwrap(), 918);
        assert_eq!(resolve_inquiry_response_handle(&second, KEY).unwrap(), 918);
    }

    #[test]
    fn rejects_tampering_wrong_keys_and_malformed_values() {
        let handle = issue_inquiry_response_handle(918, KEY).unwrap();
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
    }
}
