//! Private HTTP protocol between the API and Attune workload managers.

use crate::models::{Id, WorkerRole};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Duration;
use uuid::Uuid;

pub const MANAGER_CONTROL_PATH: &str = "/api/v1/internal/managers";
pub const SESSION_TTL_SECONDS: i32 = 120;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerEnrollmentGrant {
    pub role: WorkerRole,
    pub pool: String,
    /// Lowercase hexadecimal SHA-256 of a random, dedicated enrollment secret.
    pub secret_sha256: String,
}

impl ManagerEnrollmentGrant {
    pub fn validate_all(grants: &[Self]) -> Result<()> {
        let mut scopes = BTreeSet::new();
        let mut digests = BTreeSet::new();
        for grant in grants {
            if !valid_manager_name(&grant.pool)
                || grant.secret_sha256.len() != 64
                || !grant
                    .secret_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || !scopes.insert((grant.role as u8, &grant.pool))
                || !digests.insert(&grant.secret_sha256)
            {
                return Err(Error::configuration(
                    "Invalid or duplicate workload manager enrollment grant",
                ));
            }
        }
        Ok(())
    }
}

pub fn valid_manager_name(value: &str) -> bool {
    matches!(value.len(), 1..=63)
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerEnrollment {
    pub role: WorkerRole,
    pub pool: String,
    pub name: String,
    /// Generate once per process start and reuse when retrying enrollment.
    pub instance: Uuid,
}

#[derive(Deserialize, Serialize)]
pub struct ManagerSession {
    pub id: Id,
    pub instance: Uuid,
    pub expires_at: DateTime<Utc>,
    pub token: String,
}

/// A pooled manager transport. Credentials are passed per request and never
/// forwarded across redirects or through an environment-selected HTTP proxy.
#[derive(Clone)]
pub struct ManagerControlClient {
    client: reqwest::Client,
    base_url: String,
}

impl ManagerControlClient {
    pub fn new(api_url: &str, allow_plain_http: bool) -> Result<Self> {
        let url = url::Url::parse(api_url)
            .map_err(|_| Error::configuration("Invalid manager API URL"))?;
        if url.host().is_none()
            || url.fragment().is_some()
            || url.query().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https" || allow_plain_http && url.scheme() == "http")
        {
            return Err(Error::configuration("Invalid manager API URL or scheme"));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::configuration("Could not create manager API client"))?;
        Ok(Self {
            client,
            base_url: format!(
                "{}{MANAGER_CONTROL_PATH}",
                url.as_str().trim_end_matches('/')
            ),
        })
    }

    pub async fn enroll(
        &self,
        secret: &str,
        enrollment: &ManagerEnrollment,
    ) -> Result<ManagerSession> {
        let session: ManagerSession = checked_response(
            self.client
                .post(format!("{}/enroll", self.base_url))
                .bearer_auth(secret)
                .json(enrollment),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if session.instance != enrollment.instance || session.id <= 0 {
            return Err(invalid_response());
        }
        Ok(session)
    }

    pub async fn renew(&self, current: &ManagerSession) -> Result<ManagerSession> {
        let session: ManagerSession = checked_response(
            self.client
                .post(format!("{}/session", self.base_url))
                .bearer_auth(&current.token),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if session.id != current.id || session.instance != current.instance {
            return Err(invalid_response());
        }
        Ok(session)
    }

    pub async fn withdraw(&self, current: &ManagerSession) -> Result<()> {
        checked_response(
            self.client
                .delete(format!("{}/session", self.base_url))
                .bearer_auth(&current.token),
        )
        .await?;
        Ok(())
    }
}

async fn checked_response(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let response = request
        .send()
        .await
        .map_err(|_| Error::RetryableTransport("Manager API request failed".into()))?;
    match response.status() {
        status if status.is_success() => Ok(response),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Err(
            Error::PermissionDenied("Manager API credential denied".into()),
        ),
        reqwest::StatusCode::CONFLICT => {
            Err(Error::invalid_state("Manager enrollment slot conflict"))
        }
        status if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Err(Error::RetryableTransport(format!(
                "Manager API temporarily unavailable ({status})"
            )))
        }
        status => Err(Error::external_service(format!(
            "Manager API request rejected ({status})"
        ))),
    }
}

fn invalid_response() -> Error {
    Error::RetryableTransport("Invalid manager API response".into())
}
