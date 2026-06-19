//! WebSocket server for real-time notifications

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, warn};

use attune_common::auth::{
    jwt::STANDARD_EXECUTION_ACCESS_REF, validate_token, JwtConfig, TokenType,
};
use attune_common::config::Config;
use attune_common::{
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::{
        event::{EnforcementRepository, EventRepository},
        execution::ExecutionRepository,
        identity::{IdentityRepository, IdentityRoleAssignmentRepository, PermissionSetRepository},
        rule::RuleRepository,
        FindById,
    },
};

use crate::service::Notification;
use crate::subscriber_manager::{ClientId, SubscriberManager, SubscriptionFilter};

/// Role name that grants the holder unrestricted filter ACL (e.g. ability to
/// subscribe to `User(other_id)` filters for arbitrary identities).
const ADMIN_ROLE: &str = "admin";
const WS_SELECTED_PROTOCOL: &str = "attune.v1";
const WS_TOKEN_PROTOCOL_PREFIX: &str = "attune.jwt.";

/// How often each WebSocket connection's task loop re-checks the JWT `exp`
/// claim. A 30-second cadence bounds post-expiration liveness without adding
/// meaningful overhead.
const TOKEN_EXPIRATION_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// WebSocket close code emitted when a connection is torn down because its
/// auth token has expired. Codes 4000–4999 are reserved for application use.
const CLOSE_CODE_TOKEN_EXPIRED: u16 = 4401;

/// WebSocket server for handling client connections
pub struct WebSocketServer {
    config: Config,
    pub notification_tx: broadcast::Sender<Notification>,
    subscriber_manager: Arc<SubscriberManager>,
    shutdown_tx: broadcast::Sender<()>,
    db_pool: PgPool,
}

impl WebSocketServer {
    /// Create a new WebSocket server
    pub fn new(
        config: Config,
        notification_tx: broadcast::Sender<Notification>,
        subscriber_manager: Arc<SubscriberManager>,
        shutdown_tx: broadcast::Sender<()>,
        db_pool: PgPool,
    ) -> Self {
        Self {
            config,
            notification_tx,
            subscriber_manager,
            shutdown_tx,
            db_pool,
        }
    }

    /// Clone method for spawning tasks
    pub fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            notification_tx: self.notification_tx.clone(),
            subscriber_manager: self.subscriber_manager.clone(),
            shutdown_tx: self.shutdown_tx.clone(),
            db_pool: self.db_pool.clone(),
        }
    }

    /// Start the WebSocket server
    pub async fn start(&self) -> Result<()> {
        let jwt_secret = self.config.security.jwt_secret.clone().unwrap_or_else(|| {
            warn!(
                "JWT_SECRET not set in config; falling back to default insecure secret. \
                     WebSocket auth will only succeed against tokens signed with the same default."
            );
            "insecure_default_secret_change_in_production".to_string()
        });

        let jwt_config = JwtConfig {
            secret: jwt_secret,
            access_token_expiration: self.config.security.jwt_access_expiration as i64,
            refresh_token_expiration: self.config.security.jwt_refresh_expiration as i64,
        };

        let app_state = Arc::new(AppState {
            notification_tx: self.notification_tx.clone(),
            subscriber_manager: self.subscriber_manager.clone(),
            jwt_config: Arc::new(jwt_config),
            db_pool: self.db_pool.clone(),
        });

        // Build router with WebSocket endpoint
        let app = Router::new()
            .route("/ws", get(websocket_handler))
            .route("/health", get(health_handler))
            .route("/stats", get(stats_handler))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(app_state);

        let notifier_config = self
            .config
            .notifier
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Notifier configuration not found in config"))?;

        let addr = format!("{}:{}", notifier_config.host, notifier_config.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .context(format!("Failed to bind to {}", addr))?;

        info!("WebSocket server listening on {}", addr);

        axum::serve(listener, app)
            .await
            .context("WebSocket server error")?;

        Ok(())
    }
}

/// Shared application state
struct AppState {
    #[allow(dead_code)]
    notification_tx: broadcast::Sender<Notification>,
    subscriber_manager: Arc<SubscriberManager>,
    jwt_config: Arc<JwtConfig>,
    db_pool: PgPool,
}

struct VerifiedWsToken {
    identity_id: i64,
    token_type: TokenType,
    token_exp: i64,
    permission_set_refs: Vec<String>,
    sensor_trigger_refs: Vec<String>,
}

#[derive(Clone)]
struct WebSocketAuthContext {
    identity_id: i64,
    token_type: TokenType,
    token_exp: i64,
    sensor_trigger_refs: Vec<String>,
    roles: Vec<String>,
    grants: Vec<Grant>,
}

struct ClientMessageContext<'a> {
    subscriber_manager: &'a SubscriberManager,
    auth: &'a WebSocketAuthContext,
    db_pool: &'a PgPool,
    ctrl_tx: &'a mpsc::UnboundedSender<OutgoingFrame>,
}

/// Health check endpoint
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// Stats endpoint
async fn stats_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = serde_json::json!({
        "connected_clients": state.subscriber_manager.client_count(),
        "total_subscriptions": state.subscriber_manager.subscription_count(),
    });
    (StatusCode::OK, Json(stats))
}

/// Extract a WebSocket JWT from secure request metadata.
///
/// Non-browser clients should use `Authorization: Bearer <jwt>`. Browser
/// WebSocket clients cannot set arbitrary headers, so the web UI sends the JWT
/// as a secondary `Sec-WebSocket-Protocol` value:
/// `attune.v1, attune.jwt.<jwt>`. Tokens are intentionally not accepted via
/// query string because URLs are commonly logged by proxies and access logs.
fn extract_ws_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer_token)
    {
        return Some(token.to_string());
    }

    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_token_subprotocol)
        .map(str::to_string)
}

fn parse_bearer_token(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.len() <= "Bearer ".len() || !value[..7].eq_ignore_ascii_case("Bearer ") {
        return None;
    }

    let token = value[7..].trim();
    (!token.is_empty()).then_some(token)
}

fn parse_token_subprotocol(value: &str) -> Option<&str> {
    value
        .split(',')
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(WS_TOKEN_PROTOCOL_PREFIX))
        .filter(|token| !token.is_empty())
}

/// Verify a token against the JWT config and ensure it's an allowed type.
///
/// Returns the verified WebSocket auth context seed on success, or an error
/// string suitable for logging/response on failure. Access, execution, and
/// sensor tokens are accepted.
fn verify_ws_token(
    token: &str,
    jwt_config: &JwtConfig,
) -> std::result::Result<VerifiedWsToken, &'static str> {
    let claims = validate_token(token, jwt_config).map_err(|_| "invalid_or_expired_token")?;

    match claims.token_type {
        TokenType::Access | TokenType::Execution | TokenType::Sensor => {}
        TokenType::Refresh => return Err("refresh_tokens_not_allowed"),
        TokenType::Worker => return Err("worker_tokens_not_allowed"),
    }

    let identity_id: i64 = claims.sub.parse().map_err(|_| "invalid_subject_in_token")?;
    let permission_set_refs = if claims.token_type == TokenType::Execution {
        execution_permission_set_refs_from_claims(&claims)
    } else {
        Vec::new()
    };

    let sensor_trigger_refs = if claims.token_type == TokenType::Sensor {
        sensor_trigger_refs_from_claims(&claims)
    } else {
        Vec::new()
    };

    Ok(VerifiedWsToken {
        identity_id,
        token_type: claims.token_type,
        token_exp: claims.exp,
        permission_set_refs,
        sensor_trigger_refs,
    })
}

fn execution_permission_set_refs_from_claims(claims: &attune_common::auth::Claims) -> Vec<String> {
    claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("permission_set_refs"))
        .and_then(serde_json::Value::as_array)
        .map(|refs| {
            refs.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != STANDARD_EXECUTION_ACCESS_REF)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn sensor_trigger_refs_from_claims(claims: &attune_common::auth::Claims) -> Vec<String> {
    claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("trigger_types"))
        .and_then(serde_json::Value::as_array)
        .map(|refs| {
            refs.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Returns true if the token's `exp` (Unix seconds) has been reached or
/// exceeded relative to `now`. An `exp` of `0` (or any non-positive value) is
/// treated as already expired — defence in depth against malformed claims
/// reaching this function.
fn is_token_expired(exp: i64, now: i64) -> bool {
    if exp <= 0 {
        return true;
    }
    now >= exp
}

/// Decide whether the connecting identity is allowed to subscribe to `filter`.
///
/// `User(other_id)` filters require either self-subscription
/// (`other_id == identity_id`) or that the identity holds the `admin` role.
async fn filter_allowed_for_identity(
    filter: &SubscriptionFilter,
    identity_id: i64,
    token_type: &TokenType,
    sensor_trigger_refs: &[String],
    roles: &[String],
    grants: &[Grant],
    db_pool: &PgPool,
) -> bool {
    if *token_type == TokenType::Sensor {
        return match filter {
            SubscriptionFilter::TriggerRef(trigger_ref) => sensor_trigger_refs
                .iter()
                .any(|allowed_ref| allowed_ref == trigger_ref),
            _ => false,
        };
    }

    match filter {
        SubscriptionFilter::User(target_id) => *target_id == identity_id || is_admin(roles),
        SubscriptionFilter::All => {
            is_admin(roles)
                || [
                    Resource::Events,
                    Resource::Enforcements,
                    Resource::Executions,
                    Resource::Rules,
                ]
                .into_iter()
                .all(|resource| {
                    has_operational_read(grants, resource, AuthorizationContext::new(identity_id))
                })
        }
        SubscriptionFilter::EntityType(entity_type) => entity_type_resource(entity_type)
            .is_none_or(|resource| {
                has_operational_read(grants, resource, AuthorizationContext::new(identity_id))
            }),
        SubscriptionFilter::Entity {
            entity_type,
            entity_id,
        } => {
            let Some(resource) = entity_type_resource(entity_type) else {
                return true;
            };
            let Some(ctx) =
                operational_entity_context(db_pool, identity_id, entity_type, *entity_id).await
            else {
                return false;
            };
            has_operational_read(grants, resource, ctx)
        }
        SubscriptionFilter::NotificationType(notification_type) => {
            notification_type_resource(notification_type).is_none_or(|resource| {
                has_operational_read(grants, resource, AuthorizationContext::new(identity_id))
            })
        }
        SubscriptionFilter::TriggerRef(_) => is_admin(roles),
    }
}

fn has_operational_read(
    grants: &[Grant],
    resource: Resource,
    context: AuthorizationContext,
) -> bool {
    grants
        .iter()
        .any(|grant| grant.allows(resource, Action::Read, &context))
}

fn entity_type_resource(entity_type: &str) -> Option<Resource> {
    match entity_type {
        "event" => Some(Resource::Events),
        "enforcement" => Some(Resource::Enforcements),
        "execution" => Some(Resource::Executions),
        "rule_lifecycle" => Some(Resource::Rules),
        _ => None,
    }
}

fn notification_type_resource(notification_type: &str) -> Option<Resource> {
    if notification_type == "rule_lifecycle_changed" {
        Some(Resource::Rules)
    } else if notification_type.contains("event") {
        Some(Resource::Events)
    } else if notification_type.contains("enforcement") {
        Some(Resource::Enforcements)
    } else if notification_type.contains("execution") {
        Some(Resource::Executions)
    } else {
        None
    }
}

async fn operational_entity_context(
    db_pool: &PgPool,
    identity_id: i64,
    entity_type: &str,
    entity_id: i64,
) -> Option<AuthorizationContext> {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(entity_id);

    match entity_type {
        "event" => {
            let event = EventRepository::find_by_id(db_pool, entity_id)
                .await
                .ok()
                .flatten()?;
            ctx.target_ref = Some(event.trigger_ref.clone());
            ctx.pack_ref = event
                .trigger_ref
                .split_once('.')
                .map(|(pack, _)| pack.to_string());
        }
        "enforcement" => {
            let enforcement = EnforcementRepository::find_by_id(db_pool, entity_id)
                .await
                .ok()
                .flatten()?;
            ctx.target_ref = Some(enforcement.rule_ref.clone());
            ctx.pack_ref = enforcement
                .rule_ref
                .split_once('.')
                .map(|(pack, _)| pack.to_string());
        }
        "execution" => {
            let execution = ExecutionRepository::find_by_id(db_pool, entity_id)
                .await
                .ok()
                .flatten()?;
            ctx.target_ref = Some(execution.action_ref.clone());
            ctx.pack_ref = execution
                .action_ref
                .split_once('.')
                .map(|(pack, _)| pack.to_string());
            ctx.owner_identity_id = execution.executor;
            ctx.execution_owner_identity_id = execution.executor;
        }
        "rule_lifecycle" => {
            let rule = RuleRepository::find_by_id(db_pool, entity_id)
                .await
                .ok()
                .flatten()?;
            ctx.target_ref = Some(rule.r#ref.clone());
            ctx.pack_ref = Some(rule.pack_ref.clone());
        }
        _ => return None,
    }

    Some(ctx)
}

/// Returns true if `roles` contains the admin role.
fn is_admin(roles: &[String]) -> bool {
    roles.iter().any(|r| r == ADMIN_ROLE)
}

/// WebSocket handler - validates JWT then upgrades HTTP connection to WebSocket
async fn websocket_handler(
    ws: WebSocketUpgrade,
    Query(_query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    let token = match extract_ws_token(&headers) {
        Some(t) => t,
        None => {
            warn!(
                "WebSocket upgrade rejected: missing token in Authorization header or WebSocket subprotocol"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing_token"})),
            )
                .into_response();
        }
    };

    let verified_token = match verify_ws_token(&token, &state.jwt_config) {
        Ok(v) => v,
        Err(reason) => {
            warn!(reason = %reason, "WebSocket upgrade rejected: token validation failed");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": reason})),
            )
                .into_response();
        }
    };

    // Defence in depth: `validate_token` already enforces `exp`, but reject
    // explicitly here so logic downstream can rely on a non-expired token.
    let now = chrono::Utc::now().timestamp();
    if is_token_expired(verified_token.token_exp, now) {
        warn!(
            identity_id = verified_token.identity_id,
            token_exp = verified_token.token_exp,
            now,
            "WebSocket upgrade rejected: token already expired"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_or_expired_token"})),
        )
            .into_response();
    }

    // Look up role assignments for the connecting identity. Fail-closed on DB
    // errors — a flaky DB must not silently grant admin privileges.
    let roles = match IdentityRoleAssignmentRepository::find_role_names_by_identity(
        &state.db_pool,
        verified_token.identity_id,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(
                identity_id = verified_token.identity_id,
                error = %e,
                "WebSocket upgrade rejected: failed to look up identity roles"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "role_lookup_failed"})),
            )
                .into_response();
        }
    };
    let grants = match load_effective_grants(
        &state.db_pool,
        verified_token.identity_id,
        verified_token.token_type.clone(),
        &roles,
        &verified_token.permission_set_refs,
    )
    .await
    {
        Ok(grants) => grants,
        Err(e) => {
            error!(
                identity_id = verified_token.identity_id,
                error = %e,
                "WebSocket upgrade rejected: failed to load permission grants"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "permission_lookup_failed"})),
            )
                .into_response();
        }
    };

    let auth = WebSocketAuthContext {
        identity_id: verified_token.identity_id,
        token_type: verified_token.token_type,
        token_exp: verified_token.token_exp,
        sensor_trigger_refs: verified_token.sensor_trigger_refs,
        roles,
        grants,
    };

    debug!(
        identity_id = auth.identity_id,
        token_type = ?auth.token_type,
        roles = ?auth.roles,
        "WebSocket upgrade authorized"
    );

    ws.protocols([WS_SELECTED_PROTOCOL])
        .on_upgrade(move |socket| handle_websocket(socket, state, auth))
}

async fn load_effective_grants(
    db_pool: &PgPool,
    identity_id: i64,
    token_type: TokenType,
    roles: &[String],
    execution_permission_set_refs: &[String],
) -> Result<Vec<Grant>> {
    let identity = IdentityRepository::find_by_id(db_pool, identity_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("identity not found"))?;
    if identity.frozen {
        return Err(anyhow::anyhow!("identity frozen"));
    }

    let mut permission_sets = match token_type {
        TokenType::Access => {
            let mut sets = PermissionSetRepository::find_by_identity(db_pool, identity_id).await?;
            sets.extend(PermissionSetRepository::find_by_roles(db_pool, roles).await?);
            sets
        }
        TokenType::Execution => {
            PermissionSetRepository::find_by_refs(db_pool, execution_permission_set_refs).await?
        }
        _ => Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    permission_sets.retain(|permission_set| seen.insert(permission_set.id));

    let mut grants = Vec::new();
    for permission_set in permission_sets {
        let set_grants: Vec<Grant> =
            serde_json::from_value(permission_set.grants).with_context(|| {
                format!("invalid grants in permission set {}", permission_set.r#ref)
            })?;
        grants.extend(set_grants);
    }

    Ok(grants)
}

/// Handle individual WebSocket connection
async fn handle_websocket(
    socket: WebSocket,
    state: Arc<AppState>,
    auth: WebSocketAuthContext,
) {
    let client_id = state.subscriber_manager.generate_client_id();
    info!(
        "New WebSocket connection: {} (identity_id={}, roles={:?})",
        client_id, auth.identity_id, auth.roles
    );

    // Split the socket into sender and receiver
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create channel for sending notifications to this client
    let (tx, mut rx) = mpsc::unbounded_channel::<Notification>();

    // Register the subscriber with the verified identity
    state.subscriber_manager.register(
        client_id.clone(),
        Some(auth.identity_id),
        auth.roles.clone(),
        auth.token_exp,
        tx,
    );

    // Send welcome message
    let welcome = ClientMessage::Welcome {
        client_id: client_id.clone(),
        message: "Connected to Attune Notifier".to_string(),
    };
    if let Ok(json) = serde_json::to_string(&welcome) {
        let _ = ws_sender.send(Message::Text(json.into())).await;
    }

    // Channel for control messages (errors, close frames, etc.) the receive
    // loop wants to push back to the client. Multiplexed with the
    // notification stream by the outgoing task.
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<OutgoingFrame>();

    // Spawn task to handle outgoing notifications and control messages
    let client_id_clone = client_id.clone();
    let subscriber_manager_clone = state.subscriber_manager.clone();
    let outgoing_task = tokio::spawn(async move {
        loop {
            let frame = tokio::select! {
                maybe_n = rx.recv() => match maybe_n {
                    Some(n) => OutgoingFrame::Message(ClientMessage::Notification(n)),
                    None => break,
                },
                maybe_c = ctrl_rx.recv() => match maybe_c {
                    Some(c) => c,
                    None => continue,
                },
            };
            match frame {
                OutgoingFrame::Message(msg) => match serde_json::to_string(&msg) {
                    Ok(json) => {
                        if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                            error!("Failed to send message to {}: {}", client_id_clone, e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Failed to serialize outgoing message: {}", e);
                    }
                },
                OutgoingFrame::Close { code, reason } => {
                    let _ = ws_sender
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        })))
                        .await;
                    break;
                }
            }
        }
        debug!("Outgoing task stopped for client: {}", client_id_clone);
        subscriber_manager_clone.unregister(&client_id_clone);
    });

    // Handle incoming messages from client (subscriptions, etc.) and
    // periodically check that the access token is still valid. The
    // periodic-tick interval is short enough that connections are torn down
    // promptly after `exp`, but long enough not to thrash.
    let subscriber_manager_clone = state.subscriber_manager.clone();
    let client_id_clone = client_id.clone();
    let mut exp_interval = tokio::time::interval(TOKEN_EXPIRATION_CHECK_INTERVAL);
    // Skip the immediate first tick — we already verified `exp` at upgrade time.
    exp_interval.tick().await;
    loop {
        tokio::select! {
            _ = exp_interval.tick() => {
                let now = chrono::Utc::now().timestamp();
                if is_token_expired(auth.token_exp, now) {
                    info!(
                        "WebSocket connection {} closed due to expired token",
                        client_id_clone
                    );
                    let _ = ctrl_tx.send(OutgoingFrame::Close {
                        code: CLOSE_CODE_TOKEN_EXPIRED,
                        reason: "token expired".to_string(),
                    });
                    break;
                }
            }
            maybe_msg = ws_receiver.next() => {
                let msg = match maybe_msg {
                    Some(m) => m,
                    None => break,
                };
                match msg {
                    Ok(Message::Text(text)) => {
                        let ctx = ClientMessageContext {
                            subscriber_manager: &subscriber_manager_clone,
                            auth: &auth,
                            db_pool: &state.db_pool,
                            ctrl_tx: &ctrl_tx,
                        };
                        handle_client_message(&client_id_clone, &text, &ctx).await;
                    }
                    Ok(Message::Binary(_)) => {
                        warn!("Received binary message from {}, ignoring", client_id_clone);
                    }
                    Ok(Message::Close(_)) => {
                        info!("Client {} closed connection", client_id_clone);
                        break;
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                        // Handled automatically by axum
                    }
                    Err(e) => {
                        error!("WebSocket error for {}: {}", client_id_clone, e);
                        break;
                    }
                }
            }
        }
    }

    // Clean up
    subscriber_manager_clone.unregister(&client_id);
    outgoing_task.abort();
    info!("WebSocket connection closed: {}", client_id);
}

/// Push an `Error` frame back to the client via the control channel.
fn send_error_frame(ctrl_tx: &mpsc::UnboundedSender<OutgoingFrame>, message: impl Into<String>) {
    let _ = ctrl_tx.send(OutgoingFrame::Message(ClientMessage::Error {
        message: message.into(),
    }));
}

/// Handle incoming message from client. Errors are surfaced to the client as
/// `ClientMessage::Error` frames rather than aborting the connection.
async fn handle_client_message(
    client_id: &ClientId,
    message: &str,
    ctx: &ClientMessageContext<'_>,
) {
    let msg: ServerMessage = match serde_json::from_str(message) {
        Ok(m) => m,
        Err(e) => {
            warn!("Malformed JSON from {}: {}", client_id, e);
            send_error_frame(ctx.ctrl_tx, format!("Malformed message: {}", e));
            return;
        }
    };

    match msg {
        ServerMessage::Subscribe { filter } => {
            let subscription_filter = match parse_subscription_filter(&filter) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Invalid filter from {}: {}", client_id, e);
                    send_error_frame(ctx.ctrl_tx, format!("Invalid filter '{}': {}", filter, e));
                    return;
                }
            };
            if !filter_allowed_for_identity(
                &subscription_filter,
                ctx.auth.identity_id,
                &ctx.auth.token_type,
                &ctx.auth.sensor_trigger_refs,
                &ctx.auth.roles,
                &ctx.auth.grants,
                ctx.db_pool,
            )
            .await
            {
                warn!(
                    identity_id = ctx.auth.identity_id,
                    requested_filter = %filter,
                    "Subscribe denied by ACL"
                );
                send_error_frame(
                    ctx.ctrl_tx,
                    "Unauthorized to subscribe to requested filter".to_string(),
                );
                return;
            }
            ctx.subscriber_manager.subscribe(client_id, subscription_filter);
            info!("Client {} subscribed to: {:?}", client_id, filter);
        }
        ServerMessage::Unsubscribe { filter } => {
            let subscription_filter = match parse_subscription_filter(&filter) {
                Ok(f) => f,
                Err(e) => {
                    warn!("Invalid unsubscribe filter from {}: {}", client_id, e);
                    send_error_frame(ctx.ctrl_tx, format!("Invalid filter '{}': {}", filter, e));
                    return;
                }
            };
            ctx.subscriber_manager.unsubscribe(client_id, &subscription_filter);
            info!("Client {} unsubscribed from: {:?}", client_id, filter);
        }
        ServerMessage::Ping => {
            debug!("Received ping from {}", client_id);
            // Pong is handled automatically
        }
    }
}

/// Parse subscription filter from string
fn parse_subscription_filter(filter_str: &str) -> Result<SubscriptionFilter> {
    // Format: "type:value" or "all"
    if filter_str == "all" {
        return Ok(SubscriptionFilter::All);
    }

    let parts: Vec<&str> = filter_str.split(':').collect();
    if parts.len() < 2 {
        anyhow::bail!("Invalid filter format: {}", filter_str);
    }

    match parts[0] {
        "entity_type" => Ok(SubscriptionFilter::EntityType(parts[1].to_string())),
        "notification_type" => Ok(SubscriptionFilter::NotificationType(parts[1].to_string())),
        "trigger_ref" => Ok(SubscriptionFilter::TriggerRef(parts[1].to_string())),
        "user" => {
            let user_id: i64 = parts[1].parse().context("Invalid user ID")?;
            Ok(SubscriptionFilter::User(user_id))
        }
        "entity" => {
            if parts.len() < 3 {
                anyhow::bail!("Entity filter requires type and id: entity:type:id");
            }
            let entity_id: i64 = parts[2].parse().context("Invalid entity ID")?;
            Ok(SubscriptionFilter::Entity {
                entity_type: parts[1].to_string(),
                entity_id,
            })
        }
        _ => anyhow::bail!("Unknown filter type: {}", parts[0]),
    }
}

/// Messages sent from server to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum ClientMessage {
    #[serde(rename = "welcome")]
    Welcome { client_id: String, message: String },

    #[serde(rename = "notification")]
    Notification(Notification),

    #[serde(rename = "error")]
    Error { message: String },
}

/// Frames the receive loop can push to the outgoing task. Either a serializable
/// `ClientMessage` or a `Close` frame (which terminates the connection).
enum OutgoingFrame {
    Message(ClientMessage),
    Close { code: u16, reason: String },
}

/// Messages sent from client to server
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "subscribe")]
    Subscribe { filter: String },

    #[serde(rename = "unsubscribe")]
    Unsubscribe { filter: String },

    #[serde(rename = "ping")]
    Ping,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_subscription_filter_all() {
        let filter = parse_subscription_filter("all").unwrap();
        assert_eq!(filter, SubscriptionFilter::All);
    }

    #[test]
    fn test_parse_subscription_filter_entity_type() {
        let filter = parse_subscription_filter("entity_type:execution").unwrap();
        assert_eq!(
            filter,
            SubscriptionFilter::EntityType("execution".to_string())
        );
    }

    #[test]
    fn test_parse_subscription_filter_notification_type() {
        let filter =
            parse_subscription_filter("notification_type:execution_status_changed").unwrap();
        assert_eq!(
            filter,
            SubscriptionFilter::NotificationType("execution_status_changed".to_string())
        );
    }

    #[test]
    fn test_parse_subscription_filter_user() {
        let filter = parse_subscription_filter("user:123").unwrap();
        assert_eq!(filter, SubscriptionFilter::User(123));
    }

    #[test]
    fn test_parse_subscription_filter_entity() {
        let filter = parse_subscription_filter("entity:execution:456").unwrap();
        assert_eq!(
            filter,
            SubscriptionFilter::Entity {
                entity_type: "execution".to_string(),
                entity_id: 456
            }
        );
    }

    #[test]
    fn test_parse_subscription_filter_trigger_ref() {
        let filter = parse_subscription_filter("trigger_ref:core.intervaltimer").unwrap();
        assert_eq!(
            filter,
            SubscriptionFilter::TriggerRef("core.intervaltimer".to_string())
        );
    }

    #[test]
    fn test_parse_subscription_filter_invalid() {
        let result = parse_subscription_filter("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_subscription_filter_invalid_user_id() {
        let result = parse_subscription_filter("user:not_a_number");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_subscription_filter_entity_missing_id() {
        let result = parse_subscription_filter("entity:execution");
        assert!(result.is_err());
    }

    // -------- Auth / ACL helpers --------

    fn jwt_test_config() -> JwtConfig {
        attune_common::auth::install_crypto_provider();
        JwtConfig {
            secret: "ws_test_secret".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 604800,
        }
    }

    #[test]
    fn test_parse_bearer_token_present() {
        assert_eq!(
            parse_bearer_token("Bearer abc.def.ghi"),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn test_parse_bearer_token_case_insensitive_scheme() {
        assert_eq!(parse_bearer_token("bearer x.y.z"), Some("x.y.z"));
    }

    #[test]
    fn test_parse_bearer_token_empty_rejected() {
        assert_eq!(parse_bearer_token("Bearer   "), None);
    }

    #[test]
    fn test_parse_token_subprotocol_present() {
        assert_eq!(
            parse_token_subprotocol("attune.v1, attune.jwt.abc.def.ghi"),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn test_parse_token_subprotocol_missing() {
        assert_eq!(parse_token_subprotocol("attune.v1"), None);
    }

    #[test]
    fn test_extract_ws_token_prefers_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer header.token".parse().unwrap(),
        );
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "attune.v1, attune.jwt.protocol.token".parse().unwrap(),
        );
        assert_eq!(extract_ws_token(&headers).as_deref(), Some("header.token"));
    }

    #[test]
    fn test_extract_ws_token_from_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "attune.v1, attune.jwt.protocol.token".parse().unwrap(),
        );
        assert_eq!(
            extract_ws_token(&headers).as_deref(),
            Some("protocol.token")
        );
    }

    #[test]
    fn test_verify_ws_token_access_ok() {
        let cfg = jwt_test_config();
        let token = attune_common::auth::generate_access_token(42, "alice", &cfg).unwrap();
        let verified = verify_ws_token(&token, &cfg).expect("should verify");
        assert_eq!(verified.identity_id, 42);
        assert_eq!(verified.token_type, TokenType::Access);
        assert!(verified.token_exp > chrono::Utc::now().timestamp());
        assert!(verified.permission_set_refs.is_empty());
        assert!(verified.sensor_trigger_refs.is_empty());
    }

    #[test]
    fn test_verify_ws_token_execution_ok() {
        let cfg = jwt_test_config();
        let token = attune_common::auth::generate_execution_token(7, 1234, "core.echo", &cfg, None)
            .unwrap();
        let verified = verify_ws_token(&token, &cfg).expect("should verify");
        assert_eq!(verified.identity_id, 7);
        assert_eq!(verified.token_type, TokenType::Execution);
        assert!(verified.token_exp > 0);
        assert!(verified.permission_set_refs.is_empty());
        assert!(verified.sensor_trigger_refs.is_empty());
    }

    #[test]
    fn test_verify_ws_token_refresh_rejected() {
        let cfg = jwt_test_config();
        let token = attune_common::auth::generate_refresh_token(1, "bob", &cfg).unwrap();
        assert!(verify_ws_token(&token, &cfg).is_err());
    }

    #[test]
    fn test_verify_ws_token_sensor_allowed() {
        let cfg = jwt_test_config();
        let token = attune_common::auth::generate_sensor_token(
            5,
            "sensor:core.timer",
            vec!["core.timer".to_string()],
            &cfg,
            None,
        )
        .unwrap();
        assert!(verify_ws_token(&token, &cfg).is_ok());
    }

    #[test]
    fn test_verify_ws_token_invalid_garbage() {
        let cfg = jwt_test_config();
        assert!(verify_ws_token("not.a.token", &cfg).is_err());
    }

    #[test]
    fn test_verify_ws_token_wrong_secret() {
        let cfg = jwt_test_config();
        let other = JwtConfig {
            secret: "different".to_string(),
            ..cfg.clone()
        };
        let token = attune_common::auth::generate_access_token(1, "x", &cfg).unwrap();
        assert!(verify_ws_token(&token, &other).is_err());
    }

    fn dummy_pool() -> PgPool {
        PgPool::connect_lazy("postgresql://attune:attune@localhost/attune").unwrap()
    }

    fn read_grant(resource: Resource) -> Grant {
        Grant {
            resource,
            actions: vec![Action::Read],
            constraints: None,
        }
    }

    #[tokio::test]
    async fn test_filter_acl_user_self_allowed() {
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                99,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_user_other_denied() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_user_admin_role_allowed() {
        let roles = vec!["admin".to_string()];
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                42,
                &TokenType::Access,
                &[],
                &roles,
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_user_non_admin_role_denied() {
        let roles = vec!["user".to_string()];
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                42,
                &TokenType::Access,
                &[],
                &roles,
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_user_empty_roles_denied() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_user_admin_among_many_roles_allowed() {
        let roles = vec![
            "user".to_string(),
            "admin".to_string(),
            "operator".to_string(),
        ];
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::User(99),
                42,
                &TokenType::Access,
                &[],
                &roles,
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_all_requires_admin_or_all_operational_reads() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::All,
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        let grants = vec![
            read_grant(Resource::Events),
            read_grant(Resource::Enforcements),
            read_grant(Resource::Executions),
        ];
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::All,
                42,
                &TokenType::Access,
                &[],
                &[],
                &grants,
                &dummy_pool(),
            )
            .await
        );
        let grants = vec![
            read_grant(Resource::Events),
            read_grant(Resource::Enforcements),
            read_grant(Resource::Executions),
            read_grant(Resource::Rules),
        ];
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::All,
                42,
                &TokenType::Access,
                &[],
                &[],
                &grants,
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_entity_type_requires_matching_read() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::EntityType("execution".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::EntityType("execution".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[read_grant(Resource::Executions)],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::EntityType("rule_lifecycle".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::EntityType("rule_lifecycle".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[read_grant(Resource::Rules)],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_entity_denies_missing_record_or_permission() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::Entity {
                    entity_type: "execution".to_string(),
                    entity_id: 1
                },
                42,
                &TokenType::Access,
                &[],
                &[],
                &[read_grant(Resource::Executions)],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_notification_type_requires_matching_read() {
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::NotificationType("execution_status_changed".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::NotificationType("execution_status_changed".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[read_grant(Resource::Executions)],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::NotificationType("rule_lifecycle_changed".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::NotificationType("rule_lifecycle_changed".to_string()),
                42,
                &TokenType::Access,
                &[],
                &[],
                &[read_grant(Resource::Rules)],
                &dummy_pool(),
            )
            .await
        );
    }

    #[tokio::test]
    async fn test_filter_acl_sensor_token_trigger_ref_scoped() {
        let allowed_refs = vec![
            "core.intervaltimer".to_string(),
            "core.crontimer".to_string(),
        ];
        assert!(
            filter_allowed_for_identity(
                &SubscriptionFilter::TriggerRef("core.intervaltimer".to_string()),
                42,
                &TokenType::Sensor,
                &allowed_refs,
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::TriggerRef("core.webhook".to_string()),
                42,
                &TokenType::Sensor,
                &allowed_refs,
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
        assert!(
            !filter_allowed_for_identity(
                &SubscriptionFilter::NotificationType("event_created".to_string()),
                42,
                &TokenType::Sensor,
                &allowed_refs,
                &[],
                &[],
                &dummy_pool(),
            )
            .await
        );
    }

    // -------- Token expiration helpers --------

    #[test]
    fn test_is_token_expired_future() {
        let now = 1_000_000;
        assert!(!is_token_expired(now + 60, now));
    }

    #[test]
    fn test_is_token_expired_past() {
        let now = 1_000_000;
        assert!(is_token_expired(now - 1, now));
    }

    #[test]
    fn test_is_token_expired_exact() {
        let now = 1_000_000;
        // exp == now is treated as expired (the token's lifetime has elapsed)
        assert!(is_token_expired(now, now));
    }

    #[test]
    fn test_is_token_expired_zero_treated_as_expired() {
        assert!(is_token_expired(0, 1_000_000));
    }

    #[test]
    fn test_is_token_expired_negative_treated_as_expired() {
        assert!(is_token_expired(-1, 1_000_000));
    }

    #[test]
    fn test_is_admin_helper() {
        assert!(is_admin(&["admin".to_string()]));
        assert!(is_admin(&["user".to_string(), "admin".to_string()]));
        assert!(!is_admin(&[]));
        assert!(!is_admin(&["user".to_string()]));
        // Case-sensitive: only exact "admin" matches
        assert!(!is_admin(&["Admin".to_string()]));
        assert!(!is_admin(&["ADMIN".to_string()]));
    }
}
