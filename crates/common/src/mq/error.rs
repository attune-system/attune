//! Message Queue Error Types

use thiserror::Error;

/// Result type for message queue operations
pub type MqResult<T> = Result<T, MqError>;

/// Message queue error types
#[derive(Error, Debug)]
pub enum MqError {
    /// Connection error
    #[error("Connection error: {0}")]
    Connection(String),

    /// Channel error
    #[error("Channel error: {0}")]
    Channel(String),

    /// Publishing error
    #[error("Publishing error: {0}")]
    Publish(String),

    /// Consumption error
    #[error("Consumption error: {0}")]
    Consume(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Deserialization error
    #[error("Deserialization error: {0}")]
    Deserialization(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Exchange declaration error
    #[error("Exchange declaration error: {0}")]
    ExchangeDeclaration(String),

    /// Queue declaration error
    #[error("Queue declaration error: {0}")]
    QueueDeclaration(String),

    /// Queue binding error
    #[error("Queue binding error: {0}")]
    QueueBinding(String),

    /// Acknowledgment error
    #[error("Acknowledgment error: {0}")]
    Acknowledgment(String),

    /// Rejection error
    #[error("Rejection error: {0}")]
    Rejection(String),

    /// Timeout error
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Invalid message format
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// Connection pool error
    #[error("Connection pool error: {0}")]
    Pool(String),

    /// Replica-local cleanup failed and should be retried.
    #[error("Cleanup error: {0}")]
    Cleanup(String),

    /// Dead letter queue error
    #[error("Dead letter queue error: {0}")]
    DeadLetterQueue(String),

    /// Consumer cancelled
    #[error("Consumer was cancelled: {0}")]
    ConsumerCancelled(String),

    /// Message not found
    #[error("Message not found: {0}")]
    NotFound(String),

    /// Lapin (RabbitMQ client) error
    #[error("RabbitMQ error: {0}")]
    Lapin(#[from] lapin::Error),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic error
    #[error("Message queue error: {0}")]
    Other(String),
}

impl MqError {
    /// Check if error is retriable
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            MqError::Connection(_)
                | MqError::Channel(_)
                | MqError::Publish(_)
                | MqError::Timeout(_)
                | MqError::Pool(_)
                | MqError::Cleanup(_)
                | MqError::Lapin(_)
        )
    }

    /// Check if error is a connection issue
    pub fn is_connection_error(&self) -> bool {
        matches!(self, MqError::Connection(_) | MqError::Pool(_))
    }

    /// Check if error is a serialization issue
    pub fn is_serialization_error(&self) -> bool {
        matches!(
            self,
            MqError::Serialization(_) | MqError::Deserialization(_) | MqError::Json(_)
        )
    }
}

impl From<String> for MqError {
    fn from(s: String) -> Self {
        MqError::Other(s)
    }
}

impl From<&str> for MqError {
    fn from(s: &str) -> Self {
        MqError::Other(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = MqError::Connection("Failed to connect".to_string());
        assert_eq!(err.to_string(), "Connection error: Failed to connect");
    }

    #[test]
    fn test_is_retriable() {
        assert!(MqError::Connection("test".to_string()).is_retriable());
        assert!(MqError::Timeout("test".to_string()).is_retriable());
        assert!(MqError::Cleanup("test".to_string()).is_retriable());
        assert!(!MqError::Config("test".to_string()).is_retriable());
    }

    #[test]
    fn test_is_connection_error() {
        assert!(MqError::Connection("test".to_string()).is_connection_error());
        assert!(MqError::Pool("test".to_string()).is_connection_error());
        assert!(!MqError::Serialization("test".to_string()).is_connection_error());
    }

    #[test]
    fn test_is_serialization_error() {
        assert!(MqError::Serialization("test".to_string()).is_serialization_error());
        assert!(MqError::Deserialization("test".to_string()).is_serialization_error());
        assert!(!MqError::Connection("test".to_string()).is_serialization_error());
    }

    #[test]
    fn test_from_string() {
        let err: MqError = "test error".into();
        assert_eq!(err.to_string(), "Message queue error: test error");
    }
}
