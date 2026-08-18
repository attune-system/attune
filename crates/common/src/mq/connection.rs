//! RabbitMQ Connection Management
//!
//! This module provides connection management for RabbitMQ, including:
//! - Connection pooling for efficient resource usage
//! - Automatic reconnection on connection failures
//! - Health checking for monitoring
//! - Channel creation and management

use lapin::{
    options::{ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions},
    types::FieldTable,
    Channel, Connection as LapinConnection, ConnectionProperties, ExchangeKind,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::{
    config::{ExchangeConfig, MessageQueueConfig, QueueConfig, RabbitMqConfig},
    consumer::{Consumer, ConsumerConfig},
    error::{MqError, MqResult},
    ExchangeType,
};

pub(crate) fn is_expected_shutdown_error_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("invalid connection state: closing")
        || normalized.contains("invalid connection state: closed")
        || normalized.contains("invalid channel state: closing")
        || normalized.contains("invalid channel state: closed")
}

pub(crate) fn is_expected_shutdown_error(error: &lapin::Error) -> bool {
    is_expected_shutdown_error_message(&error.to_string())
}

/// RabbitMQ connection wrapper with reconnection support
#[derive(Clone)]
pub struct Connection {
    /// Underlying lapin connection (Arc-wrapped for sharing)
    connection: Arc<RwLock<Option<Arc<LapinConnection>>>>,
    /// Connection configuration
    config: RabbitMqConfig,
    /// Connection URL
    url: String,
}

impl Connection {
    /// Create a new connection from configuration
    pub async fn from_config(config: &MessageQueueConfig) -> MqResult<Self> {
        if !config.enabled {
            return Err(MqError::Config(
                "Message queue is disabled in configuration".to_string(),
            ));
        }

        config.rabbitmq.validate()?;

        let url = config.rabbitmq.connection_url();
        let connection = Self::connect_internal(&url, &config.rabbitmq).await?;

        Ok(Self {
            connection: Arc::new(RwLock::new(Some(Arc::new(connection)))),
            config: config.rabbitmq.clone(),
            url,
        })
    }

    /// Create a new connection with explicit URL
    pub async fn connect(url: &str) -> MqResult<Self> {
        let config = RabbitMqConfig::default();
        let connection = Self::connect_internal(url, &config).await?;

        Ok(Self {
            connection: Arc::new(RwLock::new(Some(Arc::new(connection)))),
            config,
            url: url.to_string(),
        })
    }

    /// Internal connection method
    async fn connect_internal(url: &str, _config: &RabbitMqConfig) -> MqResult<LapinConnection> {
        info!("Connecting to RabbitMQ at {}", url);

        let connection = LapinConnection::connect(url, ConnectionProperties::default())
            .await
            .map_err(|e| MqError::Connection(format!("Failed to connect: {}", e)))?;

        info!("Successfully connected to RabbitMQ");
        Ok(connection)
    }

    /// Get or reconnect to RabbitMQ
    async fn get_connection(&self) -> MqResult<Arc<LapinConnection>> {
        let conn_guard = self.connection.read().await;
        if let Some(ref conn) = *conn_guard {
            if conn.status().connected() {
                return Ok(Arc::clone(conn));
            }
        }
        drop(conn_guard);

        // Connection is not available, attempt reconnect
        self.reconnect().await
    }

    /// Reconnect to RabbitMQ
    async fn reconnect(&self) -> MqResult<Arc<LapinConnection>> {
        let mut conn_guard = self.connection.write().await;

        // Double-check if another task already reconnected
        if let Some(ref conn) = *conn_guard {
            if conn.status().connected() {
                return Ok(Arc::clone(conn));
            }
        }

        warn!("Attempting to reconnect to RabbitMQ");

        let mut attempts = 0;
        let max_attempts = self.config.max_reconnect_attempts;

        loop {
            match Self::connect_internal(&self.url, &self.config).await {
                Ok(new_conn) => {
                    info!("Reconnected to RabbitMQ after {} attempts", attempts + 1);
                    let arc_conn = Arc::new(new_conn);
                    *conn_guard = Some(Arc::clone(&arc_conn));
                    return Ok(arc_conn);
                }
                Err(e) => {
                    attempts += 1;
                    if max_attempts > 0 && attempts >= max_attempts {
                        error!("Failed to reconnect after {} attempts: {}", attempts, e);
                        return Err(MqError::Connection(format!(
                            "Max reconnection attempts ({}) exceeded",
                            max_attempts
                        )));
                    }

                    warn!(
                        "Reconnection attempt {} failed: {}. Retrying in {:?}...",
                        attempts,
                        e,
                        self.config.reconnect_delay()
                    );
                    tokio::time::sleep(self.config.reconnect_delay()).await;
                }
            }
        }
    }

    /// Create a new channel
    pub async fn create_channel(&self) -> MqResult<Channel> {
        let connection = self.get_connection().await?;

        connection
            .create_channel()
            .await
            .map_err(|e| MqError::Channel(format!("Failed to create channel: {}", e)))
    }

    /// Create an ephemeral per-instance topic consumer queue bound to one or more routing keys.
    ///
    /// The queue is server-named, exclusive, and auto-delete, so each service instance receives
    /// its own copy of messages (broadcast semantics across replicas).
    pub async fn create_ephemeral_topic_consumer(
        &self,
        exchange: &str,
        routing_keys: &[&str],
        consumer_tag: &str,
        prefetch_count: u16,
    ) -> MqResult<Consumer> {
        if routing_keys.is_empty() {
            return Err(MqError::Config(
                "routing_keys must not be empty for ephemeral topic consumer".to_string(),
            ));
        }

        let channel = self.create_channel().await?;
        let queue = channel
            .queue_declare(
                "".into(),
                QueueDeclareOptions {
                    durable: false,
                    exclusive: true,
                    auto_delete: true,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| {
                MqError::QueueDeclaration(format!("Failed to declare ephemeral queue: {e}"))
            })?;
        let queue_name = queue.name().as_str().to_string();

        for routing_key in routing_keys {
            channel
                .queue_bind(
                    queue_name.as_str().into(),
                    exchange.into(),
                    (*routing_key).into(),
                    QueueBindOptions::default(),
                    FieldTable::default(),
                )
                .await
                .map_err(|e| {
                    MqError::QueueBinding(format!(
                        "Failed to bind ephemeral queue '{}' to exchange '{}' with routing key '{}': {}",
                        queue_name, exchange, routing_key, e
                    ))
                })?;
        }

        drop(channel);

        Consumer::new(
            self,
            ConsumerConfig {
                queue: queue_name,
                tag: consumer_tag.to_string(),
                prefetch_count,
                auto_ack: false,
                exclusive: true,
            },
        )
        .await
    }

    /// Check if connection is healthy
    pub async fn is_healthy(&self) -> bool {
        let conn_guard = self.connection.read().await;
        if let Some(ref conn) = *conn_guard {
            conn.status().connected()
        } else {
            false
        }
    }

    /// Close the connection
    pub async fn close(&self) -> MqResult<()> {
        let mut conn_guard = self.connection.write().await;
        if let Some(conn) = conn_guard.take() {
            let status = conn.status();
            if status.closing() || status.closed() {
                info!("Connection already closing or closed");
                return Ok(());
            }

            match conn.close(200, "Normal shutdown".into()).await {
                Ok(()) => info!("Connection closed"),
                Err(e) if is_expected_shutdown_error(&e) => {
                    info!("Connection was already shutting down");
                }
                Err(e) => {
                    return Err(MqError::Connection(format!(
                        "Failed to close connection: {}",
                        e
                    )));
                }
            }
        }
        Ok(())
    }

    /// Declare an exchange
    pub async fn declare_exchange(&self, config: &ExchangeConfig) -> MqResult<()> {
        let channel = self.create_channel().await?;

        let kind = match config.r#type {
            ExchangeType::Direct => ExchangeKind::Direct,
            ExchangeType::Topic => ExchangeKind::Topic,
            ExchangeType::Fanout => ExchangeKind::Fanout,
            ExchangeType::Headers => ExchangeKind::Headers,
        };

        debug!(
            "Declaring exchange '{}' of type '{}'",
            config.name, config.r#type
        );

        channel
            .exchange_declare(
                config.name.as_str().into(),
                kind,
                ExchangeDeclareOptions {
                    durable: config.durable,
                    auto_delete: config.auto_delete,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| {
                MqError::ExchangeDeclaration(format!(
                    "Failed to declare exchange '{}': {}",
                    config.name, e
                ))
            })?;

        info!("Exchange '{}' declared successfully", config.name);
        Ok(())
    }

    /// Declare a queue
    pub async fn declare_queue(&self, config: &QueueConfig) -> MqResult<()> {
        let channel = self.create_channel().await?;

        debug!("Declaring queue '{}'", config.name);

        channel
            .queue_declare(
                config.name.as_str().into(),
                QueueDeclareOptions {
                    durable: config.durable,
                    exclusive: config.exclusive,
                    auto_delete: config.auto_delete,
                    ..Default::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| {
                MqError::QueueDeclaration(format!(
                    "Failed to declare queue '{}': {}",
                    config.name, e
                ))
            })?;

        info!("Queue '{}' declared successfully", config.name);
        Ok(())
    }

    /// Bind a queue to an exchange
    pub async fn bind_queue(&self, queue: &str, exchange: &str, routing_key: &str) -> MqResult<()> {
        let channel = self.create_channel().await?;

        debug!(
            "Binding queue '{}' to exchange '{}' with routing key '{}'",
            queue, exchange, routing_key
        );

        channel
            .queue_bind(
                queue.into(),
                exchange.into(),
                routing_key.into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(|e| {
                MqError::QueueBinding(format!(
                    "Failed to bind queue '{}' to exchange '{}': {}",
                    queue, exchange, e
                ))
            })?;

        info!(
            "Queue '{}' bound to exchange '{}' with routing key '{}'",
            queue, exchange, routing_key
        );
        Ok(())
    }

    /// Declare a queue with dead letter exchange
    pub async fn declare_queue_with_dlx(
        &self,
        config: &QueueConfig,
        dlx_exchange: &str,
    ) -> MqResult<()> {
        self.declare_queue_with_dlx_and_ttl(config, dlx_exchange, None)
            .await
    }

    /// Declare a queue with dead letter exchange and optional TTL
    pub async fn declare_queue_with_dlx_and_ttl(
        &self,
        config: &QueueConfig,
        dlx_exchange: &str,
        ttl_ms: Option<u64>,
    ) -> MqResult<()> {
        let channel = self.create_channel().await?;

        let ttl_info = if let Some(ttl) = ttl_ms {
            format!(" and TTL {}ms", ttl)
        } else {
            String::new()
        };

        debug!(
            "Declaring queue '{}' with dead letter exchange '{}'{}",
            config.name, dlx_exchange, ttl_info
        );

        let mut args = FieldTable::default();
        args.insert(
            "x-dead-letter-exchange".into(),
            lapin::types::AMQPValue::LongString(dlx_exchange.into()),
        );

        // Add message TTL if specified
        if let Some(ttl) = ttl_ms {
            args.insert(
                "x-message-ttl".into(),
                lapin::types::AMQPValue::LongInt(ttl as i32),
            );
        }

        channel
            .queue_declare(
                config.name.as_str().into(),
                QueueDeclareOptions {
                    durable: config.durable,
                    exclusive: config.exclusive,
                    auto_delete: config.auto_delete,
                    ..Default::default()
                },
                args,
            )
            .await
            .map_err(|e| {
                MqError::QueueDeclaration(format!(
                    "Failed to declare queue '{}' with DLX{}: {}",
                    config.name, ttl_info, e
                ))
            })?;

        info!(
            "Queue '{}' declared with dead letter exchange '{}'{}",
            config.name, dlx_exchange, ttl_info
        );
        Ok(())
    }

    /// Setup common infrastructure (exchanges, DLX) - safe to call from any service
    ///
    /// This sets up the shared infrastructure that all services need:
    /// - All exchanges (events, executions, notifications)
    /// - Dead letter exchange (if enabled)
    ///
    /// This is idempotent and can be called by multiple services safely.
    pub async fn setup_common_infrastructure(&self, config: &MessageQueueConfig) -> MqResult<()> {
        info!("Setting up common RabbitMQ infrastructure (exchanges and DLX)");

        // Declare exchanges
        self.declare_exchange(&config.rabbitmq.exchanges.events)
            .await?;
        self.declare_exchange(&config.rabbitmq.exchanges.executions)
            .await?;
        self.declare_exchange(&config.rabbitmq.exchanges.notifications)
            .await?;
        self.declare_exchange(&config.rabbitmq.exchanges.metadata)
            .await?;

        // Declare dead letter exchange if enabled
        if config.rabbitmq.dead_letter.enabled {
            let dlx_config = ExchangeConfig {
                name: config.rabbitmq.dead_letter.exchange.clone(),
                r#type: ExchangeType::Direct,
                durable: true,
                auto_delete: false,
            };
            self.declare_exchange(&dlx_config).await?;

            // Declare dead letter queue (derive name from exchange)
            let dlq_name = format!("{}.queue", config.rabbitmq.dead_letter.exchange);
            let dlq_config = QueueConfig {
                name: dlq_name.clone(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            };
            self.declare_queue(&dlq_config).await?;

            // Bind DLQ to DLX
            self.bind_queue(&dlq_name, &config.rabbitmq.dead_letter.exchange, "#")
                .await?;
        }

        info!("Common RabbitMQ infrastructure setup complete");
        Ok(())
    }

    /// Setup executor-specific queues and bindings
    pub async fn setup_executor_infrastructure(&self, config: &MessageQueueConfig) -> MqResult<()> {
        info!("Setting up Executor infrastructure");

        let dlx = if config.rabbitmq.dead_letter.enabled {
            Some(config.rabbitmq.dead_letter.exchange.as_str())
        } else {
            None
        };

        // Declare executor-specific events queue (only receives event.created messages,
        // unlike the sensor's catch-all events queue which is bound with `#`)
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.executor_events, dlx)
            .await?;

        // Declare executor queues
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.enforcements, dlx)
            .await?;
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.execution_requests, dlx)
            .await?;
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.execution_status, dlx)
            .await?;
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.execution_completed, dlx)
            .await?;
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.inquiry_responses, dlx)
            .await?;
        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.pack_tests, dlx)
            .await?;

        // Bind queues to exchanges
        self.bind_queue(
            &config.rabbitmq.queues.enforcements.name,
            &config.rabbitmq.exchanges.executions.name,
            "enforcement.#",
        )
        .await?;

        self.bind_queue(
            &config.rabbitmq.queues.execution_requests.name,
            &config.rabbitmq.exchanges.executions.name,
            "execution.requested",
        )
        .await?;

        self.bind_queue(
            &config.rabbitmq.queues.execution_status.name,
            &config.rabbitmq.exchanges.executions.name,
            "execution.status.changed",
        )
        .await?;

        self.bind_queue(
            &config.rabbitmq.queues.execution_completed.name,
            &config.rabbitmq.exchanges.executions.name,
            "execution.completed",
        )
        .await?;

        self.bind_queue(
            &config.rabbitmq.queues.inquiry_responses.name,
            &config.rabbitmq.exchanges.executions.name,
            "inquiry.responded",
        )
        .await?;

        self.bind_queue(
            &config.rabbitmq.queues.pack_tests.name,
            &config.rabbitmq.exchanges.executions.name,
            "pack.test.requested",
        )
        .await?;

        // Bind executor events queue to only event.created routing key
        // (the sensor's attune.events.queue uses `#` and gets all message types)
        self.bind_queue(
            &config.rabbitmq.queues.executor_events.name,
            &config.rabbitmq.exchanges.events.name,
            "event.created",
        )
        .await?;

        info!("Executor infrastructure setup complete");
        Ok(())
    }

    /// Setup worker-specific queue for a worker instance
    pub async fn setup_worker_infrastructure(
        &self,
        worker_id: i64,
        config: &MessageQueueConfig,
    ) -> MqResult<()> {
        info!(
            "Setting up Worker infrastructure for worker ID {}",
            worker_id
        );

        let dlx = if config.rabbitmq.dead_letter.enabled {
            Some(config.rabbitmq.dead_letter.exchange.as_str())
        } else {
            None
        };

        // --- Execution dispatch queue ---
        let queue_name = format!("worker.{}.executions", worker_id);
        let queue_config = QueueConfig {
            name: queue_name.clone(),
            durable: true,
            exclusive: false,
            auto_delete: false,
        };

        // Worker queues use TTL to expire unprocessed messages
        let ttl_ms = Some(config.rabbitmq.worker_queue_ttl_ms);

        self.declare_queue_with_optional_dlx_and_ttl(&queue_config, dlx, ttl_ms)
            .await?;

        // Bind to execution dispatch routing key
        self.bind_queue(
            &queue_name,
            &config.rabbitmq.exchanges.executions.name,
            &format!("execution.dispatch.worker.{}", worker_id),
        )
        .await?;

        // --- Pack registration queue ---
        // Each worker gets its own queue for pack.registered events so that
        // every worker instance can independently set up runtime environments
        // (e.g., Python virtualenvs) when a new pack is registered.
        let packs_queue_name = format!("worker.{}.packs", worker_id);
        let packs_queue_config = QueueConfig {
            name: packs_queue_name.clone(),
            durable: true,
            exclusive: false,
            auto_delete: false,
        };

        self.declare_queue_with_optional_dlx(&packs_queue_config, dlx)
            .await?;

        // Bind to pack.registered routing key on the events exchange
        self.bind_queue(
            &packs_queue_name,
            &config.rabbitmq.exchanges.events.name,
            "pack.registered",
        )
        .await?;

        // Bind to pack.deleted routing key on the same queue
        self.bind_queue(
            &packs_queue_name,
            &config.rabbitmq.exchanges.events.name,
            "pack.deleted",
        )
        .await?;

        // --- Cancel queue ---
        // Each worker gets its own queue for execution cancel requests so that
        // the API can target a specific worker to gracefully stop a running process.
        let cancel_queue_name = format!("worker.{}.cancel", worker_id);
        let cancel_queue_config = QueueConfig {
            name: cancel_queue_name.clone(),
            durable: true,
            exclusive: false,
            auto_delete: false,
        };

        self.declare_queue_with_optional_dlx(&cancel_queue_config, dlx)
            .await?;

        // Bind to worker-specific cancel routing key on the executions exchange
        self.bind_queue(
            &cancel_queue_name,
            &config.rabbitmq.exchanges.executions.name,
            &format!("execution.cancel.worker.{}", worker_id),
        )
        .await?;

        // --- Pack test queue ---
        // Each worker gets its own queue for pack.test dispatch messages so a
        // pack's test suite can be executed on a worker that supports the
        // required runtimes (e.g., python) rather than on the API container.
        let pack_tests_queue_name = format!("worker.{}.packtests", worker_id);
        let pack_tests_queue_config = QueueConfig {
            name: pack_tests_queue_name.clone(),
            durable: true,
            exclusive: false,
            auto_delete: false,
        };

        self.declare_queue_with_optional_dlx(&pack_tests_queue_config, dlx)
            .await?;

        // Bind to worker-specific pack test routing key on the executions exchange
        self.bind_queue(
            &pack_tests_queue_name,
            &config.rabbitmq.exchanges.executions.name,
            &format!("pack.test.dispatch.worker.{}", worker_id),
        )
        .await?;

        info!(
            "Worker infrastructure setup complete for worker ID {}",
            worker_id
        );
        Ok(())
    }

    /// Setup sensor-specific queues and bindings
    pub async fn setup_sensor_infrastructure(&self, config: &MessageQueueConfig) -> MqResult<()> {
        info!("Setting up Sensor infrastructure");

        let dlx = if config.rabbitmq.dead_letter.enabled {
            Some(config.rabbitmq.dead_letter.exchange.as_str())
        } else {
            None
        };

        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.events, dlx)
            .await?;

        // Bind to all events
        self.bind_queue(
            &config.rabbitmq.queues.events.name,
            &config.rabbitmq.exchanges.events.name,
            "#",
        )
        .await?;

        info!("Sensor infrastructure setup complete");
        Ok(())
    }

    /// Setup notifier-specific queues and bindings
    pub async fn setup_notifier_infrastructure(&self, config: &MessageQueueConfig) -> MqResult<()> {
        info!("Setting up Notifier infrastructure");

        let dlx = if config.rabbitmq.dead_letter.enabled {
            Some(config.rabbitmq.dead_letter.exchange.as_str())
        } else {
            None
        };

        self.declare_queue_with_optional_dlx(&config.rabbitmq.queues.notifications, dlx)
            .await?;

        // Bind to notifications exchange (fanout, no routing key)
        self.bind_queue(
            &config.rabbitmq.queues.notifications.name,
            &config.rabbitmq.exchanges.notifications.name,
            "",
        )
        .await?;

        info!("Notifier infrastructure setup complete");
        Ok(())
    }

    /// Helper to declare queue with optional DLX
    async fn declare_queue_with_optional_dlx(
        &self,
        config: &QueueConfig,
        dlx: Option<&str>,
    ) -> MqResult<()> {
        self.declare_queue_with_optional_dlx_and_ttl(config, dlx, None)
            .await
    }

    /// Helper to declare queue with optional DLX and TTL
    async fn declare_queue_with_optional_dlx_and_ttl(
        &self,
        config: &QueueConfig,
        dlx: Option<&str>,
        ttl_ms: Option<u64>,
    ) -> MqResult<()> {
        if let Some(dlx_exchange) = dlx {
            self.declare_queue_with_dlx_and_ttl(config, dlx_exchange, ttl_ms)
                .await
        } else {
            if ttl_ms.is_some() {
                warn!(
                    "Queue '{}' configured with TTL but no DLX - messages will be dropped",
                    config.name
                );
            }
            self.declare_queue(config).await
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::is_expected_shutdown_error_message;

    #[test]
    fn detects_expected_shutdown_connection_state_errors() {
        assert!(is_expected_shutdown_error_message(
            "invalid connection state: Closing (connection.close)"
        ));
        assert!(is_expected_shutdown_error_message(
            "invalid channel state: Closing (channel.close)"
        ));
        assert!(is_expected_shutdown_error_message(
            "invalid connection state: Closed (connection.close)"
        ));
    }

    #[test]
    fn ignores_non_shutdown_errors() {
        assert!(!is_expected_shutdown_error_message(
            "protocol error: access refused"
        ));
    }
}

/// Connection pool for managing multiple RabbitMQ connections
pub struct ConnectionPool {
    /// Pool of connections
    connections: Vec<Connection>,
    /// Current index for round-robin selection
    current: Arc<RwLock<usize>>,
}

impl ConnectionPool {
    /// Create a new connection pool
    pub async fn new(config: &MessageQueueConfig, size: usize) -> MqResult<Self> {
        let mut connections = Vec::with_capacity(size);

        for i in 0..size {
            debug!("Creating connection {} of {}", i + 1, size);
            let conn = Connection::from_config(config).await?;
            connections.push(conn);
        }

        info!("Connection pool created with {} connections", size);

        Ok(Self {
            connections,
            current: Arc::new(RwLock::new(0)),
        })
    }

    /// Get a connection from the pool (round-robin)
    pub async fn get(&self) -> MqResult<Connection> {
        if self.connections.is_empty() {
            return Err(MqError::Pool("Connection pool is empty".to_string()));
        }

        let mut current = self.current.write().await;
        let index = *current % self.connections.len();
        *current = (*current + 1) % self.connections.len();

        Ok(self.connections[index].clone())
    }

    /// Get pool size
    pub fn size(&self) -> usize {
        self.connections.len()
    }

    /// Check if all connections are healthy
    pub async fn is_healthy(&self) -> bool {
        for conn in &self.connections {
            if !conn.is_healthy().await {
                return false;
            }
        }
        true
    }

    /// Close all connections in the pool
    pub async fn close_all(&self) -> MqResult<()> {
        for conn in &self.connections {
            conn.close().await?;
        }
        info!("All connections in pool closed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_url_parsing() {
        let config = RabbitMqConfig {
            host: "localhost".to_string(),
            port: 5672,
            username: "guest".to_string(),
            password: "guest".to_string(),
            vhost: "/".to_string(),
            ..Default::default()
        };

        let url = config.connection_url();
        assert_eq!(url, "amqp://guest:guest@localhost:5672//");
    }

    #[test]
    fn test_connection_validation() {
        let mut config = RabbitMqConfig::default();
        assert!(config.validate().is_ok());

        config.host = String::new();
        assert!(config.validate().is_err());
    }

    // Integration tests would go here (require running RabbitMQ instance)
    // These should be in a separate integration test file
}
