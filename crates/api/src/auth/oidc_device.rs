//! RFC 8628 provider exchange. The CLI holds only an encrypted, expiring session.

use super::oidc::{
    complete_provider_login, fetch_discovery_document, oidc_config, oidc_http_client, IdTokenNonce,
};
use crate::{dto::auth::OidcDevicePollResponse, middleware::error::ApiError, state::SharedState};
use attune_common::{
    config::OidcConfig,
    crypto,
    device_auth::{DeviceAuthorizationResponse, DevicePollResponse, DeviceWaitReason},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PURPOSE: &str = "attune.oidc.device.v1";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceSession {
    purpose: String,
    provider_code: String,
    issuer: String,
    token_endpoint: String,
    configuration: String,
    expires_at_ms: i64,
    next_poll_at_ms: i64,
    interval: u64,
}

#[derive(Deserialize)]
struct ProviderAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "attune_common::device_auth::default_poll_interval")]
    interval: u64,
}

#[derive(Deserialize)]
struct ProviderTokens {
    access_token: String,
    id_token: String,
}

fn device_client(oidc: &OidcConfig) -> (&str, Option<&str>) {
    match &oidc.device_client {
        Some(client) => (
            &client.client_id,
            client
                .client_secret
                .as_deref()
                .filter(|secret| !secret.is_empty()),
        ),
        None => (
            oidc.client_id.as_deref().unwrap_or_default(),
            oidc.client_secret
                .as_deref()
                .filter(|secret| !secret.is_empty()),
        ),
    }
}

fn configuration_binding(state: &SharedState, oidc: &OidcConfig) -> Result<String, ApiError> {
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(oidc).map_err(|_| {
        ApiError::InternalServerError("Cannot bind OIDC device configuration".into())
    })?);
    digest.update(state.jwt_config.secret.as_bytes());
    Ok(hex::encode(digest.finalize()))
}

fn encryption_key(state: &SharedState) -> Result<&str, ApiError> {
    state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::NotImplemented(
                "OIDC device login requires security.encryption_key on the API server".into(),
            )
        })
}

fn seal(state: &SharedState, session: &DeviceSession) -> Result<String, ApiError> {
    let data = serde_json::to_string(session)
        .map_err(|_| ApiError::InternalServerError("Cannot serialize device session".into()))?;
    crypto::encrypt(&data, encryption_key(state)?).map_err(|_| {
        ApiError::InternalServerError(
            "Cannot protect device session; check the API encryption key".into(),
        )
    })
}

fn open(state: &SharedState, code: &str) -> Result<DeviceSession, ApiError> {
    let data = crypto::decrypt(code, encryption_key(state)?)
        .map_err(|_| ApiError::BadRequest("Invalid device authorization session".into()))?;
    let session: DeviceSession = serde_json::from_str(&data)
        .map_err(|_| ApiError::BadRequest("Invalid device authorization session".into()))?;
    if session.purpose != PURPOSE {
        return Err(ApiError::BadRequest(
            "Invalid device authorization session purpose".into(),
        ));
    }
    Ok(session)
}

fn provider_request(url: &str, oidc: &OidcConfig) -> Result<reqwest::RequestBuilder, ApiError> {
    validate_device_uri(url)?;
    let (client_id, secret) = device_client(oidc);
    let request = oidc_http_client().post(url);
    Ok(match secret {
        Some(secret) => request.basic_auth(
            url::form_urlencoded::byte_serialize(client_id.as_bytes()).collect::<String>(),
            Some(url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>()),
        ),
        None => request,
    })
}

pub fn validate_device_uri(uri: &str) -> Result<(), ApiError> {
    let url = url::Url::parse(uri).map_err(|_| {
        ApiError::BadGateway("OIDC provider returned an invalid device endpoint URL".into())
    })?;
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
    {
        return Err(ApiError::BadGateway("OIDC device endpoints must use HTTPS; HTTP is allowed only for loopback development providers".into()));
    }
    Ok(())
}

async fn provider_json(mut response: reqwest::Response) -> Result<serde_json::Value, ApiError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::BadGateway("Cannot read OIDC device response".into()))?
    {
        if body.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err(ApiError::BadGateway(
                "OIDC device response exceeds the size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| {
        ApiError::BadGateway("OIDC provider returned an invalid device response".into())
    })
}

fn provider_rejection(body: &serde_json::Value) -> ApiError {
    match body.get("error").and_then(serde_json::Value::as_str) {
        Some("unsupported_grant_type" | "unauthorized_client") => ApiError::NotImplemented(
            "Enable the Device Authorization Grant for the configured OIDC device client at the identity provider".into(),
        ),
        Some("invalid_client") => ApiError::BadGateway("OIDC provider rejected the device client credentials; check server OIDC configuration".into()),
        _ => ApiError::BadGateway("OIDC provider rejected device authorization; start a new login or check the provider configuration".into()),
    }
}

pub async fn start(state: &SharedState) -> Result<DeviceAuthorizationResponse, ApiError> {
    encryption_key(state)?;
    let oidc = oidc_config(state)?;
    let discovery = fetch_discovery_document(&oidc).await?;
    let endpoint = discovery.device_authorization_endpoint.as_deref().ok_or_else(|| ApiError::NotImplemented(
        "The configured OIDC provider does not advertise device_authorization_endpoint. Device login requires RFC 8628 support; enable it at the provider.".into(),
    ))?;
    let scopes = ["openid", "email", "profile"]
        .into_iter()
        .map(str::to_string)
        .chain(oidc.scopes.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(" ");
    let started = Utc::now().timestamp_millis();
    let response = provider_request(endpoint, &oidc)?
        .form(&[("client_id", device_client(&oidc).0), ("scope", &scopes)])
        .send()
        .await
        .map_err(|_| {
            ApiError::BadGateway("Cannot reach the OIDC device authorization endpoint".into())
        })?;
    let success = response.status().is_success();
    let body = provider_json(response).await?;
    if !success {
        return Err(provider_rejection(&body));
    }
    let result: ProviderAuthorization = serde_json::from_value(body).map_err(|_| {
        ApiError::BadGateway(
            "OIDC provider returned incomplete device authorization instructions".into(),
        )
    })?;
    if result.device_code.is_empty()
        || result.device_code.len() > 16384
        || result.user_code.is_empty()
        || result.user_code.len() > 128
        || result.user_code.chars().any(char::is_control)
        || result.expires_in == 0
        || result.expires_in > 86400
        || result.interval == 0
        || result.interval > 86400
    {
        return Err(ApiError::BadGateway(
            "OIDC provider returned invalid device authorization instructions".into(),
        ));
    }
    validate_device_uri(&result.verification_uri)?;
    if let Some(uri) = &result.verification_uri_complete {
        validate_device_uri(uri)?;
    }
    let token_endpoint = discovery
        .metadata
        .token_endpoint()
        .ok_or_else(|| ApiError::BadGateway("OIDC provider has no token endpoint".into()))?
        .url()
        .to_string();
    let expires_at_ms = started + result.expires_in as i64 * 1000;
    let remaining = (expires_at_ms - Utc::now().timestamp_millis()) / 1000;
    if remaining <= 0 {
        return Err(ApiError::BadGateway(
            "Device authorization expired before the provider responded".into(),
        ));
    }
    let session = DeviceSession {
        purpose: PURPOSE.into(),
        provider_code: result.device_code,
        issuer: discovery.metadata.issuer().to_string(),
        token_endpoint,
        configuration: configuration_binding(state, &oidc)?,
        expires_at_ms,
        next_poll_at_ms: Utc::now().timestamp_millis() + result.interval as i64 * 1000,
        interval: result.interval,
    };
    Ok(DeviceAuthorizationResponse {
        device_code: seal(state, &session)?,
        user_code: result.user_code,
        verification_uri: result.verification_uri,
        verification_uri_complete: result.verification_uri_complete,
        expires_in: remaining as u64,
        interval: result.interval,
    })
}

fn waiting(
    state: &SharedState,
    mut session: DeviceSession,
    reason: DeviceWaitReason,
) -> Result<OidcDevicePollResponse, ApiError> {
    session.interval = match reason {
        DeviceWaitReason::SlowDown => session.interval.saturating_add(5),
        DeviceWaitReason::ProviderTimeout => session.interval.saturating_mul(2),
        DeviceWaitReason::AuthorizationPending => session.interval,
    }
    .min(86400);
    session.next_poll_at_ms = Utc::now().timestamp_millis() + session.interval as i64 * 1000;
    Ok(DevicePollResponse::Waiting {
        interval: session.interval,
        reason,
        device_code: seal(state, &session)?,
    })
}

pub async fn poll(state: &SharedState, code: &str) -> Result<OidcDevicePollResponse, ApiError> {
    let session = open(state, code)?;
    let oidc = oidc_config(state)?;
    if session.configuration != configuration_binding(state, &oidc)? {
        return Err(ApiError::BadRequest(
            "OIDC configuration changed; start a new device login".into(),
        ));
    }
    if Utc::now().timestamp_millis() >= session.expires_at_ms {
        return Ok(DevicePollResponse::Expired);
    }
    if Utc::now().timestamp_millis() < session.next_poll_at_ms {
        return waiting(state, session, DeviceWaitReason::SlowDown);
    }
    let discovery = fetch_discovery_document(&oidc).await?;
    if session.issuer != discovery.metadata.issuer().to_string()
        || discovery
            .metadata
            .token_endpoint()
            .map(|endpoint| endpoint.url().as_str())
            != Some(session.token_endpoint.as_str())
    {
        return Err(ApiError::BadRequest(
            "OIDC provider metadata changed; start a new device login".into(),
        ));
    }
    let request = provider_request(&session.token_endpoint, &oidc)?.form(&[
        ("client_id", device_client(&oidc).0),
        ("grant_type", GRANT),
        ("device_code", session.provider_code.as_str()),
    ]);
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return waiting(state, session, DeviceWaitReason::ProviderTimeout)
        }
        Err(_) => {
            return Err(ApiError::BadGateway(
                "Cannot reach the OIDC device token endpoint".into(),
            ))
        }
    };
    let success = response.status().is_success();
    let body = provider_json(response).await?;
    if !success {
        return match body.get("error").and_then(serde_json::Value::as_str) {
            Some("authorization_pending") => {
                waiting(state, session, DeviceWaitReason::AuthorizationPending)
            }
            Some("slow_down") => waiting(state, session, DeviceWaitReason::SlowDown),
            Some("access_denied") => Ok(DevicePollResponse::AccessDenied),
            Some("expired_token") => Ok(DevicePollResponse::Expired),
            _ => Err(provider_rejection(&body)),
        };
    }
    if Utc::now().timestamp_millis() >= session.expires_at_ms {
        return Ok(DevicePollResponse::Expired);
    }
    let tokens: ProviderTokens = serde_json::from_value(body).map_err(|_| {
        ApiError::Unauthorized(
            "OIDC device grant did not return an access token and ID token".into(),
        )
    })?;
    let authenticated = complete_provider_login(
        state,
        &oidc,
        &discovery,
        tokens.id_token,
        openidconnect::AccessToken::new(tokens.access_token),
        device_client(&oidc).0,
        IdTokenNonce::Device,
    )
    .await?;
    Ok(DevicePollResponse::Authorized {
        tokens: authenticated.token_response,
    })
}
