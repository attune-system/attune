//! Work queue dispatcher - polls business queues and creates executions.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use attune_common::{
    models::{
        enums::{ExecutionStatus, WorkQueueDispatchStatus, WorkQueueItemStatus},
        key::Key,
        pack::Pack,
        work_queue::{
            WorkQueue, WorkQueueConfig, WorkQueueDispatch, WorkQueueItem, WorkQueueTunableValue,
        },
    },
    mq::{ExecutionRequestedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::{
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
        prepare_secret_values, JsonPointer, RenderedJson, SecretPathSource, SecretSource,
        ENTITY_EXECUTION_CONFIG,
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
    parsed_config: WorkQueueConfig,
    pack_config: JsonValue,
    pack_ref: Option<String>,
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
    release_id: Option<i64>,
    release_digest: Option<String>,
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
                    release_id: execution.pack_release,
                    release_digest: execution.pack_release_digest.clone(),
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
        // Action lifecycle is checked at enqueue admission. Deferred work uses
        // its pinned executable; ordinary disablement is not cancellation.
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
            parsed_config,
            pack_config: pack
                .as_ref()
                .map(|pack| pack.config.clone())
                .unwrap_or(JsonValue::Null),
            pack_ref: pack.as_ref().map(|pack| pack.r#ref.clone()),
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
        attune_common::crypto::keystore::decrypt_json(&key.value, encryption_key, key.id)
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
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let mut items = WorkQueueItemRepository::lease_next_batch(
            &mut *tx,
            LeaseWorkQueueItemsInput {
                separate_requesters: context
                    .parsed_config
                    .dispatch
                    .as_ref()
                    .is_some_and(|dispatch| dispatch.separate_requesters),
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
        let accepted_queue = attune_common::repositories::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::template(&mut tx, queue, &items).await?;
        let accepted_context = if let Some(pinned) = &accepted_queue {
            Some(ResolvedQueueContext {
                parsed_config: serde_json::from_value(pinned.config.clone())?,
                pack_config: JsonValue::Null,
                pack_ref: pinned.pack_ref.clone(),
                concurrency: context.concurrency,
                batch_size: 1,
                inter_execution_delay_seconds: context.inter_execution_delay_seconds,
            })
        } else {
            None
        };
        let queue = accepted_queue.as_ref().unwrap_or(queue);
        let context = accepted_context.as_ref().unwrap_or(context);
        for item in &mut items {
            let admitted = item.payload_binding()?;
            let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *tx,
                attune_common::secret_values::ENTITY_QUEUE_ITEM_PAYLOAD,
                item.id,
            )
            .await?;
            if !secrets.is_empty() {
                // Enqueue accepts literal caller-supplied payloads, not key
                // resolution. Local encrypted values may flow through an
                // unattributed batch without granting Key or API authority.
                for secret in &secrets {
                    let provenance: attune_common::secret_provenance::SecretProvenance = secret
                        .source_ref
                        .as_deref()
                        .and_then(|body| serde_json::from_str(body).ok())
                        .ok_or_else(|| anyhow!("Queue payload provenance is unavailable"))?;
                    if secret.source_kind != "provenance" || provenance.origins.is_empty()
                        || provenance.origins.iter().any(|origin| !matches!(origin, attune_common::secret_provenance::SecretOrigin::Local { source_kind, .. } if source_kind == "parameter_schema" || source_kind == "queue_item")) {
                        return Err(anyhow!("Queue payload origins cannot establish delivery authority"));
                    }
                }
                let encryption_key = encryption_key
                    .ok_or_else(|| anyhow!("Queue payload encryption key is unavailable"))?;
                item.payload = attune_common::secret_values::restore_secret_values(
                    item.payload.clone(),
                    &secrets,
                    encryption_key,
                )?;
            }
            attune_common::value_schema::CompiledContract::new(&admitted.contract)?
                .validate(&item.payload)?;
        }

        let dispatch_snapshot: attune_common::models::ExecutionExecutableSnapshot = items[0]
            .executable_snapshot
            .clone()
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| {
                anyhow!(
                    "Queue item {} has no immutable executable snapshot",
                    items[0].id
                )
            })?;
        if items.iter().any(|item| {
            item.pack_release != Some(dispatch_snapshot.release.id)
                || item.pack_release_digest.as_deref()
                    != Some(dispatch_snapshot.release.digest.as_str())
                || item.executable_snapshot != items[0].executable_snapshot
        }) {
            return Err(anyhow!(
                "A work queue dispatch batch cannot mix pack releases or executable snapshots"
            ));
        }
        let dispatch_action = &dispatch_snapshot.executable.action;
        let recorded = !queue.is_adhoc || attune_common::repositories::execution_admission::ExecutionAdmissionRepository::has_queue_item_admission(&mut tx, &items).await?;
        let admission = if recorded {
            Some(attune_common::repositories::execution_admission::ExecutionAdmissionRepository::queue_batch(&mut tx, queue, &items, &dispatch_snapshot).await?)
        } else {
            None
        };
        let permission_set_refs = admission
            .as_ref()
            .map(|admission| admission.authority().selected_refs.clone())
            .unwrap_or_else(|| {
                queue.permission_set_refs.clone().unwrap_or_else(|| {
                    dispatch_action
                        .default_execution_permission_set_refs
                        .clone()
                })
            });
        let binding = if accepted_queue.is_some() {
            attune_common::repositories::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::schema(&mut tx, items[0].id).await?
        } else if recorded {
            attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action_pair(&mut tx, &dispatch_snapshot).await?
        } else {
            attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action(&mut tx, &dispatch_snapshot).await?
        };
        let input_contract = attune_common::value_schema::CompiledContract::new(&binding.input)?;

        let executor_identity = {
            let mut iter = items.iter().map(|item| item.requested_by_identity);
            let first = iter.next().flatten();
            first.filter(|id| iter.all(|other| other == Some(*id)))
        };
        let (pack_config, pack_secret_paths, pack_revision) = if let Some(reference) =
            &context.pack_ref
        {
            let pack = PackRepository::find_by_ref(&mut *tx, reference)
                .await?
                .ok_or_else(|| anyhow!("Queue configuration owner is unavailable"))?;
            if recorded {
                let mut authorities = Vec::new();
                for item in &items {
                    let authority = attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::item_authorities(&mut tx, item.id).await?;
                    match authority {
                        Some(item_authorities) => authorities.extend(item_authorities),
                        None => {
                            authorities.clear();
                            break;
                        }
                    }
                }
                attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::pack_template_values(&mut tx, &pack, &authorities, encryption_key).await?
            } else {
                let authority = match executor_identity {
                    Some(identity) => Some(
                        attune_common::delegation::DelegationAuthority::load_for_share(
                            &mut tx, identity,
                        )
                        .await?,
                    ),
                    None => None,
                };
                attune_common::repositories::component_configuration::ComponentConfigurationRepository::pack_template_values(
                &mut tx,&pack,authority.as_ref(),encryption_key).await?
            }
        } else {
            (JsonValue::Null, Vec::new(), None)
        };
        let key_context = Self::build_action_params_context(
            queue,
            &context.parsed_config,
            &pack_config,
            &items,
            None,
        )?;
        let key_refs = key_context.referenced_key_refs(&queue.action_params)?;
        let keys = if key_refs.is_empty() {
            std::collections::BTreeMap::new()
        } else {
            if recorded {
                let mut authorities = Vec::new();
                for item in &items {
                    let item_authorities = attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::item_authorities(&mut tx, item.id).await?.ok_or_else(|| anyhow!("Accepted queue item input authority is unavailable"))?;
                    authorities.extend(item_authorities);
                }
                attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::resolve_keys(&mut tx, &authorities, &key_refs, encryption_key).await?
            } else {
                let identity = executor_identity.ok_or_else(|| {
                    anyhow!("Queue key access requires one attributed requester per batch")
                })?;
                let authority = attune_common::delegation::DelegationAuthority::load_for_share(
                    &mut tx, identity,
                )
                .await?;
                attune_common::key_access::resolve_explicit_keys(
                    &mut *tx,
                    &authority,
                    &key_refs,
                    encryption_key,
                )
                .await?
            }
        };

        let rendered_config = Self::build_execution_config(
            queue,
            &context.parsed_config,
            &pack_config,
            context.pack_ref.clone(),
            &pack_secret_paths,
            pack_revision,
            &items,
            keys,
        )?;
        let supplied_config = rendered_config.value.clone();
        let config_with_defaults = if accepted_queue.is_some() {
            rendered_config.value.clone()
        } else {
            input_contract.apply_defaults(&rendered_config.value)?
        };
        if accepted_queue.is_none() {
            input_contract.validate(&config_with_defaults)?;
        }
        attune_common::secret_values::validate_contract_secret_destination_paths(
            &config_with_defaults,
            &input_contract,
            &rendered_config.secret_paths,
        )?;
        let (execution_config, secret_inputs) =
            attune_common::secret_values::merge_contract_secret_redactions(
                config_with_defaults,
                &rendered_config.secret_path_sources,
                &input_contract,
            )?;
        let ad_hoc_secret_inputs = accepted_queue.as_ref().map(|_| secret_inputs.clone());
        let mut prepared_secrets = if secret_inputs.is_empty() || ad_hoc_secret_inputs.is_some() {
            Vec::new()
        } else {
            let encryption_key = encryption_key.ok_or_else(|| {
                anyhow!(
                    "Cannot store secret queue execution parameters without security.encryption_key"
                )
            })?;
            prepare_secret_values(secret_inputs, encryption_key)?
        };
        if admission.is_none() && !permission_set_refs.is_empty() {
            let identity = executor_identity.ok_or_else(|| {
                anyhow::anyhow!("Queue API access requires one attributed requester per batch")
            })?;
            let authority =
                attune_common::delegation::DelegationAuthority::load_for_share(&mut tx, identity)
                    .await?;
            authority
                .require_refs_for_share(&mut tx, &permission_set_refs)
                .await?;
        }
        let reserved_dispatch_id =
            if queue.batch_mode == attune_common::models::WorkQueueBatchMode::Batch {
                Some(Self::reserve_dispatch_id(&mut tx).await?)
            } else {
                None
            };
        let trace_tag = Self::resolve_queue_trace_tag(
            queue,
            &context.parsed_config,
            if recorded {
                &pack_config
            } else {
                &context.pack_config
            },
            &items,
            reserved_dispatch_id,
        )?;
        let mut execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(dispatch_action.id),
                action_ref: dispatch_action.r#ref.clone(),
                config: if accepted_queue.is_some() {
                    None
                } else {
                    Some(execution_config)
                },
                env_vars: None,
                parent: None,
                enforcement: None,
                // Batch contributors do not acquire ownership of all results.
                // Runtime sponsor and per-item attribution live in admission.
                executor: if admission.is_some() {
                    None
                } else {
                    executor_identity
                },
                permission_set_refs,
                artifact_retention_policy: dispatch_action.artifact_retention_policy,
                artifact_retention_limit: dispatch_action.artifact_retention_limit,
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: Some(trace_tag),
                timeout_seconds: Some(dispatch_action.timeout_seconds.unwrap_or(
                    attune_common::config::app_default_execution_timeout_seconds() as i32,
                )),
                result: None,
                workflow_task: None,
            },
            &dispatch_snapshot,
        )
        .await?;
        let mut initial_admission = None;
        let mut accepted_batch_fence = None;
        if let Some(admission) = &admission {
            let record = admission
                .attach(
                    &mut tx,
                    attune_common::repositories::runtime_authority::RuntimeOwner::Execution(
                        execution.id,
                    ),
                )
                .await?;
            initial_admission = Some(record.id);
            attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_queue_execution(&mut tx, execution.id, &items.iter().map(|item| item.id).collect::<Vec<_>>()).await?;
            if accepted_queue.is_some()
                && queue.batch_mode == attune_common::models::WorkQueueBatchMode::Batch
            {
                // record_admission permits only this newly staged record to be
                // unsealed. Seal ALL accepted contributors and independent source
                // dependencies before defaults, initialization or worker delivery
                // can consume canonical destination authority.
                accepted_batch_fence = Some(attune_common::repositories::accepted_batch_dependency::AcceptedBatchDependencyRepository::capture(
                    &mut tx, record.id, &items.iter().map(|item| item.id).collect::<Vec<_>>(),
                ).await?);
            }
        }
        attune_common::repositories::schema_contract::SchemaContractRepository::attach(
            &mut tx,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            execution.id,
            &binding,
        )
        .await?;
        if let Some(inputs) = ad_hoc_secret_inputs {
            let (config, inputs) = attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::workflow_child_defaults(&mut tx, execution.id, &supplied_config, inputs, encryption_key).await?;
            prepared_secrets = if inputs.is_empty() {
                Vec::new()
            } else {
                prepare_secret_values(
                    inputs,
                    encryption_key.ok_or_else(|| {
                        anyhow!("Ad-hoc dispatch input encryption is unavailable")
                    })?,
                )?
            };
            execution = ExecutionRepository::initialize_config_with_conn(
                &mut tx,
                execution.id,
                initial_admission,
                &dispatch_snapshot,
                Some(config),
                &prepared_secrets,
            )
            .await?;
        }
        if !prepared_secrets.is_empty() && accepted_queue.is_none() {
            ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                execution.id,
                &prepared_secrets,
            )
            .await?;
        }
        let input_receipts = if recorded {
            attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
                &mut tx, execution.id, encryption_key.ok_or_else(|| anyhow!("Accepted input receipt encryption is unavailable"))?,
            ).await?
        } else {
            Vec::new()
        };

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

        if let Some(admission) = &admission {
            admission.ensure_fresh_on(&mut tx).await?;
        }
        if let Some(fence) = &accepted_batch_fence {
            fence.ensure_fresh_on(&mut tx).await?;
        }
        for receipt in &input_receipts {
            receipt.ensure_fresh_on(&mut tx).await?;
        }
        tx.commit().await?;
        for receipt in &input_receipts {
            receipt.ensure_fresh()?;
        }
        if let Some(admission) = &admission {
            admission.effect_fence().ensure_fresh()?;
        }
        if let Some(fence) = &accepted_batch_fence {
            fence.ensure_fresh()?;
        }

        Ok(Some(PreparedDispatch {
            dispatch_id: dispatch.id,
            execution_id: execution.id,
            action_id: Some(dispatch_action.id),
            action_ref: dispatch_action.r#ref.clone(),
            config: execution.config.clone(),
            release_id: execution.pack_release,
            release_digest: execution.pack_release_digest.clone(),
            queue: queue.clone(),
            leased_item_ids: items.iter().map(|item| item.id).collect(),
        }))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Render queue config with distinct template, batch, event, schema, and authorized key inputs"
    )]
    fn build_execution_config(
        queue: &WorkQueue,
        parsed_config: &WorkQueueConfig,
        pack_config: &JsonValue,
        pack_ref: Option<String>,
        pack_secret_paths: &[JsonPointer],
        pack_revision: Option<i64>,
        items: &[WorkQueueItem],
        keys: std::collections::BTreeMap<String, attune_common::key_access::ResolvedKey>,
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
            Self::build_action_params_context(queue, parsed_config, pack_config, items, None)?;
        let mut context = context;
        context.set_template_origin("queue", &queue.r#ref);
        context.set_resolved_keys(keys);
        if let Some(revision) = pack_revision {
            context.set_bound_pack_config(pack_config.clone(), revision, pack_secret_paths);
        } else {
            context.set_pack_config_with_secret_paths(
                pack_config.clone(),
                pack_ref,
                pack_secret_paths,
            );
        }
        Self::mark_queue_item_secret_paths(&context, queue, items)?;
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
        match queue.batch_mode {
            attune_common::models::WorkQueueBatchMode::Single => {
                let item = items
                    .first()
                    .ok_or_else(|| anyhow!("single-item queue dispatch had no leased item"))?;
                let item_secret_paths = item.payload_secret_paths()?;
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
                            queue_id: queue.id,
                            pack_ref: queue.pack_ref.clone(),
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
                    let item_secret_paths = item.payload_secret_paths()?;
                    for path in &item_secret_paths {
                        secret_path_sources.push(SecretPathSource {
                            path: format!("/items/{idx}{path}"),
                            source: SecretSource::QueueItem {
                                queue_id: queue.id,
                                pack_ref: queue.pack_ref.clone(),
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
    ) -> Result<WorkflowContext> {
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
                let item_secret_paths = item.payload_secret_paths()?;
                if item_secret_paths.is_empty() {
                    context.set_current_item(item.payload.clone(), 0);
                } else {
                    context.set_current_item_with_secret_paths(
                        item.payload.clone(),
                        0,
                        &item_secret_paths,
                        |path| SecretSource::QueueItem {
                            queue_id: queue.id,
                            pack_ref: queue.pack_ref.clone(),
                            queue_ref: Some(queue.r#ref.clone()),
                            item_id: Some(item.id),
                            path: path.clone(),
                        },
                    );
                }
                context.set_var("queue_item", Self::build_queue_item_context(item));
            }
        }
        Ok(context)
    }

    fn mark_queue_item_secret_paths(
        context: &WorkflowContext,
        queue: &WorkQueue,
        items: &[WorkQueueItem],
    ) -> Result<()> {
        for (idx, item) in items.iter().enumerate() {
            let item_secret_paths = item.payload_secret_paths()?;
            context.mark_secret_pointer_paths(
                &format!("items.{idx}"),
                &item_secret_paths,
                |path| SecretSource::QueueItem {
                    queue_id: queue.id,
                    pack_ref: queue.pack_ref.clone(),
                    queue_ref: Some(queue.r#ref.clone()),
                    item_id: Some(item.id),
                    path: path.clone(),
                },
            );
            context.mark_secret_pointer_paths(
                &format!("queue_items.{idx}.payload"),
                &item_secret_paths,
                |path| SecretSource::QueueItem {
                    queue_id: queue.id,
                    pack_ref: queue.pack_ref.clone(),
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
                        queue_id: queue.id,
                        pack_ref: queue.pack_ref.clone(),
                        queue_ref: Some(queue.r#ref.clone()),
                        item_id: Some(item.id),
                        path: path.clone(),
                    },
                );
            }
        }
        Ok(())
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
            )?;
            Self::mark_queue_item_secret_paths(&context, queue, items)?;
            let rendered = context
                .render_json_with_sensitivity(&JsonValue::String(template.clone()))
                .with_context(|| {
                    format!(
                        "failed to render trace_tag_template for queue '{}'",
                        queue.r#ref
                    )
                })?;
            if !rendered.secret_paths.is_empty() {
                return Err(anyhow!("Queue trace tags cannot contain secret values"));
            }
            let rendered_string = match rendered.value {
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
        // Publication/recovery starts a fresh source-only transaction. Validate
        // retained delivery proofs, never reseal with current grants or rewrite
        // the config with a cached pre-seal ciphertext/provenance buffer.
        let mut source = self.pool.begin().await?;
        let record =
            attune_common::repositories::execution_admission::ExecutionAdmissionRepository::find(
                &mut source,
                attune_common::repositories::runtime_authority::RuntimeOwner::Execution(
                    dispatch.execution_id,
                ),
            )
            .await?;
        let runtime_fence = if let Some(record) = &record {
            Some(attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(&mut source, record.id).await?)
        } else {
            None
        };
        let input_receipts = if record.is_some() {
            attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(
                &mut source, dispatch.execution_id, self.encryption_key.as_deref().ok_or_else(|| anyhow!("Accepted input receipt encryption is unavailable"))?,
            ).await?
        } else if dispatch.queue.is_adhoc {
            Vec::new()
        } else {
            return Err(anyhow!(
                "Managed queue publication has no retained execution admission"
            ));
        };
        for receipt in &input_receipts {
            receipt.ensure_fresh()?;
        }
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
            release_id: dispatch.release_id,
            release_digest: dispatch.release_digest,
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

        for receipt in &input_receipts {
            receipt.ensure_fresh_on(&mut source).await?;
        }
        if let Some(fence) = &runtime_fence {
            fence.ensure_fresh_on(&mut source).await?;
        }
        source.commit().await?;
        for receipt in &input_receipts {
            receipt.ensure_fresh()?;
        }
        if let Some(fence) = &runtime_fence {
            fence.ensure_fresh()?;
        }

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

        for receipt in &input_receipts {
            receipt.ensure_fresh()?;
        }
        if let Some(fence) = &runtime_fence {
            fence.ensure_fresh()?;
        }
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
            enabled_override: None,
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
            retired_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    fn sample_item(id: i64, priority: i32, payload: JsonValue) -> WorkQueueItem {
        let schema = json!({});
        let contract = attune_common::value_schema::compile(
            attune_common::value_schema::SourceIdentity::Assigned {
                component_ref: "core.inbox".into(),
                assignment_id: Uuid::new_v4(),
            },
            attune_common::value_schema::SchemaContext::FlatDocument,
            &schema,
            &attune_common::value_schema::RegistryView::new(),
            attune_common::value_schema::Limits::default(),
        )
        .unwrap();
        let schema_binding =
            serde_json::to_value(attune_common::value_schema::ValueSchemaBinding {
                authored_schema: schema,
                contract,
            })
            .unwrap();
        WorkQueueItem {
            id,
            queue: 1,
            queue_ref: "core.inbox".to_string(),
            pack_release: None,
            pack_release_digest: None,
            executable_snapshot: None,
            item_key: Some(format!("item-{id}")),
            priority,
            status: WorkQueueItemStatus::Queued,
            payload,
            schema_binding: Some(schema_binding),
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
            None,
            &items,
            Default::default(),
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
            None,
            &[item],
            Default::default(),
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
            None,
            &[item],
            Default::default(),
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
    fn queue_pack_templates_record_the_configuration_revision_instead_of_mutable_schema_origins() {
        let queue = sample_queue(
            WorkQueueBatchMode::Single,
            json!({"token":"{{ config.token }}"}),
            json!({}),
        );
        let parsed = serde_json::from_value(queue.config.clone()).unwrap();
        let item = sample_item(42, 1, json!({}));
        let rendered = WorkQueueDispatcher::build_execution_config(
            &queue,
            &parsed,
            &json!({"token":"queue-config-fixture"}),
            Some("test.pack".into()),
            &["/token".into()],
            Some(19),
            &[item],
            Default::default(),
        )
        .unwrap();
        assert_eq!(rendered.value["token"], "queue-config-fixture");
        assert!(rendered.secret_path_sources.iter().any(|source| source.path == "/token" && source.source.provenance().origins.iter().any(|origin|
            matches!(origin,attune_common::secret_provenance::SecretOrigin::Entity {entity_type,entity_id,path}
                if entity_type == attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION && *entity_id == 19 && path == "/token"))));
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
        let encrypted = attune_common::crypto::keystore::encrypt_json(
            &json!({"dispatch": {"batch_size": 3}}),
            &encryption_key,
            1,
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
        let encrypted = attune_common::crypto::keystore::encrypt_json(
            &json!({"dispatch": {"batch_size": 3}}),
            &encryption_key,
            1,
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
            None,
            &items,
            Default::default(),
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
