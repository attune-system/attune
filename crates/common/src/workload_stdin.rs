//! Private exact-byte stdin authorization. The public manifest references only
//! randomized encrypted context, never a plaintext input/plan digest. Payloads
//! travel solely over the authenticated controller-to-CP channel, not audit,
//! tracing, URL parameters or general resource DTOs.
use crate::{workload_launch::LaunchHandle, Error, Result};
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

pub const MAX_PRIVATE_STDIN_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_PRIVATE_STDIN_BYTES.div_ceil(3) * 4;
fn invalid() -> Error {
    Error::validation("Invalid private launch stdin payload")
}

#[derive(Clone, ToSchema)]
#[schema(value_type=String)]
pub struct LaunchStdinPayload(Vec<u8>);
impl LaunchStdinPayload {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_PRIVATE_STDIN_BYTES {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl std::fmt::Debug for LaunchStdinPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchStdinPayload")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}
impl Serialize for LaunchStdinPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}
impl<'de> Deserialize<'de> for LaunchStdinPayload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let fail = || serde::de::Error::custom("Invalid private launch stdin payload");
        let encoded = String::deserialize(deserializer).map_err(|_| fail())?;
        if encoded.len() > MAX_ENCODED_BYTES {
            return Err(fail());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|_| fail())?;
        if bytes.len() > MAX_PRIVATE_STDIN_BYTES
            || base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded
        {
            return Err(fail());
        }
        Ok(Self(bytes))
    }
}

/// The controller holds these exact immutable bytes until the acknowledged
/// authorization, then writes them once without re-reading a mutable file or
/// changing chunks. Lost/uncertain ACK is fail-closed: no retry may create a
/// second effect. LAUNCH_LEASE_HEADER and LAUNCH_IO_HEADER carry credentials.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeLaunchStdinRequest {
    pub handle: LaunchHandle,
    pub payload: LaunchStdinPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_payload_roundtrip_is_exact_and_debug_and_errors_are_redacted() {
        let bytes = b"{\"selected\":\"owned-private-stdin-fixture\"}".to_vec();
        let payload = LaunchStdinPayload::from_bytes(bytes.clone()).unwrap();
        let debug = format!("{payload:?}");
        assert!(!debug.contains("owned-private-stdin-fixture"));
        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded: LaunchStdinPayload = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.as_bytes(), bytes);
        for malformed in ["\"YWJj\\n\"", "\"YQ\"", "\"YR==\"", "\"____\"", "42"] {
            let error = serde_json::from_str::<LaunchStdinPayload>(malformed)
                .unwrap_err()
                .to_string();
            assert!(error.contains("Invalid private launch stdin payload"));
            assert!(!error.contains("YWJj"));
        }
    }
    #[test]
    fn rejects_oversized_input_before_unbounded_base64_decoding() {
        assert!(LaunchStdinPayload::from_bytes(vec![0; MAX_PRIVATE_STDIN_BYTES + 1]).is_err());
        let oversized = format!("\"{}\"", "A".repeat(MAX_ENCODED_BYTES + 4));
        assert!(serde_json::from_str::<LaunchStdinPayload>(&oversized).is_err());
    }
}
