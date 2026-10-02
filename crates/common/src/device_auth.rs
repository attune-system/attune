use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub fn default_poll_interval() -> u64 {
    5
}

/// Device-code instructions returned by Attune's OIDC broker.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct DeviceAuthorizationResponse {
    /// Opaque, encrypted authorization session. Never display this value.
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_poll_interval")]
    pub interval: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceWaitReason {
    AuthorizationPending,
    SlowDown,
    ProviderTimeout,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DevicePollResponse<T> {
    Waiting {
        reason: DeviceWaitReason,
        device_code: String,
        interval: u64,
    },
    Authorized {
        tokens: T,
    },
    AccessDenied,
    Expired,
}
