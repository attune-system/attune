//! Retry Manager
//!
//! This module provides intelligent retry logic for failed executions.
//! It determines whether failures are retriable, manages retry attempts,
//! and implements exponential backoff for retry scheduling.
//!
//! # Retry Strategy
//!
//! - **Retriable Failures:** Worker unavailability, timeouts, transient errors
//! - **Non-Retriable Failures:** Validation errors, missing actions, permission errors
//! - **Backoff:** Exponential with jitter (1s, 2s, 4s, 8s, ...)
//! - **Max Retries:** Configurable per action (default: 0, no retries)

use attune_common::{
    error::{Error, Result},
    models::{Execution, ExecutionStatus, Id},
    repositories::{
        execution::{CreateExecutionInput, UpdateExecutionInput},
        execution_secret_value::ExecutionSecretValueRepository,
        ExecutionRepository, FindById, Update,
    },
    secret_values::ENTITY_EXECUTION_CONFIG,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{debug, info};

/// Retry manager for execution failures
pub struct RetryManager {
    /// Database connection pool
    pool: PgPool,
    /// Retry configuration
    config: RetryConfig,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Enable automatic retries
    pub enabled: bool,
    /// Base backoff duration in seconds
    pub base_backoff_secs: u64,
    /// Maximum backoff duration in seconds
    pub max_backoff_secs: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
    /// Add jitter to backoff (0.0 - 1.0)
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_backoff_secs: 1,
            max_backoff_secs: 300, // 5 minutes
            backoff_multiplier: 2.0,
            jitter_factor: 0.2, // 20% jitter
        }
    }
}

/// Reason for retry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryReason {
    /// Worker was unavailable
    WorkerUnavailable,
    /// Execution timed out in queue
    QueueTimeout,
    /// Worker heartbeat became stale
    WorkerHeartbeatStale,
    /// Transient error in execution
    TransientError,
    /// Manual retry requested by user
    ManualRetry,
    /// Unknown/other reason
    Unknown,
}

impl RetryReason {
    /// Get string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorkerUnavailable => "worker_unavailable",
            Self::QueueTimeout => "queue_timeout",
            Self::WorkerHeartbeatStale => "worker_heartbeat_stale",
            Self::TransientError => "transient_error",
            Self::ManualRetry => "manual_retry",
            Self::Unknown => "unknown",
        }
    }

    /// Detect retry reason from execution error
    pub fn from_error(error: &str) -> Self {
        let error_lower = error.to_lowercase();

        if error_lower.contains("worker queue ttl expired")
            || error_lower.contains("worker unavailable")
        {
            Self::WorkerUnavailable
        } else if error_lower.contains("timeout") || error_lower.contains("timed out") {
            Self::QueueTimeout
        } else if error_lower.contains("heartbeat") || error_lower.contains("stale") {
            Self::WorkerHeartbeatStale
        } else if error_lower.contains("transient")
            || error_lower.contains("temporary")
            || error_lower.contains("connection")
        {
            Self::TransientError
        } else {
            Self::Unknown
        }
    }
}

impl std::fmt::Display for RetryReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Result of retry analysis
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RetryAnalysis {
    /// Whether the execution should be retried
    pub should_retry: bool,
    /// Reason for retry decision
    pub reason: Option<RetryReason>,
    /// Suggested backoff delay
    pub backoff_delay: Option<Duration>,
    /// Current retry attempt (0-based)
    pub retry_count: i32,
    /// Maximum retry attempts allowed
    pub max_retries: i32,
}

impl RetryManager {
    /// Create a new retry manager
    #[allow(dead_code)]
    pub fn new(pool: PgPool, config: RetryConfig) -> Self {
        Self { pool, config }
    }

    /// Create with default configuration
    #[allow(dead_code)]
    pub fn with_defaults(pool: PgPool) -> Self {
        Self::new(pool, RetryConfig::default())
    }

    /// Analyze if an execution should be retried
    #[allow(dead_code)]
    pub async fn analyze_execution(&self, execution_id: Id) -> Result<RetryAnalysis> {
        // Fetch execution
        let execution = ExecutionRepository::find_by_id(&self.pool, execution_id)
            .await?
            .ok_or_else(|| Error::not_found("Execution", "id", execution_id.to_string()))?;

        // Check if retries are enabled globally
        if !self.config.enabled {
            return Ok(RetryAnalysis {
                should_retry: false,
                reason: None,
                backoff_delay: None,
                retry_count: execution.retry_count,
                max_retries: 0,
            });
        }

        // Only retry failed executions
        if execution.status != ExecutionStatus::Failed {
            return Ok(RetryAnalysis {
                should_retry: false,
                reason: None,
                backoff_delay: None,
                retry_count: 0,
                max_retries: 0,
            });
        }

        let retry_count = execution.retry_count;
        let max_retries = execution.max_retries.unwrap_or(0);

        // Check if retries are exhausted
        if max_retries == 0 || retry_count >= max_retries {
            debug!(
                "Execution {} retry limit reached: {}/{}",
                execution_id, retry_count, max_retries
            );
            return Ok(RetryAnalysis {
                should_retry: false,
                reason: None,
                backoff_delay: None,
                retry_count,
                max_retries,
            });
        }

        // Determine if failure is retriable
        let retry_reason = self.detect_retry_reason(&execution);
        let is_retriable = self.is_failure_retriable(&execution, retry_reason);

        if !is_retriable {
            debug!(
                "Execution {} failure is not retriable: {:?}",
                execution_id, retry_reason
            );
            return Ok(RetryAnalysis {
                should_retry: false,
                reason: Some(retry_reason),
                backoff_delay: None,
                retry_count,
                max_retries,
            });
        }

        // Calculate backoff delay
        let backoff_delay = self.calculate_backoff(retry_count);

        info!(
            "Execution {} should be retried: attempt {}/{}, reason: {:?}, delay: {:?}",
            execution_id,
            retry_count + 1,
            max_retries,
            retry_reason,
            backoff_delay
        );

        Ok(RetryAnalysis {
            should_retry: true,
            reason: Some(retry_reason),
            backoff_delay: Some(backoff_delay),
            retry_count,
            max_retries,
        })
    }

    /// Create a retry execution from a failed execution
    #[allow(dead_code)]
    pub async fn create_retry_execution(
        &self,
        execution_id: Id,
        reason: RetryReason,
    ) -> Result<Execution> {
        // Fetch original execution
        let original = ExecutionRepository::find_by_id(&self.pool, execution_id)
            .await?
            .ok_or_else(|| Error::not_found("Execution", "id", execution_id.to_string()))?;
        if !matches!(
            original.status,
            ExecutionStatus::Failed | ExecutionStatus::Timeout
        ) {
            return Err(Error::validation(
                "Only failed or timed-out executions can admit retries",
            ));
        }

        let retry_count = original.retry_count;
        let max_retries = original.max_retries.unwrap_or(0);
        let original_execution_id = original.original_execution.unwrap_or(execution_id);

        // Create new execution (reusing original parameters)
        let retry_execution = CreateExecutionInput {
            action: original.action,
            action_ref: original.action_ref.clone(),
            config: original.config.clone(),
            env_vars: original.env_vars.clone(),
            parent: original.parent,
            enforcement: original.enforcement,
            // Preserve the original triggering identity so the retried
            // execution is minted a callback token with the same security
            // context as the original.
            executor: original.executor,
            permission_set_refs: original.permission_set_refs.clone(),
            artifact_retention_policy: original.artifact_retention_policy,
            artifact_retention_limit: original.artifact_retention_limit,
            worker_selector: original.worker_selector.clone(),
            worker_tolerations: original.worker_tolerations.clone(),
            worker_affinity: original.worker_affinity.clone(),
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: original.trace_tag.clone(),
            timeout_seconds: original.timeout_seconds,
            result: None,
            workflow_task: original.workflow_task.clone(),
        };

        let snapshot = original.executable_snapshot.as_ref().ok_or_else(|| {
            Error::validation(format!(
                "Execution {} has no executable snapshot for retry",
                original.id
            ))
        })?;
        let mut transaction = self.pool.begin().await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
            &mut transaction,
        )
        .await?;
        let binding = attune_common::repositories::schema_contract::SchemaContractRepository::find(
            &mut transaction,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            original.id,
        )
        .await?
        .ok_or_else(|| Error::validation("Retry source is missing its schema binding"))?;
        let admission = if !snapshot.executable.action.is_adhoc
            || attune_common::repositories::execution_admission::ExecutionAdmissionRepository::find(
                &mut transaction,
                attune_common::repositories::runtime_authority::RuntimeOwner::Execution(
                    original.id,
                ),
            )
            .await?
            .is_some()
        {
            Some(attune_common::repositories::execution_admission::ExecutionAdmissionRepository::inherit(&mut transaction, attune_common::repositories::runtime_authority::RuntimeOwner::Execution(original.id), snapshot).await?)
        } else {
            None
        };
        let created = ExecutionRepository::create_retry(
            &mut *transaction,
            retry_execution,
            snapshot,
            retry_count + 1,
            Some(max_retries),
            Some(reason.as_str().to_string()),
            original_execution_id,
        )
        .await?;
        if let Some(admission) = admission {
            admission
                .attach(
                    &mut transaction,
                    attune_common::repositories::runtime_authority::RuntimeOwner::Execution(
                        created.id,
                    ),
                )
                .await?;
        }
        attune_common::repositories::schema_contract::SchemaContractRepository::attach(
            &mut transaction,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            created.id,
            &binding,
        )
        .await?;
        attune_common::repositories::rule_authority::RuleAuthorityRepository::copy_execution(
            &mut transaction,
            original.id,
            created.id,
        )
        .await?;
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::copy(&mut transaction, "execution", original.id, "execution", created.id).await?;
        ExecutionSecretValueRepository::copy_entity(
            &mut *transaction,
            ENTITY_EXECUTION_CONFIG,
            original.id,
            ENTITY_EXECUTION_CONFIG,
            created.id,
        )
        .await?;
        transaction.commit().await?;

        info!(
            "Created retry execution {} for original {} (attempt {}/{})",
            created.id,
            execution_id,
            retry_count + 1,
            max_retries
        );

        Ok(created)
    }

    /// Detect retry reason from execution
    fn detect_retry_reason(&self, execution: &Execution) -> RetryReason {
        if let Some(result) = &execution.result {
            if let Some(error) = result.get("error").and_then(|e| e.as_str()) {
                return RetryReason::from_error(error);
            }
            if let Some(message) = result.get("message").and_then(|m| m.as_str()) {
                return RetryReason::from_error(message);
            }
        }
        RetryReason::Unknown
    }

    /// Check if failure is retriable
    fn is_failure_retriable(&self, _execution: &Execution, reason: RetryReason) -> bool {
        match reason {
            // These are retriable
            RetryReason::WorkerUnavailable => true,
            RetryReason::QueueTimeout => true,
            RetryReason::WorkerHeartbeatStale => true,
            RetryReason::TransientError => true,
            RetryReason::ManualRetry => true,
            // Unknown failures are not automatically retried
            RetryReason::Unknown => false,
        }
    }

    /// Calculate exponential backoff with jitter
    fn calculate_backoff(&self, retry_count: i32) -> Duration {
        calculate_backoff_duration(&self.config, retry_count)
    }

    /// Update execution with retry metadata
    #[allow(dead_code)]
    pub async fn mark_as_retry(
        &self,
        execution_id: Id,
        original_execution_id: Id,
        retry_count: i32,
        reason: RetryReason,
    ) -> Result<()> {
        let mut config = json!({
            "retry_count": retry_count,
            "original_execution": original_execution_id,
            "retry_reason": reason.as_str(),
            "retry_at": Utc::now().to_rfc3339(),
        });

        // Fetch current config and merge
        if let Some(execution) = ExecutionRepository::find_by_id(&self.pool, execution_id).await? {
            if let Some(existing_config) = execution.config {
                if let Some(obj) = config.as_object_mut() {
                    if let Some(existing_obj) = existing_config.as_object() {
                        for (k, v) in existing_obj {
                            obj.entry(k).or_insert(v.clone());
                        }
                    }
                }
            }
        }

        ExecutionRepository::update(
            &self.pool,
            execution_id,
            UpdateExecutionInput {
                ..Default::default()
            },
        )
        .await?;

        Ok(())
    }
}

/// Calculate exponential backoff with jitter from a retry config.
///
/// Extracted as a free function so it can be tested without a database pool.
fn calculate_backoff_duration(config: &RetryConfig, retry_count: i32) -> Duration {
    let base_secs = config.base_backoff_secs as f64;
    let multiplier = config.backoff_multiplier;
    let max_secs = config.max_backoff_secs as f64;
    let jitter_factor = config.jitter_factor;

    // Calculate exponential backoff: base * multiplier^retry_count
    let backoff_secs = base_secs * multiplier.powi(retry_count);

    // Cap at max
    let backoff_secs = backoff_secs.min(max_secs);

    // Add jitter: random value between (1 - jitter) and (1 + jitter)
    let jitter = 1.0 + (rand::random::<f64>() * 2.0 - 1.0) * jitter_factor;
    let backoff_with_jitter = backoff_secs * jitter;

    Duration::from_secs(backoff_with_jitter.max(0.0) as u64)
}

/// Check if an error message indicates a retriable failure
#[allow(dead_code)]
pub fn is_error_retriable(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();

    // Retriable patterns
    error_lower.contains("worker queue ttl expired")
        || error_lower.contains("worker unavailable")
        || error_lower.contains("timeout")
        || error_lower.contains("timed out")
        || error_lower.contains("heartbeat")
        || error_lower.contains("stale")
        || error_lower.contains("transient")
        || error_lower.contains("temporary")
        || error_lower.contains("connection refused")
        || error_lower.contains("connection reset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_reason_detection() {
        assert_eq!(
            RetryReason::from_error("Worker queue TTL expired"),
            RetryReason::WorkerUnavailable
        );
        assert_eq!(
            RetryReason::from_error("Execution timed out"),
            RetryReason::QueueTimeout
        );
        assert_eq!(
            RetryReason::from_error("Worker heartbeat is stale"),
            RetryReason::WorkerHeartbeatStale
        );
        assert_eq!(
            RetryReason::from_error("Transient connection error"),
            RetryReason::TransientError
        );
        assert_eq!(
            RetryReason::from_error("Invalid parameter format"),
            RetryReason::Unknown
        );
    }

    #[test]
    fn test_is_error_retriable() {
        assert!(is_error_retriable("Worker queue TTL expired"));
        assert!(is_error_retriable("Execution timed out"));
        assert!(is_error_retriable("Worker heartbeat stale"));
        assert!(is_error_retriable("Transient error"));
        assert!(!is_error_retriable("Invalid parameter"));
        assert!(!is_error_retriable("Permission denied"));
    }

    #[test]
    fn test_backoff_calculation() {
        let config = RetryConfig::default();

        let backoff0 = calculate_backoff_duration(&config, 0);
        let backoff1 = calculate_backoff_duration(&config, 1);
        let backoff2 = calculate_backoff_duration(&config, 2);

        // First attempt: ~1s (with jitter 0..2s)
        assert!(backoff0.as_secs() <= 2);
        // Second attempt: ~2s
        assert!(backoff1.as_secs() >= 1 && backoff1.as_secs() <= 3);
        // Third attempt: ~4s
        assert!(backoff2.as_secs() >= 2 && backoff2.as_secs() <= 6);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.base_backoff_secs, 1);
        assert_eq!(config.max_backoff_secs, 300);
        assert_eq!(config.backoff_multiplier, 2.0);
        assert_eq!(config.jitter_factor, 0.2);
    }
}
