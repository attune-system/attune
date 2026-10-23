//! Republish an actual already-failed parent from its canonical root receipt.
//! No selector, output recipe or terminal writer runs during replay.
use super::super::{accepted_input::AcceptedInputRepository, immutable_input_origin};
use super::*;
use crate::secret_values::ENTITY_EXECUTION_RESULT;

pub struct NativeWaitCompletionReplayRepository;
pub struct PreparedNativeWaitCompletionReplay {
    parent: Execution,
    workflow: WorkflowExecution,
    wait: WorkflowTaskWait,
    fields: Vec<StoredSecretValue>,
    schema: super::super::schema_contract::ActionSchemaBinding,
    released: ReleasedWaitInput,
    root_released: Option<ReleasedWaitInput>,
    receipts: Vec<ReceiptContext>,
    sources: Vec<super::super::native_input_snapshot::NativeSourceEvidence>,
    fence: ApprovalEffectFence,
    output_release: (Id, Id, Id, Id, Id, String),
    original_release: (Id, String),
    completed: Vec<Execution>,
    applied: bool,
    progress_only: bool,
}
impl NativeWaitCompletionReplayRepository {
    pub async fn prepare(
        conn: &mut PgConnection,
        wait: &WorkflowTaskWait,
        key: Option<&str>,
    ) -> Result<Option<PreparedNativeWaitCompletionReplay>> {
        let workflow = Workflows::find_by_id(&mut *conn, wait.workflow_execution)
            .await?
            .ok_or_else(denied)?;
        if workflow.status != ExecutionStatus::Failed {
            return Ok(None);
        }
        if Admissions::find(conn, RuntimeOwner::Execution(workflow.execution))
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let key = key.ok_or_else(denied)?;
        let parent = Executions::find_by_id(&mut *conn, workflow.execution)
            .await?
            .ok_or_else(denied)?;
        let actual = Waits::find_by_workflow_task(&mut *conn, workflow.id, &wait.task_name)
            .await?
            .ok_or_else(denied)?;
        if parent.status != ExecutionStatus::Failed
            || serde_json::to_value(actual)? != serde_json::to_value(wait)?
            || !workflow.failed_tasks.contains(&wait.task_name)
        {
            return Err(denied());
        }
        let schema = Schemas::find(conn, BindingOwner::Execution, parent.id)
            .await?
            .ok_or_else(denied)?;
        schema.validate()?;
        let fence = AcceptedInputRepository::revalidate_execution(conn, parent.id, key).await?;
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, parent.id).await?;
        if fields.len() != 1 || !fields[0].json_path.is_empty() {
            return Err(denied());
        }
        let (ordinary, receipts) = immutable_input_origin::prepare_input_receipts(
            conn,
            ENTITY_EXECUTION_RESULT,
            parent.id,
            &fields,
            key,
        )
        .await?;
        if !ordinary.is_empty() || receipts.len() != 1 {
            return Err(denied());
        }
        immutable_input_origin::bind_input_receipt_buffers(
            conn,
            ENTITY_EXECUTION_RESULT,
            parent.id,
            &fields,
        )
        .await?;
        let root = &receipts[0];
        if root.site()
            != &(InputSite {
                execution: parent.id,
                kind: InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: String::new(),
            })
        {
            return Err(denied());
        }
        let mut sources = vec![];
        let mut selected = None;
        for metadata in root.native_source_metadata() {
            let source = Snapshots::source_evidence(conn, metadata, key).await?;
            if let Some(output) = source.wait_completion() {
                if output.execution() == parent.id && output.workflow() == workflow.id {
                    if selected.is_some()
                        || (output.original_wait() == wait.id
                            && output.original_task_name() != wait.task_name)
                        || output.original_output_contract() != &schema
                        || parent.result.as_ref() != Some(&crate::secret_values::redaction_marker())
                    {
                        return Err(denied());
                    }
                    let mut expected = output.projected_workflow().clone();
                    expected.status = ExecutionStatus::Failed;
                    expected.error_message = Some(
                        root.accepted_value()
                            .get("error")
                            .and_then(Value::as_str)
                            .ok_or_else(denied)?
                            .into(),
                    );
                    expected.updated = workflow.updated;
                    let mut expected_parent = output.original_parent().clone();
                    expected_parent.status = ExecutionStatus::Failed;
                    expected_parent.result = parent.result.clone();
                    expected_parent.updated = parent.updated;
                    if serde_json::to_value(expected)? != serde_json::to_value(&workflow)?
                        || serde_json::to_value(expected_parent)? != serde_json::to_value(&parent)?
                    {
                        return Err(denied());
                    }
                    selected = Some((
                        output.snapshot(),
                        output.original_snapshot(),
                        output.original_task_path().to_owned(),
                        output.original_release_transaction().to_owned(),
                        output.original_wait(),
                    ));
                }
            }
            if let Some(policy) = source.cache_read_policy() {
                policy.ensure_fresh()?;
            }
            sources.push(source);
        }
        let (snapshot, original, _path, original_tx, final_wait) = selected.ok_or_else(denied)?;
        let final_observation =
            Snapshots::prepare_completed_wait_observation(conn, root, key).await?;
        if final_observation.outcome_binding()?.snapshot != original {
            return Err(denied());
        }
        let output_release:(Id,Id,Id,Id,Id,String)=sqlx::query_as("SELECT snapshot,execution,workflow,wait,original_snapshot,release_transaction FROM runtime_native_wait_completion_release WHERE snapshot=$1")
            .bind(snapshot).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if output_release.1 != parent.id
            || output_release.2 != workflow.id
            || output_release.3 != final_wait
            || output_release.4 != original
        {
            return Err(denied());
        }
        let progress_only = final_wait != wait.id;
        let (released, root_released, original_release) = if progress_only {
            if workflow.current_tasks.contains(&wait.task_name)
                || workflow.completed_tasks.contains(&wait.task_name)
                || !matches!(
                    wait.state,
                    WorkflowTaskWaitState::Failed | WorkflowTaskWaitState::TimedOut
                )
            {
                return Err(denied());
            }
            let earlier =
                Snapshots::prepare_completed_wait_observation_for(conn, root, wait.id, key).await?;
            let earlier_release: (Id, String) = sqlx::query_as(
                "SELECT snapshot,release_transaction FROM runtime_native_wait_release WHERE wait=$1",
            )
            .bind(wait.id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(denied)?;
            if earlier_release.0 != earlier.outcome_binding()?.snapshot
                || earlier_release.1.is_empty()
            {
                return Err(denied());
            }
            (earlier, Some(final_observation), earlier_release)
        } else {
            (final_observation, None, (original, original_tx))
        };
        let replay = PreparedNativeWaitCompletionReplay {
            parent,
            workflow,
            wait: wait.clone(),
            fields,
            schema,
            released,
            root_released,
            receipts,
            sources,
            fence,
            output_release,
            original_release,
            completed: vec![],
            applied: false,
            progress_only,
        };
        replay.ensure_sources_on(conn).await?;
        Ok(Some(replay))
    }
}
impl PreparedNativeWaitCompletionReplay {
    pub fn workflow(&self) -> Id {
        self.workflow.id
    }
    pub fn wait(&self) -> Id {
        self.wait.id
    }
    pub fn completions(&self) -> &[Execution] {
        &self.completed
    }
    async fn ensure_sources_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.fence.ensure_fresh_on(conn).await?;
        self.released.ensure_fresh_on(conn).await?;
        if let Some(root) = &self.root_released {
            root.ensure_fresh_on(conn).await?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        let output:Option<(Id,Id,Id,Id,Id,String)>=sqlx::query_as("SELECT snapshot,execution,workflow,wait,original_snapshot,release_transaction FROM runtime_native_wait_completion_release WHERE snapshot=$1")
            .bind(self.output_release.0).fetch_optional(&mut *conn).await?;
        let original: Option<(Id, String)> = sqlx::query_as(
            "SELECT snapshot,release_transaction FROM runtime_native_wait_release WHERE wait=$1",
        )
        .bind(self.wait.id)
        .fetch_optional(&mut *conn)
        .await?;
        if output.as_ref() != Some(&self.output_release)
            || original.as_ref() != Some(&self.original_release)
        {
            return Err(denied());
        }
        self.source_fresh()
    }
    fn source_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.released.ensure_fresh()?;
        if let Some(root) = &self.root_released {
            root.ensure_fresh()?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        for source in &self.sources {
            if let Some(policy) = source.cache_read_policy() {
                policy.ensure_fresh()?;
            }
        }
        Ok(())
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, _key: &str) -> Result<Vec<Execution>> {
        if self.applied {
            return Err(denied());
        }
        self.ensure_sources_on(conn).await?;
        let workflow = Workflows::find_by_id_for_update(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let parent = Executions::find_by_id_for_update(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(denied)?;
        let wait =
            Waits::find_by_workflow_task_for_update(conn, self.workflow.id, &self.wait.task_name)
                .await?
                .ok_or_else(denied)?;
        if serde_json::to_value(workflow)? != serde_json::to_value(&self.workflow)?
            || serde_json::to_value(parent)? != serde_json::to_value(&self.parent)?
            || serde_json::to_value(wait)? != serde_json::to_value(&self.wait)?
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, self.parent.id)
                .await?
                != self.fields
            || Schemas::find(conn, BindingOwner::Execution, self.parent.id)
                .await?
                .as_ref()
                != Some(&self.schema)
        {
            return Err(denied());
        }
        self.ensure_sources_on(conn).await?;
        if !self.progress_only {
            self.completed.push(self.parent.clone());
        }
        self.applied = true;
        Ok(vec![])
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if !self.applied {
            return Err(denied());
        }
        self.ensure_sources_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if !self.applied {
            return Err(denied());
        }
        self.source_fresh()
    }
}
