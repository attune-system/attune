//! Re-publication of an EXISTING admitted/accepted Requested execution. Nothing
//! here inherits, attaches, initializes, seals, renders, or selects new authority.
use super::{
    accepted_execution_source::{prepare_observation, same_execution},
    execution::{ExecutionRepository, SELECT_COLUMNS},
    execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::ReceiptContext,
    native_retry_materialization::{
        NativeRetryMaterializationRepository as Retries, PreparedRetryPublication,
    },
    runtime_authority::{
        ApprovalEffectFence, RuntimeAuthorityRepository as Authority, RuntimeOwner,
    },
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    FindById,
};
use crate::{
    models::{Execution, ExecutionStatus, Id},
    secret_values::{StoredSecretValue, ENTITY_EXECUTION_CONFIG},
    Error, Result,
};
use sqlx::PgConnection;
use std::sync::OnceLock;

pub struct RequestedExecutionPublicationRepository;

#[expect(
    clippy::large_enum_variant,
    reason = "Publication consumes the complete owned standalone or workflow proof exactly once"
)]
enum Continuation {
    Standalone {
        schema: ActionSchemaBinding,
        fields: Vec<StoredSecretValue>,
        fence: ApprovalEffectFence,
        receipts: Vec<ReceiptContext>,
    },
    Workflow(PreparedRetryPublication),
}

/// Can only be issued from actual repository records in the caller's transaction.
pub struct PreparedRequestedPublication {
    execution: Execution,
    transaction: String,
    continuation: Continuation,
    locked: OnceLock<()>,
}

impl RequestedExecutionPublicationRepository {
    pub async fn prepare(
        conn: &mut PgConnection,
        expected: &Execution,
        key: &str,
    ) -> Result<Option<PreparedRequestedPublication>> {
        let actual = ExecutionRepository::find_by_id(&mut *conn, expected.id)
            .await?
            .ok_or_else(denied)?;
        same_execution(expected, &actual)?;
        if actual.status != ExecutionStatus::Requested {
            return Ok(None);
        }
        if actual.worker.is_some() {
            return Err(denied());
        }
        let continuation = if let Some(task) = &actual.workflow_task {
            let prepared = if actual.retry_count > 0
                || task.retry_count > 0
                || actual.original_execution.is_some()
            {
                let Some(prepared) =
                    Retries::prepare_requested_recovery(conn, &actual, key).await?
                else {
                    return Ok(None);
                };
                prepared
            } else {
                Retries::prepare_existing_workflow(conn, &actual, key).await?
            };
            Continuation::Workflow(prepared)
        } else {
            let admission = Admissions::require(conn, RuntimeOwner::Execution(actual.id)).await?;
            // A task-scoped capsule cannot silently become standalone merely
            // because mutable task metadata disappeared.
            if matches!(
                &admission.authority.binding,
                crate::runtime_authority::InvocationBinding::WorkflowTask { .. }
            ) {
                return Err(denied());
            }
            if let crate::runtime_authority::RequestAttribution::ParentExecution { execution_id } =
                &admission.authority.attribution
            {
                if actual.parent != Some(*execution_id) {
                    return Err(denied());
                }
            }
            if actual.retry_count > 0 || actual.original_execution.is_some() {
                if actual.retry_count <= 0 {
                    return Err(denied());
                }
                let sources: Vec<Id> = sqlx::query_scalar("SELECT original.id FROM runtime_admission_dependency edge JOIN runtime_authority_admission original ON original.id=edge.dependency WHERE edge.admission=$1 AND edge.dependency_kind='retry' AND original.owner_kind='execution' ORDER BY original.id LIMIT 2")
                    .bind(admission.id).fetch_all(&mut *conn).await?;
                let [source] = sources.as_slice() else {
                    return Err(denied());
                };
                let original = Authority::require_retained_admission(conn, *source).await?;
                let mut selection = original.authority.0;
                selection.attempt = admission.authority.attempt;
                if selection != admission.authority.0 {
                    return Err(denied());
                }
            }
            let fence = Authority::retained_admission_fence(conn, admission.id).await?;
            let schema = Schemas::find(conn, BindingOwner::Execution, actual.id)
                .await?
                .ok_or_else(denied)?;
            schema.validate()?;
            let (fields, receipts) = prepare_observation(conn, &actual, &schema, key).await?;
            Continuation::Standalone {
                schema,
                fields,
                fence,
                receipts,
            }
        };
        let prepared = PreparedRequestedPublication {
            execution: actual,
            transaction: transaction(conn).await?,
            continuation,
            locked: OnceLock::new(),
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(Some(prepared))
    }
}

impl PreparedRequestedPublication {
    pub fn execution(&self) -> &Execution {
        &self.execution
    }

    pub async fn lock_target(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        match &self.continuation {
            Continuation::Workflow(prepared) => prepared.lock_target(conn).await?,
            Continuation::Standalone { schema, fields, .. } => {
                // Owner SHARE fences were prepared already. Never wait while
                // upgrading behind another source holder or a parent canceller.
                if let Some(parent) = self.execution.parent {
                    let status: Option<ExecutionStatus> = sqlx::query_scalar(
                        "SELECT status FROM execution WHERE id=$1 FOR SHARE NOWAIT",
                    )
                    .bind(parent)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(lock_error)?;
                    if status.is_some_and(|status| {
                        matches!(
                            status,
                            ExecutionStatus::Canceling
                                | ExecutionStatus::Cancelled
                                | ExecutionStatus::Abandoned
                        )
                    }) {
                        return Err(denied());
                    }
                }
                let actual = sqlx::query_as::<_, Execution>(&format!(
                    "SELECT {SELECT_COLUMNS} FROM execution WHERE id=$1 FOR UPDATE NOWAIT"
                ))
                .bind(self.execution.id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(lock_error)?
                .ok_or_else(denied)?;
                same_execution(&self.execution, &actual)?;
                if actual.status != ExecutionStatus::Requested
                    || actual.worker.is_some()
                    || Schemas::find(conn, BindingOwner::Execution, actual.id)
                        .await?
                        .as_ref()
                        != Some(schema)
                    || Secrets::find_stored_by_entity(
                        &mut *conn,
                        ENTITY_EXECUTION_CONFIG,
                        actual.id,
                    )
                    .await?
                        != *fields
                {
                    return Err(denied());
                }
            }
        }
        self.ensure_fresh_on(conn).await?;
        self.locked.set(()).map_err(|_| denied())
    }

    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if transaction(conn).await? != self.transaction {
            return Err(denied());
        }
        match &self.continuation {
            Continuation::Workflow(prepared) => prepared.ensure_fresh_on(conn).await,
            Continuation::Standalone {
                fence, receipts, ..
            } => {
                fence.ensure_fresh_on(conn).await?;
                for receipt in receipts {
                    receipt.ensure_fresh_on(conn).await?;
                }
                Ok(())
            }
        }
    }

    pub async fn publish_bounded<F>(&self, conn: &mut PgConnection, publish: F) -> Result<()>
    where
        F: std::future::Future<Output = crate::mq::MqResult<()>>,
    {
        if self.locked.get().is_none() {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await?;
        await_publication(publish).await?;
        self.ensure_fresh_on(conn).await
    }
}

pub(crate) async fn await_publication<F>(publish: F) -> Result<()>
where
    F: std::future::Future<Output = crate::mq::MqResult<()>>,
{
    tokio::time::timeout(std::time::Duration::from_secs(5), publish).await
        .map_err(|_| Error::Timeout("Requested publication acknowledgement is unknown; retain the existing row for recovery".into()))??;
    Ok(())
}

async fn transaction(conn: &mut PgConnection) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
        .fetch_one(conn)
        .await?)
}
fn lock_error(error: sqlx::Error) -> Error {
    if error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
        == Some("55P03")
    {
        Error::InvalidState(
            "Requested publication target is busy; retry without changing its authority".into(),
        )
    } else {
        error.into()
    }
}
fn denied() -> Error {
    Error::PermissionDenied("Retained Requested publication is unavailable".into())
}

/// Existing cache children retain both input receipts and a dispatch slot. An
/// archived cache page alone is not permission to dispatch a different slot or
/// to turn an indexed child into an ordinary task after cursor reclamation.
pub(crate) struct PreparedCachePublication {
    workflow: Id,
    task: String,
    index: i32,
    dispatch: Id,
    iteration: Id,
    namespace: Id,
    generation: Id,
    snapshots: Vec<Id>,
    fence: ApprovalEffectFence,
    controls: Vec<ReceiptContext>,
    source: super::native_input_snapshot::NativeSourceEvidence,
}

pub(crate) async fn prepare_cache_publication(
    conn: &mut PgConnection,
    child: &Execution,
    binding: &crate::runtime_authority::InvocationBinding,
    key: &str,
) -> Result<Option<PreparedCachePublication>> {
    use super::{
        declared_input_materialization::pointer_to_node,
        inherited_input::InheritedInputRepository as Inputs,
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
        workflow_policy::task_at_authored_path,
    };
    let crate::runtime_authority::InvocationBinding::WorkflowTask { task_path, .. } = binding
    else {
        return Err(denied());
    };
    let parent = ExecutionRepository::find_by_id(&mut *conn, child.parent.ok_or_else(denied)?)
        .await?
        .ok_or_else(denied)?;
    let definition = &parent
        .executable_snapshot
        .as_ref()
        .ok_or_else(denied)?
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    let authored = task_at_authored_path(definition, task_path).ok_or_else(denied)?;
    if authored
        .get("iterate_cache")
        .is_none_or(serde_json::Value::is_null)
    {
        return Ok(None);
    }
    let task = child.workflow_task.as_ref().ok_or_else(denied)?;
    let index = task
        .task_index
        .filter(|index| *index >= 0)
        .ok_or_else(denied)?;
    if task.task_batch.is_none_or(|count| count <= 0) {
        return Err(denied());
    }
    let batch = usize::try_from(task.task_batch.ok_or_else(denied)?).map_err(|_| denied())?;
    let iteration =
        Iterations::find_by_workflow_task(&mut *conn, task.workflow_execution, &task.task_name)
            .await?
            .ok_or_else(denied)?;
    // Exact source keys only. Hold these SHARE pins before any workflow/owner
    // UPDATE locks; final checks below never reconstruct or scan a source graph.
    let _: Id = sqlx::query_scalar("SELECT id FROM workflow_cache_iteration WHERE id=$1 FOR SHARE")
        .bind(iteration.id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
    let dispatch = child.original_execution.unwrap_or(child.id);
    let assigned: Option<Id> = sqlx::query_scalar("SELECT execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index=$3 FOR SHARE")
        .bind(task.workflow_execution).bind(&task.task_name).bind(index).fetch_optional(&mut *conn).await?.flatten();
    if assigned != Some(dispatch) {
        return Err(denied());
    }
    // Exact slot lookup, not a membership inventory. Constant-input cache tasks
    // still have a genuine page/dispatch even without a VALUE reference to item.
    let bound: Vec<Id> = sqlx::query_scalar("SELECT snapshot FROM runtime_native_cache_frontier WHERE bound_iteration=$1 AND ordinal=$2 AND bound_transaction=prepared_transaction ORDER BY snapshot LIMIT 2 FOR SHARE")
        .bind(iteration.id).bind(i64::from(index)).fetch_all(&mut *conn).await?;
    let [snapshot] = bound.as_slice() else {
        return Err(denied());
    };
    let storage = Snapshots::storage_evidence(conn, *snapshot).await?;
    let cache = storage.cache().ok_or_else(denied)?;
    if cache.iteration() != iteration.id
        || cache.workflow() != task.workflow_execution
        || cache.namespace() != iteration.namespace
        || cache.generation() != iteration.generation
        || cache.ordinal() != i64::from(index)
        || cache.entries().len() != batch
    {
        return Err(denied());
    }
    let parent_admission = Admissions::require(conn, RuntimeOwner::Execution(parent.id)).await?;
    let path = format!("/workflow{}/params", pointer_to_node(definition, authored)?);
    let evidence = Snapshots::evidence(
        conn,
        *snapshot,
        parent_admission.id,
        &path,
        "/item",
        key,
        false,
    )
    .await?;
    let source = Snapshots::source_evidence(conn, storage.metadata(), key).await?;
    if evidence.parents.len() > 256 {
        return Err(denied());
    }
    let mut controls = Vec::new();
    for (receipt, site) in evidence.parents {
        let origin = crate::secret_provenance::SecretOrigin::Entity {
            entity_type: site.kind.origin_kind(site.purpose),
            entity_id: receipt,
            path: site.path.clone(),
        };
        controls.push(Inputs::receipt_context(conn, &site, &origin, key).await?);
    }
    let prepared = PreparedCachePublication {
        workflow: task.workflow_execution,
        task: task.task_name.clone(),
        index,
        dispatch,
        iteration: iteration.id,
        namespace: iteration.namespace,
        generation: iteration.generation,
        snapshots: bound,
        fence: evidence.fence,
        controls,
        source,
    };
    prepared.ensure_fresh_on(conn).await?;
    Ok(Some(prepared))
}

impl PreparedCachePublication {
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.fence.ensure_fresh_on(conn).await?;
        self.source
            .cache_read_policy()
            .ok_or_else(denied)?
            .ensure_fresh()?;
        for control in &self.controls {
            control.ensure_fresh_on(conn).await?;
        }
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_cache_iteration WHERE id=$1 AND workflow_execution=$2 AND task_name=$3 AND namespace=$4 AND generation=$5 AND state='scanning' AND next_batch_index>$6) AND EXISTS(SELECT 1 FROM workflow_task_dispatch WHERE workflow_execution=$2 AND task_name=$3 AND task_index=$6 AND execution_id=$7) AND (SELECT count(*) FROM runtime_native_cache_frontier WHERE snapshot=ANY($8) AND bound_iteration=$1 AND ordinal=$6 AND bound_transaction=prepared_transaction)=$9")
            .bind(self.iteration).bind(self.workflow).bind(&self.task).bind(self.namespace).bind(self.generation).bind(i64::from(self.index)).bind(self.dispatch).bind(&self.snapshots).bind(i64::try_from(self.snapshots.len()).map_err(|_| denied())?).fetch_one(conn).await?;
        if !valid {
            return Err(denied());
        }
        Ok(())
    }
}
