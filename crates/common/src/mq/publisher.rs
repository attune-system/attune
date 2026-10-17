//! Message Publisher
//!
//! This module provides functionality for publishing messages to RabbitMQ exchanges.
//! It supports:
//! - Asynchronous message publishing
//! - Message confirmation (publisher confirms)
//! - Automatic routing based on message type
//! - Error handling and retries

use lapin::{
    options::{BasicPublishOptions, ConfirmSelectOptions},
    BasicProperties, Channel, Confirmation, PublisherConfirm,
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::{
    error::{MqError, MqResult},
    messages::MessageEnvelope,
    Connection, DeliveryMode,
};

// Re-export for convenience
pub use super::config::PublisherConfig;

/// An unconfirmed submission is not a broker acknowledgement. Callers retain
/// the existing configured policy; enabling confirms is never implicit here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishConfirmation {
    Acknowledged,
    Unconfirmed,
}

/// Classify the actual Lapin result without formatting returned message bodies.
/// Nack and missing requested confirmation are publish failures, not connection
/// failures that should trigger another automatic publication.
pub fn classify_confirmation(
    requested: bool,
    confirmation: Confirmation,
) -> MqResult<PublishConfirmation> {
    match confirmation {
        Confirmation::Ack(_) => Ok(PublishConfirmation::Acknowledged),
        Confirmation::Nack(_) => Err(MqError::Publish(
            "Broker negatively acknowledged publication".into(),
        )),
        Confirmation::NotRequested if requested => Err(MqError::Publish(
            "Publisher confirmation was requested but was not supplied".into(),
        )),
        Confirmation::NotRequested => Ok(PublishConfirmation::Unconfirmed),
    }
}

/// Message publisher for sending messages to RabbitMQ
pub struct Publisher {
    /// RabbitMQ channel (recreated on recoverable publish failures)
    channel: RwLock<Channel>,
    /// Underlying connection used to recreate channels
    connection: Connection,
    /// Publisher configuration
    config: PublisherConfig,
}

impl Publisher {
    /// Create a new publisher from a connection
    pub async fn new(connection: &Connection, config: PublisherConfig) -> MqResult<Self> {
        let channel = Self::create_publish_channel(connection, &config).await?;

        Ok(Self {
            channel: RwLock::new(channel),
            connection: connection.clone(),
            config,
        })
    }

    async fn create_publish_channel(
        connection: &Connection,
        config: &PublisherConfig,
    ) -> MqResult<Channel> {
        let channel = connection.create_channel().await?;

        // Enable publisher confirms if configured
        if config.confirm_publish {
            channel
                .confirm_select(ConfirmSelectOptions::default())
                .await
                .map_err(|e| MqError::Channel(format!("Failed to enable confirms: {}", e)))?;
            debug!("Publisher confirms enabled");
        }

        Ok(channel)
    }

    fn is_recoverable_publish_error(error: &MqError) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("invalid channel state")
            || message.contains("invalid connection state")
            || message.contains("channel closed")
            || message.contains("connection closed")
            || message.contains("connection reset")
            || message.contains("broken pipe")
    }

    async fn reset_channel(&self) -> MqResult<()> {
        let new_channel = Self::create_publish_channel(&self.connection, &self.config).await?;
        let mut guard = self.channel.write().await;
        *guard = new_channel;
        Ok(())
    }

    async fn finish_publication(
        &self,
        confirmation: PublisherConfirm,
    ) -> MqResult<PublishConfirmation> {
        if !self.config.confirm_publish {
            // Preserve the explicit disabled policy, including the old drop
            // behavior if another user of channel() enabled confirms itself.
            drop(confirmation);
            return classify_confirmation(false, Confirmation::NotRequested);
        }
        let confirmation = confirmation
            .await
            .map_err(|e| MqError::Publish(format!("Message not confirmed: {}", e)))?;
        classify_confirmation(true, confirmation)
    }

    async fn publish_envelope_once<T>(
        &self,
        envelope: &MessageEnvelope<T>,
        exchange: &str,
        routing_key: &str,
    ) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let payload = envelope
            .to_bytes()
            .map_err(|e| MqError::Serialization(format!("Failed to serialize envelope: {}", e)))?;

        debug!(
            "Publishing message {} to exchange '{}' with routing key '{}'",
            envelope.message_id, exchange, routing_key
        );

        let properties = BasicProperties::default()
            .with_delivery_mode(DeliveryMode::Persistent as u8)
            .with_message_id(envelope.message_id.to_string().into())
            .with_correlation_id(envelope.correlation_id.to_string().into())
            .with_timestamp(envelope.timestamp.timestamp() as u64)
            .with_content_type("application/json".into());

        let channel = { self.channel.read().await.clone() };
        let confirmation = channel
            .basic_publish(
                exchange.into(),
                routing_key.into(),
                BasicPublishOptions::default(),
                &payload,
                properties,
            )
            .await
            .map_err(|e| MqError::Publish(format!("Failed to publish message: {}", e)))?;

        match self.finish_publication(confirmation).await? {
            PublishConfirmation::Acknowledged => {
                info!(
                    "Message {} acknowledged by broker on '{}'",
                    envelope.message_id, exchange
                );
            }
            PublishConfirmation::Unconfirmed => {
                info!(
                    "Message {} submitted to '{}' without publisher confirmation",
                    envelope.message_id, exchange
                );
            }
        }

        Ok(())
    }

    /// Publish a message envelope to its designated exchange
    /// With confirms enabled, success requires an actual broker Ack. With
    /// confirms explicitly disabled, success means unconfirmed submission only.
    pub async fn publish_envelope<T>(&self, envelope: &MessageEnvelope<T>) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let exchange = envelope.message_type.exchange();
        let routing_key = envelope.message_type.routing_key();

        self.publish_envelope_with_routing(envelope, &exchange, &routing_key)
            .await
    }

    /// Publish a message envelope with explicit exchange and routing key
    pub async fn publish_envelope_with_routing<T>(
        &self,
        envelope: &MessageEnvelope<T>,
        exchange: &str,
        routing_key: &str,
    ) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        match self
            .publish_envelope_once(envelope, exchange, routing_key)
            .await
        {
            Ok(()) => Ok(()),
            Err(first_err) => {
                if !Self::is_recoverable_publish_error(&first_err) {
                    return Err(first_err);
                }

                warn!(
                    "Recoverable publish error detected ({}). Recreating channel and retrying once",
                    first_err
                );

                self.reset_channel().await?;
                self.publish_envelope_once(envelope, exchange, routing_key)
                    .await
            }
        }
    }

    /// Publish a raw message with custom properties under the same configured
    /// confirmation policy as envelopes. Disabled confirms mean submission only.
    pub async fn publish_raw(
        &self,
        exchange: &str,
        routing_key: &str,
        payload: &[u8],
        properties: BasicProperties,
    ) -> MqResult<()> {
        debug!(
            "Publishing raw message to exchange '{}' with routing key '{}'",
            exchange, routing_key
        );

        let channel = { self.channel.read().await.clone() };
        match channel
            .basic_publish(
                exchange.into(),
                routing_key.into(),
                BasicPublishOptions::default(),
                payload,
                properties.clone(),
            )
            .await
        {
            Ok(confirmation) => self.finish_publication(confirmation).await.map(|_| ()),
            Err(e) => {
                let first_err = MqError::Publish(format!("Failed to publish raw message: {}", e));
                if !Self::is_recoverable_publish_error(&first_err) {
                    return Err(first_err);
                }
                warn!(
                    "Recoverable raw publish error detected ({}). Recreating channel and retrying once",
                    first_err
                );
                self.reset_channel().await?;
                let retry_channel = { self.channel.read().await.clone() };
                let confirmation = retry_channel
                    .basic_publish(
                        exchange.into(),
                        routing_key.into(),
                        BasicPublishOptions::default(),
                        payload,
                        properties,
                    )
                    .await
                    .map_err(|e| {
                        MqError::Publish(format!("Failed to publish raw message: {}", e))
                    })?;
                self.finish_publication(confirmation).await.map(|_| ())
            }
        }
    }

    /// Get the underlying channel
    pub async fn channel(&self) -> Channel {
        self.channel.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[allow(dead_code)]
    struct TestPayload {
        data: String,
    }

    #[test]
    fn test_publisher_config_defaults() {
        let config = PublisherConfig {
            confirm_publish: true,
            timeout_secs: 5,
            exchange: "test.exchange".to_string(),
        };

        assert!(config.confirm_publish);
        assert_eq!(config.timeout_secs, 5);
    }

    #[test]
    fn actual_broker_ack_is_the_confirmed_success_case() {
        assert_eq!(
            classify_confirmation(true, Confirmation::Ack(None)).unwrap(),
            PublishConfirmation::Acknowledged
        );
    }

    #[test]
    fn actual_broker_nack_is_a_publish_error_not_an_inline_republish_trigger() {
        for requested in [true, false] {
            let error = classify_confirmation(requested, Confirmation::Nack(None)).unwrap_err();
            assert!(matches!(&error, MqError::Publish(_)));
            assert_eq!(
                error.to_string(),
                "Publishing error: Broker negatively acknowledged publication"
            );
            assert!(error.is_retriable());
            assert!(!Publisher::is_recoverable_publish_error(&error));
        }
    }

    #[test]
    fn missing_requested_confirmation_is_not_success_or_an_inline_retry() {
        let error = classify_confirmation(true, Confirmation::NotRequested).unwrap_err();
        assert!(matches!(&error, MqError::Publish(_)));
        assert_eq!(
            error.to_string(),
            "Publishing error: Publisher confirmation was requested but was not supplied"
        );
        assert!(!Publisher::is_recoverable_publish_error(&error));
    }

    #[test]
    fn explicitly_disabled_confirms_mean_unconfirmed_submission_not_ack() {
        assert_eq!(
            classify_confirmation(false, Confirmation::NotRequested).unwrap(),
            PublishConfirmation::Unconfirmed
        );
    }

    #[test]
    fn classifier_does_not_change_default_or_explicit_confirmation_policy() {
        let default: PublisherConfig =
            serde_json::from_value(serde_json::json!({"exchange":"test.exchange"})).unwrap();
        let disabled: PublisherConfig = serde_json::from_value(
            serde_json::json!({"exchange":"test.exchange","confirm_publish":false}),
        )
        .unwrap();
        assert!(default.confirm_publish);
        assert!(!disabled.confirm_publish);
        assert_eq!(
            classify_confirmation(disabled.confirm_publish, Confirmation::NotRequested).unwrap(),
            PublishConfirmation::Unconfirmed
        );
        assert!(
            classify_confirmation(default.confirm_publish, Confirmation::NotRequested).is_err()
        );
    }

    // Integration tests would require a running RabbitMQ instance
    // and should be in a separate integration test file
}
