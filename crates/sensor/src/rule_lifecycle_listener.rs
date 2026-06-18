//! Rule Lifecycle Listener
//!
//! This module listens for rule lifecycle events (created, enabled, disabled)
//! and notifies the sensor manager to update sensor process lifecycles accordingly.

use anyhow::Result;
use attune_common::mq::{
    Connection, Consumer, ConsumerConfig, MessageEnvelope, MessageType, PackDeletedPayload,
    PackRegisteredPayload, RuleCreatedPayload, RuleDisabledPayload, RuleEnabledPayload,
    TriggerChangedPayload,
};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{error, info, warn};

use crate::sensor_manager::SensorManager;

/// Rule lifecycle listener
pub struct RuleLifecycleListener {
    db: PgPool,
    connection: Connection,
    sensor_manager: Arc<SensorManager>,
    pack_transport: Arc<dyn attune_common::pack_transport::PackFileTransport>,
    consumer: Arc<RwLock<Option<Arc<Consumer>>>>,
    task_handle: RwLock<Option<JoinHandle<()>>>,
    trigger_id_cache: Arc<RwLock<HashMap<String, (i64, Instant)>>>,
}

const TRIGGER_ID_CACHE_TTL: Duration = Duration::from_secs(30);

impl RuleLifecycleListener {
    /// Create a new rule lifecycle listener
    pub fn new(
        db: PgPool,
        connection: Connection,
        sensor_manager: Arc<SensorManager>,
        pack_transport: Arc<dyn attune_common::pack_transport::PackFileTransport>,
    ) -> Self {
        Self {
            db,
            connection,
            sensor_manager,
            pack_transport,
            consumer: Arc::new(RwLock::new(None)),
            task_handle: RwLock::new(None),
            trigger_id_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start listening for rule lifecycle events
    pub async fn start(&self) -> Result<()> {
        info!("Starting rule lifecycle listener");

        // Create consumer configuration
        let consumer_config = ConsumerConfig {
            queue: "attune.rules.lifecycle.queue".to_string(),
            tag: "sensor-rule-lifecycle".to_string(),
            prefetch_count: 10,
            auto_ack: false,
            exclusive: false,
        };

        // Create consumer
        let consumer = Arc::new(Consumer::new(&self.connection, consumer_config).await?);

        // Bind queue to exchange with routing keys
        let exchange = "attune.events";
        let queue = "attune.rules.lifecycle.queue";

        // Declare queue
        consumer
            .channel()
            .queue_declare(
                queue.into(),
                lapin::options::QueueDeclareOptions {
                    durable: true,
                    exclusive: false,
                    auto_delete: false,
                    ..Default::default()
                },
                lapin::types::FieldTable::default(),
            )
            .await?;

        // Bind to routing keys
        for routing_key in &[
            "rule.created",
            "rule.enabled",
            "rule.disabled",
            "pack.registered",
            "pack.deleted",
            "metadata.trigger.changed",
        ] {
            consumer
                .channel()
                .queue_bind(
                    queue.into(),
                    exchange.into(),
                    (*routing_key).into(),
                    lapin::options::QueueBindOptions::default(),
                    lapin::types::FieldTable::default(),
                )
                .await?;
            info!(
                "Bound queue {} to exchange {} with routing key {}",
                queue, exchange, routing_key
            );
        }

        // Store consumer reference (for cleanup on drop)
        *self.consumer.write().await = Some(consumer.clone());

        // Clone references for the spawned task
        let db = self.db.clone();
        let sensor_manager = self.sensor_manager.clone();
        let pack_transport = self.pack_transport.clone();
        let trigger_id_cache = self.trigger_id_cache.clone();
        let consumer = consumer.clone();

        // Start consuming messages in a background task while retaining a shared consumer
        // handle so shutdown can cancel the consumer cooperatively before closing the
        // shared RabbitMQ connection.
        let handle = tokio::spawn(async move {
            let result = consumer
                .consume_with_handler::<JsonValue, _, _>(move |envelope| {
                    let db = db.clone();
                    let sensor_manager = sensor_manager.clone();
                    let pack_transport = pack_transport.clone();
                    let trigger_id_cache = trigger_id_cache.clone();

                    async move {
                        if let Err(e) = Self::handle_message(
                            &db,
                            &sensor_manager,
                            &pack_transport,
                            &trigger_id_cache,
                            envelope,
                        )
                        .await
                        {
                            error!("Failed to handle rule lifecycle message: {}", e);
                            return Err(attune_common::mq::MqError::Other(format!(
                                "Handler error: {}",
                                e
                            )));
                        }
                        Ok(())
                    }
                })
                .await;

            if let Err(e) = result {
                error!("Rule lifecycle listener stopped with error: {}", e);
            } else {
                info!("Rule lifecycle listener stopped");
            }
        });

        *self.task_handle.write().await = Some(handle);

        info!("Rule lifecycle listener started");

        Ok(())
    }

    /// Stop the listener
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping rule lifecycle listener");

        if let Some(consumer) = self.consumer.read().await.as_ref().cloned() {
            if let Err(e) = consumer.stop().await {
                warn!("Failed to stop rule lifecycle consumer cleanly: {}", e);
            }
        }

        if let Some(handle) = self.task_handle.write().await.take() {
            let mut handle = handle;
            match timeout(Duration::from_secs(5), &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if e.is_cancelled() => {}
                Ok(Err(e)) => warn!("Rule lifecycle listener task ended unexpectedly: {}", e),
                Err(_) => {
                    warn!("Rule lifecycle listener did not stop in time; aborting task");
                    handle.abort();
                    let _ = handle.await;
                }
            }
        }

        self.consumer.write().await.take();

        info!("Rule lifecycle listener stopped");

        Ok(())
    }

    /// Handle a rule lifecycle message
    async fn handle_message(
        db: &PgPool,
        sensor_manager: &Arc<SensorManager>,
        pack_transport: &Arc<dyn attune_common::pack_transport::PackFileTransport>,
        trigger_id_cache: &Arc<RwLock<HashMap<String, (i64, Instant)>>>,
        envelope: MessageEnvelope<JsonValue>,
    ) -> Result<()> {
        match envelope.message_type {
            MessageType::RuleCreated => {
                let payload: RuleCreatedPayload = serde_json::from_value(envelope.payload)?;
                Self::handle_rule_created(db, sensor_manager, payload).await?;
            }
            MessageType::RuleEnabled => {
                let payload: RuleEnabledPayload = serde_json::from_value(envelope.payload)?;
                Self::handle_rule_enabled(db, sensor_manager, trigger_id_cache, payload).await?;
            }
            MessageType::RuleDisabled => {
                let payload: RuleDisabledPayload = serde_json::from_value(envelope.payload)?;
                Self::handle_rule_disabled(sensor_manager, db, trigger_id_cache, payload).await?;
            }
            MessageType::PackRegistered => {
                let payload: PackRegisteredPayload = serde_json::from_value(envelope.payload)?;
                Self::handle_pack_registered(sensor_manager, pack_transport, payload).await?;
            }
            MessageType::PackDeleted => {
                let payload: PackDeletedPayload = serde_json::from_value(envelope.payload)?;
                Self::handle_pack_deleted(sensor_manager, pack_transport, payload).await?;
            }
            MessageType::TriggerChanged => {
                let payload: TriggerChangedPayload = serde_json::from_value(envelope.payload)?;
                trigger_id_cache.write().await.remove(&payload.trigger_ref);
            }
            _ => {
                warn!("Unexpected message type: {:?}", envelope.message_type);
            }
        }

        Ok(())
    }

    /// Handle rule created event
    async fn handle_rule_created(
        _db: &PgPool,
        sensor_manager: &Arc<SensorManager>,
        payload: RuleCreatedPayload,
    ) -> Result<()> {
        info!(
            "Handling RuleCreated: rule={}, trigger={}",
            payload.rule_ref, payload.trigger_ref
        );

        // Notify sensor manager about rule change (may need to start sensors)
        if let Some(trigger_id) = payload.trigger_id {
            if let Err(e) = sensor_manager.handle_rule_change(trigger_id).await {
                error!(
                    "Failed to handle sensor lifecycle for trigger {}: {}",
                    trigger_id, e
                );
            }
        }

        Ok(())
    }

    /// Handle rule enabled event
    async fn handle_rule_enabled(
        db: &PgPool,
        sensor_manager: &Arc<SensorManager>,
        trigger_id_cache: &Arc<RwLock<HashMap<String, (i64, Instant)>>>,
        payload: RuleEnabledPayload,
    ) -> Result<()> {
        info!(
            "Handling RuleEnabled: rule={}, trigger={}",
            payload.rule_ref, payload.trigger_ref
        );

        let trigger_id =
            match Self::get_trigger_id_by_ref(db, trigger_id_cache, &payload.trigger_ref).await {
                Ok(Some(id)) => id,
                Ok(None) => {
                    warn!(
                        "Trigger '{}' not found for rule {}",
                        payload.trigger_ref, payload.rule_id
                    );
                    return Ok(());
                }
                Err(e) => {
                    error!(
                        "Failed to fetch trigger '{}' for rule {}: {}",
                        payload.trigger_ref, payload.rule_id, e
                    );
                    return Err(e);
                }
            };

        // Notify sensor manager about rule change (may need to start sensors)
        if let Err(e) = sensor_manager.handle_rule_change(trigger_id).await {
            error!(
                "Failed to handle sensor lifecycle for trigger {}: {}",
                trigger_id, e
            );
        }

        Ok(())
    }

    /// Handle rule disabled event
    async fn handle_rule_disabled(
        sensor_manager: &Arc<SensorManager>,
        db: &PgPool,
        trigger_id_cache: &Arc<RwLock<HashMap<String, (i64, Instant)>>>,
        payload: RuleDisabledPayload,
    ) -> Result<()> {
        info!(
            "Handling RuleDisabled: rule={}, trigger={}",
            payload.rule_ref, payload.trigger_ref
        );

        let trigger_id =
            match Self::get_trigger_id_by_ref(db, trigger_id_cache, &payload.trigger_ref).await {
                Ok(Some(id)) => id,
                Ok(None) => {
                    warn!(
                        "Trigger '{}' not found for rule {}",
                        payload.trigger_ref, payload.rule_id
                    );
                    return Ok(());
                }
                Err(e) => {
                    error!(
                        "Failed to fetch trigger '{}' for rule {}: {}",
                        payload.trigger_ref, payload.rule_id, e
                    );
                    return Err(e);
                }
            };

        // Notify sensor manager about rule change (may need to stop sensors)
        if let Err(e) = sensor_manager.handle_rule_change(trigger_id).await {
            error!(
                "Failed to handle sensor lifecycle for trigger {}: {}",
                trigger_id, e
            );
        }

        Ok(())
    }

    /// Helper function to get trigger_id for a trigger ref
    async fn get_trigger_id_by_ref(
        db: &PgPool,
        cache: &Arc<RwLock<HashMap<String, (i64, Instant)>>>,
        trigger_ref: &str,
    ) -> Result<Option<i64>> {
        if let Some((cached_id, cached_at)) = cache.read().await.get(trigger_ref).cloned() {
            if cached_at.elapsed() <= TRIGGER_ID_CACHE_TTL {
                return Ok(Some(cached_id));
            }
        }

        let trigger_id = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT id
            FROM trigger
            WHERE ref = $1
            "#,
        )
        .bind(trigger_ref)
        .fetch_optional(db)
        .await?;

        if let Some(id) = trigger_id {
            cache
                .write()
                .await
                .insert(trigger_ref.to_string(), (id, Instant::now()));
        }

        Ok(trigger_id)
    }

    /// Handle pack registered event — sync pack files if using API transport
    async fn handle_pack_registered(
        sensor_manager: &Arc<SensorManager>,
        pack_transport: &Arc<dyn attune_common::pack_transport::PackFileTransport>,
        payload: PackRegisteredPayload,
    ) -> Result<()> {
        info!(
            "Handling PackRegistered: pack={} (version {})",
            payload.pack_ref, payload.version,
        );

        if pack_transport.transport_mode() != "volume" {
            match pack_transport.sync_pack(&payload.pack_ref).await {
                Ok(()) => info!(
                    "Pack '{}' synced via {} transport",
                    payload.pack_ref,
                    pack_transport.transport_mode(),
                ),
                Err(e) => warn!("Failed to sync pack '{}': {}", payload.pack_ref, e),
            }
        }

        // Notify sensor manager that a pack changed — it may need to
        // start or restart sensors associated with this pack.
        if let Err(e) = sensor_manager.handle_pack_change(&payload.pack_ref).await {
            warn!(
                "Failed to handle sensor lifecycle for pack '{}': {}",
                payload.pack_ref, e,
            );
        }

        Ok(())
    }

    /// Handle pack deleted event — remove local pack files and stop affected sensors
    async fn handle_pack_deleted(
        sensor_manager: &Arc<SensorManager>,
        pack_transport: &Arc<dyn attune_common::pack_transport::PackFileTransport>,
        payload: PackDeletedPayload,
    ) -> Result<()> {
        info!("Handling PackDeleted: pack={}", payload.pack_ref);

        // Stop sensors that belong to this pack
        if let Err(e) = sensor_manager.handle_pack_deleted(&payload.pack_ref).await {
            warn!(
                "Failed to stop sensors for deleted pack '{}': {}",
                payload.pack_ref, e,
            );
        }

        // Remove local pack files
        match pack_transport.remove_pack(&payload.pack_ref).await {
            Ok(()) => info!("Pack '{}' removed locally", payload.pack_ref),
            Err(e) => warn!(
                "Failed to remove pack '{}' locally: {}",
                payload.pack_ref, e,
            ),
        }

        Ok(())
    }
}
