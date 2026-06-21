//! Completion Listener - Handles execution completion notifications
//!
//! This module is responsible for:
//! - Listening for ExecutionCompleted messages from workers
//! - Releasing queue slots via QueueManager
//! - Updating execution status in database (if needed)
//! - Detecting inquiry requests in execution results
//! - Creating inquiries for human-in-the-loop workflows
//! - Enabling FIFO execution ordering by notifying waiting executions
//! - Advancing workflow orchestration when child task executions complete

use anyhow::Result;
use attune_common::{
    models::{
        enums::ExecutionStatus, Execution, WorkQueueAck, WorkQueueAckItem, WorkQueueDispatch,
        WorkQueueDispatchStatus, WorkQueueItem, WorkQueueItemStatus,
    },
    mq::{
        Consumer, ExecutionCompletedPayload, ExecutionRequestedPayload, MessageEnvelope,
        MessageType, MqError, Publisher,
    },
    repositories::{
        execution::{ExecutionRepository, UpdateExecutionInput},
        work_queue::{
            UpdateWorkQueueDispatchInput, UpdateWorkQueueItemInput, WorkQueueDispatchRepository,
            WorkQueueItemRepository, WorkQueueItemSearchFilters, WorkQueueRepository,
        },
        FindById, Patch, Update,
    },
};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::{
    inquiry_handler::InquiryHandler, queue_manager::ExecutionQueueManager,
    scheduler::ExecutionScheduler, work_queue_events,
};

/// Completion listener that handles execution completion messages
pub struct CompletionListener {
    pool: PgPool,
    consumer: Arc<Consumer>,
    publisher: Arc<Publisher>,
    queue_manager: Arc<ExecutionQueueManager>,
    /// Round-robin counter shared with the scheduler for dispatching workflow
    /// successor tasks to workers.
    round_robin_counter: Arc<AtomicUsize>,
    /// Root directory for file-backed artifacts (workflow logs).
    artifacts_dir: Arc<String>,
    encryption_key: Option<String>,
}

struct QueueDispatchCompletionFailure {
    retry_limit: u32,
    error_code: &'static str,
    error_message: String,
}

impl CompletionListener {
    fn retryable_mq_error(error: &anyhow::Error) -> Option<MqError> {
        let mq_error = error.downcast_ref::<MqError>()?;
        Some(match mq_error {
            MqError::Connection(msg) => MqError::Connection(msg.clone()),
            MqError::Channel(msg) => MqError::Channel(msg.clone()),
            MqError::Publish(msg) => MqError::Publish(msg.clone()),
            MqError::Timeout(msg) => MqError::Timeout(msg.clone()),
            MqError::Pool(msg) => MqError::Pool(msg.clone()),
            MqError::Lapin(err) => MqError::Connection(err.to_string()),
            _ => return None,
        })
    }

    /// Create a new completion listener
    pub fn new(
        pool: PgPool,
        consumer: Arc<Consumer>,
        publisher: Arc<Publisher>,
        queue_manager: Arc<ExecutionQueueManager>,
        artifacts_dir: impl Into<String>,
        encryption_key: Option<String>,
    ) -> Self {
        Self {
            pool,
            consumer,
            publisher,
            queue_manager,
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
            artifacts_dir: Arc::new(artifacts_dir.into()),
            encryption_key,
        }
    }

    /// Start processing execution completed messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting completion listener");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let queue_manager = self.queue_manager.clone();
        let round_robin_counter = self.round_robin_counter.clone();
        let artifacts_dir = self.artifacts_dir.clone();
        let encryption_key = self.encryption_key.clone();

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(
                move |envelope: MessageEnvelope<ExecutionCompletedPayload>| {
                    let pool = pool.clone();
                    let publisher = publisher.clone();
                    let queue_manager = queue_manager.clone();
                    let round_robin_counter = round_robin_counter.clone();
                    let artifacts_dir = artifacts_dir.clone();
                    let encryption_key = encryption_key.clone();

                    async move {
                        if let Err(e) = Self::process_execution_completed(
                            &pool,
                            &publisher,
                            &queue_manager,
                            &round_robin_counter,
                            artifacts_dir.as_str(),
                            encryption_key.as_deref(),
                            &envelope,
                        )
                        .await
                        {
                            error!("Error processing execution completion: {}", e);
                            // Return error to trigger nack with requeue
                            if let Some(mq_err) = Self::retryable_mq_error(&e) {
                                return Err(mq_err);
                            }
                            return Err(
                                format!("Failed to process execution completion: {}", e).into()
                            );
                        }
                        Ok(())
                    }
                },
            )
            .await?;

        Ok(())
    }

    /// Process an execution completed message
    async fn process_execution_completed(
        pool: &PgPool,
        publisher: &Publisher,
        queue_manager: &ExecutionQueueManager,
        round_robin_counter: &AtomicUsize,
        artifacts_dir: &str,
        encryption_key: Option<&str>,
        envelope: &MessageEnvelope<ExecutionCompletedPayload>,
    ) -> Result<()> {
        debug!("Processing execution completed message: {:?}", envelope);

        let execution_id = envelope.payload.execution_id;
        let action_id = envelope.payload.action_id;

        info!(
            "Processing completion for execution: {} (action: {})",
            execution_id, action_id
        );

        // Verify execution exists in database
        let mut execution = ExecutionRepository::find_by_id(pool, execution_id).await?;

        if let Some(ref exec) = execution.clone() {
            execution = Some(Self::handle_queue_dispatch_completion(pool, publisher, exec).await?);
        }

        if let Some(ref exec) = execution {
            debug!(
                "Execution {} found with status: {:?}",
                execution_id, exec.status
            );

            // Check if this execution is a workflow child task and advance the
            // workflow orchestration (schedule successor tasks or complete the
            // workflow).
            if exec.workflow_task.is_some() {
                info!(
                    "Execution {} is a workflow task, advancing workflow",
                    execution_id
                );
                match ExecutionScheduler::maybe_retry_workflow_task(pool, publisher, exec).await {
                    Ok(true) => {
                        info!(
                            "Execution {} scheduled a workflow retry; delaying workflow advancement",
                            execution_id
                        );
                        return Ok(());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        error!(
                            "Failed to schedule workflow retry for execution {}: {}",
                            execution_id, e
                        );
                        if let Some(mq_err) = Self::retryable_mq_error(&e) {
                            return Err(mq_err.into());
                        }
                    }
                }
                if let Err(e) = ExecutionScheduler::advance_workflow(
                    pool,
                    publisher,
                    round_robin_counter,
                    artifacts_dir,
                    encryption_key,
                    exec,
                )
                .await
                {
                    error!(
                        "Failed to advance workflow for execution {}: {}",
                        execution_id, e
                    );
                    if let Some(mq_err) = Self::retryable_mq_error(&e) {
                        return Err(mq_err.into());
                    }
                    // Non-retryable workflow advancement errors are logged but
                    // do not fail the entire completion processing path.
                }
            }

            // Check if execution result contains an inquiry request
            if let Some(result) = &exec.result {
                if InquiryHandler::has_inquiry_request(result) {
                    info!(
                        "Execution {} result contains inquiry request, creating inquiry",
                        execution_id
                    );

                    match InquiryHandler::create_inquiry_from_result(
                        pool,
                        publisher,
                        execution_id,
                        result,
                    )
                    .await
                    {
                        Ok(inquiry) => {
                            info!(
                                "Created inquiry {} for execution {}, execution paused for response",
                                inquiry.id, execution_id
                            );
                        }
                        Err(e) => {
                            error!(
                                "Failed to create inquiry for execution {}: {}",
                                execution_id, e
                            );
                            // Continue processing - don't fail the entire completion
                        }
                    }
                }
            }
        } else {
            warn!(
                "Execution {} not found in database, but still releasing queue slot",
                execution_id
            );
        }

        // Release queue slot for this action
        info!(
            "Releasing queue slot for action {} (execution {} completed)",
            action_id, execution_id
        );

        match queue_manager.release_active_slot(execution_id).await {
            Ok(release) => {
                if let Some(release) = release {
                    if let Some(next_execution_id) = release.next_execution_id {
                        info!(
                            "Queue slot released for action {}, next execution {} can proceed",
                            action_id, next_execution_id
                        );
                        if let Err(republish_err) = Self::publish_execution_requested(
                            pool,
                            publisher,
                            action_id,
                            next_execution_id,
                        )
                        .await
                        {
                            queue_manager
                                .restore_active_slot(execution_id, &release)
                                .await?;
                            return Err(republish_err);
                        }
                    } else {
                        debug!(
                            "Queue slot released for action {}, no executions waiting",
                            action_id
                        );
                    }
                } else {
                    debug!(
                        "Execution {} had no active queue slot to release",
                        execution_id
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to release queue slot for action {}: {}",
                    action_id, e
                );
                return Err(e);
            }
        }

        // Get queue statistics for logging
        if let Some(stats) = queue_manager.get_queue_stats(action_id).await {
            debug!(
                "Queue stats for action {}: {} active, {} queued, {} total completed",
                action_id, stats.active_count, stats.queue_length, stats.total_completed
            );
        }

        info!(
            "Successfully processed completion for execution: {} (action: {})",
            execution_id, action_id
        );

        Ok(())
    }

    async fn handle_queue_dispatch_completion(
        pool: &PgPool,
        publisher: &Publisher,
        execution: &Execution,
    ) -> Result<Execution> {
        let Some(dispatch) =
            WorkQueueDispatchRepository::find_by_execution(pool, execution.id).await?
        else {
            return Ok(execution.clone());
        };

        if Self::is_terminal_dispatch_status(dispatch.status) {
            return Ok(execution.clone());
        }

        let leased_items = WorkQueueItemRepository::search(
            pool,
            &WorkQueueItemSearchFilters {
                leased_execution: Some(execution.id),
                statuses: Some(vec![WorkQueueItemStatus::Leased]),
                limit: dispatch.leased_item_count.max(1) as u32,
                ..Default::default()
            },
        )
        .await?
        .rows;

        let queue = WorkQueueRepository::find_by_id(pool, dispatch.queue).await?;
        let expected_ack_version = queue
            .as_ref()
            .map(|queue| Self::expected_ack_version_from_queue_config(&queue.config))
            .unwrap_or_else(|| {
                warn!(
                    "Queue dispatch {} references missing queue {}, defaulting queue_ack version to 1",
                    dispatch.id, dispatch.queue
                );
                1
            });
        let retry_limit = queue
            .as_ref()
            .map(|queue| Self::retry_limit_from_queue_config(&queue.config))
            .unwrap_or(0);

        if let Err(message) = Self::validate_execution_queue_metadata(
            execution,
            &dispatch,
            &leased_items,
            expected_ack_version,
        ) {
            return Self::fail_queue_dispatch_completion(
                pool,
                execution,
                &dispatch,
                &leased_items,
                QueueDispatchCompletionFailure {
                    retry_limit,
                    error_code: "queue_dispatch_metadata_mismatch",
                    error_message: message,
                },
                publisher,
            )
            .await;
        }

        if leased_items.is_empty() {
            return Self::fail_queue_dispatch_completion(
                pool,
                execution,
                &dispatch,
                &leased_items,
                QueueDispatchCompletionFailure {
                    retry_limit,
                    error_code: "queue_dispatch_items_missing",
                    error_message: format!(
                        "queue dispatch {} for execution {} had no leased items to finalize",
                        dispatch.id, execution.id
                    ),
                },
                publisher,
            )
            .await;
        }

        let expected_item_ids: Vec<_> = leased_items.iter().map(|item| item.id).collect();
        let queue_ack = match Self::validated_queue_ack_for_execution(
            execution,
            &expected_item_ids,
            expected_ack_version,
        ) {
            Ok(queue_ack) => queue_ack,
            Err((error_code, message)) => {
                return Self::fail_queue_dispatch_completion(
                    pool,
                    execution,
                    &dispatch,
                    &leased_items,
                    QueueDispatchCompletionFailure {
                        retry_limit,
                        error_code,
                        error_message: message,
                    },
                    publisher,
                )
                .await;
            }
        };

        let mut tx = pool.begin().await?;
        for leased_item in &leased_items {
            let ack_item = queue_ack
                .item_for(leased_item.id)
                .expect("validated queue acknowledgement must include every leased item");
            let requested_status = Self::ack_status_to_item_status(ack_item);
            let new_status =
                Self::apply_retry_limit(requested_status, leased_item.attempt_count, retry_limit);
            let ack_summary = Self::ack_summary_json(ack_item, new_status);
            let last_error = Self::ack_item_error_json(ack_item, new_status, retry_limit);

            let updated_item = WorkQueueItemRepository::update_if_statuses(
                &mut *tx,
                leased_item.id,
                &[WorkQueueItemStatus::Leased],
                UpdateWorkQueueItemInput {
                    status: Some(new_status),
                    leased_execution: Some(Patch::Clear),
                    lease_token: Some(Patch::Clear),
                    lease_expires_at: Some(Patch::Clear),
                    last_error: Some(match last_error {
                        Some(last_error) => Patch::Set(last_error),
                        None => Patch::Clear,
                    }),
                    ack_summary: Some(Patch::Set(ack_summary)),
                    ..Default::default()
                },
            )
            .await?;

            if updated_item.is_none() {
                return Err(anyhow::anyhow!(
                    "leased queue item {} disappeared while finalizing dispatch {}",
                    leased_item.id,
                    dispatch.id
                ));
            }
        }

        WorkQueueDispatchRepository::update(
            &mut *tx,
            dispatch.id,
            UpdateWorkQueueDispatchInput {
                status: Some(WorkQueueDispatchStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await?;

        info!(
            "Finalized queue dispatch {} for execution {} with {} leased item(s)",
            dispatch.id,
            execution.id,
            leased_items.len()
        );

        if let Some(queue) = queue.as_ref() {
            if let Err(error) = work_queue_events::maybe_emit_queue_empty(
                pool,
                publisher,
                queue,
                &dispatch,
                execution.id,
                leased_items.len(),
                WorkQueueDispatchStatus::Completed,
            )
            .await
            {
                warn!(
                    "Failed to emit queue_empty event for queue '{}' dispatch {}: {}",
                    queue.r#ref, dispatch.id, error
                );
            }
        }

        Ok(execution.clone())
    }

    async fn fail_queue_dispatch_completion(
        pool: &PgPool,
        execution: &Execution,
        dispatch: &WorkQueueDispatch,
        leased_items: &[WorkQueueItem],
        failure: QueueDispatchCompletionFailure,
        publisher: &Publisher,
    ) -> Result<Execution> {
        let mut tx = pool.begin().await?;
        let retry_error = json!({
            "code": failure.error_code,
            "message": failure.error_message,
            "execution_id": execution.id,
            "dispatch_id": dispatch.id,
        });

        for leased_item in leased_items {
            let new_status = Self::apply_retry_limit(
                WorkQueueItemStatus::Retry,
                leased_item.attempt_count,
                failure.retry_limit,
            );
            let updated_item = WorkQueueItemRepository::update_if_statuses(
                &mut *tx,
                leased_item.id,
                &[WorkQueueItemStatus::Leased],
                UpdateWorkQueueItemInput {
                    status: Some(new_status),
                    leased_execution: Some(Patch::Clear),
                    lease_token: Some(Patch::Clear),
                    lease_expires_at: Some(Patch::Clear),
                    last_error: Some(Patch::Set(retry_error.clone())),
                    ack_summary: Some(Patch::Set(json!({
                        "status": "retry",
                        "effective_status": new_status,
                    }))),
                    ..Default::default()
                },
            )
            .await?;

            if updated_item.is_none() {
                return Err(anyhow::anyhow!(
                    "leased queue item {} disappeared while failing dispatch {}",
                    leased_item.id,
                    dispatch.id
                ));
            }
        }

        let dispatch_status = Self::dispatch_status_for_execution(execution.status);
        WorkQueueDispatchRepository::update(
            &mut *tx,
            dispatch.id,
            UpdateWorkQueueDispatchInput {
                status: Some(dispatch_status),
                ..Default::default()
            },
        )
        .await?;

        let updated_execution = if execution.status == ExecutionStatus::Completed {
            ExecutionRepository::update(
                &mut *tx,
                execution.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Failed),
                    result: Some(Self::queue_ack_failure_result(
                        execution.result.clone(),
                        failure.error_code,
                        retry_error["message"]
                            .as_str()
                            .unwrap_or("queue acknowledgement failed"),
                    )),
                    ..Default::default()
                },
            )
            .await?
        } else {
            execution.clone()
        };

        tx.commit().await?;

        warn!(
            "Queue dispatch {} for execution {} failed queue acknowledgement validation: {}",
            dispatch.id,
            execution.id,
            retry_error["message"]
                .as_str()
                .unwrap_or("queue acknowledgement failed")
        );

        if let Some(queue) = WorkQueueRepository::find_by_id(pool, dispatch.queue).await? {
            if let Err(error) = work_queue_events::maybe_emit_queue_empty(
                pool,
                publisher,
                &queue,
                dispatch,
                updated_execution.id,
                leased_items.len(),
                dispatch_status,
            )
            .await
            {
                warn!(
                    "Failed to emit queue_empty event for queue '{}' dispatch {}: {}",
                    queue.r#ref, dispatch.id, error
                );
            }
        }

        Ok(updated_execution)
    }

    fn expected_ack_version_from_queue_config(queue: &serde_json::Value) -> i32 {
        serde_json::from_value::<attune_common::models::WorkQueueConfig>(queue.clone())
            .ok()
            .and_then(|config| config.ack_contract.map(|ack| ack.version))
            .filter(|version| *version >= 1)
            .unwrap_or(1)
    }

    fn retry_limit_from_queue_config(queue: &serde_json::Value) -> u32 {
        serde_json::from_value::<attune_common::models::WorkQueueConfig>(queue.clone())
            .ok()
            .and_then(|config| config.dispatch.and_then(|dispatch| dispatch.retry_limit))
            .unwrap_or(0)
    }

    fn validated_queue_ack_for_execution(
        execution: &Execution,
        expected_item_ids: &[i64],
        expected_ack_version: i32,
    ) -> std::result::Result<WorkQueueAck, (&'static str, String)> {
        if !Self::can_honor_queue_ack_for_status(execution.status) {
            return Err((
                "queue_dispatch_execution_failed",
                format!(
                    "queue dispatch execution {} completed with status {:?} before acknowledging leased items",
                    execution.id, execution.status
                ),
            ));
        }

        let queue_ack = match execution.result.as_ref() {
            Some(result) => match WorkQueueAck::from_execution_result(result) {
                Ok(Some(queue_ack)) => queue_ack,
                Ok(None) => {
                    return Err(match execution.status {
                        ExecutionStatus::Completed => (
                            "queue_ack_missing",
                            format!(
                                "queue dispatch execution {} completed without execution.result.queue_ack",
                                execution.id
                            ),
                        ),
                        _ => (
                            "queue_dispatch_execution_failed",
                            format!(
                                "queue dispatch execution {} completed with status {:?} before acknowledging leased items",
                                execution.id, execution.status
                            ),
                        ),
                    });
                }
                Err(message) => return Err(("queue_ack_malformed", message)),
            },
            None => {
                return Err(match execution.status {
                    ExecutionStatus::Completed => (
                        "queue_ack_missing",
                        format!(
                            "queue dispatch execution {} completed without an execution result",
                            execution.id
                        ),
                    ),
                    _ => (
                        "queue_dispatch_execution_failed",
                        format!(
                            "queue dispatch execution {} completed with status {:?} before acknowledging leased items",
                            execution.id, execution.status
                        ),
                    ),
                });
            }
        };

        queue_ack
            .validate_for_items(expected_item_ids, expected_ack_version)
            .map_err(|message| ("queue_ack_invalid", message))?;

        Ok(queue_ack)
    }

    fn can_honor_queue_ack_for_status(status: ExecutionStatus) -> bool {
        matches!(
            status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Canceling
                | ExecutionStatus::Timeout
                | ExecutionStatus::Abandoned
        )
    }

    fn validate_execution_queue_metadata(
        execution: &Execution,
        dispatch: &WorkQueueDispatch,
        leased_items: &[WorkQueueItem],
        expected_ack_version: i32,
    ) -> std::result::Result<(), String> {
        let Some(config) = execution.config.as_ref() else {
            return Ok(());
        };
        let Some(queue_metadata) = config.get("queue") else {
            return Ok(());
        };
        let queue_metadata = queue_metadata
            .as_object()
            .ok_or_else(|| "execution.config.queue must be an object when present".to_string())?;

        if let Some(queue_id) = queue_metadata.get("id") {
            let queue_id = queue_id
                .as_i64()
                .ok_or_else(|| "execution.config.queue.id must be an integer".to_string())?;
            if queue_id != dispatch.queue {
                return Err(format!(
                    "execution.config.queue.id {} does not match dispatch queue {}",
                    queue_id, dispatch.queue
                ));
            }
        }

        if let Some(queue_ref) = queue_metadata.get("ref") {
            let queue_ref = queue_ref
                .as_str()
                .ok_or_else(|| "execution.config.queue.ref must be a string".to_string())?;
            if queue_ref != dispatch.queue_ref {
                return Err(format!(
                    "execution.config.queue.ref '{}' does not match dispatch queue_ref '{}'",
                    queue_ref, dispatch.queue_ref
                ));
            }
        }

        if let Some(leased_item_count) = queue_metadata.get("leased_item_count") {
            let leased_item_count = leased_item_count.as_u64().ok_or_else(|| {
                "execution.config.queue.leased_item_count must be a positive integer".to_string()
            })?;
            if leased_item_count != leased_items.len() as u64 {
                return Err(format!(
                    "execution.config.queue.leased_item_count {} does not match {} leased item(s)",
                    leased_item_count,
                    leased_items.len()
                ));
            }
        }

        if let Some(ack_contract_version) = queue_metadata.get("ack_contract_version") {
            let ack_contract_version = ack_contract_version.as_i64().ok_or_else(|| {
                "execution.config.queue.ack_contract_version must be an integer".to_string()
            })?;
            if ack_contract_version != i64::from(expected_ack_version) {
                return Err(format!(
                    "execution.config.queue.ack_contract_version {} does not match expected version {}",
                    ack_contract_version, expected_ack_version
                ));
            }
        }

        if let Some(items) = queue_metadata.get("items") {
            let items = items
                .as_array()
                .ok_or_else(|| "execution.config.queue.items must be an array".to_string())?;
            let metadata_ids = items
                .iter()
                .map(|item| {
                    item.get("id").and_then(|id| id.as_i64()).ok_or_else(|| {
                        "execution.config.queue.items[*].id must be an integer".to_string()
                    })
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let leased_ids: Vec<_> = leased_items.iter().map(|item| item.id).collect();
            if metadata_ids != leased_ids {
                return Err(format!(
                    "execution.config.queue.items ids {:?} do not match leased item ids {:?}",
                    metadata_ids, leased_ids
                ));
            }
        }

        Ok(())
    }

    fn is_terminal_dispatch_status(status: WorkQueueDispatchStatus) -> bool {
        matches!(
            status,
            WorkQueueDispatchStatus::Completed
                | WorkQueueDispatchStatus::Failed
                | WorkQueueDispatchStatus::Released
                | WorkQueueDispatchStatus::Cancelled
        )
    }

    fn dispatch_status_for_execution(status: ExecutionStatus) -> WorkQueueDispatchStatus {
        match status {
            ExecutionStatus::Cancelled
            | ExecutionStatus::Canceling
            | ExecutionStatus::Abandoned => WorkQueueDispatchStatus::Cancelled,
            _ => WorkQueueDispatchStatus::Failed,
        }
    }

    fn ack_status_to_item_status(ack_item: &WorkQueueAckItem) -> WorkQueueItemStatus {
        match ack_item.status {
            attune_common::models::WorkQueueAckItemStatus::Completed => {
                WorkQueueItemStatus::Completed
            }
            attune_common::models::WorkQueueAckItemStatus::Retry => WorkQueueItemStatus::Retry,
            attune_common::models::WorkQueueAckItemStatus::Failed => WorkQueueItemStatus::Failed,
            attune_common::models::WorkQueueAckItemStatus::Skipped => WorkQueueItemStatus::Skipped,
        }
    }

    fn apply_retry_limit(
        status: WorkQueueItemStatus,
        attempt_count: i32,
        retry_limit: u32,
    ) -> WorkQueueItemStatus {
        if status == WorkQueueItemStatus::Retry && attempt_count > retry_limit as i32 {
            return WorkQueueItemStatus::Failed;
        }
        status
    }

    fn ack_summary_json(
        ack_item: &WorkQueueAckItem,
        effective_status: WorkQueueItemStatus,
    ) -> JsonValue {
        let mut summary = json!({
            "status": ack_item.status,
        });

        if effective_status != Self::ack_status_to_item_status(ack_item) {
            summary["effective_status"] = json!(effective_status);
        }

        if let Some(value) = &ack_item.summary {
            summary["summary"] = value.clone();
        }
        if let Some(value) = &ack_item.error {
            summary["error"] = value.clone();
        }

        summary
    }

    fn ack_item_error_json(
        ack_item: &WorkQueueAckItem,
        effective_status: WorkQueueItemStatus,
        retry_limit: u32,
    ) -> Option<JsonValue> {
        match effective_status {
            WorkQueueItemStatus::Retry => Some(ack_item.error.clone().unwrap_or_else(|| {
                json!({
                    "code": "queue_dispatch_retry",
                    "message": "queue dispatch requested retry without error details",
                })
            })),
            WorkQueueItemStatus::Failed
                if Self::ack_status_to_item_status(ack_item) == WorkQueueItemStatus::Retry =>
            {
                Some(json!({
                    "code": "queue_dispatch_retry_limit_exhausted",
                    "message": format!(
                        "queue item exhausted retry limit after {} attempt(s); configured retry_limit={}",
                        retry_limit as i32 + 1,
                        retry_limit
                    ),
                    "retry_error": ack_item.error.clone(),
                }))
            }
            WorkQueueItemStatus::Failed => Some(ack_item.error.clone().unwrap_or_else(|| {
                json!({
                    "code": "queue_dispatch_failed",
                    "message": "queue dispatch reported failure without error details",
                })
            })),
            _ => None,
        }
    }

    fn queue_ack_failure_result(
        existing_result: Option<JsonValue>,
        error_code: &str,
        error_message: &str,
    ) -> JsonValue {
        let mut result = match existing_result {
            Some(JsonValue::Object(object)) => JsonValue::Object(object),
            Some(other) => json!({ "data": other }),
            None => json!({}),
        };

        result["succeeded"] = json!(false);
        result["error"] = json!(error_message);
        result["queue_ack_error"] = json!({
            "code": error_code,
            "message": error_message,
        });
        result
    }

    async fn publish_execution_requested(
        pool: &PgPool,
        publisher: &Publisher,
        action_id: i64,
        execution_id: i64,
    ) -> Result<()> {
        let execution = ExecutionRepository::find_by_id(pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution {} not found", execution_id))?;

        let payload = ExecutionRequestedPayload {
            execution_id,
            action_id: Some(action_id),
            action_ref: execution.action_ref.clone(),
            parent_id: execution.parent,
            enforcement_id: execution.enforcement,
            config: execution.config.clone(),
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-completion-listener");

        publisher.publish_envelope(&envelope).await?;

        debug!(
            "Republished deferred ExecutionRequested for execution {}",
            execution_id
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue_manager::ExecutionQueueManager;
    use chrono::Utc;

    #[test]
    fn retry_limit_defaults_to_zero() {
        assert_eq!(
            CompletionListener::retry_limit_from_queue_config(&json!({})),
            0
        );
    }

    #[test]
    fn retry_limit_is_exhausted_after_configured_retries() {
        assert_eq!(
            CompletionListener::apply_retry_limit(WorkQueueItemStatus::Retry, 1, 0),
            WorkQueueItemStatus::Failed
        );
        assert_eq!(
            CompletionListener::apply_retry_limit(WorkQueueItemStatus::Retry, 1, 1),
            WorkQueueItemStatus::Retry
        );
        assert_eq!(
            CompletionListener::apply_retry_limit(WorkQueueItemStatus::Retry, 2, 1),
            WorkQueueItemStatus::Failed
        );
    }

    fn queue_dispatch_execution(status: ExecutionStatus, result: Option<JsonValue>) -> Execution {
        Execution {
            id: 42,
            action: None,
            action_ref: "core.queue_dispatch".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status,
            trace_tag: None,
            result,
            retry_count: 0,
            max_retries: None,
            retry_reason: None,
            original_execution: None,
            started_at: None,
            timeout_seconds: None,
            workflow_task: None,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_release_active_slot_releases_slot() {
        let queue_manager = Arc::new(ExecutionQueueManager::with_defaults());
        let action_id = 1;

        // Simulate acquiring a slot
        queue_manager
            .enqueue_and_wait(action_id, 100, 1, None)
            .await
            .unwrap();

        // Verify slot is active
        let stats = queue_manager.get_queue_stats(action_id).await.unwrap();
        assert_eq!(stats.active_count, 1);
        assert_eq!(stats.queue_length, 0);

        // Simulate completion notification
        let release = queue_manager.release_active_slot(100).await.unwrap();
        assert!(release.is_some());
        assert_eq!(release.unwrap().next_execution_id, None);

        // Verify slot is released
        let stats = queue_manager.get_queue_stats(action_id).await.unwrap();
        assert_eq!(stats.active_count, 0);
    }

    #[tokio::test]
    async fn test_release_active_slot_wakes_waiting() {
        let queue_manager = Arc::new(ExecutionQueueManager::with_defaults());
        let action_id = 1;

        // Fill capacity
        queue_manager
            .enqueue_and_wait(action_id, 100, 1, None)
            .await
            .unwrap();

        // Queue another execution
        let queue_manager_clone = queue_manager.clone();
        let handle = tokio::spawn(async move {
            queue_manager_clone
                .enqueue_and_wait(action_id, 101, 1, None)
                .await
                .unwrap();
        });

        // Give it time to queue
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify one is queued
        let stats = queue_manager.get_queue_stats(action_id).await.unwrap();
        assert_eq!(stats.active_count, 1);
        assert_eq!(stats.queue_length, 1);

        // Notify completion
        let release = queue_manager.release_active_slot(100).await.unwrap();
        assert_eq!(release.unwrap().next_execution_id, Some(101));

        // Wait for queued execution to proceed
        handle.await.unwrap();

        // Verify stats
        let stats = queue_manager.get_queue_stats(action_id).await.unwrap();
        assert_eq!(stats.active_count, 1); // Second execution now active
        assert_eq!(stats.queue_length, 0);
        assert_eq!(stats.total_completed, 1);
    }

    #[tokio::test]
    async fn test_multiple_completions_fifo_order() {
        let queue_manager = Arc::new(ExecutionQueueManager::with_defaults());
        let action_id = 1;

        // Fill capacity
        queue_manager
            .enqueue_and_wait(action_id, 100, 1, None)
            .await
            .unwrap();

        // Queue multiple executions
        let execution_order = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let mut handles = vec![];

        for exec_id in 101..=103 {
            let queue_manager = queue_manager.clone();
            let order = execution_order.clone();

            let handle = tokio::spawn(async move {
                queue_manager
                    .enqueue_and_wait(action_id, exec_id, 1, None)
                    .await
                    .unwrap();
                order.lock().await.push(exec_id);
            });

            handles.push(handle);
        }

        // Give time to queue
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Release them one by one
        for execution_id in 100..103 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let release = queue_manager
                .release_active_slot(execution_id)
                .await
                .unwrap();
            assert!(release.is_some());
        }

        // Wait for all to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify FIFO order
        let order = execution_order.lock().await;
        assert_eq!(*order, vec![101, 102, 103]);
    }

    #[tokio::test]
    async fn test_completion_with_no_queue() {
        let queue_manager = Arc::new(ExecutionQueueManager::with_defaults());
        let execution_id = 999; // Non-existent execution

        // Should succeed but not notify anyone
        let result = queue_manager.release_active_slot(execution_id).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_valid_queue_ack_is_accepted_for_failed_execution() {
        let execution = queue_dispatch_execution(
            ExecutionStatus::Failed,
            Some(json!({
                "queue_ack": {
                    "version": 1,
                    "items": [
                        { "id": 10, "status": "completed" },
                        { "id": 11, "status": "failed", "error": { "message": "bad item" } }
                    ]
                }
            })),
        );

        let queue_ack =
            CompletionListener::validated_queue_ack_for_execution(&execution, &[10, 11], 1)
                .expect("failed execution should still honor a valid queue_ack");

        assert_eq!(queue_ack.items.len(), 2);
        assert_eq!(
            queue_ack.item_for(10).unwrap().status,
            attune_common::models::WorkQueueAckItemStatus::Completed
        );
        assert_eq!(
            queue_ack.item_for(11).unwrap().status,
            attune_common::models::WorkQueueAckItemStatus::Failed
        );
    }

    #[test]
    fn test_missing_queue_ack_for_failed_execution_stays_conservative() {
        let execution = queue_dispatch_execution(
            ExecutionStatus::Cancelled,
            Some(json!({
                "error": "worker cancelled execution"
            })),
        );

        let error = CompletionListener::validated_queue_ack_for_execution(&execution, &[10], 1)
            .unwrap_err();

        assert_eq!(error.0, "queue_dispatch_execution_failed");
        assert!(error.1.contains("before acknowledging leased items"));
    }

    #[test]
    fn test_non_terminal_execution_status_cannot_apply_queue_ack() {
        let execution = queue_dispatch_execution(
            ExecutionStatus::Running,
            Some(json!({
                "queue_ack": {
                    "version": 1,
                    "items": [
                        { "id": 10, "status": "completed" }
                    ]
                }
            })),
        );

        let error = CompletionListener::validated_queue_ack_for_execution(&execution, &[10], 1)
            .unwrap_err();

        assert_eq!(error.0, "queue_dispatch_execution_failed");
        assert!(error.1.contains("status Running"));
    }

    #[test]
    fn test_queue_ack_version_mismatch_is_rejected() {
        let execution = queue_dispatch_execution(
            ExecutionStatus::Completed,
            Some(json!({
                "queue_ack": {
                    "version": 2,
                    "items": [
                        { "id": 10, "status": "completed" }
                    ]
                }
            })),
        );

        let error = CompletionListener::validated_queue_ack_for_execution(&execution, &[10], 1)
            .unwrap_err();

        assert_eq!(error.0, "queue_ack_invalid");
        assert!(error.1.contains("queue_ack.version must be 1"));
    }

    #[test]
    fn test_queue_ack_metadata_validation_detects_item_id_mismatch() {
        let execution = Execution {
            id: 52,
            action: None,
            action_ref: "core.queue_dispatch".to_string(),
            config: Some(json!({
                "queue": {
                    "id": 7,
                    "ref": "core.inbox",
                    "leased_item_count": 1,
                    "ack_contract_version": 1,
                    "items": [
                        { "id": 999 }
                    ]
                }
            })),
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Completed,
            trace_tag: None,
            result: None,
            retry_count: 0,
            max_retries: None,
            retry_reason: None,
            original_execution: None,
            started_at: None,
            timeout_seconds: None,
            workflow_task: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let dispatch = WorkQueueDispatch {
            id: 8,
            queue: 7,
            queue_ref: "core.inbox".to_string(),
            execution: execution.id,
            status: WorkQueueDispatchStatus::Leased,
            leased_item_count: 1,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let leased_items = vec![WorkQueueItem {
            id: 10,
            queue: 7,
            queue_ref: "core.inbox".to_string(),
            item_key: Some("order-1".to_string()),
            priority: 1,
            status: WorkQueueItemStatus::Leased,
            payload: json!({"order": 1}),
            metadata: json!({}),
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: Some(execution.id),
            lease_token: Some(uuid::Uuid::new_v4()),
            lease_expires_at: Some(Utc::now()),
            attempt_count: 1,
            last_error: None,
            ack_summary: None,
            created: Utc::now(),
            updated: Utc::now(),
        }];

        let error = CompletionListener::validate_execution_queue_metadata(
            &execution,
            &dispatch,
            &leased_items,
            1,
        )
        .unwrap_err();

        assert!(error.contains("do not match leased item ids"));
    }

    #[test]
    fn test_queue_ack_failure_result_preserves_existing_payload() {
        let result = CompletionListener::queue_ack_failure_result(
            Some(json!({
                "data": {
                    "foo": "bar"
                }
            })),
            "queue_ack_missing",
            "missing queue ack",
        );

        assert_eq!(result["succeeded"], false);
        assert_eq!(result["error"], "missing queue ack");
        assert_eq!(result["data"]["foo"], "bar");
        assert_eq!(result["queue_ack_error"]["code"], "queue_ack_missing");
    }
}
