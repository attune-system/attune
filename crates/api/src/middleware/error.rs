//! Error handling middleware and response types

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// Standard API error response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error message
    pub error: String,
    /// Optional error code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ErrorResponse {
    /// Create a new error response
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: None,
            details: None,
        }
    }

    /// Set error code
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Set error details
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// API error type that can be converted to HTTP responses
#[derive(Debug)]
pub enum ApiError {
    /// Bad request (400)
    BadRequest(String),
    /// Unauthorized (401)
    Unauthorized(String),
    /// Forbidden (403)
    Forbidden(String),
    /// Not found (404)
    NotFound(String),
    /// Conflict (409)
    Conflict(String),
    /// Unprocessable entity (422)
    UnprocessableEntity(String),
    /// Too many requests (429)
    TooManyRequests(String),
    /// Internal server error (500)
    InternalServerError(String),
    /// Not implemented (501)
    NotImplemented(String),
    /// Retryable database error (503)
    RetryableDatabaseError,
    /// Database error
    DatabaseError(String),
    /// Validation error
    ValidationError(String),
}

impl ApiError {
    /// Get the HTTP status code for this error
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::ValidationError(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            ApiError::RetryableDatabaseError => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::InternalServerError(_) | ApiError::DatabaseError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Get the error message
    pub fn message(&self) -> &str {
        match self {
            ApiError::BadRequest(msg)
            | ApiError::Unauthorized(msg)
            | ApiError::Forbidden(msg)
            | ApiError::NotFound(msg)
            | ApiError::Conflict(msg)
            | ApiError::UnprocessableEntity(msg)
            | ApiError::TooManyRequests(msg)
            | ApiError::NotImplemented(msg)
            | ApiError::InternalServerError(msg)
            | ApiError::DatabaseError(msg)
            | ApiError::ValidationError(msg) => msg,
            ApiError::RetryableDatabaseError => {
                "Database operation temporarily unavailable; please retry"
            }
        }
    }

    /// Get the error code
    pub fn code(&self) -> &str {
        match self {
            ApiError::BadRequest(_) => "BAD_REQUEST",
            ApiError::Unauthorized(_) => "UNAUTHORIZED",
            ApiError::Forbidden(_) => "FORBIDDEN",
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::Conflict(_) => "CONFLICT",
            ApiError::UnprocessableEntity(_) => "UNPROCESSABLE_ENTITY",
            ApiError::TooManyRequests(_) => "TOO_MANY_REQUESTS",
            ApiError::NotImplemented(_) => "NOT_IMPLEMENTED",
            ApiError::ValidationError(_) => "VALIDATION_ERROR",
            ApiError::RetryableDatabaseError => "RETRYABLE_DATABASE_ERROR",
            ApiError::DatabaseError(_) => "DATABASE_ERROR",
            ApiError::InternalServerError(_) => "INTERNAL_SERVER_ERROR",
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_response = ErrorResponse::new(self.message()).with_code(self.code());

        (status, Json(error_response)).into_response()
    }
}

// Convert from common error types
impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => ApiError::NotFound("Resource not found".to_string()),
            sqlx::Error::Database(db_err) => {
                // PostgreSQL error codes:
                //   23505 = unique_violation    → 409 Conflict
                //   23503 = foreign_key_violation → 422 Unprocessable Entity
                //   23514 = check_violation     → 422 Unprocessable Entity
                //   40001 = serialization_failure → 503 Service Unavailable
                //   55P03 = lock_not_available  → 503 Service Unavailable
                //   P0001 = raise_exception     → 400 Bad Request (trigger-raised errors)
                let pg_code = db_err.code().map(|c| c.to_string()).unwrap_or_default();
                if matches!(pg_code.as_str(), "40001" | "55P03") {
                    ApiError::RetryableDatabaseError
                } else if pg_code == "23505" {
                    // Unique constraint violation — duplicate key
                    let detail = db_err
                        .constraint()
                        .map(|c| format!(" ({})", c))
                        .unwrap_or_default();
                    ApiError::Conflict(format!("Already exists{}", detail))
                } else if pg_code == "23503" {
                    // Foreign key violation — the referenced row doesn't exist
                    let detail = db_err
                        .constraint()
                        .map(|c| format!(" ({})", c))
                        .unwrap_or_default();
                    ApiError::UnprocessableEntity(format!(
                        "Referenced entity does not exist{}",
                        detail
                    ))
                } else if pg_code == "23514" {
                    // CHECK constraint violation — value doesn't meet constraint
                    let detail = db_err
                        .constraint()
                        .map(|c| format!(": {}", c))
                        .unwrap_or_default();
                    ApiError::UnprocessableEntity(format!("Validation constraint failed{}", detail))
                } else if pg_code == "P0001" {
                    // RAISE EXCEPTION from a trigger or function
                    // Extract the human-readable message from the exception
                    let msg = db_err.message().to_string();
                    ApiError::BadRequest(msg)
                } else if let Some(constraint) = db_err.constraint() {
                    ApiError::Conflict(format!("Constraint violation: {}", constraint))
                } else {
                    ApiError::DatabaseError(format!("Database error: {}", db_err))
                }
            }
            _ => ApiError::DatabaseError(format!("Database error: {}", err)),
        }
    }
}

impl From<attune_common::error::Error> for ApiError {
    fn from(err: attune_common::error::Error) -> Self {
        match err {
            attune_common::error::Error::NotFound {
                entity,
                field,
                value,
            } => ApiError::NotFound(format!("{} with {}={} not found", entity, field, value)),
            attune_common::error::Error::AlreadyExists {
                entity,
                field,
                value,
            } => ApiError::Conflict(format!(
                "{} with {}={} already exists",
                entity, field, value
            )),
            attune_common::error::Error::Validation(msg) => ApiError::BadRequest(msg),
            attune_common::error::Error::SchemaValidation(msg) => ApiError::BadRequest(msg),
            attune_common::error::Error::Database(err) => ApiError::from(err),
            attune_common::error::Error::InvalidState(msg) => ApiError::BadRequest(msg),
            // A pinned cache snapshot vanished/expired; the caller should
            // restart the scan against the current active generation.
            attune_common::error::Error::CacheSnapshotExpired(msg) => ApiError::Conflict(msg),
            // Duplicate external identifiers conflict with generation uniqueness.
            attune_common::error::Error::CacheDuplicateExternalId => ApiError::Conflict(
                "cache ingest contains duplicate external identifiers".to_string(),
            ),
            attune_common::error::Error::CacheQuotaExceeded { message, .. } => {
                ApiError::Conflict(message.to_string())
            }
            attune_common::error::Error::PermissionDenied(msg) => ApiError::Forbidden(msg),
            attune_common::error::Error::AuthenticationFailed(msg) => ApiError::Unauthorized(msg),
            attune_common::error::Error::Configuration(msg) => ApiError::InternalServerError(msg),
            attune_common::error::Error::Serialization(err) => {
                ApiError::InternalServerError(format!("{}", err))
            }
            attune_common::error::Error::Io(msg)
            | attune_common::error::Error::Encryption(msg)
            | attune_common::error::Error::Timeout(msg)
            | attune_common::error::Error::ExternalService(msg)
            | attune_common::error::Error::Worker(msg)
            | attune_common::error::Error::Execution(msg)
            | attune_common::error::Error::Internal(msg) => ApiError::InternalServerError(msg),
            attune_common::error::Error::Other(err) => {
                ApiError::InternalServerError(format!("{}", err))
            }
        }
    }
}

impl From<validator::ValidationErrors> for ApiError {
    fn from(err: validator::ValidationErrors) -> Self {
        ApiError::ValidationError(format!("Validation failed: {}", err))
    }
}

impl From<crate::auth::jwt::JwtError> for ApiError {
    fn from(err: crate::auth::jwt::JwtError) -> Self {
        match err {
            crate::auth::jwt::JwtError::Expired => {
                ApiError::Unauthorized("Token has expired".to_string())
            }
            crate::auth::jwt::JwtError::Invalid => {
                ApiError::Unauthorized("Invalid token".to_string())
            }
            crate::auth::jwt::JwtError::EncodeError(msg) => {
                ApiError::InternalServerError(format!("Failed to encode token: {}", msg))
            }
            crate::auth::jwt::JwtError::DecodeError(msg) => {
                ApiError::Unauthorized(format!("Failed to decode token: {}", msg))
            }
        }
    }
}

impl From<crate::auth::password::PasswordError> for ApiError {
    fn from(err: crate::auth::password::PasswordError) -> Self {
        match err {
            crate::auth::password::PasswordError::HashError(msg) => {
                ApiError::InternalServerError(format!("Failed to hash password: {}", msg))
            }
            crate::auth::password::PasswordError::VerifyError(msg) => {
                ApiError::InternalServerError(format!("Failed to verify password: {}", msg))
            }
            crate::auth::password::PasswordError::InvalidHash => {
                ApiError::InternalServerError("Invalid password hash format".to_string())
            }
        }
    }
}

impl From<std::num::ParseIntError> for ApiError {
    fn from(err: std::num::ParseIntError) -> Self {
        ApiError::BadRequest(format!("Invalid number format: {}", err))
    }
}

/// Result type alias for API handlers
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::{borrow::Cow, error::Error, fmt};

    #[derive(Debug)]
    struct TestDatabaseError {
        code: &'static str,
    }

    impl fmt::Display for TestDatabaseError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("sensitive database detail")
        }
    }

    impl Error for TestDatabaseError {}

    impl sqlx::error::DatabaseError for TestDatabaseError {
        fn message(&self) -> &str {
            "sensitive database detail"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }

        fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    #[tokio::test]
    async fn retryable_database_error_has_stable_response() {
        let response = ApiError::RetryableDatabaseError.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            error.error,
            "Database operation temporarily unavailable; please retry"
        );
        assert_eq!(error.code.as_deref(), Some("RETRYABLE_DATABASE_ERROR"));
        assert_eq!(error.details, None);
    }

    #[test]
    fn retryable_postgres_codes_map_without_database_details() {
        for code in ["55P03", "40001"] {
            let error = ApiError::from(sqlx::Error::Database(Box::new(TestDatabaseError { code })));

            assert!(matches!(error, ApiError::RetryableDatabaseError));
            assert_eq!(error.status_code(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(error.code(), "RETRYABLE_DATABASE_ERROR");
            assert_eq!(
                error.message(),
                "Database operation temporarily unavailable; please retry"
            );
            assert!(!error.message().contains("sensitive database detail"));
        }
    }
}
