//! HTTP request audit middleware.
//!
//! Emits one [`AuditEvent`](attune_common::audit::AuditEvent) per request
//! describing the HTTP method, path, response status, duration, and (when
//! available) the authenticated identity. Emission is non-blocking via the
//! shared [`AuditEmitter`].

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use std::{net::SocketAddr, sync::Arc, time::Instant};
use uuid::Uuid;

use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};
use attune_common::auth::jwt::{extract_token_from_header, validate_token, Claims};

use crate::{
    middleware::logging::request_log_path,
    state::{AppState, SharedState},
};

/// Per-request identifier inserted into request extensions so downstream
/// handlers (and explicit emit sites) can correlate explicit emits with the
/// HTTP-level audit event.
#[derive(Debug, Clone, Copy)]
pub struct RequestId(pub Uuid);

/// Paths that are *never* audited. These are noisy operational endpoints or
/// the audit-log read API itself (which would otherwise flood the log every
/// time the UI polls).
fn is_skipped_path(path: &str) -> bool {
    path == "/health"
        || path == "/api/v1/info"
        || path == "/healthz"
        || path == "/ready"
        || path.starts_with("/docs")
        || path.starts_with("/api-spec")
        || path.starts_with("/api/v1/audit-events")
}

/// Map an HTTP status code to an audit [`AuditOutcome`].
fn status_to_outcome(status: StatusCode) -> AuditOutcome {
    let code = status.as_u16();
    if code == 401 || code == 403 {
        AuditOutcome::Denied
    } else if code >= 400 {
        AuditOutcome::Failure
    } else {
        AuditOutcome::Success
    }
}

/// Best-effort client IP extraction.
///
/// Honours `X-Forwarded-For` (first hop) and `X-Real-IP` if present, otherwise
/// falls back to the connection peer address.
fn extract_client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if !real.trim().is_empty() {
            return Some(real.trim().to_string());
        }
    }
    peer.map(|p| p.ip().to_string())
}

fn extract_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Audit middleware. Should be installed *outside* the auth layer so the
/// auth layer's response (e.g. 401) is captured.
pub async fn audit_request(
    State(state): State<SharedState>,
    mut req: Request,
    next: Next,
) -> Response {
    let request_id = RequestId(Uuid::new_v4());
    req.extensions_mut().insert(request_id);

    let method = req.method().clone();
    let path = request_log_path(&req);
    let skip = is_skipped_path(&path);

    // Decode the bearer token (best-effort) before invoking the inner stack,
    // so we can attribute the request to an actor even if the inner handler
    // consumes the request extensions.
    let early_claims: Option<Claims> = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_token_from_header)
        .and_then(|tok| validate_token(tok, &state.jwt_config).ok());

    let start = Instant::now();
    let headers_snapshot = req.headers().clone();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);

    let response = next.run(req).await;

    if skip || !state.audit_emitter.is_active() {
        return response;
    }

    let duration_ms = (start.elapsed().as_millis() as i64).min(i32::MAX as i64) as i32;
    let status = response.status();
    let outcome = status_to_outcome(status);

    let event_type = format!(
        "api.{}.{}",
        method.as_str().to_lowercase(),
        outcome_label(&outcome)
    );

    let mut builder = AuditEventBuilder::new(AuditCategory::Api, event_type, outcome).http(
        method.as_str(),
        &path,
        status.as_u16() as i32,
        duration_ms,
    );
    builder = builder.request_id(request_id.0);

    if let Some(ip) = extract_client_ip(&headers_snapshot, peer) {
        builder = builder.actor_ip_str(&ip);
    }
    if let Some(ua) = extract_user_agent(&headers_snapshot) {
        builder = builder.actor_user_agent(ua);
    }
    if let Some(claims) = early_claims {
        if let Ok(id) = claims.sub.parse::<i64>() {
            builder = builder.actor_identity(id);
        }
        builder = builder.actor_login(claims.login.clone());
        builder = builder.actor_token_type(format!("{:?}", claims.token_type).to_lowercase());
    }

    state.audit_emitter.emit(builder.build());
    response
}

fn outcome_label(outcome: &AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Success => "success",
        AuditOutcome::Failure => "failure",
        AuditOutcome::Denied => "denied",
    }
}

/// Helper to clone the audit emitter out of state. Useful in handlers that
/// want to record an explicit, semantic event in addition to (or instead of)
/// the generic HTTP-level event from the middleware.
pub fn audit_emitter(state: &Arc<AppState>) -> attune_common::audit::AuditEmitter {
    state.audit_emitter.clone()
}
