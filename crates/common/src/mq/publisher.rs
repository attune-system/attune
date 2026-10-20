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

/// Opt-in attempt guard failure. The proof owner's error is returned intact but
/// is deliberately omitted from this transport error's Display and Debug.
pub enum GuardedPublicationError<E> {
    Guard(E),
    Transport(MqError),
}

impl<E> std::fmt::Debug for GuardedPublicationError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Guard(_) => f.write_str("GuardedPublicationError::Guard"),
            Self::Transport(error) => f.debug_tuple("Transport").field(error).finish(),
        }
    }
}

impl<E> std::fmt::Display for GuardedPublicationError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Guard(_) => f.write_str("Publication freshness guard rejected the attempt"),
            Self::Transport(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

impl<E> std::error::Error for GuardedPublicationError<E> {}

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

    /// Opt-in guards borrow their real proof owners. A deadline only limits
    /// waiting and is never accepted in place of the callback.
    ///
    /// GUARANTEE LIMIT: the callback runs after channel acquisition and before
    /// every basic_publish attempt, including the recovery attempt. Lapin 4.11
    /// queues frames before awaiting its driver. This method cannot retract
    /// frames after enqueue or guard the later driver/socket handoff. Therefore
    /// this is NOT certification of broker submission before an input deadline.
    /// A timeout after enqueue has unknown submission outcome, never a safe
    /// reason to inline retry. Ordinary publisher paths remain unchanged.
    pub async fn publish_envelope_guarded<T, F, E>(
        &self,
        envelope: &MessageEnvelope<T>,
        approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
        fresh: &F,
    ) -> Result<(), GuardedPublicationError<E>>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de>,
        F: Fn() -> Result<(), E> + Sync,
    {
        let payload = envelope.to_bytes().map_err(|_| {
            GuardedPublicationError::Transport(MqError::Serialization(
                "Failed to serialize guarded envelope".into(),
            ))
        })?;
        let exchange = envelope.message_type.exchange();
        let routing_key = envelope.message_type.routing_key();
        let properties = BasicProperties::default()
            .with_delivery_mode(DeliveryMode::Persistent as u8)
            .with_message_id(envelope.message_id.to_string().into())
            .with_correlation_id(envelope.correlation_id.to_string().into())
            .with_timestamp(envelope.timestamp.timestamp() as u64)
            .with_content_type("application/json".into());
        for attempt in 0..2 {
            let channel = self
                .guarded_wait(approval_deadline, fresh, async {
                    Ok(self.channel.read().await.clone())
                })
                .await?;
            // No publisher-side await between this callback and the first poll
            // of basic_publish. The driver's later handoff is outside our API.
            Self::check_guarded_attempt(approval_deadline, fresh)?;
            let published = self
                .guarded_wait(approval_deadline, fresh, async {
                    channel
                        .basic_publish(
                            exchange.clone().into(),
                            routing_key.clone().into(),
                            BasicPublishOptions::default(),
                            &payload,
                            properties.clone(),
                        )
                        .await
                        .map_err(|error| {
                            MqError::Publish(format!("Failed to publish message: {error}"))
                        })
                })
                .await;
            let result = match published {
                Ok(confirmation) => {
                    // Ack waiting uses the configured transport bound. It is not
                    // a new attempt. Preserve a local proof postcheck below:
                    // successful Ack cannot waive expiry or undo an earlier effect.
                    tokio::time::timeout(
                        std::time::Duration::from_secs(self.config.timeout_secs),
                        self.finish_publication(confirmation),
                    )
                    .await
                    .map_err(|_| {
                        GuardedPublicationError::Transport(MqError::Timeout(
                            "Guarded publication acknowledgement is unknown".into(),
                        ))
                    })?
                    .map(|_| ())
                    .map_err(GuardedPublicationError::Transport)
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => return fresh().map_err(GuardedPublicationError::Guard),
                Err(GuardedPublicationError::Transport(error))
                    if attempt == 0 && Self::is_recoverable_publish_error(&error) =>
                {
                    // Bound reconnection AND channel-write lock. The next loop
                    // reacquires the channel, then calls the guard again.
                    self.guarded_wait(approval_deadline, fresh, self.reset_channel())
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the second attempt always returns")
    }

    fn check_guarded_attempt<F, E>(
        deadline: Option<chrono::DateTime<chrono::Utc>>,
        fresh: &F,
    ) -> Result<(), GuardedPublicationError<E>>
    where
        F: Fn() -> Result<(), E>,
    {
        fresh().map_err(GuardedPublicationError::Guard)?;
        if deadline.is_some_and(|deadline| chrono::Utc::now() >= deadline) {
            // A public bound is not authority. Do not misclassify this as an
            // authorization error if the actual proof callback still succeeds.
            return Err(GuardedPublicationError::Transport(MqError::Timeout(
                "Guarded publication wait deadline elapsed".into(),
            )));
        }
        Ok(())
    }

    async fn guarded_wait<F, E, W, R>(
        &self,
        deadline: Option<chrono::DateTime<chrono::Utc>>,
        fresh: &F,
        wait: W,
    ) -> Result<R, GuardedPublicationError<E>>
    where
        F: Fn() -> Result<(), E>,
        W: std::future::Future<Output = MqResult<R>>,
    {
        Self::check_guarded_attempt(deadline, fresh)?;
        let mut bound = std::time::Duration::from_secs(self.config.timeout_secs);
        if let Some(deadline) = deadline {
            let remaining = (deadline - chrono::Utc::now())
                .to_std()
                .unwrap_or(std::time::Duration::ZERO);
            bound = bound.min(remaining);
        }
        let mut wait = std::pin::pin!(wait);
        let guarded = std::future::poll_fn(|cx| {
            // Tokio timeout can poll its inner future before observing its timer.
            // Check the absolute bound again on EVERY inner poll, including the
            // first enqueue poll, rather than relying on timer selection order.
            if let Err(error) = Self::check_guarded_attempt(deadline, fresh) {
                return std::task::Poll::Ready(Err(error));
            }
            std::future::Future::poll(wait.as_mut(), cx)
                .map(|result| result.map_err(GuardedPublicationError::Transport))
        });
        match tokio::time::timeout(bound, guarded).await {
            Ok(result) => result,
            Err(_) => {
                // Only the real owner can classify its expiry as authorization
                // denial. A transport budget alone cannot do so. An observed MQ
                // error above is returned as-is, not overwritten by a recheck.
                fresh().map_err(GuardedPublicationError::Guard)?;
                Err(GuardedPublicationError::Transport(MqError::Timeout(
                    "Guarded publication wait exceeded; submission outcome may be unknown".into(),
                )))
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

    #[test]
    fn guarded_error_diagnostics_never_format_the_proof_owner_error() {
        let error = GuardedPublicationError::Guard("sensitive proof body sentinel");
        assert!(!error.to_string().contains("sentinel"));
        assert!(!format!("{error:?}").contains("sentinel"));
        match error {
            GuardedPublicationError::Guard(original) => {
                assert_eq!(original, "sensitive proof body sentinel");
            }
            _ => panic!("callback error must remain distinguishable from transport errors"),
        }
    }

    #[test]
    fn a_public_wait_date_does_not_establish_or_reclassify_authority() {
        let elapsed = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
        let still_valid_owner = || Ok::<(), &'static str>(());
        assert!(matches!(
            Publisher::check_guarded_attempt(elapsed, &still_valid_owner),
            Err(GuardedPublicationError::Transport(MqError::Timeout(_)))
        ));
        let expired_owner = || Err::<(), _>("actual owner expired");
        assert!(matches!(
            Publisher::check_guarded_attempt(None, &expired_owner),
            Err(GuardedPublicationError::Guard("actual owner expired"))
        ));
        // These are transport error-classification checks only. Neither closure
        // constructs or claims an accepted input receipt or external approval.
    }

    // Integration tests would require a running RabbitMQ instance
    // and should be in a separate integration test file
}
