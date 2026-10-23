//! Identical-input workflow retries. Prepare retained evidence before acquiring
//! scheduler/workflow/execution target locks; never render or recapture inputs.
use super::{
    accepted_execution_source::{prepare_observation, same_execution},
    declared_input_materialization::DeclaredInputMaterializationRepository as Materialization,
    execution::{CreateExecutionInput, ExecutionRepository},
    execution_runtime_admission::{ExecutionAdmission, ExecutionAdmissionRepository},
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::ReceiptContext,
    recorded_input_authority::RecordedInputAuthorityRepository,
    rule_authority::RuleAuthorityRepository,
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository, RuntimeOwner},
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository},
    schema_type::SchemaTypeRepository,
    workflow::WorkflowExecutionRepository,
    FindById,
};
use crate::{
    models::{Execution, ExecutionStatus, Id, WorkflowExecution, WorkflowTaskMetadata},
    secret_values::{StoredSecretValue, ENTITY_EXECUTION_CONFIG},
    Error, Result,
};
use serde_json::Value;
use sqlx::PgConnection;

pub struct NativeRetryMaterializationRepository;

pub struct PreparedNativeRetry {
    source: Execution,
    workflow: WorkflowExecution,
    admission: ExecutionAdmission,
    schema: ActionSchemaBinding,
    fields: Vec<StoredSecretValue>,
    receipts: Vec<ReceiptContext>,
    key: String,
}

pub struct PreparedRetryPublication {
    retry: Execution,
    workflow: Id,
    source: PublicationSource,
    fence: ApprovalEffectFence,
    receipts: Vec<ReceiptContext>,
    schema: ActionSchemaBinding,
    fields: Vec<StoredSecretValue>,
    cache: Option<super::requested_execution_publication::PreparedCachePublication>,
}

#[derive(Clone, Copy)]
enum PublicationSource {
    InitialTask,
    Retry(Id),
}

impl NativeRetryMaterializationRepository {
    /// Re-publish an already accepted FIRST workflow task attempt. This only
    /// replays retained proof; it never invokes cache/wait/iteration rendering,
    /// materialization, pin acquisition or child creation.
    pub(crate) async fn prepare_existing_workflow(
        conn: &mut PgConnection,
        child: &Execution,
        key: &str,
    ) -> Result<PreparedRetryPublication> {
        let task = child.workflow_task.as_ref().ok_or_else(denied)?;
        if child.status != ExecutionStatus::Requested
            || child.retry_count != 0
            || task.retry_count != 0
            || child.original_execution.is_some()
        {
            return Err(denied());
        }
        let admission =
            ExecutionAdmissionRepository::require(conn, RuntimeOwner::Execution(child.id)).await?;
        let (_, binding) = ExecutionAdmissionRepository::workflow_invocation(
            conn,
            child.parent.ok_or_else(denied)?,
            &task.task_name,
            &child.action_ref,
        )
        .await?;
        if admission.authority.binding != binding {
            return Err(denied());
        }
        let fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?;
        let schema = SchemaContractRepository::find(conn, BindingOwner::Execution, child.id)
            .await?
            .ok_or_else(denied)?;
        schema.validate()?;
        let (fields, receipts) = prepare_observation(conn, child, &schema, key).await?;
        let cache = super::requested_execution_publication::prepare_cache_publication(
            conn,
            child,
            &admission.authority.binding,
            key,
        )
        .await?;
        let prepared = PreparedRetryPublication {
            retry: child.clone(),
            workflow: task.workflow_execution,
            source: PublicationSource::InitialTask,
            fence,
            receipts,
            schema,
            fields,
            cache,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }
    /// Existing Requested recovery uses the committed retry edge, not an
    /// ephemeral completion payload or a caller-selected original execution.
    pub async fn prepare_requested_recovery(
        conn: &mut PgConnection,
        retry: &Execution,
        key: &str,
    ) -> Result<Option<PreparedRetryPublication>> {
        let task = retry.workflow_task.as_ref().ok_or_else(denied)?;
        if retry.status != ExecutionStatus::Requested {
            return Ok(None);
        }
        if retry.retry_count <= 0 || task.retry_count != retry.retry_count {
            return Err(denied());
        }
        let due: bool = sqlx::query_scalar("SELECT $1::TIMESTAMPTZ <= clock_timestamp()")
            .bind(task.next_retry_at.ok_or_else(denied)?)
            .fetch_one(&mut *conn)
            .await?;
        if !due {
            return Ok(None);
        }
        let sources: Vec<Id> = sqlx::query_scalar("SELECT original.owner_id FROM runtime_admission_dependency edge JOIN runtime_authority_admission target ON target.id=edge.admission JOIN runtime_authority_admission original ON original.id=edge.dependency WHERE target.owner_kind='execution' AND target.owner_id=$1 AND edge.dependency_kind='retry' AND original.owner_kind='execution' ORDER BY original.owner_id LIMIT 2")
            .bind(retry.id).fetch_all(&mut *conn).await?;
        let [source] = sources.as_slice() else {
            return Err(denied());
        };
        Self::prepare_publication(conn, retry, *source, key)
            .await
            .map(Some)
    }

    pub async fn prepare(
        conn: &mut PgConnection,
        expected: &Execution,
        workflow: &WorkflowExecution,
        key: &str,
    ) -> Result<PreparedNativeRetry> {
        let task = expected.workflow_task.as_ref().ok_or_else(denied)?;
        if !matches!(
            expected.status,
            ExecutionStatus::Failed | ExecutionStatus::Timeout
        ) || expected.parent != Some(workflow.execution)
            || task.workflow_execution != workflow.id
            || task.retry_count < 0
            || expected.retry_count != task.retry_count
            || task.retry_count >= task.max_retries
            || !active(workflow.status)
            || workflow.paused
            || !workflow.current_tasks.contains(&task.task_name)
        {
            return Err(denied());
        }
        SchemaTypeRepository::lock_selection(conn).await?;
        let actual = ExecutionRepository::find_by_id(&mut *conn, expected.id)
            .await?
            .ok_or_else(denied)?;
        same_execution(expected, &actual)?;
        let snapshot = actual.executable_snapshot.as_ref().ok_or_else(denied)?;
        let admission = ExecutionAdmissionRepository::inherit(
            conn,
            RuntimeOwner::Execution(actual.id),
            snapshot,
        )
        .await?;
        let schema = SchemaContractRepository::find(conn, BindingOwner::Execution, actual.id)
            .await?
            .ok_or_else(denied)?;
        schema.validate()?;
        let (fields, receipts) = prepare_observation(conn, &actual, &schema, key).await?;
        let prepared = PreparedNativeRetry {
            source: actual,
            workflow: workflow.clone(),
            admission,
            schema,
            fields,
            receipts,
            key: key.into(),
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }

    /// Backoff ends the creation transaction. Authenticate the SAME accepted
    /// retry, without resealing it or inheriting another attempt UUID.
    pub async fn prepare_publication(
        conn: &mut PgConnection,
        retry: &Execution,
        source: Id,
        key: &str,
    ) -> Result<PreparedRetryPublication> {
        let task = retry.workflow_task.as_ref().ok_or_else(denied)?;
        let admission =
            ExecutionAdmissionRepository::require(conn, RuntimeOwner::Execution(retry.id)).await?;
        let fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?;
        let inherited: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_admission_dependency edge JOIN runtime_authority_admission original ON original.id=edge.dependency WHERE edge.admission=$1 AND edge.dependency_kind='retry' AND original.owner_kind='execution' AND original.owner_id=$2)")
            .bind(admission.id).bind(source).fetch_one(&mut *conn).await?;
        if !inherited {
            return Err(denied());
        }
        let schema = SchemaContractRepository::find(conn, BindingOwner::Execution, retry.id)
            .await?
            .ok_or_else(denied)?;
        let (fields, receipts) = prepare_observation(conn, retry, &schema, key).await?;
        let cache = super::requested_execution_publication::prepare_cache_publication(
            conn,
            retry,
            &admission.authority.binding,
            key,
        )
        .await?;
        let prepared = PreparedRetryPublication {
            retry: retry.clone(),
            workflow: task.workflow_execution,
            source: PublicationSource::Retry(source),
            fence,
            receipts,
            schema,
            fields,
            cache,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }
}

impl PreparedNativeRetry {
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.admission.ensure_fresh_on(conn).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }

    /// All writes, including the first seal, belong to the caller's original
    /// retry-creation transaction. An error requires rolling that transaction back.
    pub async fn materialize(
        &self,
        conn: &mut PgConnection,
        metadata: WorkflowTaskMetadata,
    ) -> Result<Option<Execution>> {
        let previous = self.source.workflow_task.as_ref().ok_or_else(denied)?;
        let next = previous.retry_count.checked_add(1).ok_or_else(denied)?;
        let mut expected = previous.clone();
        expected.retry_count = next;
        expected.max_retries = metadata.max_retries;
        expected.next_retry_at = metadata.next_retry_at;
        expected.started_at = None;
        expected.completed_at = None;
        expected.duration_ms = None;
        expected.timed_out = false;
        let limit = self
            .workflow
            .task_graph
            .pointer(&format!(
                "/nodes/{}/retry/count",
                previous.task_name.replace('~', "~0").replace('/', "~1")
            ))
            .and_then(Value::as_i64)
            .ok_or_else(denied)?;
        if metadata != expected
            || i64::from(metadata.max_retries) != limit
            || next > metadata.max_retries
            || metadata.next_retry_at.is_none()
        {
            return Err(denied());
        }
        lock_workflow(conn, self.workflow.id).await?;
        let workflow =
            WorkflowExecutionRepository::find_by_id_for_update(&mut *conn, self.workflow.id)
                .await?
                .ok_or_else(denied)?;
        if serde_json::to_value(&workflow)? != serde_json::to_value(&self.workflow)? {
            return Err(denied());
        }
        let parent = ExecutionRepository::find_by_id_for_update(&mut *conn, workflow.execution)
            .await?
            .ok_or_else(denied)?;
        if !active(parent.status) {
            return Err(denied());
        }
        let actual = ExecutionRepository::find_by_id_for_update(&mut *conn, self.source.id)
            .await?
            .ok_or_else(denied)?;
        same_execution(&self.source, &actual)?;
        if Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_CONFIG, actual.id).await?
            != self.fields
        {
            return Err(denied());
        }
        // The workflow lock serializes competing completion deliveries. A
        // committed successor means this stale completion must not spawn again.
        let existing: Option<Id> = sqlx::query_scalar("SELECT id FROM execution WHERE original_execution = $1 AND retry_count >= $2 AND workflow_task->>'workflow_execution' = $3 AND workflow_task->>'task_name' = $4 AND (workflow_task->'task_index') IS NOT DISTINCT FROM $5::JSONB AND (workflow_task->'task_batch') IS NOT DISTINCT FROM $6::JSONB ORDER BY retry_count,id LIMIT 1")
            .bind(self.source.original_execution.unwrap_or(self.source.id)).bind(next)
            .bind(self.workflow.id.to_string()).bind(&previous.task_name)
            .bind(serde_json::to_value(previous)?.get("task_index").cloned())
            .bind(serde_json::to_value(previous)?.get("task_batch").cloned())
            .fetch_optional(&mut *conn).await?;
        if let Some(id) = existing {
            let existing = ExecutionRepository::find_by_id(&mut *conn, id)
                .await?
                .ok_or_else(denied)?;
            self.ensure_fresh_on(conn).await?;
            if existing.status != ExecutionStatus::Requested || existing.worker.is_some() {
                return Ok(None);
            }
            if existing.config != self.source.config
                || existing.parent != self.source.parent
                || serde_json::to_value(&existing.executable_snapshot)?
                    != serde_json::to_value(&self.source.executable_snapshot)?
            {
                return Err(denied());
            }
            // Recover publication after commit/broker failure without attaching,
            // resealing or rotating the already committed successor again.
            return Ok(Some(existing));
        }
        self.ensure_fresh_on(conn).await?;
        let retry = ExecutionRepository::create_retry(
            &mut *conn,
            CreateExecutionInput {
                action: actual.action,
                action_ref: actual.action_ref.clone(),
                config: actual.config.clone(),
                env_vars: actual.env_vars.clone(),
                parent: actual.parent,
                enforcement: actual.enforcement,
                executor: actual.executor,
                permission_set_refs: actual.permission_set_refs.clone(),
                artifact_retention_policy: actual.artifact_retention_policy,
                artifact_retention_limit: actual.artifact_retention_limit,
                worker_selector: actual.worker_selector.clone(),
                worker_tolerations: actual.worker_tolerations.clone(),
                worker_affinity: actual.worker_affinity.clone(),
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: actual.trace_tag.clone(),
                timeout_seconds: actual.timeout_seconds,
                result: None,
                workflow_task: Some(metadata),
            },
            actual.executable_snapshot.as_ref().ok_or_else(denied)?,
            next,
            Some(expected.max_retries),
            Some(format!("{:?}", actual.status).to_lowercase()),
            actual.original_execution.unwrap_or(actual.id),
        )
        .await?;
        // attach invokes canonical DeclaredInputRepository::copy_deliveries,
        // which checks identical config and the real inherited admission edge.
        self.admission
            .attach(conn, RuntimeOwner::Execution(retry.id))
            .await?;
        RuleAuthorityRepository::copy_execution(conn, actual.id, retry.id).await?;
        RecordedInputAuthorityRepository::copy(conn, "execution", actual.id, "execution", retry.id)
            .await?;
        SchemaContractRepository::attach(conn, BindingOwner::Execution, retry.id, &self.schema)
            .await?;
        Secrets::copy_entity(
            &mut *conn,
            ENTITY_EXECUTION_CONFIG,
            actual.id,
            ENTITY_EXECUTION_CONFIG,
            retry.id,
        )
        .await?;
        // Only this newly created retry may receive its first acceptance seal.
        let receipts = Materialization::seal_execution_inputs(conn, retry.id, &self.key).await?;
        for receipt in &receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh_on(conn).await?;
        Ok(Some(retry))
    }
}

impl PreparedRetryPublication {
    /// Must precede publication. Caller retains this transaction's locks through
    /// broker confirmation and rechecks deadlines before commit/unlock.
    pub async fn lock_target(&self, conn: &mut PgConnection) -> Result<()> {
        lock_workflow(conn, self.workflow).await?;
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(self.workflow)
            .fetch_optional(&mut *conn)
            .await?;
        let workflow =
            WorkflowExecutionRepository::find_by_id_for_update(&mut *conn, self.workflow)
                .await?
                .ok_or_else(denied)?;
        let task = self.retry.workflow_task.as_ref().ok_or_else(denied)?;
        if !active(workflow.status)
            || workflow.paused
            || !workflow.current_tasks.contains(&task.task_name)
            || self.retry.parent != Some(workflow.execution)
        {
            return Err(denied());
        }
        lock_execution(conn, workflow.execution).await?;
        let parent = ExecutionRepository::find_by_id_for_update(&mut *conn, workflow.execution)
            .await?
            .ok_or_else(denied)?;
        if !active(parent.status) {
            return Err(denied());
        }
        // Original execution IDs intentionally survive row retention. Its
        // retained admission/receipt edge is already authenticated above. If
        // the row survives, explicit cancellation or outcome changes still deny.
        if let PublicationSource::Retry(id) = self.source {
            lock_execution(conn, id).await?;
            if let Some(source) = ExecutionRepository::find_by_id_for_update(&mut *conn, id).await?
            {
                if !matches!(
                    source.status,
                    ExecutionStatus::Failed | ExecutionStatus::Timeout
                ) {
                    return Err(denied());
                }
            }
        }
        if let Some(wait) =
            super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(
                &mut *conn,
                self.workflow,
                &task.task_name,
            )
            .await?
        {
            if wait.state != crate::models::enums::WorkflowTaskWaitState::Released {
                return Err(denied());
            }
        }
        lock_execution(conn, self.retry.id).await?;
        let actual = ExecutionRepository::find_by_id_for_update(&mut *conn, self.retry.id)
            .await?
            .ok_or_else(denied)?;
        same_execution(&self.retry, &actual)?;
        if actual.status != ExecutionStatus::Requested || actual.worker.is_some() {
            return Err(denied());
        }
        if SchemaContractRepository::find(conn, BindingOwner::Execution, actual.id)
            .await?
            .as_ref()
            != Some(&self.schema)
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_CONFIG, actual.id)
                .await?
                != self.fields
        {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }

    /// Publisher completion is not a database callback acknowledgement. Broker
    /// confirms remain configuration-dependent. A timeout has UNKNOWN delivery,
    /// never success. Caller rolls back its mark and recovery may reissue the
    /// SAME row/admission after fresh checks, with at-least-once delivery.
    pub async fn publish_bounded<F>(&self, conn: &mut PgConnection, publish: F) -> Result<()>
    where
        F: std::future::Future<Output = crate::mq::MqResult<()>>,
    {
        self.ensure_fresh_on(conn).await?;
        super::requested_execution_publication::await_publication(publish).await?;
        self.ensure_fresh_on(conn).await
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.fence.ensure_fresh_on(conn).await?;
        if let Some(cache) = &self.cache {
            cache.ensure_fresh_on(conn).await?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}

async fn lock_workflow(conn: &mut PgConnection, workflow: Id) -> Result<()> {
    // Canonical source admission preparation retains owner SHARE fences. Never
    // wait behind a scheduler/canceller holding this lock and upgrading an owner.
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(workflow)
        .fetch_one(conn)
        .await?;
    if !acquired {
        return Err(denied());
    }
    Ok(())
}
async fn lock_execution(conn: &mut PgConnection, execution: Id) -> Result<()> {
    sqlx::query("SELECT id FROM execution WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(execution)
        .fetch_optional(conn)
        .await?;
    Ok(())
}
fn active(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Requested
            | ExecutionStatus::Scheduling
            | ExecutionStatus::Scheduled
            | ExecutionStatus::Running
    )
}
fn denied() -> Error {
    Error::PermissionDenied("Retained identical-input retry is unavailable".into())
}
