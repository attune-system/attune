//! Rule Lifecycle Listener
//!
//! Listens for rule lifecycle events from the notifier WebSocket stream and
//! manages timer instances accordingly.

use crate::api_client::{ApiClient, ManagedRule};
use crate::timer_manager::TimerManager;
use crate::types::{RuleLifecycleEvent, TimerConfig};
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{lookup_host, TcpStream};
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, Request};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, connect_async, WebSocketStream};
use tracing::{debug, error, info, warn};

const TIMER_TRIGGER_REFS: &[&str] = &[
    "core.intervaltimer",
    "core.crontimer",
    "core.datetimetimer",
    "core.rruletimer",
];
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const HEALTHY_SESSION_RESET_THRESHOLD: Duration = Duration::from_secs(30);
const UNAUTHORIZED_SUBSCRIPTION_ERROR_MESSAGE: &str =
    "Unauthorized to subscribe to requested filter";

/// Rule lifecycle listener
pub struct RuleLifecycleListener {
    notifier_ws_url: String,
    sensor_ref: String,
    api_client: ApiClient,
    timer_manager: TimerManager,
}

impl RuleLifecycleListener {
    /// Create a new rule lifecycle listener
    pub fn new(
        notifier_ws_url: String,
        sensor_ref: String,
        api_client: ApiClient,
        timer_manager: TimerManager,
    ) -> Self {
        Self {
            notifier_ws_url,
            sensor_ref,
            api_client,
            timer_manager,
        }
    }

    /// Start listening for rule lifecycle events.
    ///
    /// This function keeps reconnecting on disconnect/error.
    pub async fn start(self) -> Result<()> {
        let mut backoff = INITIAL_RECONNECT_BACKOFF;

        loop {
            let session_started_at = Instant::now();
            let listen_result = self.connect_and_listen().await;
            let session_duration = session_started_at.elapsed();
            let healthy_session = is_healthy_session(&listen_result, session_duration);
            let reconnect_delay = reconnect_delay_for_session(backoff, healthy_session);

            match listen_result {
                Ok(()) => {
                    info!(
                        "Rule lifecycle websocket stream ended; reconnecting in {:?}",
                        reconnect_delay
                    );
                }
                Err(error) => {
                    let error_chain = format_error_chain(&error);
                    warn!(
                        "Rule lifecycle websocket listener error: {}. Reconnecting in {:?}. chain={}",
                        error, reconnect_delay, error_chain
                    );
                }
            }

            if healthy_session && backoff != INITIAL_RECONNECT_BACKOFF {
                info!(
                    "Resetting websocket reconnect backoff from {:?} to {:?} after healthy session ({:?})",
                    backoff, INITIAL_RECONNECT_BACKOFF, session_duration
                );
            }

            tokio::time::sleep(reconnect_delay).await;
            backoff = next_reconnect_backoff(backoff, healthy_session);
        }
    }

    async fn connect_and_listen(&self) -> Result<()> {
        let token = self.api_client.get_token().await;
        let lifecycle_trigger_refs = resolve_timer_lifecycle_trigger_refs(&token)?;
        let request = build_ws_request(&self.notifier_ws_url, &token)?;
        let uri = request.uri().clone();

        if uri.scheme_str() == Some("ws") {
            let (ws_stream, _response) = self
                .connect_plain_ws(request)
                .await
                .with_context(|| format!("Failed to connect to notifier websocket at {}", uri))?;
            self.run_ws_stream(ws_stream, &lifecycle_trigger_refs).await
        } else {
            let (ws_stream, _response) = connect_async(request)
                .await
                .with_context(|| format!("Failed to connect to notifier websocket at {}", uri))?;
            self.run_ws_stream(ws_stream, &lifecycle_trigger_refs).await
        }
    }

    async fn connect_plain_ws(
        &self,
        request: Request<()>,
    ) -> Result<(
        WebSocketStream<TcpStream>,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    )> {
        let uri = request.uri().clone();
        let host = uri
            .host()
            .ok_or_else(|| anyhow::anyhow!("Notifier websocket URL is missing a host"))?;
        let port = uri.port_u16().unwrap_or(80);

        let resolved_addrs: Vec<_> = lookup_host((host, port))
            .await
            .with_context(|| format!("Failed to resolve notifier host {}:{}", host, port))?
            .collect();

        if resolved_addrs.is_empty() {
            anyhow::bail!(
                "Notifier host {}:{} resolved to no socket addresses",
                host,
                port
            );
        }

        debug!(
            "Resolved notifier websocket host {}:{} to {:?}",
            host, port, resolved_addrs
        );

        let mut last_error = None;
        for addr in resolved_addrs {
            match TcpStream::connect(addr).await {
                Ok(stream) => {
                    debug!("Connected TCP stream to notifier websocket at {}", addr);
                    return client_async(request, stream).await.with_context(|| {
                        format!("WebSocket handshake failed after TCP connect to {}", addr)
                    });
                }
                Err(error) => {
                    debug!(
                        "Failed TCP connect to notifier websocket at {}: {}",
                        addr, error
                    );
                    last_error = Some((addr, error));
                }
            }
        }

        if let Some((addr, error)) = last_error {
            Err(anyhow::anyhow!(
                "Failed TCP connect to notifier websocket at {} (last error: {})",
                addr,
                error
            ))
        } else {
            unreachable!("resolved_addrs should not be empty");
        }
    }

    async fn run_ws_stream<S>(
        &self,
        mut ws_stream: WebSocketStream<S>,
        lifecycle_trigger_refs: &[String],
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        info!(
            "Connected to notifier websocket for sensor {}",
            self.sensor_ref
        );

        for trigger_ref in lifecycle_trigger_refs {
            let subscribe = serde_json::json!({
                "type": "subscribe",
                "filter": format!("trigger_ref:{}", trigger_ref)
            });
            ws_stream
                .send(Message::Text(subscribe.to_string().into()))
                .await
                .with_context(|| {
                    format!(
                        "Failed to subscribe to trigger_ref:{} lifecycle stream",
                        trigger_ref
                    )
                })?;
        }

        self.reconcile_active_rules(lifecycle_trigger_refs).await?;

        while let Some(message) = ws_stream.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    self.handle_ws_text(text.as_ref()).await?;
                }
                Ok(Message::Close(frame)) => {
                    info!("Notifier websocket closed: {:?}", frame);
                    break;
                }
                Ok(Message::Binary(_)) => {
                    debug!("Ignoring binary websocket message");
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                Ok(Message::Frame(_)) => {}
                Err(error) => {
                    return Err(anyhow::anyhow!("WebSocket receive error: {}", error));
                }
            }
        }

        Ok(())
    }

    async fn handle_ws_text(&self, text: &str) -> Result<()> {
        let value: JsonValue =
            serde_json::from_str(text).context("Failed to parse websocket text as JSON")?;

        match value.get("type").and_then(|v| v.as_str()) {
            Some("welcome") => return Ok(()),
            Some("error") => {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if message.contains(UNAUTHORIZED_SUBSCRIPTION_ERROR_MESSAGE) {
                    warn!(
                        "Notifier rejected a trigger_ref subscription for sensor {}: {}. \
                         Continuing with remaining authorized subscriptions.",
                        self.sensor_ref, message
                    );
                    return Ok(());
                }
                return Err(anyhow::anyhow!("Notifier websocket error frame: {}", value));
            }
            Some("notification") => {}
            _ => {
                debug!("Ignoring websocket frame with unknown type: {}", value);
                return Ok(());
            }
        }

        let payload = value.get("payload").cloned().unwrap_or(JsonValue::Null);
        let Some(event) = parse_rule_lifecycle_payload(&payload) else {
            debug!("Ignoring non-rule-lifecycle notification: {}", payload);
            return Ok(());
        };

        self.handle_event(event).await
    }

    async fn reconcile_active_rules(&self, lifecycle_trigger_refs: &[String]) -> Result<()> {
        let trigger_refs: Vec<&str> = lifecycle_trigger_refs
            .iter()
            .map(std::string::String::as_str)
            .collect();
        let rules = self
            .api_client
            .list_active_rules_by_trigger_refs(&trigger_refs)
            .await
            .context("Failed to fetch active timer rules during websocket reconciliation")?;

        self.reconcile_rule_snapshot(rules).await
    }

    async fn reconcile_rule_snapshot(&self, rules: Vec<ManagedRule>) -> Result<()> {
        let snapshot_rule_ids: HashSet<i64> = rules.iter().map(|rule| rule.id).collect();

        info!(
            "Reconciling timer rules from snapshot after websocket connect: {} active rule(s)",
            rules.len()
        );

        for active_rule_id in self.timer_manager.active_rule_ids().await {
            if !snapshot_rule_ids.contains(&active_rule_id) {
                info!(
                    "Stopping timer for rule {} because it is no longer active in websocket reconciliation snapshot",
                    active_rule_id
                );
                self.timer_manager.stop_timer(active_rule_id).await;
            }
        }

        for rule in rules {
            let rule_id = rule.id;
            let rule_ref = rule.r#ref;
            let trigger_ref = rule.trigger_ref;
            let trigger_params = rule.trigger_params;

            if let Err(error) = self
                .start_timer_from_params(rule_id, &trigger_ref, Some(trigger_params))
                .await
            {
                error!(
                    "Failed to restore timer for rule {} during reconciliation: {}",
                    rule_ref, error
                );
                self.timer_manager.stop_timer(rule_id).await;
            }
        }

        Ok(())
    }

    /// Handle a rule lifecycle event
    async fn handle_event(&self, event: RuleLifecycleEvent) -> Result<()> {
        match event {
            RuleLifecycleEvent::RuleCreated {
                rule_id,
                rule_ref,
                trigger_type,
                trigger_params,
                enabled,
                ..
            } => {
                info!(
                    "Handling RuleCreated: rule_id={}, ref={}, trigger={}, enabled={}",
                    rule_id, rule_ref, trigger_type, enabled
                );

                if enabled {
                    self.start_timer_from_params(rule_id, &trigger_type, trigger_params)
                        .await?;
                } else {
                    info!("Rule {} is disabled, not starting timer", rule_id);
                }
            }
            RuleLifecycleEvent::RuleEnabled {
                rule_id,
                rule_ref,
                trigger_type,
                trigger_params,
                ..
            } => {
                info!(
                    "Handling RuleEnabled: rule_id={}, ref={}",
                    rule_id, rule_ref
                );

                self.start_timer_from_params(rule_id, &trigger_type, trigger_params)
                    .await?;
            }
            RuleLifecycleEvent::RuleDisabled {
                rule_id, rule_ref, ..
            } => {
                info!(
                    "Handling RuleDisabled: rule_id={}, ref={}",
                    rule_id, rule_ref
                );

                self.timer_manager.stop_timer(rule_id).await;
            }
            RuleLifecycleEvent::RuleDeleted {
                rule_id, rule_ref, ..
            } => {
                info!(
                    "Handling RuleDeleted: rule_id={}, ref={}",
                    rule_id, rule_ref
                );

                self.timer_manager.stop_timer(rule_id).await;
            }
        }

        Ok(())
    }

    /// Start a timer from trigger parameters
    async fn start_timer_from_params(
        &self,
        rule_id: i64,
        trigger_ref: &str,
        trigger_params: Option<JsonValue>,
    ) -> Result<()> {
        let params = trigger_params.ok_or_else(|| {
            anyhow::anyhow!("Timer trigger requires trigger_params but none provided")
        })?;

        info!(
            "Parsing timer config for rule {}: trigger_ref='{}', params={}",
            rule_id,
            trigger_ref,
            serde_json::to_string(&params).unwrap_or_else(|_| "<invalid json>".to_string())
        );

        let config = TimerConfig::from_trigger_params(trigger_ref, params)
            .context("Failed to parse trigger_params as TimerConfig")?;

        info!(
            "Starting timer for rule {} with config: {:?}",
            rule_id, config
        );

        self.timer_manager
            .start_timer(rule_id, config)
            .await
            .context("Failed to start timer")?;

        info!("Timer started successfully for rule {}", rule_id);

        Ok(())
    }
}

fn resolve_timer_lifecycle_trigger_refs(token: &str) -> Result<Vec<String>> {
    let requested_refs = match sensor_trigger_refs_from_token(token) {
        Ok(Some(sensor_scope_refs)) => {
            let timer_scope_refs: Vec<String> = TIMER_TRIGGER_REFS
                .iter()
                .filter(|trigger_ref| {
                    sensor_scope_refs
                        .iter()
                        .any(|allowed| allowed == **trigger_ref)
                })
                .map(|trigger_ref| (*trigger_ref).to_string())
                .collect();

            let disallowed_timer_refs: Vec<&str> = TIMER_TRIGGER_REFS
                .iter()
                .copied()
                .filter(|trigger_ref| {
                    !sensor_scope_refs
                        .iter()
                        .any(|allowed| allowed == trigger_ref)
                })
                .collect();

            if !disallowed_timer_refs.is_empty() {
                warn!(
                    "Sensor token scope excludes some supported lifecycle trigger refs {:?}; \
                     this sensor will subscribe only to {:?}",
                    disallowed_timer_refs, timer_scope_refs
                );
            }

            timer_scope_refs
        }
        Ok(None) => {
            debug!(
                "Token for sensor websocket listener has no trigger_types scope metadata; \
                 subscribing to all lifecycle trigger refs supported by this sensor"
            );
            TIMER_TRIGGER_REFS
                .iter()
                .map(|trigger_ref| (*trigger_ref).to_string())
                .collect()
        }
        Err(error) => {
            warn!(
                "Failed to decode token trigger scope for sensor websocket listener: {}. \
                 Subscribing to all lifecycle trigger refs supported by this sensor.",
                error
            );
            TIMER_TRIGGER_REFS
                .iter()
                .map(|trigger_ref| (*trigger_ref).to_string())
                .collect()
        }
    };

    if requested_refs.is_empty() {
        anyhow::bail!(
            "Sensor token scope does not allow any lifecycle trigger refs supported by this sensor ({:?})",
            TIMER_TRIGGER_REFS
        );
    }

    Ok(requested_refs)
}

fn sensor_trigger_refs_from_token(token: &str) -> Result<Option<Vec<String>>> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Invalid JWT format: missing payload"))?;
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| general_purpose::STANDARD.decode(payload))
        .map_err(|error| anyhow::anyhow!("Failed to decode JWT payload: {}", error))?;

    let claims: JsonValue = serde_json::from_slice(&decoded)
        .map_err(|error| anyhow::anyhow!("Failed to parse JWT payload JSON: {}", error))?;

    let Some(trigger_types) = claims
        .get("metadata")
        .and_then(|metadata| metadata.get("trigger_types"))
        .and_then(JsonValue::as_array)
    else {
        return Ok(None);
    };

    let mut trigger_refs: Vec<String> = trigger_types
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    trigger_refs.sort();
    trigger_refs.dedup();

    Ok(Some(trigger_refs))
}

fn build_ws_request(ws_url: &str, token: &str) -> Result<Request<()>> {
    let mut request = ws_url
        .into_client_request()
        .context("Failed to build websocket request")?;
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {}", token).parse()?);
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("attune.v1, attune.jwt.{}", token).parse()?,
    );
    Ok(request)
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | caused by: ")
}

fn reconnect_delay_for_session(current_backoff: Duration, healthy_session: bool) -> Duration {
    if healthy_session {
        INITIAL_RECONNECT_BACKOFF
    } else {
        current_backoff
    }
}

fn next_reconnect_backoff(current_backoff: Duration, healthy_session: bool) -> Duration {
    if healthy_session {
        INITIAL_RECONNECT_BACKOFF
    } else {
        std::cmp::min(current_backoff * 2, MAX_RECONNECT_BACKOFF)
    }
}

fn is_healthy_session(listen_result: &Result<()>, session_duration: Duration) -> bool {
    listen_result.is_ok() && session_duration >= HEALTHY_SESSION_RESET_THRESHOLD
}

fn parse_rule_lifecycle_payload(payload: &JsonValue) -> Option<RuleLifecycleEvent> {
    let event_type = payload.get("event_type")?.as_str()?;
    let rule_id = payload.get("rule_id")?.as_i64()?;
    let rule_ref = payload.get("rule_ref")?.as_str()?.to_string();
    let trigger_ref = payload.get("trigger_ref")?.as_str()?.to_string();
    let trigger_params = payload.get("trigger_params").cloned();
    let active = payload
        .get("active")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let timestamp = Utc::now();

    match event_type {
        "rule.created" => Some(RuleLifecycleEvent::RuleCreated {
            rule_id,
            rule_ref,
            trigger_type: trigger_ref,
            trigger_params,
            enabled: active,
            timestamp,
        }),
        "rule.enabled" => Some(RuleLifecycleEvent::RuleEnabled {
            rule_id,
            rule_ref,
            trigger_type: trigger_ref,
            trigger_params,
            timestamp,
        }),
        "rule.disabled" => Some(RuleLifecycleEvent::RuleDisabled {
            rule_id,
            rule_ref,
            trigger_type: trigger_ref,
            timestamp,
        }),
        "rule.deleted" => Some(RuleLifecycleEvent::RuleDeleted {
            rule_id,
            rule_ref,
            trigger_type: trigger_ref,
            timestamp,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;

    fn test_token_with_trigger_types(trigger_types: Option<Vec<&str>>) -> String {
        let mut payload = serde_json::json!({
            "sub": "42",
            "token_type": "sensor"
        });
        if let Some(trigger_types) = trigger_types {
            payload["metadata"] = serde_json::json!({
                "trigger_types": trigger_types
            });
        }

        let payload_b64 = general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("header.{}.signature", payload_b64)
    }

    #[test]
    fn test_resolve_timer_lifecycle_trigger_refs_scopes_to_token_metadata() {
        let token =
            test_token_with_trigger_types(Some(vec!["core.intervaltimer", "core.crontimer"]));

        let refs = resolve_timer_lifecycle_trigger_refs(&token).expect("scope should resolve");

        assert_eq!(
            refs,
            vec![
                "core.intervaltimer".to_string(),
                "core.crontimer".to_string()
            ]
        );
    }

    #[test]
    fn test_resolve_timer_lifecycle_trigger_refs_rejects_empty_timer_scope() {
        let token = test_token_with_trigger_types(Some(vec!["core.webhook"]));

        let result = resolve_timer_lifecycle_trigger_refs(&token);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not allow any lifecycle trigger refs supported by this sensor"));
    }

    #[test]
    fn test_resolve_timer_lifecycle_trigger_refs_defaults_to_all_for_unscoped_tokens() {
        let token = test_token_with_trigger_types(None);

        let refs = resolve_timer_lifecycle_trigger_refs(&token).expect("scope should resolve");

        assert_eq!(
            refs,
            TIMER_TRIGGER_REFS
                .iter()
                .map(|trigger_ref| (*trigger_ref).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_rule_enabled_payload() {
        let payload = serde_json::json!({
            "event_type": "rule.enabled",
            "rule_id": 42,
            "rule_ref": "core.test_rule",
            "trigger_ref": "core.intervaltimer",
            "trigger_params": { "interval": 5, "unit": "seconds" },
            "active": true
        });
        let event = parse_rule_lifecycle_payload(&payload).expect("event should parse");
        match event {
            RuleLifecycleEvent::RuleEnabled {
                rule_id,
                rule_ref,
                trigger_type,
                trigger_params,
                ..
            } => {
                assert_eq!(rule_id, 42);
                assert_eq!(rule_ref, "core.test_rule");
                assert_eq!(trigger_type, "core.intervaltimer");
                assert_eq!(
                    trigger_params,
                    Some(serde_json::json!({ "interval": 5, "unit": "seconds" }))
                );
            }
            _ => panic!("expected RuleEnabled"),
        }
    }

    #[test]
    fn test_parse_rule_disabled_payload() {
        let payload = serde_json::json!({
            "event_type": "rule.disabled",
            "rule_id": 9,
            "rule_ref": "core.rule_off",
            "trigger_ref": "core.crontimer",
            "active": false
        });
        let event = parse_rule_lifecycle_payload(&payload).expect("event should parse");
        match event {
            RuleLifecycleEvent::RuleDisabled {
                rule_id,
                rule_ref,
                trigger_type,
                ..
            } => {
                assert_eq!(rule_id, 9);
                assert_eq!(rule_ref, "core.rule_off");
                assert_eq!(trigger_type, "core.crontimer");
            }
            _ => panic!("expected RuleDisabled"),
        }
    }

    #[test]
    fn test_parse_rule_deleted_payload() {
        let payload = serde_json::json!({
            "event_type": "rule.deleted",
            "rule_id": 12,
            "rule_ref": "core.rule_gone",
            "trigger_ref": "core.intervaltimer",
            "active": false
        });
        let event = parse_rule_lifecycle_payload(&payload).expect("event should parse");
        match event {
            RuleLifecycleEvent::RuleDeleted {
                rule_id,
                rule_ref,
                trigger_type,
                ..
            } => {
                assert_eq!(rule_id, 12);
                assert_eq!(rule_ref, "core.rule_gone");
                assert_eq!(trigger_type, "core.intervaltimer");
            }
            _ => panic!("expected RuleDeleted"),
        }
    }

    #[tokio::test]
    async fn test_reconcile_rule_snapshot_keeps_unchanged_timers_and_updates_per_rule() {
        let api_client = ApiClient::new("http://localhost:8080".to_string(), "token".to_string());
        let timer_manager = TimerManager::new(api_client.clone(), "core.timer_sensor".to_string())
            .await
            .unwrap();
        let listener = RuleLifecycleListener::new(
            "ws://localhost:8081/ws".to_string(),
            "core.timer_sensor".to_string(),
            api_client,
            timer_manager.clone(),
        );

        timer_manager
            .start_timer(
                1,
                TimerConfig::Interval {
                    interval: 60,
                    unit: crate::types::TimeUnit::Seconds,
                },
            )
            .await
            .unwrap();
        timer_manager
            .start_timer(
                2,
                TimerConfig::Interval {
                    interval: 30,
                    unit: crate::types::TimeUnit::Seconds,
                },
            )
            .await
            .unwrap();

        let unchanged_job = timer_manager
            .job_uuid_for_rule(1)
            .await
            .expect("rule 1 timer should exist");

        listener
            .reconcile_rule_snapshot(vec![
                ManagedRule {
                    id: 1,
                    r#ref: "core.rule_1".to_string(),
                    trigger_ref: "core.intervaltimer".to_string(),
                    trigger_params: serde_json::json!({"interval": 60, "unit": "seconds"}),
                    enabled: true,
                },
                ManagedRule {
                    id: 3,
                    r#ref: "core.rule_3".to_string(),
                    trigger_ref: "core.intervaltimer".to_string(),
                    trigger_params: serde_json::json!({"interval": 15, "unit": "seconds"}),
                    enabled: true,
                },
            ])
            .await
            .unwrap();

        assert_eq!(timer_manager.timer_count().await, 2);
        assert_eq!(
            timer_manager.job_uuid_for_rule(1).await,
            Some(unchanged_job)
        );
        assert!(timer_manager.job_uuid_for_rule(2).await.is_none());
        assert!(timer_manager.job_uuid_for_rule(3).await.is_some());

        timer_manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_error_frame_is_fatal() {
        let api_client = ApiClient::new("http://localhost:8080".to_string(), "token".to_string());
        let timer_manager = TimerManager::new(api_client.clone(), "core.timer_sensor".to_string())
            .await
            .unwrap();
        let listener = RuleLifecycleListener::new(
            "ws://localhost:8081/ws".to_string(),
            "core.timer_sensor".to_string(),
            api_client,
            timer_manager,
        );

        let result = listener
            .handle_ws_text(r#"{"type":"error","message":"denied"}"#)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("error frame"));
    }

    #[tokio::test]
    async fn test_unauthorized_subscription_error_frame_is_non_fatal() {
        let api_client = ApiClient::new("http://localhost:8080".to_string(), "token".to_string());
        let timer_manager = TimerManager::new(api_client.clone(), "core.timer_sensor".to_string())
            .await
            .unwrap();
        let listener = RuleLifecycleListener::new(
            "ws://localhost:8081/ws".to_string(),
            "core.timer_sensor".to_string(),
            api_client,
            timer_manager,
        );

        let result = listener
            .handle_ws_text(&format!(
                r#"{{"type":"error","message":"{}"}}"#,
                UNAUTHORIZED_SUBSCRIPTION_ERROR_MESSAGE
            ))
            .await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_build_ws_request_includes_header_and_subprotocol_auth() {
        let request = build_ws_request("ws://notifier:8081/ws", "abc.def.ghi").unwrap();

        assert_eq!(request.uri().to_string(), "ws://notifier:8081/ws");
        assert_eq!(
            request.headers().get(AUTHORIZATION).unwrap(),
            "Bearer abc.def.ghi"
        );
        assert_eq!(
            request.headers().get("Sec-WebSocket-Protocol").unwrap(),
            "attune.v1, attune.jwt.abc.def.ghi"
        );
        assert!(request.headers().get("Sec-WebSocket-Key").is_some());
    }

    #[test]
    fn test_parse_ignores_unknown_event_type() {
        let payload = serde_json::json!({
            "event_type": "execution.completed",
            "rule_id": 1,
            "rule_ref": "core.other",
            "trigger_ref": "core.intervaltimer"
        });
        assert!(parse_rule_lifecycle_payload(&payload).is_none());
    }

    #[test]
    fn test_reconnect_backoff_doubles_and_caps_for_unhealthy_sessions() {
        let mut backoff = INITIAL_RECONNECT_BACKOFF;

        for _ in 0..10 {
            backoff = next_reconnect_backoff(backoff, false);
        }

        assert_eq!(backoff, MAX_RECONNECT_BACKOFF);
    }

    #[test]
    fn test_reconnect_backoff_resets_after_healthy_session() {
        let current_backoff = Duration::from_secs(16);

        assert_eq!(
            reconnect_delay_for_session(current_backoff, true),
            INITIAL_RECONNECT_BACKOFF
        );
        assert_eq!(
            next_reconnect_backoff(current_backoff, true),
            INITIAL_RECONNECT_BACKOFF
        );
    }

    #[test]
    fn test_failed_session_does_not_reset_backoff_even_if_long() {
        let session_duration = HEALTHY_SESSION_RESET_THRESHOLD + Duration::from_secs(5);
        let failed_result = Err(anyhow::anyhow!("connection dropped"));
        let current_backoff = Duration::from_secs(8);
        let healthy = is_healthy_session(&failed_result, session_duration);

        assert!(!healthy);
        assert_eq!(
            reconnect_delay_for_session(current_backoff, healthy),
            current_backoff
        );
        assert_eq!(
            next_reconnect_backoff(current_backoff, healthy),
            Duration::from_secs(16)
        );
    }
}
