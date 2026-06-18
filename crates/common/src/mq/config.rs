//! Message Queue Configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{ExchangeType, MqError, MqResult};

/// Message queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageQueueConfig {
    /// Whether message queue is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Message queue type (rabbitmq)
    #[serde(default = "default_type")]
    pub r#type: String,

    /// RabbitMQ configuration
    #[serde(default)]
    pub rabbitmq: RabbitMqConfig,
}

impl Default for MessageQueueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            r#type: "rabbitmq".to_string(),
            rabbitmq: RabbitMqConfig::default(),
        }
    }
}

/// RabbitMQ configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RabbitMqConfig {
    /// RabbitMQ host
    #[serde(default = "default_host")]
    pub host: String,

    /// RabbitMQ port
    #[serde(default = "default_port")]
    pub port: u16,

    /// RabbitMQ username
    #[serde(default = "default_username")]
    pub username: String,

    /// RabbitMQ password
    #[serde(default = "default_password")]
    pub password: String,

    /// RabbitMQ virtual host
    #[serde(default = "default_vhost")]
    pub vhost: String,

    /// Connection pool size
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat")]
    pub heartbeat_secs: u64,

    /// Reconnection delay in seconds
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_secs: u64,

    /// Maximum reconnection attempts (0 = infinite)
    #[serde(default = "default_max_reconnect_attempts")]
    pub max_reconnect_attempts: u32,

    /// Confirm publish (wait for broker confirmation)
    #[serde(default = "default_confirm_publish")]
    pub confirm_publish: bool,

    /// Publish timeout in seconds
    #[serde(default = "default_publish_timeout")]
    pub publish_timeout_secs: u64,

    /// Consumer prefetch count
    #[serde(default = "default_prefetch_count")]
    pub prefetch_count: u16,

    /// Consumer timeout in seconds
    #[serde(default = "default_consumer_timeout")]
    pub consumer_timeout_secs: u64,

    /// Queue configurations
    #[serde(default)]
    pub queues: QueuesConfig,

    /// Exchange configurations
    #[serde(default)]
    pub exchanges: ExchangesConfig,

    /// Dead letter queue configuration
    #[serde(default)]
    pub dead_letter: DeadLetterConfig,

    /// Worker queue message TTL in milliseconds (default 5 minutes)
    #[serde(default = "default_worker_queue_ttl")]
    pub worker_queue_ttl_ms: u64,
}

impl Default for RabbitMqConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            username: default_username(),
            password: default_password(),
            vhost: default_vhost(),
            pool_size: default_pool_size(),
            connection_timeout_secs: default_connection_timeout(),
            heartbeat_secs: default_heartbeat(),
            reconnect_delay_secs: default_reconnect_delay(),
            max_reconnect_attempts: default_max_reconnect_attempts(),
            confirm_publish: default_confirm_publish(),
            publish_timeout_secs: default_publish_timeout(),
            prefetch_count: default_prefetch_count(),
            consumer_timeout_secs: default_consumer_timeout(),
            queues: QueuesConfig::default(),
            exchanges: ExchangesConfig::default(),
            dead_letter: DeadLetterConfig::default(),
            worker_queue_ttl_ms: default_worker_queue_ttl(),
        }
    }
}

impl RabbitMqConfig {
    /// Get connection URL
    pub fn connection_url(&self) -> String {
        format!(
            "amqp://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.vhost
        )
    }

    /// Get connection timeout as Duration
    pub fn connection_timeout(&self) -> Duration {
        Duration::from_secs(self.connection_timeout_secs)
    }

    /// Get heartbeat as Duration
    pub fn heartbeat(&self) -> Duration {
        Duration::from_secs(self.heartbeat_secs)
    }

    /// Get reconnect delay as Duration
    pub fn reconnect_delay(&self) -> Duration {
        Duration::from_secs(self.reconnect_delay_secs)
    }

    /// Get publish timeout as Duration
    pub fn publish_timeout(&self) -> Duration {
        Duration::from_secs(self.publish_timeout_secs)
    }

    /// Get consumer timeout as Duration
    pub fn consumer_timeout(&self) -> Duration {
        Duration::from_secs(self.consumer_timeout_secs)
    }

    /// Get worker queue TTL as Duration
    pub fn worker_queue_ttl(&self) -> Duration {
        Duration::from_millis(self.worker_queue_ttl_ms)
    }

    /// Validate configuration
    pub fn validate(&self) -> MqResult<()> {
        if self.host.is_empty() {
            return Err(MqError::Config("Host cannot be empty".to_string()));
        }
        if self.username.is_empty() {
            return Err(MqError::Config("Username cannot be empty".to_string()));
        }
        if self.pool_size == 0 {
            return Err(MqError::Config(
                "Pool size must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }
}

/// Queue configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuesConfig {
    /// Events queue configuration (sensor catch-all, bound with `#`)
    pub events: QueueConfig,

    /// Executor events queue configuration (bound only to `event.created`)
    #[serde(default = "default_executor_events_queue")]
    pub executor_events: QueueConfig,

    /// Executions queue configuration (legacy - to be deprecated)
    pub executions: QueueConfig,

    /// Enforcement created queue configuration
    pub enforcements: QueueConfig,

    /// Execution requests queue configuration
    pub execution_requests: QueueConfig,

    /// Execution status updates queue configuration
    pub execution_status: QueueConfig,

    /// Execution completed queue configuration
    pub execution_completed: QueueConfig,

    /// Inquiry responses queue configuration
    pub inquiry_responses: QueueConfig,

    /// Notifications queue configuration
    pub notifications: QueueConfig,
}

fn default_executor_events_queue() -> QueueConfig {
    QueueConfig {
        name: "attune.executor.events.queue".to_string(),
        durable: true,
        exclusive: false,
        auto_delete: false,
    }
}

impl Default for QueuesConfig {
    fn default() -> Self {
        Self {
            events: QueueConfig {
                name: "attune.events.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            executor_events: QueueConfig {
                name: "attune.executor.events.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            executions: QueueConfig {
                name: "attune.executions.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            enforcements: QueueConfig {
                name: "attune.enforcements.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            execution_requests: QueueConfig {
                name: "attune.execution.requests.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            execution_status: QueueConfig {
                name: "attune.execution.status.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            execution_completed: QueueConfig {
                name: "attune.execution.completed.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            inquiry_responses: QueueConfig {
                name: "attune.inquiry.responses.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
            notifications: QueueConfig {
                name: "attune.notifications.queue".to_string(),
                durable: true,
                exclusive: false,
                auto_delete: false,
            },
        }
    }
}

/// Queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    /// Queue name
    pub name: String,

    /// Durable (survives broker restart)
    #[serde(default = "default_true")]
    pub durable: bool,

    /// Exclusive (only accessible by this connection)
    #[serde(default)]
    pub exclusive: bool,

    /// Auto-delete (deleted when last consumer disconnects)
    #[serde(default)]
    pub auto_delete: bool,
}

/// Exchange configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangesConfig {
    /// Events exchange configuration
    pub events: ExchangeConfig,

    /// Executions exchange configuration
    pub executions: ExchangeConfig,

    /// Notifications exchange configuration
    pub notifications: ExchangeConfig,

    /// Metadata exchange configuration
    pub metadata: ExchangeConfig,
}

impl Default for ExchangesConfig {
    fn default() -> Self {
        Self {
            events: ExchangeConfig {
                name: "attune.events".to_string(),
                r#type: ExchangeType::Topic,
                durable: true,
                auto_delete: false,
            },
            executions: ExchangeConfig {
                name: "attune.executions".to_string(),
                r#type: ExchangeType::Topic,
                durable: true,
                auto_delete: false,
            },
            notifications: ExchangeConfig {
                name: "attune.notifications".to_string(),
                r#type: ExchangeType::Fanout,
                durable: true,
                auto_delete: false,
            },
            metadata: ExchangeConfig {
                name: "attune.metadata".to_string(),
                r#type: ExchangeType::Topic,
                durable: true,
                auto_delete: false,
            },
        }
    }
}

/// Exchange configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeConfig {
    /// Exchange name
    pub name: String,

    /// Exchange type
    pub r#type: ExchangeType,

    /// Durable (survives broker restart)
    #[serde(default = "default_true")]
    pub durable: bool,

    /// Auto-delete (deleted when last queue unbinds)
    #[serde(default)]
    pub auto_delete: bool,
}

/// Dead letter queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterConfig {
    /// Enable dead letter queues
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Dead letter exchange name
    #[serde(default = "default_dlx_exchange")]
    pub exchange: String,

    /// Message TTL in dead letter queue (milliseconds)
    #[serde(default = "default_dlq_ttl")]
    pub ttl_ms: u64,
}

impl Default for DeadLetterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exchange: "attune.dlx".to_string(),
            ttl_ms: 86400000, // 24 hours
        }
    }
}

impl DeadLetterConfig {
    /// Get TTL as Duration
    pub fn ttl(&self) -> Duration {
        Duration::from_millis(self.ttl_ms)
    }
}

/// Publisher configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherConfig {
    /// Confirm publish (wait for broker confirmation)
    #[serde(default = "default_confirm_publish")]
    pub confirm_publish: bool,

    /// Publish timeout in seconds
    #[serde(default = "default_publish_timeout")]
    pub timeout_secs: u64,

    /// Default exchange name
    pub exchange: String,
}

impl PublisherConfig {
    /// Get timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

/// Consumer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerConfig {
    /// Queue name to consume from
    pub queue: String,

    /// Consumer tag (identifier)
    pub tag: String,

    /// Prefetch count (number of unacknowledged messages)
    #[serde(default = "default_prefetch_count")]
    pub prefetch_count: u16,

    /// Auto-acknowledge messages
    #[serde(default)]
    pub auto_ack: bool,

    /// Exclusive consumer
    #[serde(default)]
    pub exclusive: bool,
}

// Default value functions

fn default_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_type() -> String {
    "rabbitmq".to_string()
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    5672
}

fn default_username() -> String {
    "guest".to_string()
}

fn default_password() -> String {
    "guest".to_string()
}

fn default_vhost() -> String {
    "/".to_string()
}

fn default_pool_size() -> usize {
    10
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_heartbeat() -> u64 {
    60
}

fn default_reconnect_delay() -> u64 {
    5
}

fn default_max_reconnect_attempts() -> u32 {
    10
}

fn default_confirm_publish() -> bool {
    true
}

fn default_publish_timeout() -> u64 {
    5
}

fn default_prefetch_count() -> u16 {
    10
}

fn default_consumer_timeout() -> u64 {
    300
}

fn default_dlx_exchange() -> String {
    "attune.dlx".to_string()
}

fn default_dlq_ttl() -> u64 {
    86400000 // 24 hours in milliseconds
}

fn default_worker_queue_ttl() -> u64 {
    300000 // 5 minutes in milliseconds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MessageQueueConfig::default();
        assert!(config.enabled);
        assert_eq!(config.r#type, "rabbitmq");
        assert_eq!(config.rabbitmq.host, "localhost");
        assert_eq!(config.rabbitmq.port, 5672);
    }

    #[test]
    fn test_connection_url() {
        let config = RabbitMqConfig::default();
        let url = config.connection_url();
        assert!(url.starts_with("amqp://"));
        assert!(url.contains("localhost"));
        assert!(url.contains("5672"));
    }

    #[test]
    fn test_validate() {
        let mut config = RabbitMqConfig::default();
        assert!(config.validate().is_ok());

        config.host = String::new();
        assert!(config.validate().is_err());

        config.host = "localhost".to_string();
        config.pool_size = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_duration_conversions() {
        let config = RabbitMqConfig::default();
        assert_eq!(config.connection_timeout().as_secs(), 30);
        assert_eq!(config.heartbeat().as_secs(), 60);
        assert_eq!(config.reconnect_delay().as_secs(), 5);
    }

    #[test]
    fn test_dead_letter_config() {
        let config = DeadLetterConfig::default();
        assert!(config.enabled);
        assert_eq!(config.exchange, "attune.dlx");
        assert_eq!(config.ttl().as_secs(), 86400); // 24 hours
    }

    #[test]
    fn test_worker_queue_ttl() {
        let config = RabbitMqConfig::default();
        assert_eq!(config.worker_queue_ttl().as_secs(), 300); // 5 minutes
        assert_eq!(config.worker_queue_ttl_ms, 300000);
    }

    #[test]
    fn test_default_queues() {
        let queues = QueuesConfig::default();
        assert_eq!(queues.events.name, "attune.events.queue");
        assert_eq!(queues.executor_events.name, "attune.executor.events.queue");
        assert_eq!(queues.executions.name, "attune.executions.queue");
        assert_eq!(
            queues.execution_completed.name,
            "attune.execution.completed.queue"
        );
        assert_eq!(
            queues.inquiry_responses.name,
            "attune.inquiry.responses.queue"
        );
        assert_eq!(queues.notifications.name, "attune.notifications.queue");
        assert!(queues.events.durable);
    }

    #[test]
    fn test_default_exchanges() {
        let exchanges = ExchangesConfig::default();
        assert_eq!(exchanges.events.name, "attune.events");
        assert_eq!(exchanges.executions.name, "attune.executions");
        assert_eq!(exchanges.notifications.name, "attune.notifications");
        assert_eq!(exchanges.metadata.name, "attune.metadata");
        assert!(matches!(exchanges.events.r#type, ExchangeType::Topic));
        assert!(matches!(exchanges.executions.r#type, ExchangeType::Topic));
        assert!(matches!(exchanges.metadata.r#type, ExchangeType::Topic));
        assert!(matches!(
            exchanges.notifications.r#type,
            ExchangeType::Fanout
        ));
    }
}
