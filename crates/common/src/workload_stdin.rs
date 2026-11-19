//! Private exact-byte stdin authorization. The public manifest references only
//! randomized encrypted context, never a plaintext input/plan digest. Payloads
//! travel solely over the authenticated controller-to-CP channel, not audit,
//! tracing, URL parameters or general resource DTOs.
use crate::{
    workload_launch::{LaunchHandle, LaunchPurpose},
    Error, Result,
};
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

pub const MAX_PRIVATE_STDIN_BYTES: usize = 8 * 1024 * 1024;
/// Existing sensor bootstrap ceiling. This is not a general/API body limit.
pub const MAX_SENSOR_BOOTSTRAP_BYTES: usize = 32 * 1024 * 1024;
/// Padded Base64 plus the fixed handle envelope; other control bodies retain
/// their smaller limits. Admission must precede reading/decoding this body.
pub const MAX_PRIVATE_STDIN_REQUEST_BYTES: usize = 12 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_PRIVATE_STDIN_BYTES.div_ceil(3) * 4;
fn invalid() -> Error {
    Error::validation("Invalid private launch stdin payload")
}

#[derive(Clone, ToSchema)]
#[schema(value_type=String)]
pub struct LaunchStdinPayload {
    bytes: Vec<u8>,
    sensor_bootstrap: bool,
}
impl LaunchStdinPayload {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_PRIVATE_STDIN_BYTES {
            return Err(invalid());
        }
        Ok(Self {
            bytes,
            sensor_bootstrap: false,
        })
    }
    /// Trusted sensor producers use this only after generation/source admission.
    /// Native controls check the purpose before writing any invocation or input.
    pub fn for_sensor_bootstrap(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_SENSOR_BOOTSTRAP_BYTES {
            return Err(invalid());
        }
        Ok(Self {
            bytes,
            sensor_bootstrap: true,
        })
    }
    pub fn validate_for(&self, purpose: LaunchPurpose) -> Result<()> {
        let limit = if self.sensor_bootstrap {
            if purpose != LaunchPurpose::Sensor {
                return Err(invalid());
            }
            MAX_SENSOR_BOOTSTRAP_BYTES
        } else {
            MAX_PRIVATE_STDIN_BYTES
        };
        if self.bytes.len() > limit {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::fmt::Debug for LaunchStdinPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchStdinPayload")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
impl Serialize for LaunchStdinPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if self.sensor_bootstrap {
            return Err(serde::ser::Error::custom(
                "Sensor bootstrap cannot use the general stdin API",
            ));
        }
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.bytes))
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
        Self::from_bytes(bytes).map_err(|_| fail())
    }
}

/// The controller holds these exact immutable bytes until the acknowledged
/// authorization, then writes them once without re-reading a mutable file or
/// changing chunks. Lost/uncertain ACK is fail-closed: no retry may create a
/// second effect. LAUNCH_LEASE_HEADER and LAUNCH_IO_HEADER carry credentials.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuthorizeLaunchStdinRequest {
    pub handle: LaunchHandle,
    pub payload: LaunchStdinPayload,
}
impl<'de> Deserialize<'de> for AuthorizeLaunchStdinRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            handle: LaunchHandle,
            payload: LaunchStdinPayload,
        }
        let wire = Wire::deserialize(deserializer)
            .map_err(|_| serde::de::Error::custom("Invalid private launch stdin request"))?;
        Ok(Self {
            handle: wire.handle,
            payload: wire.payload,
        })
    }
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
    #[test]
    fn sensor_bootstrap_limit_is_purpose_bound_and_does_not_widen_wire_payloads() {
        let payload =
            LaunchStdinPayload::for_sensor_bootstrap(vec![7; MAX_SENSOR_BOOTSTRAP_BYTES]).unwrap();
        assert!(payload.validate_for(LaunchPurpose::Sensor).is_ok());
        for purpose in [
            LaunchPurpose::Action,
            LaunchPurpose::Setup,
            LaunchPurpose::RuntimeProbe,
            LaunchPurpose::CandidateTest,
        ] {
            assert!(payload.validate_for(purpose).is_err());
        }
        assert!(serde_json::to_vec(&payload).is_err());
        assert!(
            LaunchStdinPayload::for_sensor_bootstrap(vec![0; MAX_SENSOR_BOOTSTRAP_BYTES + 1])
                .is_err()
        );
        assert!(LaunchStdinPayload::from_bytes(vec![0; MAX_PRIVATE_STDIN_BYTES + 1]).is_err());
        let small =
            LaunchStdinPayload::for_sensor_bootstrap(b"owned-private-bootstrap".to_vec()).unwrap();
        assert!(serde_json::to_vec(&small).is_err());
        assert!(!format!("{small:?}").contains("owned-private"));
    }
    #[test]
    fn request_shape_failures_do_not_echo_untrusted_private_fields() {
        let error=serde_json::from_str::<AuthorizeLaunchStdinRequest>(
            r#"{"owned-private-field-name":"owned-private-field-value","handle":{},"payload":"opaque"}"#,
        ).unwrap_err().to_string();
        assert!(error.contains("Invalid private launch stdin request"));
        assert!(!error.contains("owned-private"));
    }
}
