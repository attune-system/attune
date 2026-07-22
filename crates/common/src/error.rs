//! Error types for Attune services
//!
//! This module provides a unified error handling approach across all services.

use thiserror::Error;

use crate::mq::MqError;

/// Result type alias using Attune's Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for Attune services
#[derive(Debug, Error)]
pub enum Error {
    /// Database errors
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Serialization/deserialization errors
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(String),

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Not found errors
    #[error("Not found: {entity} with {field}={value}")]
    NotFound {
        entity: String,
        field: String,
        value: String,
    },

    /// Already exists errors
    #[error("Already exists: {entity} with {field}={value}")]
    AlreadyExists {
        entity: String,
        field: String,
        value: String,
    },

    /// Invalid state errors
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// A pinned cache snapshot generation is no longer readable.
    ///
    /// Returned when a scan targets a generation that has expired, been
    /// removed, been tombstoned, or does not belong to the requested
    /// namespace. It is distinct from an empty page so callers can restart
    /// from the current active generation instead of treating it as
    /// end-of-data.
    #[error("Cache snapshot expired: {0}")]
    CacheSnapshotExpired(String),

    /// A cache ingest chunk contains external identifiers that already exist
    /// in the target generation. Raw identifiers are intentionally omitted to
    /// avoid leaking caller data.
    #[error("Cache ingest contains duplicate external identifiers")]
    CacheDuplicateExternalId,

    /// Permission denied errors
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Authentication errors
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Encryption/decryption errors
    #[error("Encryption error: {0}")]
    Encryption(String),

    /// Timeout errors
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// External service errors
    #[error("External service error: {0}")]
    ExternalService(String),

    /// Worker errors
    #[error("Worker error: {0}")]
    Worker(String),

    /// Execution errors
    #[error("Execution error: {0}")]
    Execution(String),

    /// Schema validation errors
    #[error("Schema validation error: {0}")]
    SchemaValidation(String),

    /// Generic internal errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// Wrapped anyhow errors for compatibility
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Create a NotFound error
    pub fn not_found(
        entity: impl Into<String>,
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::NotFound {
            entity: entity.into(),
            field: field.into(),
            value: value.into(),
        }
    }

    /// Create an AlreadyExists error
    pub fn already_exists(
        entity: impl Into<String>,
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::AlreadyExists {
            entity: entity.into(),
            field: field.into(),
            value: value.into(),
        }
    }

    /// Create a Validation error
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    /// Create an InvalidState error
    pub fn invalid_state(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }

    /// Create a CacheSnapshotExpired error
    pub fn cache_snapshot_expired(msg: impl Into<String>) -> Self {
        Self::CacheSnapshotExpired(msg.into())
    }

    /// Create a CacheDuplicateExternalId error
    pub fn cache_duplicate_external_id() -> Self {
        Self::CacheDuplicateExternalId
    }

    /// Create a PermissionDenied error
    pub fn permission_denied(msg: impl Into<String>) -> Self {
        Self::PermissionDenied(msg.into())
    }

    /// Create an AuthenticationFailed error
    pub fn authentication_failed(msg: impl Into<String>) -> Self {
        Self::AuthenticationFailed(msg.into())
    }

    /// Create a Configuration error
    pub fn configuration(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    /// Create an Encryption error
    pub fn encryption(msg: impl Into<String>) -> Self {
        Self::Encryption(msg.into())
    }

    /// Create a Timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Create an ExternalService error
    pub fn external_service(msg: impl Into<String>) -> Self {
        Self::ExternalService(msg.into())
    }

    /// Create a Worker error
    pub fn worker(msg: impl Into<String>) -> Self {
        Self::Worker(msg.into())
    }

    /// Create an Execution error
    pub fn execution(msg: impl Into<String>) -> Self {
        Self::Execution(msg.into())
    }

    /// Create a SchemaValidation error
    pub fn schema_validation(msg: impl Into<String>) -> Self {
        Self::SchemaValidation(msg.into())
    }

    /// Create an Internal error
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// Create an I/O error
    pub fn io(msg: impl Into<String>) -> Self {
        Self::Io(msg.into())
    }

    /// Check if this is a database error
    pub fn is_database(&self) -> bool {
        matches!(self, Self::Database(_))
    }

    /// Check if this is a not found error
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self,
            Self::AuthenticationFailed(_) | Self::PermissionDenied(_)
        )
    }
}

/// Convert MqError to Error
impl From<MqError> for Error {
    fn from(err: MqError) -> Self {
        Self::Internal(format!("Message queue error: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_found_error() {
        let err = Error::not_found("Pack", "ref", "mypack");
        assert!(err.is_not_found());
        assert_eq!(err.to_string(), "Not found: Pack with ref=mypack");
    }

    #[test]
    fn test_already_exists_error() {
        let err = Error::already_exists("Action", "ref", "myaction");
        assert_eq!(err.to_string(), "Already exists: Action with ref=myaction");
    }

    #[test]
    fn test_validation_error() {
        let err = Error::validation("Invalid input");
        assert_eq!(err.to_string(), "Validation error: Invalid input");
    }

    #[test]
    fn test_is_auth_error() {
        let err1 = Error::authentication_failed("Invalid token");
        assert!(err1.is_auth_error());

        let err2 = Error::permission_denied("No access");
        assert!(err2.is_auth_error());

        let err3 = Error::validation("Bad input");
        assert!(!err3.is_auth_error());
    }

    #[test]
    fn test_cache_snapshot_expired_error() {
        let err = Error::cache_snapshot_expired("generation 7 is no longer readable");
        assert!(matches!(err, Error::CacheSnapshotExpired(_)));
        assert_eq!(
            err.to_string(),
            "Cache snapshot expired: generation 7 is no longer readable"
        );
    }

    #[test]
    fn test_cache_duplicate_external_id_error() {
        let err = Error::cache_duplicate_external_id();
        assert!(matches!(err, Error::CacheDuplicateExternalId));
        // The message must never echo caller-supplied identifiers.
        assert_eq!(
            err.to_string(),
            "Cache ingest contains duplicate external identifiers"
        );
    }
}
