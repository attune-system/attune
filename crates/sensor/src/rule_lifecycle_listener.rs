//! Best-effort lifecycle prompts for managed sensor reconciliation.

use anyhow::Result;
use attune_common::mq::{
    Connection, Consumer, MessageEnvelope, MessageType, MqError, PackDeletedPayload,
};
use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

use crate::sensor_manager::SensorManager;

const LIFECYCLE_ROUTING_KEYS: &[&str] = &[
    "rule.created",
    "rule.enabled",
    "rule.disabled",
    "rule.deleted",
    "pack.registered",
    "pack.deleted",
    "metadata.trigger.changed",
];
const CONSUMER_RECOVERY_DELAY: Duration = Duration::from_secs(2);

pub struct RuleLifecycleListener {
    connection: Connection,
    sensor_manager: Arc<SensorManager>,
    consumer: Arc<RwLock<Option<Arc<Consumer>>>>,
    task_handle: RwLock<Option<JoinHandle<()>>>,
    stopping: Arc<AtomicBool>,
}

impl RuleLifecycleListener {
    pub fn new(connection: Connection, sensor_manager: Arc<SensorManager>) -> Self {
        Self {
            connection,
            sensor_manager,
            consumer: Arc::new(RwLock::new(None)),
            task_handle: RwLock::new(None),
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting replica-local sensor lifecycle listener");
        self.stopping.store(false, Ordering::Release);

        let connection = self.connection.clone();
        let sensor_manager = self.sensor_manager.clone();
        let current_consumer = self.consumer.clone();
        let stopping = self.stopping.clone();
        let handle = tokio::spawn(async move {
            while !stopping.load(Ordering::Acquire) {
                match connection
                    .create_ephemeral_topic_consumer(
                        "attune.events",
                        LIFECYCLE_ROUTING_KEYS,
                        "sensor.lifecycle.prompts",
                        10,
                    )
                    .await
                {
                    Ok(consumer) => {
                        let consumer = Arc::new(consumer);
                        *current_consumer.write().await = Some(consumer.clone());
                        let manager = sensor_manager.clone();
                        let result = consumer
                            .consume_once_with_handler(move |envelope| {
                                let manager = manager.clone();
                                async move { Self::handle_prompt(&manager, envelope).await }
                            })
                            .await;
                        current_consumer.write().await.take();
                        if let Err(error) = result {
                            warn!("Sensor lifecycle prompt consumer ended: {}", error);
                        }
                    }
                    Err(error) => {
                        warn!("Failed to create sensor lifecycle prompt queue: {}", error)
                    }
                }

                if !stopping.load(Ordering::Acquire) {
                    tokio::time::sleep(CONSUMER_RECOVERY_DELAY).await;
                }
            }
            info!("Sensor lifecycle listener stopped");
        });
        *self.task_handle.write().await = Some(handle);
        Ok(())
    }

    async fn handle_prompt(
        sensor_manager: &SensorManager,
        envelope: MessageEnvelope<JsonValue>,
    ) -> Result<(), MqError> {
        if envelope.message_type == MessageType::PackDeleted {
            let payload: PackDeletedPayload =
                serde_json::from_value(envelope.payload).map_err(|error| {
                    MqError::Deserialization(format!(
                        "Failed to parse PackDeleted payload: {error}"
                    ))
                })?;
            sensor_manager
                .handle_pack_deleted(
                    payload.pack_id,
                    &payload.pack_ref,
                    &payload.runtime_environment_paths,
                    &payload.release_digests,
                )
                .await
                .map_err(|error| MqError::Cleanup(error.to_string()))?;
        }

        // PostgreSQL contains desired state. The message only shortens the wait
        // until this replica's next authoritative reconciliation.
        sensor_manager.prompt_lifecycle_reconciliation();
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        info!("Stopping sensor lifecycle listener");
        self.stopping.store(true, Ordering::Release);
        if let Some(consumer) = self.consumer.read().await.as_ref().cloned() {
            if let Err(error) = consumer.stop().await {
                warn!(
                    "Failed to stop lifecycle prompt consumer cleanly: {}",
                    error
                );
            }
        }
        if let Some(mut handle) = self.task_handle.write().await.take() {
            match timeout(Duration::from_secs(5), &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if error.is_cancelled() => {}
                Ok(Err(error)) => warn!("Lifecycle listener task failed: {}", error),
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                }
            }
        }
        self.consumer.write().await.take();
        Ok(())
    }
}
