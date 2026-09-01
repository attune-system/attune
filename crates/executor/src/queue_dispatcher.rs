//! Work queue dispatcher - polls business queues and creates executions.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use attune_common::{
    crypto::decrypt_json,
    models::{
        action::Action,
        enums::{ExecutionStatus, WorkQueueDispatchStatus, WorkQueueItemStatus},
        key::Key,
        pack::Pack,
        work_queue::{
            WorkQueue, WorkQueueConfig, WorkQueueDispatch, WorkQueueItem, WorkQueueTunableValue,
        },
    },
    mq::{ExecutionRequestedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::{
        action::ActionRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        key::KeyRepository,
        pack::PackRepository,
        work_queue::{
            CreateWorkQueueDispatchInput, LeaseWorkQueueItemsInput, ReleaseWorkQueueLeaseInput,
            UpdateWorkQueueDispatchInput, WorkQueueDispatchRepository,
            WorkQueueDispatchSearchFilters, WorkQueueItemRepository, WorkQueueItemSearchFilters,
            WorkQueueRepository, WorkQueueSearchFilters,
        },
        Create, FindById, FindByRef, Update,
    },
    secret_values::{
        merge_schema_secret_redactions, prepare_secret_values, secret_paths_from_schema,
        validate_secret_destination_paths, JsonPointer, RenderedJson, SecretPathSource,
        SecretSource, ENTITY_EXECUTION_CONFIG,
    },
    trace_tag::normalize_trace_tag,
};
use chrono::Utc;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::work_queue_events;
use crate::workflow::context::WorkflowContext;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_LEASE_DURATION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_QUEUES_PER_POLL: u32 = 1_000;
const MAX_LEASED_DISPATCHES_PER_POLL: u32 = 1_000;

#[derive(Debug, Clone)]
struct QueueDispatcherConfig {
    poll_interval: Duration,
    lease_duration: Duration,
}

impl Default for QueueDispatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            lease_duration: DEFAULT_LEASE_DURATION,
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedQueueContext {
    action: Action,
    parsed_config: WorkQueueConfig,
    pack_config: JsonValue,
    pack_ref: Option<String>,
    pack_secret_paths: Vec<JsonPointer>,
    concurrency: u32,
    batch_size: u32,
    inter_execution_delay_seconds: u32,
}

#[derive(Debug, Clone)]
struct PreparedDispatch {
    dispatch_id: i64,
    execution_id: i64,
    action_id: Option<i64>,
    action_ref: String,
    config: Option<JsonValue>,
    queue: WorkQueue,
    leased_item_ids: Vec<i64>,
}

/// Polling dispatcher for first-class work queues.
pub struct WorkQueueDispatcher {
    pool: PgPool,
    publisher: Arc<Publisher>,
    encryption_key: Option<String>,
    config: QueueDispatcherConfig,
}

impl WorkQueueDispatcher {
    pub fn new(pool: PgPool, publisher: Arc<Publisher>, encryption_key: Option<String>) -> Self {
        Self {
            pool,
            publisher,
            encryption_key,
            config: QueueDispatcherConfig::default(),
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting work queue dispatcher (poll interval: {:?}, lease duration: {:?})",
            self.config.poll_interval, self.config.lease_duration
        );

        loop {
            if let Err(error) = self.poll_once().await {
                error!("Work queue dispatch poll failed: {error:#}");
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn poll_once(&self) -> Result<()> {
        self.reconcile_leased_dispatches().await?;

        let queues = WorkQueueRepository::search(
            &self.pool,
            &WorkQueueSearchFilters {
                enabled: Some(true),
                limit: MAX_QUEUES_PER_POLL,
                ..Default::default()
            },
        )
        .await?;

        for queue in queues.rows {
            if let Err(error) = self.dispatch_ready_batches_for_queue(&queue).await {
                error!(
                    "Failed to dispatch queued work for queue '{}' ({}): {error:#}",
                    queue.r#ref, queue.id
                );
            }
        }

        Ok(())
    }

    async fn reconcile_leased_dispatches(&self) -> Result<()> {
        let leased_dispatches = WorkQueueDispatchRepository::search(
            &self.pool,
            &WorkQueueDispatchSearchFilters {
                statuses: Some(vec![WorkQueueDispatchStatus::Leased]),
                limit: MAX_LEASED_DISPATCHES_PER_POLL,
                ..Default::default()
            },
        )
        .await?;

        for dispatch in leased_dispatches.rows {
            let execution = ExecutionRepository::find_by_id(&self.pool, dispatch.execution).await?;

            let Some(execution) = execution else {
                warn!(
                    "Queue dispatch {} references missing execution {}, releasing leased items",
                    dispatch.id, dispatch.execution
                );
                Self::release_orphaned_dispatch_items(&self.pool, dispatch.execution).await?;
                WorkQueueDispatchRepository::update(
                    &self.pool,
                    dispatch.id,
                    UpdateWorkQueueDispatchInput {
                        status: Some(WorkQueueDispatchStatus::Released),
                        ..Default::default()
                    },
                )
                .await?;
                continue;
            };

            if execution.status == ExecutionStatus::Requested {
                let queue = WorkQueueRepository::find_by_id(&self.pool, dispatch.queue)
                    .await?
                    .ok_or_else(|| {
                        anyhow!(
                            "queue dispatch {} references missing queue {}",
                            dispatch.id,
                            dispatch.queue
                        )
                    })?;
                let leased_item_ids = WorkQueueItemRepository::search(
                    &self.pool,
                    &WorkQueueItemSearchFilters {
                        leased_execution: Some(execution.id),
                        statuses: Some(vec![WorkQueueItemStatus::Leased]),
                        limit: dispatch.leased_item_count.max(1) as u32,
                        ..Default::default()
                    },
                )
                .await?
                .rows
                .into_iter()
                .map(|item| item.id)
                .collect();
                let prepared = PreparedDispatch {
                    dispatch_id: dispatch.id,
                    execution_id: execution.id,
                    action_id: execution.action,
                    action_ref: execution.action_ref.clone(),
                    config: execution.config.clone(),
                    queue,
                    leased_item_ids,
                };
                self.publish_dispatch(prepared).await?;
                continue;
            }

            WorkQueueDispatchRepository::update(
                &self.pool,
                dispatch.id,
                UpdateWorkQueueDispatchInput {
                    status: Some(WorkQueueDispatchStatus::Dispatched),
                    ..Default::default()
                },
            )
            .await?;

            if let Some(queue) = WorkQueueRepository::find_by_id(&self.pool, dispatch.queue).await?
            {
                let leased_item_ids = WorkQueueItemRepository::search(
                    &self.pool,
                    &WorkQueueItemSearchFilters {
                        leased_execution: Some(execution.id),
                        statuses: Some(vec![WorkQueueItemStatus::Leased]),
                        limit: dispatch.leased_item_count.max(1) as u32,
                        ..Default::default()
                    },
                )
                .await?
                .rows
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>();
                if let Err(error) = work_queue_events::maybe_emit_queue_started(
                    &self.pool,
                    &self.publisher,
                    &queue,
                    dispatch.id,
                    execution.id,
                    &leased_item_ids,
                )
                .await
                {
                    warn!(
                        "Failed to emit queue_started event for queue '{}' dispatch {} during leased-dispatch reconciliation: {}",
                        queue.r#ref, dispatch.id, error
                    );
                }
            }
        }

        Ok(())
    }

    async fn release_orphaned_dispatch_items(pool: &PgPool, execution_id: i64) -> Result<()> {
        let items = WorkQueueItemRepository::search(
            pool,
            &WorkQueueItemSearchFilters {
                leased_execution: Some(execution_id),
                statuses: Some(vec![WorkQueueItemStatus::Leased]),
                limit: 1_000,
                ..Default::default()
            },
        )
        .await?;

        let mut lease_tokens = BTreeMap::new();
        for item in items.rows {
            if let Some(token) = item.lease_token {
                lease_tokens.insert(token, ());
            }
        }

        for lease_token in lease_tokens.into_keys() {
            WorkQueueItemRepository::release_lease(
                pool,
                ReleaseWorkQueueLeaseInput {
                    lease_token,
                    new_status: WorkQueueItemStatus::Retry,
                    leased_execution: None,
                    last_error: Some(json!({
                        "code": "dispatch_execution_missing",
                        "message": format!(
                            "queue dispatch execution {} was missing during recovery",
                            execution_id
                        ),
                    })),
                    ack_summary: None,
                },
            )
            .await?;
        }

        Ok(())
    }

    async fn dispatch_ready_batches_for_queue(&self, queue: &WorkQueue) -> Result<()> {
        let context =
            Self::resolve_queue_context(&self.pool, self.encryption_key.as_deref(), queue).await?;
        let active_dispatches = WorkQueueDispatchRepository::search(
            &self.pool,
            &WorkQueueDispatchSearchFilters {
                queue: Some(queue.id),
                statuses: Some(vec![
                    WorkQueueDispatchStatus::Leased,
                    WorkQueueDispatchStatus::Dispatched,
                ]),
                limit: 1,
                ..Default::default()
            },
        )
        .await?
        .total;

        let active_dispatches = active_dispatches.min(u64::from(u32::MAX)) as u32;
        if active_dispatches >= context.concurrency {
            debug!(
                "Queue '{}' is at dispatch capacity ({}/{})",
                queue.r#ref, active_dispatches, context.concurrency
            );
            return Ok(());
        }

        if let Some(delay_until) = Self::sequential_delay_until(
            WorkQueueDispatchRepository::latest_terminal_for_queue(&self.pool, queue.id)
                .await?
                .as_ref(),
            context.concurrency,
            context.inter_execution_delay_seconds,
        ) {
            let now = Utc::now();
            if delay_until > now {
                debug!(
                    "Queue '{}' is waiting for sequential inter-execution delay until {}",
                    queue.r#ref, delay_until
                );
                return Ok(());
            }
        }

        let open_slots = context.concurrency - active_dispatches;
        for _ in 0..open_slots {
            let Some(dispatch) = Self::prepare_next_dispatch(
                &self.pool,
                self.encryption_key.as_deref(),
                queue,
                &context,
                self.config.lease_duration,
            )
            .await?
            else {
                break;
            };

            self.publish_dispatch(dispatch).await?;
        }

        Ok(())
    }

    async fn resolve_queue_context(
        pool: &PgPool,
        encryption_key: Option<&str>,
        queue: &WorkQueue,
    ) -> Result<ResolvedQueueContext> {
        let action = if let Some(action_id) = queue.dispatch_action {
            ActionRepository::find_by_id(pool, action_id).await?
        } else {
            None
        };
        let action = match action {
            Some(action) => Some(action),
            None => ActionRepository::find_by_ref(pool, &queue.dispatch_action_ref).await?,
        }
        .ok_or_else(|| {
            anyhow!(
                "dispatch action '{}' for queue '{}' no longer exists",
                queue.dispatch_action_ref,
                queue.r#ref
            )
        })?;

        let pack = if let Some(pack_id) = queue.pack {
            PackRepository::find_by_id(pool, pack_id).await?
        } else if let Some(pack_ref) = &queue.pack_ref {
            PackRepository::find_by_ref(pool, pack_ref).await?
        } else {
            None
        };

        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone())
            .with_context(|| format!("invalid config persisted for queue '{}'", queue.r#ref))?;

        let concurrency = Self::resolve_u32_tunable(
            pool,
            encryption_key,
            pack.as_ref(),
            parsed_config
                .dispatch
                .as_ref()
                .and_then(|dispatch| dispatch.concurrency.as_ref()),
            1,
            "concurrency",
            queue,
        )
        .await?;

        let batch_size_default = 1;
        let batch_size = if queue.batch_mode == attune_common::models::WorkQueueBatchMode::Single {
            1
        } else {
            Self::resolve_u32_tunable(
                pool,
                encryption_key,
                pack.as_ref(),
                parsed_config
                    .dispatch
                    .as_ref()
                    .and_then(|dispatch| dispatch.batch_size.as_ref()),
                batch_size_default,
                "batch_size",
                queue,
            )
            .await?
        };
        let inter_execution_delay_seconds = parsed_config
            .dispatch
            .as_ref()
            .and_then(|dispatch| dispatch.inter_execution_delay_seconds)
            .unwrap_or(0);

        Ok(ResolvedQueueContext {
            action,
            parsed_config,
            pack_config: pack
                .as_ref()
                .map(|pack| pack.config.clone())
                .unwrap_or(JsonValue::Null),
            pack_ref: pack.as_ref().map(|pack| pack.r#ref.clone()),
            pack_secret_paths: pack
                .as_ref()
                .map(|pack| secret_paths_from_schema(Some(&pack.conf_schema)))
                .unwrap_or_default(),
            concurrency,
            batch_size,
            inter_execution_delay_seconds,
        })
    }

    async fn resolve_u32_tunable(
        pool: &PgPool,
        encryption_key: Option<&str>,
        pack: Option<&Pack>,
        tunable: Option<&WorkQueueTunableValue>,
        default: u32,
        name: &str,
        queue: &WorkQueue,
    ) -> Result<u32> {
        let Some(tunable) = tunable else {
            return Ok(default);
        };

        let resolved = Self::resolve_tunable_value(pool, encryption_key, pack, tunable).await?;
        let parsed_resolved = resolved.as_ref().and_then(Self::parse_positive_u32);
        let parsed = parsed_resolved
            .or_else(|| tunable.fallback.as_ref().and_then(Self::parse_positive_u32))
            .unwrap_or(default);

        if parsed == default && parsed_resolved.is_none() {
            if let Some(resolved_value) = resolved.as_ref() {
                warn!(
                    "Queue '{}' resolved non-positive/non-integer {} tunable {}, falling back to {}",
                    queue.r#ref,
                    name,
                    resolved_value,
                    default
                );
            }
        }

        Ok(parsed)
    }

    async fn resolve_tunable_value(
        pool: &PgPool,
        encryption_key: Option<&str>,
        pack: Option<&Pack>,
        tunable: &WorkQueueTunableValue,
    ) -> Result<Option<JsonValue>> {
        use attune_common::models::WorkQueueTunableSource;

        let primary = match tunable.source {
            WorkQueueTunableSource::Literal => tunable.value.clone(),
            WorkQueueTunableSource::PackConfig => pack.and_then(|pack| {
                tunable
                    .path
                    .as_deref()
                    .and_then(|path| Self::json_path_get(&pack.config, path).cloned())
            }),
            WorkQueueTunableSource::Keystore => {
                let Some(key_ref) = tunable.key_ref.as_deref() else {
                    return Ok(tunable.fallback.clone());
                };
                let key = KeyRepository::find_by_ref(pool, key_ref).await?;
                match key {
                    Some(key) => {
                        let value = Self::resolve_key_value(&key, encryption_key)?;
                        if let Some(path) = tunable.path.as_deref() {
                            Self::json_path_get(&value, path).cloned()
                        } else {
                            Some(value)
                        }
                    }
                    None => None,
                }
            }
        };

        Ok(primary.or_else(|| tunable.fallback.clone()))
    }

    fn resolve_key_value(key: &Key, encryption_key: Option<&str>) -> Result<JsonValue> {
        if !key.encrypted {
            return Ok(key.value.clone());
        }

        let encryption_key = encryption_key.ok_or_else(|| {
            anyhow!(
                "encrypted key '{}' requires security.encryption_key for queue dispatch",
                key.r#ref
            )
        })?;
        decrypt_json(&key.value, encryption_key)
            .with_context(|| format!("failed to decrypt queue tunable key '{}'", key.r#ref))
    }

    fn parse_positive_u32(value: &JsonValue) -> Option<u32> {
        match value {
            JsonValue::Number(number) => number
                .as_u64()
                .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0),
            JsonValue::String(value) => value.parse::<u32>().ok().filter(|value| *value > 0),
            _ => None,
        }
    }

    fn sequential_delay_until(
        latest_terminal_dispatch: Option<&WorkQueueDispatch>,
        concurrency: u32,
        delay_seconds: u32,
    ) -> Option<chrono::DateTime<Utc>> {
        if concurrency != 1 || delay_seconds == 0 {
            return None;
        }

        latest_terminal_dispatch.and_then(|dispatch| {
            chrono::Duration::try_seconds(i64::from(delay_seconds))
                .map(|delay| dispatch.updated + delay)
        })
    }

    async fn prepare_next_dispatch(
        pool: &PgPool,
        encryption_key: Option<&str>,
        queue: &WorkQueue,
        context: &ResolvedQueueContext,
        lease_duration: Duration,
    ) -> Result<Option<PreparedDispatch>> {
        let batch_limit = if queue.batch_mode == attune_common::models::WorkQueueBatchMode::Single {
            1
        } else {
            context.batch_size.max(1)
        };

        let lease_token = Uuid::new_v4();
        let lease_expires_at = Utc::now()
            + chrono::Duration::from_std(lease_duration)
                .unwrap_or_else(|_| chrono::Duration::hours(24));

        let mut tx = pool.begin().await?;
        let items = WorkQueueItemRepository::lease_next_batch(
            &mut *tx,
            LeaseWorkQueueItemsInput {
                queue: queue.id,
                ready_statuses: vec![WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry],
                limit: i64::from(batch_limit),
                batch_coalescing: context
                    .parsed_config
                    .dispatch
                    .as_ref()
                    .and_then(|dispatch| dispatch.coalescing.clone()),
                leased_execution: None,
                lease_token,
                lease_expires_at,
            },
        )
        .await?;

        if items.is_empty() {
            tx.commit().await?;
            return Ok(None);
        }

        let rendered_config = Self::build_execution_config(
            queue,
            &context.parsed_config,
            &context.pack_config,
            context.pack_ref.clone(),
            &context.pack_secret_paths,
            &items,
        )?;
        validate_secret_destination_paths(
            context.action.param_schema.as_ref(),
            &rendered_config.secret_paths,
        )?;
        let (execution_config, secret_inputs) = merge_schema_secret_redactions(
            rendered_config.value,
            &rendered_config.secret_path_sources,
            context.action.param_schema.as_ref(),
        );
        let prepared_secrets = if secret_inputs.is_empty() {
            Vec::new()
        } else {
            let encryption_key = encryption_key.ok_or_else(|| {
                anyhow!(
                    "Cannot store secret queue execution parameters without security.encryption_key"
                )
            })?;
            prepare_secret_values(secret_inputs, encryption_key)?
        };
        // SECURITY: Inherit the triggering identity from the queue items'
        // `requested_by_identity` field if all leased items share the same
        // initiator. Otherwise fall back to the system identity so the worker
        // still mints a callback token with a known `sub` claim.
        //
        // SECURITY: This precedence is now sound end-to-end. Rule-driven
        // executions run as `rule.owner_identity` (see enforcement_processor),
        // and when those executions enqueue items via the API, the queue item
        // is stamped with `requested_by_identity` = current identity. So the
        // per-item identity already captures the rule owner — no separate
        // rule lookup is needed here. The system fallback only fires for
        // legacy items with NULL `requested_by_identity` (e.g. items enqueued
        // before this column existed) and for queues fed by anonymous sources.
        const SYSTEM_IDENTITY_ID: i64 = 1;
        let executor_identity = {
            let mut iter = items.iter().map(|item| item.requested_by_identity);
            let first = iter.next().flatten();
            if let Some(id) = first {
                if iter.all(|other| other == Some(id)) {
                    Some(id)
                } else {
                    Some(SYSTEM_IDENTITY_ID)
                }
            } else {
                Some(SYSTEM_IDENTITY_ID)
            }
        };
        let reserved_dispatch_id =
            if queue.batch_mode == attune_common::models::WorkQueueBatchMode::Batch {
                Some(Self::reserve_dispatch_id(&mut tx).await?)
            } else {
                None
            };
        let trace_tag = Self::resolve_queue_trace_tag(
            queue,
            &context.parsed_config,
            &context.pack_config,
            &items,
            reserved_dispatch_id,
        )?;
        let execution = ExecutionRepository::create(
            &mut *tx,
            CreateExecutionInput {
                action: Some(context.action.id),
                action_ref: context.action.r#ref.clone(),
                config: Some(execution_config),
                env_vars: None,
                parent: None,
                enforcement: None,
                executor: executor_identity,
                permission_set_refs: queue.permission_set_refs.clone().unwrap_or_else(|| {
                    context.action.default_execution_permission_set_refs.clone()
                }),
                artifact_retention_policy: context.action.artifact_retention_policy,
                artifact_retention_limit: context.action.artifact_retention_limit,
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: Some(trace_tag),
                timeout_seconds: Some(context.action.timeout_seconds.unwrap_or(
                    attune_common::config::app_default_execution_timeout_seconds() as i32,
                )),
                result: None,
                workflow_task: None,
            },
        )
        .await?;
        if !prepared_secrets.is_empty() {
            ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                execution.id,
                &prepared_secrets,
            )
            .await?;
        }

        WorkQueueItemRepository::attach_execution_to_lease(&mut *tx, lease_token, execution.id)
            .await?;

        let dispatch = WorkQueueDispatchRepository::create(
            &mut *tx,
            CreateWorkQueueDispatchInput {
                id: reserved_dispatch_id,
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                execution: execution.id,
                status: WorkQueueDispatchStatus::Leased,
                leased_item_count: items.len() as i32,
            },
        )
        .await?;

        tx.commit().await?;

        Ok(Some(PreparedDispatch {
            dispatch_id: dispatch.id,
            execution_id: execution.id,
            action_id: Some(context.action.id),
            action_ref: context.action.r#ref.clone(),
            config: execution.config.clone(),
            queue: queue.clone(),
            leased_item_ids: items.iter().map(|item| item.id).collect(),
        }))
    }

    fn build_execution_config(
        queue: &WorkQueue,
        parsed_config: &WorkQueueConfig,
        pack_config: &JsonValue,
        pack_ref: Option<String>,
        pack_secret_paths: &[JsonPointer],
        items: &[WorkQueueItem],
    ) -> Result<RenderedJson> {
        if items.is_empty() {
            return Err(anyhow!(
                "cannot build execution config for queue '{}' without leased items",
                queue.r#ref
            ));
        }

        if queue
            .action_params
            .as_object()
            .is_none_or(|action_params| action_params.is_empty())
        {
            return Self::build_default_execution_config(queue, items);
        }

        let context =
            Self::build_action_params_context(queue, parsed_config, pack_config, items, None);
        let mut context = context;
        context.set_pack_config_with_secret_paths(pack_config.clone(), pack_ref, pack_secret_paths);
        Self::mark_queue_item_secret_paths(&context, queue, items);
        let rendered = context
            .render_json_with_sensitivity(&queue.action_params)
            .map_err(|error| {
                anyhow!(
                    "failed to render action_params for queue '{}': {}",
                    queue.r#ref,
                    error
                )
            })?;

        if !rendered.value.is_object() {
            return Err(anyhow!(
                "queue '{}' action_params must render to a JSON object",
                queue.r#ref
            ));
        }

        Ok(rendered)
    }

    fn build_default_execution_config(
        queue: &WorkQueue,
        items: &[WorkQueueItem],
    ) -> Result<RenderedJson> {
        let item_secret_paths = secret_paths_from_schema(Some(&queue.item_schema));
        match queue.batch_mode {
            attune_common::models::WorkQueueBatchMode::Single => {
                let item = items
                    .first()
                    .ok_or_else(|| anyhow!("single-item queue dispatch had no leased item"))?;
                let value = if item.payload.is_object() {
                    item.payload.clone()
                } else {
                    json!({ "item": item.payload.clone() })
                };
                let secret_path_sources = item_secret_paths
                    .iter()
                    .map(|path| SecretPathSource {
                        path: if item.payload.is_object() {
                            path.clone()
                        } else {
                            format!("/item{path}")
                        },
                        source: SecretSource::QueueItem {
                            queue_ref: Some(queue.r#ref.clone()),
                            item_id: Some(item.id),
                            path: path.clone(),
                        },
                    })
                    .collect::<Vec<_>>();
                Ok(Self::rendered_from_queue_path_sources(
                    value,
                    secret_path_sources,
                ))
            }
            attune_common::models::WorkQueueBatchMode::Batch => {
                let value = json!({
                    "items": items.iter().map(|item| item.payload.clone()).collect::<Vec<_>>()
                });
                let mut secret_path_sources = Vec::new();
                for (idx, item) in items.iter().enumerate() {
                    for path in &item_secret_paths {
                        secret_path_sources.push(SecretPathSource {
                            path: format!("/items/{idx}{path}"),
                            source: SecretSource::QueueItem {
                                queue_ref: Some(queue.r#ref.clone()),
                                item_id: Some(item.id),
                                path: path.clone(),
                            },
                        });
                    }
                }
                Ok(Self::rendered_from_queue_path_sources(
                    value,
                    secret_path_sources,
                ))
            }
        }
    }

    fn build_action_params_context(
        queue: &WorkQueue,
        parsed_config: &WorkQueueConfig,
        pack_config: &JsonValue,
        items: &[WorkQueueItem],
        dispatch_id: Option<i64>,
    ) -> WorkflowContext {
        let mut context = WorkflowContext::new(JsonValue::Null, HashMap::new());
        context.set_pack_config(pack_config.clone());
        context.set_var(
            "queue",
            Self::build_queue_metadata(queue, parsed_config, items, dispatch_id),
        );
        context.set_var(
            "items",
            JsonValue::Array(items.iter().map(|item| item.payload.clone()).collect()),
        );
        context.set_var(
            "queue_items",
            JsonValue::Array(items.iter().map(Self::build_queue_item_context).collect()),
        );
        if queue.batch_mode == attune_common::models::WorkQueueBatchMode::Single {
            if let Some(item) = items.first() {
                let item_secret_paths = secret_paths_from_schema(Some(&queue.item_schema));
                if item_secret_paths.is_empty() {
                    context.set_current_item(item.payload.clone(), 0);
                } else {
                    context.set_current_item_with_secret_paths(
                        item.payload.clone(),
                        0,
                        &item_secret_paths,
                        |path| SecretSource::QueueItem {
                            queue_ref: Some(queue.r#ref.clone()),
                            item_id: Some(item.id),
                            path: path.clone(),
                        },
                    );
                }
                context.set_var("queue_item", Self::build_queue_item_context(item));
            }
        }
        context
    }

    fn mark_queue_item_secret_paths(
        context: &WorkflowContext,
        queue: &WorkQueue,
        items: &[WorkQueueItem],
    ) {
        let item_secret_paths = secret_paths_from_schema(Some(&queue.item_schema));
        if item_secret_paths.is_empty() {
            return;
        }

        for (idx, item) in items.iter().enumerate() {
            context.mark_secret_pointer_paths(
                &format!("items.{idx}"),
                &item_secret_paths,
                |path| SecretSource::QueueItem {
                    queue_ref: Some(queue.r#ref.clone()),
                    item_id: Some(item.id),
                    path: path.clone(),
                },
            );
            context.mark_secret_pointer_paths(
                &format!("queue_items.{idx}.payload"),
                &item_secret_paths,
                |path| SecretSource::QueueItem {
                    queue_ref: Some(queue.r#ref.clone()),
                    item_id: Some(item.id),
                    path: path.clone(),
                },
            );
            if idx == 0 && queue.batch_mode == attune_common::models::WorkQueueBatchMode::Single {
                context.mark_secret_pointer_paths(
                    "queue_item.payload",
                    &item_secret_paths,
                    |path| SecretSource::QueueItem {
                        queue_ref: Some(queue.r#ref.clone()),
                        item_id: Some(item.id),
                        path: path.clone(),
                    },
                );
            }
        }
    }

    fn build_queue_item_context(item: &WorkQueueItem) -> JsonValue {
        json!({
            "id": item.id,
            "item_key": item.item_key,
            "priority": item.priority,
            "payload": item.payload,
            "metadata": item.metadata,
            "enqueue_source": item.enqueue_source,
            "attempt_count": item.attempt_count,
            "requested_by_identity": item.requested_by_identity,
            "requested_by_execution": item.requested_by_execution,
            "requested_by_enforcement": item.requested_by_enforcement,
        })
    }

    fn build_queue_metadata(
        queue: &WorkQueue,
        parsed_config: &WorkQueueConfig,
        items: &[WorkQueueItem],
        dispatch_id: Option<i64>,
    ) -> JsonValue {
        let ack_version = parsed_config
            .ack_contract
            .as_ref()
            .map(|ack| ack.version)
            .unwrap_or(1);

        json!({
            "id": queue.id,
            "ref": queue.r#ref,
            "batch_mode": queue.batch_mode,
            "leased_item_count": items.len(),
            "dispatch_id": dispatch_id,
            "ack_contract_version": ack_version,
        })
    }

    async fn reserve_dispatch_id(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<i64> {
        let id = sqlx::query_scalar::<_, i64>(
            "SELECT nextval(pg_get_serial_sequence('work_queue_dispatch', 'id'))",
        )
        .fetch_one(&mut **tx)
        .await?;
        Ok(id)
    }

    fn resolve_queue_trace_tag(
        queue: &WorkQueue,
        parsed_config: &WorkQueueConfig,
        pack_config: &JsonValue,
        items: &[WorkQueueItem],
        dispatch_id: Option<i64>,
    ) -> Result<String> {
        if let Some(template) = &queue.trace_tag_template {
            let context = Self::build_action_params_context(
                queue,
                parsed_config,
                pack_config,
                items,
                dispatch_id,
            );
            let rendered = context
                .render_json(&JsonValue::String(template.clone()))
                .with_context(|| {
                    format!(
                        "failed to render trace_tag_template for queue '{}'",
                        queue.r#ref
                    )
                })?;
            let rendered_string = match rendered {
                JsonValue::Null => String::new(),
                JsonValue::String(value) => value,
                other => other.to_string(),
            };
            // A configured template that renders to an empty/whitespace value
            // should not block dispatch; fall back to the default trace tag.
            if !rendered_string.trim().is_empty() {
                return Ok(normalize_trace_tag(&rendered_string)?);
            }
        }

        if let Some(source_trace_tag) = Self::source_queue_item_trace_tag(queue, items) {
            return Ok(source_trace_tag);
        }

        Self::default_queue_trace_tag(queue, items, dispatch_id)
    }

    fn source_queue_item_trace_tag(queue: &WorkQueue, items: &[WorkQueueItem]) -> Option<String> {
        match queue.batch_mode {
            attune_common::models::WorkQueueBatchMode::Single => {
                let trace_tag = items.first()?.trace_tag.as_ref()?;
                match normalize_trace_tag(trace_tag) {
                    Ok(normalized) => Some(normalized),
                    Err(error) => {
                        tracing::warn!(
                            queue_ref = %queue.r#ref,
                            item_id = items.first().map(|item| item.id).unwrap_or_default(),
                            %error,
                            "Ignoring invalid source queue-item trace_tag and falling back to default"
                        );
                        None
                    }
                }
            }
            attune_common::models::WorkQueueBatchMode::Batch => {
                let mut unique = std::collections::BTreeSet::new();
                let mut saw_missing = false;
                for item in items {
                    match item.trace_tag.as_deref() {
                        Some(value) if !value.trim().is_empty() => {
                            unique.insert(value.trim().to_string());
                        }
                        _ => {
                            saw_missing = true;
                        }
                    }
                }

                if saw_missing || unique.len() != 1 {
                    return None;
                }

                let only = unique.into_iter().next()?;
                match normalize_trace_tag(&only) {
                    Ok(normalized) => Some(normalized),
                    Err(error) => {
                        tracing::warn!(
                            queue_ref = %queue.r#ref,
                            %error,
                            "Ignoring invalid batch source trace_tag and falling back to default"
                        );
                        None
                    }
                }
            }
        }
    }

    fn default_queue_trace_tag(
        queue: &WorkQueue,
        items: &[WorkQueueItem],
        dispatch_id: Option<i64>,
    ) -> Result<String> {
        match queue.batch_mode {
            attune_common::models::WorkQueueBatchMode::Single => {
                let item_id = items
                    .first()
                    .map(|item| item.id)
                    .ok_or_else(|| anyhow!("queue '{}' had no leased items", queue.r#ref))?;
                Ok(normalize_trace_tag(&format!(
                    "{}.{}",
                    queue.r#ref, item_id
                ))?)
            }
            attune_common::models::WorkQueueBatchMode::Batch => {
                let dispatch_id = dispatch_id.ok_or_else(|| {
                    anyhow!(
                        "queue '{}' missing dispatch id while resolving batch trace tag",
                        queue.r#ref
                    )
                })?;
                Ok(normalize_trace_tag(&format!(
                    "{}.{}",
                    queue.r#ref, dispatch_id
                ))?)
            }
        }
    }

    fn rendered_from_queue_path_sources(
        value: JsonValue,
        mut secret_path_sources: Vec<SecretPathSource>,
    ) -> RenderedJson {
        secret_path_sources.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        secret_path_sources.dedup_by(|a, b| a.path == b.path && a.source == b.source);
        let mut secret_paths = secret_path_sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>();
        secret_paths.sort();
        secret_paths.dedup();
        let mut sources = secret_path_sources
            .iter()
            .map(|source| source.source.clone())
            .collect::<Vec<_>>();
        sources.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        sources.dedup();

        RenderedJson {
            value,
            secret_paths,
            sources,
            secret_path_sources,
        }
    }

    fn json_path_get<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
        let mut current = value;
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            current = match current {
                JsonValue::Object(object) => object.get(segment)?,
                JsonValue::Array(array) => {
                    let index = segment.parse::<usize>().ok()?;
                    array.get(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    async fn publish_dispatch(&self, dispatch: PreparedDispatch) -> Result<()> {
        let queue = dispatch.queue.clone();
        let leased_item_ids = dispatch.leased_item_ids.clone();
        let dispatch_id = dispatch.dispatch_id;
        let execution_id = dispatch.execution_id;

        let payload = ExecutionRequestedPayload {
            execution_id: dispatch.execution_id,
            action_id: dispatch.action_id,
            action_ref: dispatch.action_ref,
            parent_id: None,
            enforcement_id: None,
            config: dispatch.config,
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-queue-dispatcher");
        self.publisher.publish_envelope(&envelope).await?;

        WorkQueueDispatchRepository::update(
            &self.pool,
            dispatch.dispatch_id,
            UpdateWorkQueueDispatchInput {
                status: Some(WorkQueueDispatchStatus::Dispatched),
                ..Default::default()
            },
        )
        .await?;

        if let Err(error) = work_queue_events::maybe_emit_queue_started(
            &self.pool,
            &self.publisher,
            &queue,
            dispatch_id,
            execution_id,
            &leased_item_ids,
        )
        .await
        {
            warn!(
                "Failed to emit queue_started event for queue '{}' dispatch {}: {}",
                queue.r#ref, dispatch_id, error
            );
        }

        debug!(
            "Dispatched work queue execution {} via dispatch record {}",
            execution_id, dispatch_id
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::models::{
        enums::OwnerType, key::Key, ActionReferenceVisibility, WorkQueueBatchMode,
        WorkQueueDispatchStatus,
    };

    fn sample_queue(
        batch_mode: WorkQueueBatchMode,
        action_params: JsonValue,
        config: JsonValue,
    ) -> WorkQueue {
        WorkQueue {
            id: 1,
            r#ref: "core.inbox".to_string(),
            pack: Some(10),
            pack_ref: Some("core".to_string()),
            is_adhoc: false,
            label: "Inbox".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(11),
            dispatch_action_ref: "core.process".to_string(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Replace,
            batch_mode,
            item_schema: json!({}),
            action_params,
            trace_tag_template: None,
            permission_set_refs: None,
            config,
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    fn sample_item(id: i64, priority: i32, payload: JsonValue) -> WorkQueueItem {
        WorkQueueItem {
            id,
            queue: 1,
            queue_ref: "core.inbox".to_string(),
            item_key: Some(format!("item-{id}")),
            priority,
            status: WorkQueueItemStatus::Queued,
            payload,
            metadata: json!({"source": "test"}),
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            trace_tag: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    fn sample_dispatch(
        status: WorkQueueDispatchStatus,
        updated: chrono::DateTime<Utc>,
    ) -> WorkQueueDispatch {
        WorkQueueDispatch {
            id: 1,
            queue: 1,
            queue_ref: "core.inbox".to_string(),
            execution: 42,
            status,
            leased_item_count: 1,
            created: updated,
            updated,
        }
    }

    #[test]
    fn build_batch_execution_config_maps_items_and_queue_metadata() {
        let queue = sample_queue(
            WorkQueueBatchMode::Batch,
            json!({
                "payload": {
                    "items": "{{ items }}"
                },
                "queue": "{{ queue }}",
                "queue_items": "{{ queue_items }}"
            }),
            json!({
                "ack_contract": {
                    "version": 2
                }
            }),
        );
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let items = vec![
            sample_item(1001, 10, json!({"order_id": 1001})),
            sample_item(1002, 5, json!({"order_id": 1002})),
        ];

        let config = WorkQueueDispatcher::build_execution_config(
            &queue,
            &parsed_config,
            &JsonValue::Null,
            None,
            &[],
            &items,
        )
        .expect("config should build");

        assert_eq!(
            config.value["payload"]["items"],
            json!([{"order_id": 1001}, {"order_id": 1002}])
        );
        assert_eq!(
            config.value["queue"]["ack_contract_version"].as_i64(),
            Some(2)
        );
        assert_eq!(config.value["queue"]["leased_item_count"].as_i64(), Some(2));
        assert!(config.value["queue"].get("items").is_none());
        assert_eq!(config.value["queue_items"][0]["id"].as_i64(), Some(1001));
        assert_eq!(
            config.value["queue_items"][0]["payload"]["order_id"].as_i64(),
            Some(1001)
        );
    }

    #[test]
    fn build_single_execution_config_preserves_object_payload_without_mapping() {
        let queue = sample_queue(WorkQueueBatchMode::Single, json!({}), json!({}));
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({"customer": "alice", "order_id": 42}));

        let config = WorkQueueDispatcher::build_execution_config(
            &queue,
            &parsed_config,
            &JsonValue::Null,
            None,
            &[],
            &[item],
        )
        .unwrap();

        assert_eq!(
            config.value,
            json!({
                "customer": "alice",
                "order_id": 42
            })
        );
    }

    #[test]
    fn build_single_execution_config_renders_action_params_with_item_and_pack_config() {
        let queue = sample_queue(
            WorkQueueBatchMode::Single,
            json!({
                "customer": "{{ item.customer }}",
                "priority": "{{ item.priority }}",
                "region": "{{ config.region }}"
            }),
            json!({}),
        );
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({"customer": "alice", "priority": 7}));

        let config = WorkQueueDispatcher::build_execution_config(
            &queue,
            &parsed_config,
            &json!({"region": "us-east-1"}),
            None,
            &[],
            &[item],
        )
        .expect("config should build");

        assert_eq!(
            config.value,
            json!({
                "customer": "alice",
                "priority": 7,
                "region": "us-east-1"
            })
        );
    }

    #[test]
    fn resolve_queue_trace_tag_uses_rendered_template_when_non_empty() {
        let mut queue = sample_queue(WorkQueueBatchMode::Single, json!({}), json!({}));
        queue.trace_tag_template = Some("trace.{{ item.customer }}".to_string());
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({"customer": "alice"}));

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            None,
        )
        .expect("trace tag should resolve");

        assert_eq!(trace_tag, "trace.alice");
    }

    #[test]
    fn resolve_queue_trace_tag_falls_back_to_default_when_template_renders_empty_single() {
        let mut queue = sample_queue(WorkQueueBatchMode::Single, json!({}), json!({}));
        queue.trace_tag_template = Some("{{ item.tag }}".to_string());
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({"customer": "alice", "tag": ""}));

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            None,
        )
        .expect("trace tag should fall back to default");

        assert_eq!(trace_tag, "core.inbox.42");
    }

    #[test]
    fn resolve_queue_trace_tag_uses_source_queue_item_trace_tag_in_single_mode() {
        let queue = sample_queue(WorkQueueBatchMode::Single, json!({}), json!({}));
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let mut item = sample_item(42, 1, json!({"customer": "alice"}));
        item.trace_tag = Some("origin.trace.42".to_string());

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            None,
        )
        .expect("trace tag should resolve from source item");

        assert_eq!(trace_tag, "origin.trace.42");
    }

    #[test]
    fn resolve_queue_trace_tag_falls_back_to_default_when_template_is_whitespace_batch() {
        let mut queue = sample_queue(WorkQueueBatchMode::Batch, json!({}), json!({}));
        queue.trace_tag_template = Some("   ".to_string());
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(7, 1, json!({}));

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            Some(99),
        )
        .expect("trace tag should fall back to default");

        assert_eq!(trace_tag, "core.inbox.99");
    }

    #[test]
    fn resolve_queue_trace_tag_uses_shared_source_trace_tag_in_batch_mode() {
        let queue = sample_queue(WorkQueueBatchMode::Batch, json!({}), json!({}));
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let mut first = sample_item(7, 1, json!({}));
        first.trace_tag = Some("batch.shared.trace".to_string());
        let mut second = sample_item(8, 1, json!({}));
        second.trace_tag = Some("batch.shared.trace".to_string());

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[first, second],
            Some(99),
        )
        .expect("trace tag should resolve from source batch items");

        assert_eq!(trace_tag, "batch.shared.trace");
    }

    #[test]
    fn resolve_queue_trace_tag_falls_back_to_dispatch_default_for_mixed_batch_source_tags() {
        let queue = sample_queue(WorkQueueBatchMode::Batch, json!({}), json!({}));
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let mut first = sample_item(7, 1, json!({}));
        first.trace_tag = Some("batch.trace.one".to_string());
        let mut second = sample_item(8, 1, json!({}));
        second.trace_tag = Some("batch.trace.two".to_string());

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[first, second],
            Some(99),
        )
        .expect("trace tag should fall back for mixed source tags");

        assert_eq!(trace_tag, "core.inbox.99");
    }

    #[test]
    fn resolve_queue_trace_tag_treats_null_render_as_empty_single() {
        let mut queue = sample_queue(WorkQueueBatchMode::Single, json!({}), json!({}));
        // A pure expression resolving to a JSON null must be treated as empty
        // (not the literal string "null") and fall back to the default tag.
        queue.trace_tag_template = Some("{{ item.maybe }}".to_string());
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({"maybe": null}));

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            None,
        )
        .expect("trace tag should fall back to default");

        assert_eq!(trace_tag, "core.inbox.42");
    }

    #[test]
    fn resolve_queue_trace_tag_treats_null_render_as_empty_batch() {
        let mut queue = sample_queue(WorkQueueBatchMode::Batch, json!({}), json!({}));
        queue.trace_tag_template = Some("{{ items[0].maybe }}".to_string());
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(7, 1, json!({"maybe": null}));

        let trace_tag = WorkQueueDispatcher::resolve_queue_trace_tag(
            &queue,
            &parsed_config,
            &json!({}),
            &[item],
            Some(99),
        )
        .expect("trace tag should fall back to default");

        assert_eq!(trace_tag, "core.inbox.99");
    }

    #[test]
    fn sequential_delay_until_returns_none_without_delay() {
        let dispatch = sample_dispatch(WorkQueueDispatchStatus::Completed, Utc::now());

        let result = WorkQueueDispatcher::sequential_delay_until(Some(&dispatch), 1, 0);

        assert_eq!(result, None);
    }

    #[test]
    fn sequential_delay_until_returns_none_when_concurrency_is_not_one() {
        let dispatch = sample_dispatch(WorkQueueDispatchStatus::Completed, Utc::now());

        let result = WorkQueueDispatcher::sequential_delay_until(Some(&dispatch), 2, 30);

        assert_eq!(result, None);
    }

    #[test]
    fn sequential_delay_until_uses_terminal_dispatch_timestamp() {
        let updated = Utc::now();
        let dispatch = sample_dispatch(WorkQueueDispatchStatus::Completed, updated);

        let result = WorkQueueDispatcher::sequential_delay_until(Some(&dispatch), 1, 30)
            .expect("delay should be calculated");

        assert_eq!(result, updated + chrono::Duration::seconds(30));
    }

    #[test]
    fn resolve_key_value_decrypts_encrypted_tunables() {
        let encryption_key = "a".repeat(32);
        let encrypted = attune_common::crypto::encrypt_json(
            &json!({"dispatch": {"batch_size": 3}}),
            &encryption_key,
        )
        .expect("value should encrypt");
        let key = Key {
            id: 1,
            r#ref: "core.dispatch".to_string(),
            local_ref: "dispatch".to_string(),
            owner_type: OwnerType::System,
            owner: None,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Dispatch".to_string(),
            encrypted: true,
            encryption_key_hash: Some("test".to_string()),
            value: encrypted,
            created: Utc::now(),
            updated: Utc::now(),
        };

        let decrypted =
            WorkQueueDispatcher::resolve_key_value(&key, Some(encryption_key.as_str())).unwrap();

        assert_eq!(decrypted, json!({"dispatch": {"batch_size": 3}}));
    }

    #[test]
    fn resolve_key_value_requires_encryption_key_for_encrypted_values() {
        let encryption_key = "a".repeat(32);
        let encrypted = attune_common::crypto::encrypt_json(
            &json!({"dispatch": {"batch_size": 3}}),
            &encryption_key,
        )
        .expect("value should encrypt");
        let key = Key {
            id: 1,
            r#ref: "core.dispatch".to_string(),
            local_ref: "dispatch".to_string(),
            owner_type: OwnerType::System,
            owner: None,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Dispatch".to_string(),
            encrypted: true,
            encryption_key_hash: Some("test".to_string()),
            value: encrypted,
            created: Utc::now(),
            updated: Utc::now(),
        };

        let error = WorkQueueDispatcher::resolve_key_value(&key, None).unwrap_err();
        assert!(error
            .to_string()
            .contains("requires security.encryption_key"));
    }

    #[test]
    fn build_batch_execution_config_defaults_to_items_path_without_metadata() {
        let queue = sample_queue(WorkQueueBatchMode::Batch, json!({}), json!({}));
        let parsed_config: WorkQueueConfig = serde_json::from_value(queue.config.clone()).unwrap();
        let items = vec![
            sample_item(1, 10, json!({"order_id": 1})),
            sample_item(2, 5, json!({"order_id": 2})),
        ];

        let config = WorkQueueDispatcher::build_execution_config(
            &queue,
            &parsed_config,
            &JsonValue::Null,
            None,
            &[],
            &items,
        )
        .expect("config should build");

        assert_eq!(
            config.value,
            json!({
                "items": [
                    {"order_id": 1},
                    {"order_id": 2}
                ]
            })
        );
    }
}
