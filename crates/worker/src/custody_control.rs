//! Execution-only custody transport. The sole credential is a short-lived
//! CP-issued host session; there is no database, broker, signer or master key.
//! Transport errors never prove absence. Callers retain custody until an exact
//! acknowledged cancellation or separate delivery settlement permits deletion.
use attune_common::{
    log_custody::{
        CustodyAbsence, CustodyOperationAck, CustodyOperationState, PrepareCustody, ReserveCustody,
    },
    workload_launch_io::LaunchHttpPolicy,
    workload_session::WorkloadSession,
    Error, Result,
};
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

fn unavailable() -> Error {
    Error::execution("Custody control transport unavailable")
}
fn denied() -> Error {
    Error::PermissionDenied("Custody control credential unavailable".into())
}

/// Shared with the host's renewal owner. Replacement cannot overwrite a newer
/// session after a concurrent refresh, and expired sessions never fall back to
/// reusable enrollment or a local signer.
#[derive(Clone)]
pub struct HostSessionSlot(Arc<RwLock<WorkloadSession>>);
impl HostSessionSlot {
    pub fn new(session: WorkloadSession) -> Result<Self> {
        validate_session(&session)?;
        Ok(Self(Arc::new(RwLock::new(session))))
    }
    pub async fn current(&self) -> Result<WorkloadSession> {
        let session = self.0.read().await.clone();
        validate_session(&session)?;
        Ok(session)
    }
    pub async fn replace(&self, previous: &WorkloadSession, next: WorkloadSession) -> Result<bool> {
        validate_session(&next)?;
        if next.expires_at < previous.expires_at {
            return Err(denied());
        }
        let mut current = self.0.write().await;
        if current.token.as_str() != previous.token.as_str()
            || current.expires_at != previous.expires_at
        {
            return Ok(false);
        }
        validate_session(&current)?;
        *current = next;
        Ok(true)
    }
}
fn validate_session(session: &WorkloadSession) -> Result<()> {
    let now = Utc::now();
    if session.expires_at <= now || session.expires_at > now + chrono::Duration::seconds(300) {
        return Err(denied());
    }
    Ok(())
}

#[derive(Clone)]
pub struct CustodyControlClient {
    origin: reqwest::Url,
    client: reqwest::Client,
    sessions: HostSessionSlot,
}
impl CustodyControlClient {
    pub fn new(
        origin: reqwest::Url,
        sessions: HostSessionSlot,
        policy: LaunchHttpPolicy,
    ) -> Result<Self> {
        if !policy.allows_origin(&origin)
            || !(origin.scheme() == "https"
                || matches!(policy, LaunchHttpPolicy::DevelopmentPlainHttp))
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(Error::configuration(
                "Custody control requires an admitted HTTPS origin",
            ));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            origin,
            client,
            sessions,
        })
    }
    async fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        operation: &str,
        request: &T,
    ) -> Result<R> {
        let session = self.sessions.current().await?;
        let url = self
            .origin
            .join(&format!("api/v1/workload-hosts/custody/{operation}"))
            .map_err(|_| unavailable())?;
        let mut response = self
            .client
            .post(url)
            .bearer_auth(session.token.as_str())
            .json(request)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(denied());
        }
        if response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            != Some("no-store")
            || response.content_length().is_some_and(|size| size > 4096)
        {
            return Err(unavailable());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|size| size > 4096)
            {
                return Err(unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        validate_session(&session)?;
        serde_json::from_slice(&body).map_err(|_| unavailable())
    }
    pub async fn reserve(&self, request: &ReserveCustody) -> Result<CustodyOperationAck> {
        let ack: CustodyOperationAck = self.post("reserve", request).await?;
        if ack.custody != request.binding.custody
            || !matches!(
                ack.state,
                CustodyOperationState::Reserved | CustodyOperationState::Prepared
            )
        {
            return Err(unavailable());
        }
        fresh_custody(request)?;
        Ok(ack)
    }
    pub async fn prepare(&self, request: &PrepareCustody) -> Result<CustodyOperationAck> {
        let ack: CustodyOperationAck = self.post("prepare", request).await?;
        if ack.custody != request.reservation.binding.custody
            || ack.state != CustodyOperationState::Prepared
        {
            return Err(unavailable());
        }
        fresh_custody(&request.reservation)?;
        Ok(ack)
    }
    pub async fn cancel_absent(&self, request: &ReserveCustody) -> Result<CustodyAbsence> {
        let ack: CustodyAbsence = self.post("cancel-absent", request).await?;
        if ack.custody != request.binding.custody {
            return Err(unavailable());
        }
        Ok(ack)
    }
}
fn fresh_custody(request: &ReserveCustody) -> Result<()> {
    let deadline = chrono::DateTime::from_timestamp_micros(request.finalize_by.timestamp_micros())
        .ok_or_else(unavailable)?;
    if deadline <= Utc::now() {
        return Err(denied());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{log_delivery::ControlRequest, workload_launch::LaunchToken};
    use axum::{
        extract::Json,
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;
    fn session(token: &str, seconds: i64) -> WorkloadSession {
        WorkloadSession {
            token: LaunchToken::new(token.into()).unwrap(),
            expires_at: Utc::now() + chrono::Duration::seconds(seconds),
        }
    }
    fn reservation() -> ReserveCustody {
        ReserveCustody {
            binding: ControlRequest {
                execution: 1,
                version: 2,
                stream: 3,
                attempt: Uuid::new_v4(),
                source_instance: Uuid::new_v4(),
                custody: Uuid::new_v4(),
            },
            finalize_by: Utc::now() + chrono::Duration::seconds(60),
        }
    }
    #[tokio::test]
    async fn finite_custody_client_uses_shared_cp_session_without_signing_or_http_fallback(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let (sent, mut received) = tokio::sync::mpsc::channel(4);
        let app = Router::new().route(
            "/api/v1/workload-hosts/custody/reserve",
            post(
                move |headers: HeaderMap, Json(request): Json<ReserveCustody>| {
                    let sent = sent.clone();
                    async move {
                        sent.send(
                            headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_owned(),
                        )
                        .await
                        .unwrap();
                        (
                            [(axum::http::header::CACHE_CONTROL, "no-store")],
                            Json(CustodyOperationAck {
                                custody: request.binding.custody,
                                state: CustodyOperationState::Reserved,
                            }),
                        )
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = reqwest::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
        });
        let original = session("owned-first-host-session", 120);
        let slot = HostSessionSlot::new(original.clone())?;
        let outcome = async {
            if CustodyControlClient::new(
                origin.clone(),
                slot.clone(),
                LaunchHttpPolicy::TlsOrLoopback,
            )
            .is_ok()
            {
                return Err("custody client accepted implicit plaintext HTTP".into());
            }
            let client = CustodyControlClient::new(
                origin,
                slot.clone(),
                LaunchHttpPolicy::DevelopmentPlainHttp,
            )?;
            let other = client.clone();
            let request = reservation();
            client.reserve(&request).await?;
            if received.recv().await.as_deref() != Some("Bearer owned-first-host-session") {
                return Err("wrong initial session".into());
            }
            let next = session("owned-renewed-host-session", 180);
            if !slot.replace(&original, next).await? {
                return Err("session replacement failed".into());
            }
            if slot
                .replace(&original, session("stale-refresh-result", 200))
                .await?
            {
                return Err("stale refresh overwrote current session".into());
            }
            other.reserve(&request).await?;
            if received.recv().await.as_deref() != Some("Bearer owned-renewed-host-session") {
                return Err("clone retained stale session".into());
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        stop.cancel();
        server.await??;
        outcome
    }
    #[tokio::test]
    async fn custody_client_does_not_treat_404_or_transport_failure_as_absence(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let app = Router::new().route(
            "/api/v1/workload-hosts/custody/cancel-absent",
            post(|Json(request): Json<ReserveCustody>| async move {
                (
                    StatusCode::NOT_FOUND,
                    [(axum::http::header::CACHE_CONTROL, "no-store")],
                    Json(CustodyAbsence {
                        custody: request.binding.custody,
                        absent: true,
                    }),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = reqwest::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
        });
        let client = CustodyControlClient::new(
            origin,
            HostSessionSlot::new(session("owned-host-session", 120))?,
            LaunchHttpPolicy::DevelopmentPlainHttp,
        )?;
        let request = reservation();
        let rejected = client.cancel_absent(&request).await.is_err();
        stop.cancel();
        server.await??;
        if !rejected {
            return Err("404 was accepted as absence".into());
        }
        if client.cancel_absent(&request).await.is_ok() {
            return Err("transport loss was accepted as absence".into());
        }
        Ok(())
    }
}
