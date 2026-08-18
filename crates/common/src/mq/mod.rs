//! Message Queue Infrastructure
//!
//! This module provides a RabbitMQ-based message queue infrastructure for inter-service
//! communication in Attune. It supports:
//!
//! - Asynchronous message publishing and consumption
//! - Reliable message delivery with acknowledgments
//! - Dead letter queues for failed messages
//! - Automatic reconnection and error handling
//! - Message serialization and deserialization
//!
//! # Architecture
//!
//! The message queue system uses RabbitMQ with three main exchanges:
//!
//! - `attune.events` - Topic exchange for event messages from sensors
//! - `attune.executions` - Topic exchange for execution and enforcement messages
//! - `attune.notifications` - Fanout exchange for system notifications
//! - `attune.metadata` - Topic exchange for metadata invalidation/change events
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use attune_common::mq::{Connection, Publisher, PublisherConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to RabbitMQ
//!     let connection = Connection::connect("amqp://localhost:5672").await?;
//!
//!     // Create publisher with config
//!     let config = PublisherConfig {
//!         confirm_publish: true,
//!         timeout_secs: 30,
//!         exchange: "attune.events".to_string(),
//!     };
//!     let publisher = Publisher::new(&connection, config).await?;
//!
//!     // Publish a message
//!     // let message = ExecutionRequested { ... };
//!     // publisher.publish(&message).await?;
//!
//!     Ok(())
//! }
//! ```

pub mod config;
pub mod connection;
pub mod consumer;
pub mod error;
pub mod message_queue;
pub mod messages;
pub mod publisher;

pub use config::{ExchangeConfig, MessageQueueConfig, QueueConfig};
pub use connection::{Connection, ConnectionPool};
pub use consumer::{Consumer, ConsumerConfig};
pub use error::{MqError, MqResult};
pub use message_queue::MessageQueue;
pub use messages::{
    ActionChangedPayload, EnforcementCreatedPayload, EventCreatedPayload,
    ExecutionCancelRequestedPayload, ExecutionCompletedPayload, ExecutionRequestedPayload,
    ExecutionStatusChangedPayload, IdentityAuthorizationChangedPayload, InquiryCreatedPayload,
    InquiryRespondedPayload, Message, MessageEnvelope, MessageType, NotificationCreatedPayload,
    PackChangedPayload, PackDeletedPayload, PackRegisteredPayload, PackTestRequestedPayload,
    PermissionSetChangedPayload,
    RuleCreatedPayload, RuleDeletedPayload, RuleDisabledPayload, RuleEnabledPayload,
    RuntimeChangedPayload, TriggerChangedPayload,
};
pub use publisher::{Publisher, PublisherConfig};

use serde::{Deserialize, Serialize};
use std::fmt;

/// Message delivery mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    /// Non-persistent messages (faster, but may be lost on broker restart)
    NonPersistent = 1,
    /// Persistent messages (slower, but survive broker restart)
    #[default]
    Persistent = 2,
}

/// Message priority (0-9, higher is more urgent)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Priority(u8);

impl Priority {
    /// Lowest priority
    pub const MIN: Priority = Priority(0);
    /// Normal priority
    pub const NORMAL: Priority = Priority(5);
    /// Highest priority
    pub const MAX: Priority = Priority(9);

    /// Create a new priority level (clamped to 0-9)
    pub fn new(value: u8) -> Self {
        Self(value.min(9))
    }

    /// Get the priority value
    pub fn value(&self) -> u8 {
        self.0
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl From<u8> for Priority {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Message acknowledgment mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AckMode {
    /// Automatically acknowledge messages after delivery
    Auto,
    /// Manually acknowledge messages after processing
    #[default]
    Manual,
}

/// Exchange type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeType {
    /// Direct exchange - routes messages with exact routing key match
    #[default]
    Direct,
    /// Topic exchange - routes messages using pattern matching
    Topic,
    /// Fanout exchange - routes messages to all bound queues
    Fanout,
    /// Headers exchange - routes based on message headers
    Headers,
}

impl ExchangeType {
    /// Get the exchange type as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Topic => "topic",
            Self::Fanout => "fanout",
            Self::Headers => "headers",
        }
    }
}

impl fmt::Display for ExchangeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Well-known exchange names
pub mod exchanges {
    /// Events exchange for sensor-generated events
    pub const EVENTS: &str = "attune.events";
    /// Executions exchange for execution requests
    pub const EXECUTIONS: &str = "attune.executions";
    /// Notifications exchange for system notifications
    pub const NOTIFICATIONS: &str = "attune.notifications";
    /// Metadata exchange for cache invalidation/change notifications
    pub const METADATA: &str = "attune.metadata";
    /// Dead letter exchange for failed messages
    pub const DEAD_LETTER: &str = "attune.dlx";
}

/// Well-known queue names
pub mod queues {
    /// Event processing queue (sensor catch-all, bound with `#`)
    pub const EVENTS: &str = "attune.events.queue";
    /// Executor event processing queue (bound only to `event.created`)
    pub const EXECUTOR_EVENTS: &str = "attune.executor.events.queue";
    /// Execution request queue
    pub const EXECUTIONS: &str = "attune.executions.queue";
    /// Notification delivery queue
    pub const NOTIFICATIONS: &str = "attune.notifications.queue";
    /// Dead letter queue for events
    pub const EVENTS_DLQ: &str = "attune.events.dlq";
    /// Dead letter queue for executions
    pub const EXECUTIONS_DLQ: &str = "attune.executions.dlq";
    /// Dead letter queue for notifications
    pub const NOTIFICATIONS_DLQ: &str = "attune.notifications.dlq";
}

/// Well-known routing keys
pub mod routing_keys {
    /// Event created routing key
    pub const EVENT_CREATED: &str = "event.created";
    /// Execution requested routing key
    pub const EXECUTION_REQUESTED: &str = "execution.requested";
    /// Execution status changed routing key
    pub const EXECUTION_STATUS_CHANGED: &str = "execution.status.changed";
    /// Execution completed routing key
    pub const EXECUTION_COMPLETED: &str = "execution.completed";
    /// Inquiry created routing key
    pub const INQUIRY_CREATED: &str = "inquiry.created";
    /// Inquiry responded routing key
    pub const INQUIRY_RESPONDED: &str = "inquiry.responded";
    /// Notification created routing key
    pub const NOTIFICATION_CREATED: &str = "notification.created";
    /// Pack registered routing key
    pub const PACK_REGISTERED: &str = "pack.registered";
    /// Action metadata changed routing key
    pub const METADATA_ACTION_CHANGED: &str = "metadata.action.changed";
    /// Trigger metadata changed routing key
    pub const METADATA_TRIGGER_CHANGED: &str = "metadata.trigger.changed";
    /// Runtime metadata changed routing key
    pub const METADATA_RUNTIME_CHANGED: &str = "metadata.runtime.changed";
    /// Pack metadata changed routing key
    pub const METADATA_PACK_CHANGED: &str = "metadata.pack.changed";
    /// Permission set metadata changed routing key
    pub const METADATA_PERMISSION_SET_CHANGED: &str = "metadata.permission_set.changed";
    /// Identity authorization metadata changed routing key
    pub const METADATA_IDENTITY_AUTHORIZATION_CHANGED: &str =
        "metadata.identity_authorization.changed";
    /// Execution cancel requested routing key
    pub const EXECUTION_CANCEL: &str = "execution.cancel";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_clamping() {
        assert_eq!(Priority::new(15).value(), 9);
        assert_eq!(Priority::new(5).value(), 5);
        assert_eq!(Priority::new(0).value(), 0);
    }

    #[test]
    fn test_priority_constants() {
        assert_eq!(Priority::MIN.value(), 0);
        assert_eq!(Priority::NORMAL.value(), 5);
        assert_eq!(Priority::MAX.value(), 9);
    }

    #[test]
    fn test_exchange_type_string() {
        assert_eq!(ExchangeType::Direct.as_str(), "direct");
        assert_eq!(ExchangeType::Topic.as_str(), "topic");
        assert_eq!(ExchangeType::Fanout.as_str(), "fanout");
        assert_eq!(ExchangeType::Headers.as_str(), "headers");
    }

    #[test]
    fn test_delivery_mode_default() {
        assert_eq!(DeliveryMode::default(), DeliveryMode::Persistent);
    }

    #[test]
    fn test_ack_mode_default() {
        assert_eq!(AckMode::default(), AckMode::Manual);
    }
}
