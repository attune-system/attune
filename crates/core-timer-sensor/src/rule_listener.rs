//! Rule Lifecycle Listener
//!
//! Listens for rule lifecycle events from RabbitMQ and manages timer instances
//! accordingly. Handles RuleCreated, RuleEnabled, RuleDisabled, and RuleDeleted events.

use crate::api_client::ApiClient;
use crate::timer_manager::TimerManager;
use crate::types::{RuleLifecycleEvent, TimerConfig};
use anyhow::{Context, Result};
use chrono::Utc;
use futures::StreamExt;
use lapin::{options::*, types::FieldTable, Channel, Connection, ConnectionProperties, Consumer};
use serde_json::Value as JsonValue;
use tracing::{debug, error, info, warn};

/// Rule lifecycle listener
pub struct RuleLifecycleListener {
    mq_url: String,
    mq_exchange: String,
    sensor_ref: String,
    api_client: ApiClient,
    timer_manager: TimerManager,
}

impl RuleLifecycleListener {
    /// Create a new rule lifecycle listener
    pub fn new(
        mq_url: String,
        mq_exchange: String,
        sensor_ref: String,
        api_client: ApiClient,
        timer_manager: TimerManager,
    ) -> Self {
        Self {
            mq_url,
            mq_exchange,
            sensor_ref,
            api_client,
            timer_manager,
        }
    }

    /// Start listening for rule lifecycle events
    pub async fn start(self) -> Result<()> {
        info!("Connecting to RabbitMQ: {}", mask_url(&self.mq_url));

        // Connect to RabbitMQ
        let connection = Connection::connect(&self.mq_url, ConnectionProperties::default())
            .await
            .context("Failed to connect to RabbitMQ")?;

        info!("Connected to RabbitMQ");

        // Create channel
        let channel = connection
            .create_channel()
            .await
            .context("Failed to create channel")?;

        info!("Created RabbitMQ channel");

        // Declare exchange (idempotent)
        channel
            .exchange_declare(
                self.mq_exchange.as_str().into(),
                lapin::ExchangeKind::Topic,
                ExchangeDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .context("Failed to declare exchange")?;

        debug!("Exchange '{}' declared", self.mq_exchange);

        // Declare sensor-specific queue
        let queue_name = format!("sensor.{}", self.sensor_ref);
        channel
            .queue_declare(
                queue_name.as_str().into(),
                QueueDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .context("Failed to declare queue")?;

        info!("Queue '{}' declared", queue_name);

        // Bind queue to exchange with routing keys for rule lifecycle events
        let routing_keys = vec![
            "rule.created",
            "rule.enabled",
            "rule.disabled",
            "rule.deleted",
        ];

        for routing_key in &routing_keys {
            channel
                .queue_bind(
                    queue_name.as_str().into(),
                    self.mq_exchange.as_str().into(),
                    (*routing_key).into(),
                    QueueBindOptions::default(),
                    FieldTable::default(),
                )
                .await
                .with_context(|| {
                    format!("Failed to bind queue to routing key '{}'", routing_key)
                })?;

            info!(
                "Bound queue '{}' to exchange '{}' with routing key '{}'",
                queue_name, self.mq_exchange, routing_key
            );
        }

        // Load existing active rules from API for all timer trigger types
        const ALL_TIMER_TRIGGER_REFS: &[&str] = &[
            "core.intervaltimer",
            "core.crontimer",
            "core.datetimetimer",
            "core.rruletimer",
        ];

        for trigger_ref in ALL_TIMER_TRIGGER_REFS {
            info!(
                "Fetching existing active rules for trigger '{}'",
                trigger_ref
            );
            match self.api_client.fetch_rules(trigger_ref).await {
                Ok(rules) => {
                    info!(
                        "Found {} existing rules for trigger '{}'",
                        rules.len(),
                        trigger_ref
                    );
                    for rule in rules {
                        if rule.enabled {
                            if let Err(e) = self
                                .start_timer_from_params(
                                    rule.id,
                                    trigger_ref,
                                    Some(rule.trigger_params),
                                )
                                .await
                            {
                                error!("Failed to start timer for rule {}: {}", rule.id, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch existing rules for trigger '{}': {}",
                        trigger_ref, e
                    );
                    // Continue anyway - we'll handle new rules via messages
                }
            }
        }

        // Start consuming messages
        let consumer = channel
            .basic_consume(
                queue_name.as_str().into(),
                "sensor-timer-consumer".into(),
                BasicConsumeOptions {
                    no_ack: false,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .context("Failed to create consumer")?;

        info!("Started consuming messages from queue '{}'", queue_name);

        // Process messages
        self.consume_messages(consumer, channel).await
    }

    /// Consume and process messages from the queue
    async fn consume_messages(self, mut consumer: Consumer, _channel: Channel) -> Result<()> {
        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    let payload = String::from_utf8_lossy(&delivery.data);
                    debug!("Received message: {}", payload);

                    // Parse message as JSON
                    match serde_json::from_slice::<JsonValue>(&delivery.data) {
                        Ok(json_value) => {
                            // Try to parse as RuleLifecycleEvent (native format)
                            // or as a MessageEnvelope wrapping rule payload
                            let event_opt =
                                serde_json::from_value::<RuleLifecycleEvent>(json_value.clone())
                                    .ok()
                                    .or_else(|| Self::try_parse_envelope(&json_value));

                            match event_opt {
                                Some(event) => {
                                    // Filter by trigger type - only process timer events
                                    let trigger_type = event.trigger_type();
                                    let is_timer_type = matches!(
                                        trigger_type,
                                        "core.timer"
                                            | "core.intervaltimer"
                                            | "core.crontimer"
                                            | "core.datetimetimer"
                                            | "core.rruletimer"
                                    );

                                    if is_timer_type {
                                        if let Err(e) = self.handle_event(event).await {
                                            error!("Failed to handle event: {}", e);
                                        }
                                    } else {
                                        debug!(
                                            "Ignoring event for non-timer trigger type '{}'",
                                            trigger_type
                                        );
                                    }
                                }
                                None => {
                                    warn!("Failed to parse message as rule lifecycle event");
                                    debug!("Unparseable message: {}", json_value);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to parse message as JSON: {}", e);
                        }
                    }

                    // Acknowledge message
                    if let Err(e) = delivery.ack(BasicAckOptions::default()).await {
                        error!("Failed to acknowledge message: {}", e);
                    }
                }
                Err(e) => {
                    error!("Error receiving message: {}", e);
                    // Continue processing
                }
            }
        }

        info!("Message consumer stopped");
        Ok(())
    }

    /// Try to parse a MessageEnvelope-wrapped rule lifecycle event.
    /// The API publishes messages as `{message_type, payload: {rule_id, trigger_ref, ...}}`.
    fn try_parse_envelope(json: &JsonValue) -> Option<RuleLifecycleEvent> {
        let message_type = json.get("message_type")?.as_str()?;
        let payload = json.get("payload")?;
        let rule_id = payload.get("rule_id")?.as_i64()?;
        let rule_ref = payload
            .get("rule_ref")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // API uses trigger_ref, sensor model uses trigger_type
        let trigger_type = payload
            .get("trigger_ref")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let trigger_params = payload.get("trigger_params").cloned();
        let timestamp = Utc::now();

        match message_type {
            "RuleCreated" | "rule_created" | "rule.created" => {
                let enabled = payload
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                Some(RuleLifecycleEvent::RuleCreated {
                    rule_id,
                    rule_ref,
                    trigger_type,
                    trigger_params,
                    enabled,
                    timestamp,
                })
            }
            "RuleEnabled" | "rule_enabled" | "rule.enabled" => {
                Some(RuleLifecycleEvent::RuleEnabled {
                    rule_id,
                    rule_ref,
                    trigger_type,
                    trigger_params,
                    timestamp,
                })
            }
            "RuleDisabled" | "rule_disabled" | "rule.disabled" => {
                Some(RuleLifecycleEvent::RuleDisabled {
                    rule_id,
                    rule_ref,
                    trigger_type,
                    timestamp,
                })
            }
            "RuleDeleted" | "rule_deleted" | "rule.deleted" => {
                Some(RuleLifecycleEvent::RuleDeleted {
                    rule_id,
                    rule_ref,
                    trigger_type,
                    timestamp,
                })
            }
            _ => None,
        }
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

/// Mask sensitive parts of connection strings for logging
fn mask_url(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(proto_end) = url.find("://") {
            let protocol = &url[..proto_end + 3];
            let host_and_path = &url[at_pos..];
            return format!("{}***:***{}", protocol, host_and_path);
        }
    }
    "***:***@***".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_url() {
        let url = "amqp://user:password@localhost:5672/%2F";
        let masked = mask_url(url);
        assert!(!masked.contains("user"));
        assert!(!masked.contains("password"));
        assert!(masked.contains("@localhost"));
    }

    #[test]
    fn test_mask_url_no_credentials() {
        let url = "amqp://localhost:5672";
        let masked = mask_url(url);
        assert_eq!(masked, "***:***@***");
    }
}
