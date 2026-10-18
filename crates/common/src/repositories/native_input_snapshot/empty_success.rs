//! An original released empty CONTROL in a later real-child success inventory.
//! It is neither a child nor a result view.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptySuccessWitness {
    pub(super) snapshot: Id,
    original: Box<Body>,
    digest: String,
}
pub(crate) struct PreparedEmptySuccessWitness {
    source: EmptySuccessWitness,
    held: ReleasedNativeEmptyIteration,
}
impl PreparedEmptySuccessWitness {
    pub(crate) fn snapshot(&self) -> Id {
        self.source.snapshot
    }
    pub(crate) fn held_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.held.held_deadline()
    }
    pub(crate) fn name(&self) -> &str {
        let Source::EmptyIteration(source) = &self.source.original.source else {
            unreachable!("issued empty witness")
        };
        &source.task_name
    }
    pub(crate) fn receipts(&self) -> &[ReceiptContext] {
        self.held.receipts()
    }
    pub(crate) fn ensure_fresh(&self) -> Result<()> {
        self.held.ensure_fresh()
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.held.ensure_fresh_on(conn).await
    }
    pub(in crate::repositories::native_input_snapshot) fn archive(&self) -> EmptySuccessWitness {
        self.source.clone()
    }
}
impl NativeInputSnapshotRepository {
    pub(crate) async fn prepare_empty_success_witness(
        conn: &mut PgConnection,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
        task: &str,
        key: &str,
    ) -> Result<PreparedEmptySuccessWitness> {
        if !workflow.completed_tasks.iter().any(|name| name == task) {
            return Err(denied());
        }
        let held = Self::reopen_empty_iteration(conn, parent.id, task, key)
            .await?
            .ok_or_else(denied)?;
        let (body, value, _, _) = load(conn, held.snapshot(), key).await?;
        if !value.is_null() {
            return Err(denied());
        }
        let source = EmptySuccessWitness {
            snapshot: held.snapshot(),
            digest: hash(&serde_json::to_value(&body)?)?,
            original: Box::new(body),
        };
        validate(
            &source,
            parent.id,
            workflow.id,
            parent.executable_snapshot.as_ref().ok_or_else(denied)?,
        )?;
        let Source::EmptyIteration(original) = &source.original.source else {
            return Err(denied());
        };
        if super::super::schema_contract::SchemaContractRepository::find(
            conn,
            super::super::schema_contract::BindingOwner::Execution,
            parent.id,
        )
        .await?
        .as_ref()
            != Some(&original.schema)
        {
            return Err(denied());
        }
        empty_iteration::require_empty_dispatch(conn, original).await?;
        Ok(PreparedEmptySuccessWitness { source, held })
    }
}
pub(super) fn validate(
    source: &EmptySuccessWitness,
    execution: Id,
    workflow: Id,
    executable: &crate::models::ExecutionExecutableSnapshot,
) -> Result<()> {
    let Source::EmptyIteration(original) = &source.original.source else {
        return Err(denied());
    };
    if source.snapshot <= 0
        || hash(&serde_json::to_value(&source.original)?)? != source.digest
        || original.execution != execution
        || original.workflow != workflow
        || serde_json::to_value(&original.executable)? != serde_json::to_value(executable)?
    {
        return Err(denied());
    }
    bounded(&serde_json::to_value(source)?)
}
pub(super) fn name(source: &EmptySuccessWitness) -> &str {
    let Source::EmptyIteration(original) = &source.original.source else {
        unreachable!("validated empty witness")
    };
    &original.task_name
}
pub(super) async fn normalized(
    conn: &mut PgConnection,
    source: &EmptySuccessWitness,
) -> Result<()> {
    // Authenticate the exact original row here. Do not reenter the native graph
    // reader and reset its traversal budget through Success -> Empty -> load.
    let row: Option<(Id, Id, Value, String)> = sqlx::query_as("SELECT renderer_admission,policy_revision,body,body_digest FROM runtime_native_input_snapshot WHERE id=$1")
        .bind(source.snapshot).fetch_optional(&mut *conn).await?;
    let (renderer, policy, body, digest) = row.ok_or_else(denied)?;
    bounded(&body)?;
    if hash(&body)? != source.digest
        || digest != source.digest
        || body != serde_json::to_value(&source.original)?
        || renderer != source.original.renderer
        || policy != source.original.authority.policy_id
    {
        return Err(denied());
    }
    let Source::EmptyIteration(original) = &source.original.source else {
        return Err(denied());
    };
    empty_iteration::require_empty_normalized(conn, source.snapshot, original).await?;
    empty_iteration::require_empty_release(conn, source.snapshot, original).await
}
