//! Execution Manager - Handles execution orchestration and lifecycle events
//!
//! This module is responsible for:
//! - Listening for ExecutionStatusChanged messages from workers
//! - Orchestrating workflow executions (parent-child relationships)
//! - Triggering child executions when parent completes
//! - Handling execution failures and retries
//!
//! ## Ownership Model
//!
//! The Executor owns execution state until it is scheduled to a worker.
//! After scheduling, the Worker owns the state and updates the database directly.
//!
//! - **Executor owns**: Requested → Scheduling → Scheduled
//! - **Worker owns**: Running → Completed/Failed/Cancelled/Timeout
//!
//! The ExecutionManager receives status change notifications for orchestration
//! purposes (e.g., triggering child executions) but does NOT update the database.

use anyhow::Result;
use attune_common::{
    models::{enums::ExecutionStatus, Execution},
    mq::{
        Consumer, ExecutionCancelRequestedPayload, ExecutionRequestedPayload,
        ExecutionStatusChangedPayload, MessageEnvelope, MessageType, Publisher,
    },
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        workflow::{
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        Create, FindById, Update,
    },
    workflow::{CancellationPolicy, WorkflowDefinition},
};

use sqlx::PgPool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Execution manager that handles lifecycle and status updates
pub struct ExecutionManager {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
}

impl ExecutionManager {
    /// Create a new execution manager
    pub fn new(pool: PgPool, publisher: Arc<Publisher>, consumer: Arc<Consumer>) -> Self {
        Self {
            pool,
            publisher,
            consumer,
        }
    }

    /// Start processing execution status messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting execution manager");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(
                move |envelope: MessageEnvelope<ExecutionStatusChangedPayload>| {
                    let pool = pool.clone();
                    let publisher = publisher.clone();

                    async move {
                        if let Err(e) =
                            Self::process_status_change(&pool, &publisher, &envelope).await
                        {
                            error!("Error processing status change: {}", e);
                            // Return error to trigger nack with requeue
                            return Err(format!("Failed to process status change: {}", e).into());
                        }
                        Ok(())
                    }
                },
            )
            .await?;

        Ok(())
    }

    /// Process an execution status change message
    ///
    /// NOTE: This method does NOT update the database. The worker is responsible
    /// for updating execution state after the execution is scheduled. The executor
    /// only handles orchestration logic (e.g., triggering workflow children).
    async fn process_status_change(
        pool: &PgPool,
        publisher: &Publisher,
        envelope: &MessageEnvelope<ExecutionStatusChangedPayload>,
    ) -> Result<()> {
        debug!("Processing execution status change: {:?}", envelope);

        let execution_id = envelope.payload.execution_id;
        let status_str = &envelope.payload.new_status;
        let status = Self::parse_execution_status(status_str)?;

        debug!(
            "Received status change notification for execution {}: {}",
            execution_id, status_str
        );

        // Fetch execution from database (for orchestration logic)
        let execution = ExecutionRepository::find_by_id(pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution not found: {}", execution_id))?;

        // Handle orchestration logic based on status
        // Note: Worker has already updated the database directly
        match status {
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled => {
                info!(
                    "Execution {} reached terminal state: {:?}, handling orchestration",
                    execution_id, status
                );
                if status == ExecutionStatus::Cancelled {
                    Self::handle_workflow_cancellation(pool, publisher, &execution).await?;
                }
                Self::handle_completion(pool, publisher, &execution).await?;
            }
            ExecutionStatus::Canceling => {
                debug!(
                    "Execution {} entered canceling state; checking for workflow child cancellation",
                    execution_id
                );
                Self::handle_workflow_cancellation(pool, publisher, &execution).await?;
            }
            ExecutionStatus::Running => {
                debug!(
                    "Execution {} now running (worker has updated DB)",
                    execution_id
                );
            }
            _ => {
                debug!(
                    "Execution {} status changed to {:?} (no orchestration needed)",
                    execution_id, status
                );
            }
        }

        Ok(())
    }

    async fn handle_workflow_cancellation(
        pool: &PgPool,
        publisher: &Publisher,
        execution: &Execution,
    ) -> Result<()> {
        let Some(_) = WorkflowExecutionRepository::find_by_execution(pool, execution.id).await?
        else {
            return Ok(());
        };

        let policy = Self::resolve_cancellation_policy(pool, execution.id).await;
        Self::cancel_workflow_children_with_policy(pool, publisher, execution.id, policy).await
    }

    async fn resolve_cancellation_policy(
        pool: &PgPool,
        parent_execution_id: i64,
    ) -> CancellationPolicy {
        let wf_exec =
            match WorkflowExecutionRepository::find_by_execution(pool, parent_execution_id).await {
                Ok(Some(wf)) => wf,
                _ => return CancellationPolicy::default(),
            };

        let wf_def =
            match WorkflowDefinitionRepository::find_by_id(pool, wf_exec.workflow_def).await {
                Ok(Some(def)) => def,
                _ => return CancellationPolicy::default(),
            };

        match serde_json::from_value::<WorkflowDefinition>(wf_def.definition) {
            Ok(def) => def.cancellation_policy,
            Err(e) => {
                warn!(
                    "Failed to deserialize workflow definition for workflow_def {}: {}. Falling back to default cancellation policy.",
                    wf_exec.workflow_def, e
                );
                CancellationPolicy::default()
            }
        }
    }

    async fn cancel_workflow_children_with_policy(
        pool: &PgPool,
        publisher: &Publisher,
        parent_execution_id: i64,
        policy: CancellationPolicy,
    ) -> Result<()> {
        let children: Vec<Execution> = sqlx::query_as::<_, Execution>(&format!(
            "SELECT {} FROM execution WHERE parent = $1 AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled', 'abandoned')",
            attune_common::repositories::execution::SELECT_COLUMNS
        ))
        .bind(parent_execution_id)
        .fetch_all(pool)
        .await?;

        if children.is_empty() {
            return Self::finalize_cancelled_workflow_if_idle(pool, parent_execution_id).await;
        }

        info!(
            "Executor cascading cancellation from workflow execution {} to {} child execution(s) with policy {:?}",
            parent_execution_id,
            children.len(),
            policy,
        );

        for child in &children {
            let child_id = child.id;

            if matches!(
                child.status,
                ExecutionStatus::Requested
                    | ExecutionStatus::Scheduling
                    | ExecutionStatus::Scheduled
            ) {
                let update = UpdateExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    result: Some(serde_json::json!({
                        "error": "Cancelled: parent workflow execution was cancelled"
                    })),
                    ..Default::default()
                };
                ExecutionRepository::update(pool, child_id, update).await?;
            } else if matches!(
                child.status,
                ExecutionStatus::Running | ExecutionStatus::Canceling
            ) {
                match policy {
                    CancellationPolicy::CancelRunning => {
                        if child.status != ExecutionStatus::Canceling {
                            let update = UpdateExecutionInput {
                                status: Some(ExecutionStatus::Canceling),
                                ..Default::default()
                            };
                            ExecutionRepository::update(pool, child_id, update).await?;
                        }

                        if let Some(worker_id) = child.worker {
                            Self::send_cancel_to_worker(publisher, child_id, worker_id).await?;
                        } else {
                            warn!(
                                "Workflow child execution {} is {:?} but has no assigned worker",
                                child_id, child.status
                            );
                        }
                    }
                    CancellationPolicy::AllowFinish => {
                        info!(
                            "AllowFinish policy: leaving running workflow child execution {} alone",
                            child_id
                        );
                    }
                }
            }

            Box::pin(Self::cancel_workflow_children_with_policy(
                pool, publisher, child_id, policy,
            ))
            .await?;
        }

        if let Some(wf_exec) =
            WorkflowExecutionRepository::find_by_execution(pool, parent_execution_id).await?
        {
            if !matches!(
                wf_exec.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
            ) {
                let wf_update = UpdateWorkflowExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    error_message: Some(
                        "Cancelled: parent workflow execution was cancelled".to_string(),
                    ),
                    current_tasks: Some(vec![]),
                    ..Default::default()
                };
                WorkflowExecutionRepository::update(pool, wf_exec.id, wf_update).await?;
            }
        }

        Self::finalize_cancelled_workflow_if_idle(pool, parent_execution_id).await
    }

    async fn finalize_cancelled_workflow_if_idle(
        pool: &PgPool,
        parent_execution_id: i64,
    ) -> Result<()> {
        let still_running: Vec<Execution> = sqlx::query_as::<_, Execution>(&format!(
            "SELECT {} FROM execution WHERE parent = $1 AND status IN ('running', 'canceling', 'scheduling', 'scheduled', 'requested')",
            attune_common::repositories::execution::SELECT_COLUMNS
        ))
        .bind(parent_execution_id)
        .fetch_all(pool)
        .await?;

        if still_running.is_empty() {
            let update = UpdateExecutionInput {
                status: Some(ExecutionStatus::Cancelled),
                result: Some(serde_json::json!({
                    "error": "Workflow cancelled",
                    "succeeded": false,
                })),
                ..Default::default()
            };
            let _ = ExecutionRepository::update(pool, parent_execution_id, update).await?;
        }

        Ok(())
    }

    async fn send_cancel_to_worker(
        publisher: &Publisher,
        execution_id: i64,
        worker_id: i64,
    ) -> Result<()> {
        let payload = ExecutionCancelRequestedPayload {
            execution_id,
            worker_id,
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionCancelRequested, payload)
            .with_source("executor-service")
            .with_correlation_id(uuid::Uuid::new_v4());

        publisher
            .publish_envelope_with_routing(
                &envelope,
                "attune.executions",
                &format!("execution.cancel.worker.{}", worker_id),
            )
            .await?;

        Ok(())
    }

    /// Parse execution status from string
    fn parse_execution_status(status: &str) -> Result<ExecutionStatus> {
        match status.to_lowercase().as_str() {
            "requested" => Ok(ExecutionStatus::Requested),
            "scheduling" => Ok(ExecutionStatus::Scheduling),
            "scheduled" => Ok(ExecutionStatus::Scheduled),
            "running" => Ok(ExecutionStatus::Running),
            "completed" => Ok(ExecutionStatus::Completed),
            "failed" => Ok(ExecutionStatus::Failed),
            "cancelled" | "canceled" => Ok(ExecutionStatus::Cancelled),
            "canceling" => Ok(ExecutionStatus::Canceling),
            "abandoned" => Ok(ExecutionStatus::Abandoned),
            "timeout" => Ok(ExecutionStatus::Timeout),
            _ => Err(anyhow::anyhow!("Invalid execution status: {}", status)),
        }
    }

    /// Handle execution completion (success, failure, or cancellation)
    async fn handle_completion(
        pool: &PgPool,
        publisher: &Publisher,
        execution: &Execution,
    ) -> Result<()> {
        info!("Handling completion for execution: {}", execution.id);

        // Check if this execution has child executions to trigger
        if let Some(child_actions) = Self::get_child_actions(execution).await? {
            // Only trigger children on completion
            if execution.status == ExecutionStatus::Completed {
                Self::trigger_child_executions(pool, publisher, execution, &child_actions).await?;
            } else {
                warn!(
                    "Execution {} failed/canceled, skipping child executions",
                    execution.id
                );
            }
        }

        // NOTE: Completion notification is published by the worker, not here.
        // This prevents duplicate execution.completed messages that would cause
        // the queue manager to decrement active_count twice.

        Ok(())
    }

    /// Get child actions from execution result (for workflow orchestration)
    async fn get_child_actions(_execution: &Execution) -> Result<Option<Vec<String>>> {
        // TODO: Implement workflow logic
        // - Check if action has defined workflow
        // - Extract next actions from execution result
        // - Parse workflow definition

        // For now, return None (no child executions)
        Ok(None)
    }

    /// Trigger child executions for a completed parent
    async fn trigger_child_executions(
        pool: &PgPool,
        publisher: &Publisher,
        parent: &Execution,
        child_actions: &[String],
    ) -> Result<()> {
        info!(
            "Triggering {} child executions for parent: {}",
            child_actions.len(),
            parent.id
        );

        for action_ref in child_actions {
            let child_input = CreateExecutionInput {
                action: None,
                action_ref: action_ref.clone(),
                config: parent.config.clone(), // Pass parent config to child
                env_vars: parent.env_vars.clone(), // Pass parent env vars to child
                parent: Some(parent.id),       // Link to parent execution
                enforcement: parent.enforcement,
                // Inherit triggering identity from parent so the callback
                // token has the same security context.
                executor: parent.executor,
                permission_set_refs: parent.permission_set_refs.clone(),
                artifact_retention_policy: parent.artifact_retention_policy,
                artifact_retention_limit: parent.artifact_retention_limit,
                worker_selector: parent.worker_selector.clone(),
                worker_tolerations: parent.worker_tolerations.clone(),
                worker_affinity: parent.worker_affinity.clone(),
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: parent.trace_tag.clone(),
                timeout_seconds: parent.timeout_seconds,
                result: None,
                workflow_task: None, // Non-workflow execution
            };

            let child_execution = ExecutionRepository::create(pool, child_input).await?;

            info!(
                "Created child execution {} for parent {}",
                child_execution.id, parent.id
            );

            // Publish ExecutionRequested message for child
            let payload = ExecutionRequestedPayload {
                execution_id: child_execution.id,
                action_id: None, // Child executions typically don't have action_id set yet
                action_ref: action_ref.clone(),
                parent_id: Some(parent.id),
                enforcement_id: None,
                config: None,
            };

            let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
                .with_source("executor");

            publisher.publish_envelope(&envelope).await?;
        }

        Ok(())
    }

    // REMOVED: publish_completion_notification
    // This method was causing duplicate execution.completed messages.
    // The worker is responsible for publishing completion notifications,
    // not the executor. Removing this prevents double-decrementing the
    // queue manager's active_count.
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_execution_manager_creation() {
        // This is a placeholder test
        // Real tests will require database and message queue setup
    }

    #[test]
    fn test_parse_execution_status() {
        // Mock pool, publisher, consumer for testing
        // In real tests, these would be properly initialized
    }
}
