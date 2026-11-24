//! Message Consumer
//!
//! This module provides functionality for consuming messages from RabbitMQ queues.
//! It supports:
//! - Asynchronous message consumption
//! - Manual and automatic acknowledgments
//! - Message deserialization
//! - Error handling and retries
//! - Graceful shutdown

use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use lapin::{
    options::{
        BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,
    },
    types::FieldTable,
    Channel, Consumer as LapinConsumer,
};
use rand::Rng;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    hash::Hash,
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
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
    stopped: CancellationToken,
    /// Consumer configuration
    config: ConsumerConfig,
    subscription: Option<SubscriptionObservation>,
}

const SUBSCRIPTION_POLL_INTERVAL: Duration = Duration::from_secs(4);
const SUBSCRIPTION_FRESHNESS: Duration = Duration::from_secs(12);

type KeyedDelivery<K, T> = (Option<K>, (u64, MessageEnvelope<T>));

/// Local liveness of an actual broker-acknowledged consumer, including its
/// recovered session. Creating a channel or starting a task cannot set it.
#[derive(Clone, Default)]
pub struct SubscriptionObservation(Arc<Mutex<Option<ActiveSubscription>>>);

struct ActiveSubscription {
    channel: Channel,
    session: uuid::Uuid,
    progress: Instant,
}

impl SubscriptionObservation {
    pub fn is_current(&self) -> bool {
        self.0.lock().is_ok_and(|active| {
            active.as_ref().is_some_and(|session| {
                session.channel.status().connected()
                    && session.progress.elapsed() < SUBSCRIPTION_FRESHNESS
            })
        })
    }
}

struct ObservedSession<'a> {
    observation: &'a SubscriptionObservation,
    id: uuid::Uuid,
}

impl ObservedSession<'_> {
    fn progressed(&self) {
        if let Ok(mut state) = self.observation.0.lock() {
            if let Some(active) = state.as_mut().filter(|active| active.session == self.id) {
                active.progress = Instant::now();
            }
        }
    }
}

impl Drop for ObservedSession<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.observation.0.lock() {
            if active
                .as_ref()
                .is_some_and(|session| session.session == self.id)
            {
                *active = None;
            }
        }
    }
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
            stopped: CancellationToken::new(),
            config,
            subscription: None,
        })
    }

    /// Opt in before starting the loop. Unobserved consumers retain their
    /// existing event-driven idle behavior.
    pub fn with_subscription_observation(mut self) -> (Self, SubscriptionObservation) {
        let observation = SubscriptionObservation::default();
        self.subscription = Some(observation.clone());
        (self, observation)
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
    pub async fn consume_with_handler<T, F, Fut>(&self, handler: F) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send + 'static,
        Fut: Future<Output = MqResult<()>> + Send + 'static,
    {
        self.consume_recovering(handler, |consumer, channel, handler| {
            Box::pin(consumer.consume_session(channel, handler))
        })
        .await
    }

    /// Process different keys concurrently while preserving broker delivery
    /// order within each key on this consumer. An unkeyed delivery is a
    /// barrier. Classification runs in delivery order before taking a slot.
    /// Manual acknowledgement and nonzero prefetch bound the entire window.
    pub async fn consume_with_keyed_handler<T, K, C, Classification, F, Fut>(
        &self,
        parallelism: NonZeroUsize,
        classify: C,
        handler: F,
    ) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        K: Copy + Eq + Hash + Send + 'static,
        C: FnMut(&MessageEnvelope<T>) -> Classification + Send + 'static,
        Classification: Future<Output = MqResult<Option<K>>> + Send + 'static,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send + 'static,
        Fut: Future<Output = MqResult<()>> + Send + 'static,
    {
        if self.config.auto_ack || usize::from(self.config.prefetch_count) < parallelism.get() {
            return Err(MqError::Config(
                "Keyed consumers require manual acknowledgements and prefetch at least equal to parallelism"
                    .into(),
            ));
        }

        self.consume_recovering((classify, handler), |consumer, channel, (classify, handler)| {
            Box::pin(async move {
                let result = consumer
                    .consume_keyed_session(channel, parallelism, classify, handler)
                    .await;
                // Close a still-open channel so RabbitMQ can redeliver its
                // unsettled messages once the running handlers have drained.
                if channel.status().connected() {
                    match channel.close(200, "Consumer session ended".into()).await {
                        Err(error) if !is_expected_shutdown_error(&error) => {
                            warn!(queue = %consumer.config.queue, %error, "Could not close consumer channel");
                        }
                        _ => {}
                    }
                }
                result
            })
        })
        .await
    }

    async fn consume_recovering<S, RunSession>(
        &self,
        mut state: S,
        mut run_session: RunSession,
    ) -> MqResult<()>
    where
        S: Send,
        RunSession: for<'a> FnMut(&'a Self, &'a Channel, &'a mut S) -> BoxFuture<'a, MqResult<()>>,
    {
        validate_recoverable_consumer_config(&self.config)?;

        let mut channel = self.channel.clone();
        let mut backoff = Duration::from_millis(250);
        loop {
            if self.stopped.is_cancelled() {
                return Ok(());
            }
            let result = run_session(self, &channel, &mut state).await;
            if self.stopped.is_cancelled() {
                return Ok(());
            }
            match result {
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
                if self.stopped.is_cancelled() {
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
            _ = self.stopped.cancelled() => true,
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
        let observed = self.observe_session(channel);

        info!("Consuming messages from queue '{}'", self.config.queue);

        loop {
            let next = tokio::select! {
                biased;
                _ = self.stopped.cancelled() => break,
                next = consumer.next() => next,
                _ = tokio::time::sleep(SUBSCRIPTION_POLL_INTERVAL), if observed.is_some() => {
                    if let Some(session) = &observed {
                        session.progressed();
                    }
                    continue;
                }
            };
            let Some(delivery) = next else {
                break;
            };
            if let Some(session) = &observed {
                session.progressed();
            }
            match delivery {
                Ok(delivery) => {
                    let delivery_tag = delivery.delivery_tag;

                    debug!(
                        "Received message with delivery tag {} from queue '{}'",
                        delivery_tag, self.config.queue
                    );

                    let envelope = match MessageEnvelope::<T>::from_bytes(&delivery.data) {
                        Ok(envelope) => envelope,
                        Err(error) => {
                            self.reject_malformed(channel, delivery_tag, error).await?;
                            continue;
                        }
                    };

                    debug!(
                        "Processing message {} of type {:?}",
                        envelope.message_id, envelope.message_type
                    );
                    let message_id = envelope.message_id;
                    let result = handler(envelope.clone()).await;
                    self.settle_delivery(channel, delivery_tag, message_id, result)
                        .await?;
                }
                Err(e) => {
                    return Err(MqError::Consume(format!("Error receiving message: {e}")));
                }
            }
        }

        Ok(())
    }

    async fn consume_keyed_session<T, K, C, Classification, F, Fut>(
        &self,
        channel: &Channel,
        parallelism: NonZeroUsize,
        classify: &mut C,
        handler: &mut F,
    ) -> MqResult<()>
    where
        T: Clone + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + 'static,
        K: Copy + Eq + Hash + Send,
        C: FnMut(&MessageEnvelope<T>) -> Classification + Send,
        Classification: Future<Output = MqResult<Option<K>>> + Send,
        F: FnMut(MessageEnvelope<T>) -> Fut + Send,
        Fut: Future<Output = MqResult<()>> + Send,
    {
        let consumer = self.start_on(channel).await?;
        let observed = self.observe_session(channel);
        info!("Consuming messages from queue '{}'", self.config.queue);

        // The stream retains any classification already in progress if a
        // handler finishes first. Classify in broker delivery order.
        let classifying = AtomicBool::new(false);
        let mut deliveries = Box::pin(consumer.then(|delivery| {
            classifying.store(true, Ordering::Relaxed);
            if let Some(observed) = &observed {
                observed.progressed();
            }
            let decoded = delivery
                .map_err(|error| MqError::Consume(format!("Error receiving message: {error}")))
                .map(|delivery| {
                    let tag = delivery.delivery_tag;
                    let prepared =
                        MessageEnvelope::<T>::from_bytes(&delivery.data).map(|envelope| {
                            let key = classify(&envelope);
                            (envelope, key)
                        });
                    (tag, prepared)
                });
            let classifying = &classifying;
            async move {
                let _activity = ClassificationActivity(classifying);
                let (tag, prepared) = decoded?;
                let (envelope, key) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.reject_malformed(channel, tag, error).await?;
                        return Ok(None);
                    }
                };

                match key.await {
                    Ok(key) => Ok::<_, MqError>(Some((key, (tag, envelope)))),
                    Err(error) if error.is_retriable() => {
                        warn!(message_id = %envelope.message_id, %error, "Processing delivery as a barrier after classification failure");
                        Ok(Some((None, (tag, envelope))))
                    }
                    Err(error) => {
                        self.settle_delivery(channel, tag, envelope.message_id, Err(error))
                            .await?;
                        Ok(None)
                    }
                }
            }
        }));

        let window = usize::from(self.config.prefetch_count);
        let mut pending = VecDeque::<KeyedDelivery<K, T>>::new();
        let mut active = HashSet::new();
        let mut in_flight = FuturesUnordered::new();
        let mut ticker = tokio::time::interval(SUBSCRIPTION_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut result = loop {
            let stopped = self.stopped.cancelled();
            if self.stopped.is_cancelled() {
                break Ok(());
            }

            while in_flight.len() < parallelism.get() {
                let Some(index) = next_ready_delivery(&pending, &active) else {
                    break;
                };
                let (key, (tag, envelope)) = pending.remove(index).expect("ready delivery exists");
                active.insert(key);
                let message_id = envelope.message_id;
                let processing = handler(envelope);
                in_flight.push(async move {
                    self.settle_delivery(channel, tag, message_id, processing.await)
                        .await?;
                    Ok::<_, MqError>(key)
                });
            }

            tokio::select! {
                _ = stopped => break Ok(()),
                next = deliveries.next(), if pending.len() + in_flight.len() < window => {
                    match next {
                        Some(Ok(Some(delivery))) => pending.push_back(delivery),
                        Some(Ok(None)) => {}
                        Some(Err(error)) => break Err(error),
                        None => break Ok(()),
                    }
                }
                Some(finished) = in_flight.next(), if !in_flight.is_empty() => {
                    match finished {
                        Ok(key) => { active.remove(&key); }
                        Err(error) => break Err(error),
                    }
                    if let Some(observed) = &observed {
                        observed.progressed();
                    }
                }
                _ = ticker.tick() => {
                    // The prefetch window can be full of handlers blocked on DB
                    // or broker I/O. Detect loss even while intake is paused.
                    if !channel.status().connected() {
                        break Err(MqError::Channel("Consumer channel disconnected".into()));
                    }
                    if let Some(observed) = &observed {
                        if pending.is_empty()
                            && in_flight.is_empty()
                            && !classifying.load(Ordering::Relaxed)
                        {
                            observed.progressed();
                        }
                    }
                }
            }
        };

        // Broker recovery stops intake first. Finishing admitted handlers gives
        // them the chance to complete side effects and cleanup, as in the
        // serial consumer. Any failed acknowledgement can still replay.
        drop(deliveries);
        drop(pending);
        while let Some(finished) = in_flight.next().await {
            result = result.and(finished.map(|_| ()));
        }
        result
    }

    fn observe_session(&self, channel: &Channel) -> Option<ObservedSession<'_>> {
        self.subscription.as_ref().and_then(|observation| {
            let mut state = observation.0.lock().ok()?;
            if self.stopped.is_cancelled() {
                return None;
            }
            let id = uuid::Uuid::new_v4();
            *state = Some(ActiveSubscription {
                channel: channel.clone(),
                session: id,
                progress: Instant::now(),
            });
            Some(ObservedSession { observation, id })
        })
    }

    async fn reject_malformed(
        &self,
        channel: &Channel,
        delivery_tag: u64,
        error: serde_json::Error,
    ) -> MqResult<()> {
        error!(
            "Failed to deserialize message: {}. Rejecting message.",
            error
        );
        self.reject_delivery(channel, delivery_tag, false).await
    }

    async fn reject_delivery(
        &self,
        channel: &Channel,
        delivery_tag: u64,
        requeue: bool,
    ) -> MqResult<()> {
        if !self.config.auto_ack {
            channel
                .basic_nack(
                    delivery_tag,
                    BasicNackOptions {
                        requeue,
                        multiple: false,
                    },
                )
                .await
                .map_err(|error| MqError::Channel(format!("Failed to nack message: {error}")))?;
        }
        Ok(())
    }

    async fn settle_delivery(
        &self,
        channel: &Channel,
        delivery_tag: u64,
        message_id: uuid::Uuid,
        result: MqResult<()>,
    ) -> MqResult<()> {
        match result {
            Ok(()) => {
                debug!("Message {} processed successfully", message_id);
                if !self.config.auto_ack {
                    channel
                        .basic_ack(delivery_tag, BasicAckOptions::default())
                        .await
                        .map_err(|error| {
                            MqError::Channel(format!("Failed to ack message: {error}"))
                        })?;
                }
                Ok(())
            }
            Err(error) => {
                error!("Handler failed for message {}: {}", message_id, error);
                let requeue = error.is_retriable();
                if !self.config.auto_ack {
                    warn!("Rejecting message {} (requeue: {})", message_id, requeue);
                }
                self.reject_delivery(channel, delivery_tag, requeue).await
            }
        }
    }

    /// Get the underlying channel
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// Get the queue name
    pub fn queue(&self) -> &str {
        &self.config.queue
    }

    /// Stop intake and recovery. The owner must await the consumer task before
    /// closing shared resources needed by its active handlers.
    pub fn request_stop(&self) {
        self.stopped.cancel();
        if let Some(observation) = &self.subscription {
            if let Ok(mut active) = observation.0.lock() {
                *active = None;
            }
        }
    }

    /// Stop consuming and close the underlying channel.
    pub async fn stop(&self) -> MqResult<()> {
        info!("Stopping consumer for queue '{}'", self.config.queue);
        self.request_stop();

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

struct ClassificationActivity<'a>(&'a AtomicBool);

impl Drop for ClassificationActivity<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

// A busy key may be bypassed by independent work within this window. A
// barrier must wait for all older deliveries and finish before any later one.
fn next_ready_delivery<K: Copy + Eq + Hash, T>(
    pending: &VecDeque<(Option<K>, T)>,
    active: &HashSet<Option<K>>,
) -> Option<usize> {
    if active.contains(&None) {
        return None;
    }
    for (index, (key, _)) in pending.iter().enumerate() {
        match key {
            None => return (index == 0 && active.is_empty()).then_some(index),
            Some(_) if !active.contains(key) => return Some(index),
            Some(_) => {}
        }
    }
    None
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

    #[tokio::test]
    #[ignore = "requires an owned RabbitMQ broker"]
    async fn keyed_consumer_orders_related_deliveries_and_recovers_unsettled_work() {
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;
        use tokio::sync::{mpsc, Notify};

        async fn publish(channel: &Channel, queue: &str, item: u8) {
            let message = MessageEnvelope::new(super::super::MessageType::ExecutionCompleted, item);
            channel
                .basic_publish(
                    "".into(),
                    queue.into(),
                    Default::default(),
                    &message.to_bytes().unwrap(),
                    Default::default(),
                )
                .await
                .unwrap()
                .await
                .unwrap();
        }

        let url = std::env::var("ATTUNE_TEST_RABBITMQ_URL").expect("owned RabbitMQ fixture URL");
        let connection = Connection::connect(&url).await.unwrap();
        let channel = connection.create_channel().await.unwrap();
        let queue = format!("keyed-completions.{}", uuid::Uuid::new_v4().simple());
        channel
            .queue_declare(
                queue.as_str().into(),
                lapin::options::QueueDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        let result = AssertUnwindSafe(async {
            for item in 1..=5 {
                publish(&channel, &queue, item).await;
            }
            let (consumer, observation) = Consumer::new(
                &connection,
                ConsumerConfig {
                    queue: queue.clone(),
                    tag: "keyed-completions".into(),
                    prefetch_count: 5,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .unwrap()
            .with_subscription_observation();
            let consumer = Arc::new(consumer);
            let (started, mut starts) = mpsc::unbounded_channel();
            let release_first = Arc::new(Notify::new());
            let release_barrier = Arc::new(Notify::new());
            let release_fault = Arc::new(Notify::new());
            let classify_started = Arc::new(Notify::new());
            let release_classification = Arc::new(Notify::new());
            let release_shutdown = Arc::new(Notify::new());
            tasks.spawn({
                let mut first_fault_delivery = true;
                let mut first_retry_delivery = true;
                let consumer = consumer.clone();
                let release_first = release_first.clone();
                let release_barrier = release_barrier.clone();
                let release_fault = release_fault.clone();
                let classify_started = classify_started.clone();
                let release_classification = release_classification.clone();
                let release_shutdown = release_shutdown.clone();
                async move {
                    consumer
                        .consume_with_keyed_handler(
                            NonZeroUsize::new(2).unwrap(),
                            move |message: &MessageEnvelope<u8>| {
                                let item = message.payload;
                                let classify_started = classify_started.clone();
                                let release_classification = release_classification.clone();
                                async move {
                                    if item == 8 {
                                        classify_started.notify_one();
                                        release_classification.notified().await;
                                        return Err(MqError::Pool(
                                            "test classification failure".into(),
                                        ));
                                    }
                                    Ok(match item {
                                        1 | 2 => Some(1),
                                        4 => None,
                                        6 | 7 => Some(6),
                                        9 => Some(8),
                                        other => Some(other),
                                    })
                                }
                            },
                            move |message| {
                                let item = message.payload;
                                let interrupted =
                                    item == 6 && std::mem::take(&mut first_fault_delivery);
                                let retry = item == 5 && std::mem::take(&mut first_retry_delivery);
                                let started = started.clone();
                                let release_first = release_first.clone();
                                let release_barrier = release_barrier.clone();
                                let release_fault = release_fault.clone();
                                let release_shutdown = release_shutdown.clone();
                                async move {
                                    started.send(item).unwrap();
                                    if retry {
                                        return Err(MqError::Pool("test retry".into()));
                                    }
                                    match item {
                                        1 => release_first.notified().await,
                                        4 => release_barrier.notified().await,
                                        6 if interrupted => release_fault.notified().await,
                                        10 => release_shutdown.notified().await,
                                        _ => {}
                                    }
                                    Ok(())
                                }
                            },
                        )
                        .await
                }
            });

            tokio::time::timeout(Duration::from_secs(30), async {
                let mut first_started = [starts.recv().await, starts.recv().await];
                first_started.sort_unstable();
                assert_eq!(first_started, [Some(1), Some(3)]);
                release_first.notify_one();
                assert_eq!(starts.recv().await, Some(2));
                assert_eq!(starts.recv().await, Some(4));
                assert!(
                    tokio::time::timeout(Duration::from_millis(150), starts.recv())
                        .await
                        .is_err()
                );
                release_barrier.notify_one();
                assert_eq!(starts.recv().await, Some(5));
                assert_eq!(starts.recv().await, Some(5));

                publish(&channel, &queue, 6).await;
                assert_eq!(starts.recv().await, Some(6));
                consumer
                    .channel()
                    .close(200, "Test session loss".into())
                    .await
                    .unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(150), starts.recv())
                        .await
                        .is_err()
                );
                release_fault.notify_one();
                assert_eq!(starts.recv().await, Some(6));
                publish(&channel, &queue, 7).await;
                assert_eq!(starts.recv().await, Some(7));

                publish(&channel, &queue, 8).await;
                classify_started.notified().await;
                publish(&channel, &queue, 9).await;
                assert!(observation.is_current());
                tokio::time::sleep(SUBSCRIPTION_FRESHNESS + Duration::from_millis(200)).await;
                assert!(!observation.is_current(), "classification stalled");
                release_classification.notify_one();
                assert_eq!(starts.recv().await, Some(8));
                assert_eq!(starts.recv().await, Some(9));

                publish(&channel, &queue, 10).await;
                assert_eq!(starts.recv().await, Some(10));
                consumer.request_stop();
                publish(&channel, &queue, 11).await;
                assert!(
                    tokio::time::timeout(Duration::from_millis(150), tasks.join_next())
                        .await
                        .is_err(),
                    "the admitted handler must finish before shutdown"
                );
                release_shutdown.notify_one();
                tasks.join_next().await.unwrap().unwrap().unwrap();
                let next = channel
                    .basic_get(queue.as_str().into(), Default::default())
                    .await
                    .unwrap()
                    .expect("unstarted delivery remains available");
                assert_eq!(
                    MessageEnvelope::<u8>::from_bytes(&next.delivery.data)
                        .unwrap()
                        .payload,
                    11
                );
                next.delivery.acker.ack(Default::default()).await.unwrap();
            })
            .await
            .expect("keyed consumer did not finish ordering and recovery checks");
        })
        .catch_unwind()
        .await;
        tasks.shutdown().await;
        channel
            .queue_delete(queue.as_str().into(), Default::default())
            .await
            .unwrap();
        connection.close().await.unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    #[ignore = "requires an owned RabbitMQ broker"]
    async fn observation_requires_an_acknowledged_live_subscription() {
        let url = std::env::var("ATTUNE_TEST_RABBITMQ_URL").expect("owned RabbitMQ fixture URL");
        let connection = Connection::connect(&url).await.expect("fixture broker");
        let result = std::panic::AssertUnwindSafe(async {
            let channel = connection.create_channel().await.unwrap();
            let queue = format!("subscription.{}", uuid::Uuid::new_v4().simple());
            channel
                .queue_declare(
                    queue.as_str().into(),
                    lapin::options::QueueDeclareOptions {
                        exclusive: true,
                        auto_delete: true,
                        ..Default::default()
                    },
                    FieldTable::default(),
                )
                .await
                .unwrap();
            let (consumer, observation) = Consumer::new(
                &connection,
                ConsumerConfig {
                    queue,
                    tag: "subscription-boundary".into(),
                    prefetch_count: 1,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .unwrap()
            .with_subscription_observation();
            let consumer = Arc::new(consumer);
            assert!(!observation.is_current());
            let mut task = {
                let consumer = consumer.clone();
                tokio::spawn(async move {
                    consumer
                        .consume_once_with_handler(|_: MessageEnvelope<String>| async { Ok(()) })
                        .await
                })
            };
            let ready = tokio::time::timeout(Duration::from_secs(8), async {
                while !observation.is_current() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            let stopped = consumer.stop().await;
            let joined = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
            if joined.is_err() {
                task.abort();
                let _ = task.await;
            }
            assert!(
                ready.is_ok(),
                "broker subscription did not become observable"
            );
            stopped.unwrap();
            joined.unwrap().unwrap().unwrap();
            assert!(!observation.is_current());
        });
        use futures::FutureExt;
        let result = result.catch_unwind().await;
        connection.close().await.unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

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
