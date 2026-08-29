//! Message Consumer
//!
//! This module provides functionality for consuming messages from RabbitMQ queues.
//! It supports:
//! - Asynchronous message consumption
//! - Manual and automatic acknowledgments
//! - Message deserialization
//! - Error handling and retries
//! - Graceful shutdown

use futures::StreamExt;
use lapin::{
    options::{
        BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,
    },
    types::FieldTable,
    Channel, Consumer as LapinConsumer,
};
use rand::Rng;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    time::Duration,
};
use tracing::{debug, error, info, warn};

use super::{
    connection::is_expected_shutdown_error,
    error::{MqError, MqResult},
    messages::MessageEnvelope,
    Connection,
};

// Re-export for convenience
pub use super::config::ConsumerConfig;

/// Message consumer for receiving messages from RabbitMQ
pub struct Consumer {
    /// RabbitMQ channel
    channel: Channel,
    /// Shared connection used to replace failed consumer sessions.
    connection: Connection,
    /// Stops recovery attempts during service shutdown.
    stopped: Arc<AtomicBool>,
    stop_notify: Arc<tokio::sync::Notify>,
    /// Consumer configuration
    config: ConsumerConfig,
}

impl Consumer {
    /// Create a new consumer from a connection
    pub async fn new(connection: &Connection, config: ConsumerConfig) -> MqResult<Self> {
        let channel = connection.create_channel().await?;

        // Set prefetch count (QoS)
        channel
            .basic_qos(config.prefetch_count, BasicQosOptions::default())
            .await
            .map_err(|e| MqError::Channel(format!("Failed to set QoS: {}", e)))?;

        debug!(
            "Consumer created for queue '{}' with prefetch count {}",
            config.queue, config.prefetch_count
        );

        Ok(Self {
            channel,
            connection: connection.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(tokio::sync::Notify::new()),
            config,
        })
    }

    /// Start consuming messages from the queue
    pub async fn start(&self) -> MqResult<LapinConsumer> {
        self.start_on(&self.channel).await
    }

    async fn start_on(&self, channel: &Channel) -> MqResult<LapinConsumer> {
        info!("Starting consumer for queue '{}'", self.config.queue);

        let consumer = channel
            .basic_consume(
                self.config.queue.as_str().into(),
                self.config.tag.as_str().into(),
                BasicConsumeOptions {
                    no_ack: self.config.auto_ack,
                    exclusive: self.config.exclusive,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| {
                MqError::Consume(format!(
                    "Failed to start consuming from queue '{}': {}",
                    self.config.queue, e
                ))
            })?;

        info!(
            "Consumer started for queue '{}' with tag '{}'",
            self.config.queue, self.config.tag
        );

        Ok(consumer)
    }

    /// Consume durable-queue messages, rebuilding the channel after a broker
    /// disconnect, stream termination, or acknowledgement failure.
    pub async fn consume_with_handler<T, F, Fut>(&self, mut handler: F) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = MqResult<()>> + Send,
    {
        validate_recoverable_consumer_config(&self.config)?;

        let mut channel = self.channel.clone();
        let mut backoff = Duration::from_millis(250);
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            match self.consume_session(&channel, &mut handler).await {
                Ok(()) => {
                    warn!(queue = %self.config.queue, "Consumer delivery stream ended; recreating session")
                }
                Err(error) => {
                    warn!(queue = %self.config.queue, %error, "Consumer session failed; recreating session")
                }
            }

            if self
                .wait_for_recovery_backoff(jittered_recovery_delay(backoff))
                .await
            {
                return Ok(());
            }
            backoff = next_recovery_backoff(backoff);
            loop {
                if self.stopped.load(Ordering::Acquire) {
                    return Ok(());
                }
                match self.connection.create_channel().await {
                    Ok(new_channel) => match new_channel
                        .basic_qos(self.config.prefetch_count, BasicQosOptions::default())
                        .await
                    {
                        Ok(()) => {
                            channel = new_channel;
                            backoff = Duration::from_millis(250);
                            break;
                        }
                        Err(error) => {
                            warn!(queue = %self.config.queue, %error, "Failed to configure recovered consumer channel")
                        }
                    },
                    Err(error) => {
                        warn!(queue = %self.config.queue, %error, "Failed to create recovered consumer channel")
                    }
                }
                if self
                    .wait_for_recovery_backoff(jittered_recovery_delay(backoff))
                    .await
                {
                    return Ok(());
                }
                backoff = next_recovery_backoff(backoff);
            }
        }
    }

    async fn wait_for_recovery_backoff(&self, duration: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(duration) => false,
            _ = self.stop_notify.notified() => true,
        }
    }

    /// Consume one AMQP session. Server-named ephemeral queues use this from
    /// an outer topology-rebuilding loop because their queue name changes on
    /// reconnection.
    pub async fn consume_once_with_handler<T, F, Fut>(&self, mut handler: F) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = MqResult<()>> + Send,
    {
        self.consume_session(&self.channel, &mut handler).await
    }

    async fn consume_session<T, F, Fut>(&self, channel: &Channel, handler: &mut F) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = MqResult<()>> + Send,
    {
        let mut consumer = self.start_on(channel).await?;

        info!("Consuming messages from queue '{}'", self.config.queue);

        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    let delivery_tag = delivery.delivery_tag;

                    debug!(
                        "Received message with delivery tag {} from queue '{}'",
                        delivery_tag, self.config.queue
                    );

                    // Deserialize message envelope
                    let envelope = match MessageEnvelope::<T>::from_bytes(&delivery.data) {
                        Ok(env) => env,
                        Err(e) => {
                            error!("Failed to deserialize message: {}. Rejecting message.", e);

                            if !self.config.auto_ack {
                                // Reject message without requeue (send to DLQ)
                                if let Err(nack_err) = channel
                                    .basic_nack(
                                        delivery_tag,
                                        BasicNackOptions {
                                            requeue: false,
                                            multiple: false,
                                        },
                                    )
                                    .await
                                {
                                    return Err(MqError::Channel(format!(
                                        "Failed to nack malformed message: {nack_err}"
                                    )));
                                }
                            }
                            continue;
                        }
                    };

                    debug!(
                        "Processing message {} of type {:?}",
                        envelope.message_id, envelope.message_type
                    );

                    // Call handler
                    match handler(envelope.clone()).await {
                        Ok(()) => {
                            debug!("Message {} processed successfully", envelope.message_id);

                            if !self.config.auto_ack {
                                // Acknowledge message
                                if let Err(e) = channel
                                    .basic_ack(delivery_tag, BasicAckOptions::default())
                                    .await
                                {
                                    return Err(MqError::Channel(format!(
                                        "Failed to ack message: {e}"
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            error!("Handler failed for message {}: {}", envelope.message_id, e);

                            if !self.config.auto_ack {
                                // Reject message - will be requeued or sent to DLQ
                                let requeue = e.is_retriable();

                                warn!(
                                    "Rejecting message {} (requeue: {})",
                                    envelope.message_id, requeue
                                );

                                if let Err(nack_err) = channel
                                    .basic_nack(
                                        delivery_tag,
                                        BasicNackOptions {
                                            requeue,
                                            multiple: false,
                                        },
                                    )
                                    .await
                                {
                                    return Err(MqError::Channel(format!(
                                        "Failed to nack failed message: {nack_err}"
                                    )));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(MqError::Consume(format!("Error receiving message: {e}")));
                }
            }
        }

        Ok(())
    }

    /// Get the underlying channel
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// Get the queue name
    pub fn queue(&self) -> &str {
        &self.config.queue
    }

    /// Stop consuming and close the underlying channel.
    pub async fn stop(&self) -> MqResult<()> {
        info!("Stopping consumer for queue '{}'", self.config.queue);
        self.stopped.store(true, Ordering::Release);
        self.stop_notify.notify_waiters();

        let status = self.channel.status();
        if status.connected() {
            match self
                .channel
                .basic_cancel(
                    self.config.tag.as_str().into(),
                    BasicCancelOptions::default(),
                )
                .await
            {
                Ok(()) => debug!(
                    "Consumer '{}' cancelled for queue '{}'",
                    self.config.tag, self.config.queue
                ),
                Err(e) if is_expected_shutdown_error(&e) => {
                    debug!(
                        "Consumer '{}' was already shutting down for queue '{}'",
                        self.config.tag, self.config.queue
                    );
                }
                Err(e) => {
                    return Err(MqError::Consume(format!(
                        "Failed to cancel consumer '{}' on queue '{}': {}",
                        self.config.tag, self.config.queue, e
                    )));
                }
            }
        }

        let status = self.channel.status();
        if status.connected() {
            match self.channel.close(200, "Normal shutdown".into()).await {
                Ok(()) => debug!("Consumer channel closed for queue '{}'", self.config.queue),
                Err(e) if is_expected_shutdown_error(&e) => {
                    debug!(
                        "Consumer channel for queue '{}' was already shutting down",
                        self.config.queue
                    );
                }
                Err(e) => {
                    return Err(MqError::Channel(format!(
                        "Failed to close consumer channel for queue '{}': {}",
                        self.config.queue, e
                    )));
                }
            }
        }

        info!("Consumer stopped for queue '{}'", self.config.queue);
        Ok(())
    }
}

fn validate_recoverable_consumer_config(config: &ConsumerConfig) -> MqResult<()> {
    if config.exclusive {
        return Err(MqError::Config(
            "exclusive queues must rebuild their topology and use consume_once_with_handler"
                .to_string(),
        ));
    }
    Ok(())
}

fn next_recovery_backoff(current: Duration) -> Duration {
    (current * 2).min(Duration::from_secs(30))
}

fn jittered_recovery_delay(cap: Duration) -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(0..=cap.as_millis() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consumer_config() {
        let config = ConsumerConfig {
            queue: "test.queue".to_string(),
            tag: "test-consumer".to_string(),
            prefetch_count: 10,
            auto_ack: false,
            exclusive: false,
        };

        assert_eq!(config.queue, "test.queue");
        assert_eq!(config.tag, "test-consumer");
        assert_eq!(config.prefetch_count, 10);
        assert!(!config.auto_ack);
        assert!(!config.exclusive);
    }

    #[test]
    fn recovery_backoff_doubles_and_caps() {
        assert_eq!(
            next_recovery_backoff(Duration::from_millis(250)),
            Duration::from_millis(500)
        );
        assert_eq!(
            next_recovery_backoff(Duration::from_secs(20)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_recovery_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn recovery_backoff_jitter_stays_within_cap() {
        let cap = Duration::from_millis(250);
        for _ in 0..100 {
            assert!(jittered_recovery_delay(cap) <= cap);
        }
    }

    #[test]
    fn recoverable_consumer_rejects_exclusive_queue() {
        let config = ConsumerConfig {
            queue: "amq.gen-test".to_string(),
            tag: "test-consumer".to_string(),
            prefetch_count: 10,
            auto_ack: false,
            exclusive: true,
        };

        let error = validate_recoverable_consumer_config(&config).unwrap_err();
        assert!(error
            .to_string()
            .contains("exclusive queues must rebuild their topology"));
    }

    // Integration tests would require a running RabbitMQ instance
    // and should be in a separate integration test file
}
