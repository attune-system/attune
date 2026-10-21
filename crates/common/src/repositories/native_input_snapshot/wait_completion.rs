//! Unhandled wait output. The original wait archive is a separate source, not
//! an ordinary child, and is retained with its own authority and frozen owner.
use super::super::{
    native_wait_materialization::{PreparedNativeWaitCompletion, WaitCompletionOutcome},
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    FindById,
};
use super::*;
use crate::models::{Execution, ExecutionStatus, WorkflowExecution};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitCompletionSource {
    pub(super) original_snapshot: Id,
    original: Box<Body>,
    original_digest: String,
    pub(crate) execution: Id,
    pub(super) workflow: Id,
    pub(crate) transaction: String,
    parent: Execution,
    workflow_original: WorkflowExecution,
    config_fields: Vec<ArchivedField>,
    result_fields: Vec<ArchivedField>,
    variable_fields: Vec<ArchivedField>,
    registration: Option<NativeWaitCompletionRegistration>,
    pub(super) projected: WorkflowExecution,
    pub(super) contract: ActionSchemaBinding,
    pub(super) target: BoundRuntimeTarget,
    pub(super) read_owner: super::super::runtime_read::RuntimeReadOwner,
    pub(crate) parents: Vec<(Id, InputSite)>,
    controls: Vec<(Id, InputSite)>,
    values: Vec<(Id, InputSite)>,
    pub(crate) origins: Vec<SecretOrigin>,
}
impl WaitCompletionSource {
    pub(super) fn original_authority(&self) -> &SelectedRuntimeAuthority {
        &self.original.authority
    }
    pub(super) fn original_body(&self) -> &Body {
        &self.original
    }
    pub(super) fn storage_nodes(&self) -> Result<usize> {
        let wait = original(self)?;
        Ok(1 + self.controls.len()
            + self.values.len()
            + self.config_fields.len()
            + self.result_fields.len()
            + self.variable_fields.len()
            + wait.fields.len()
            + wait.contributors.len()
            + wait.origins.len())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWaitCompletionRegistration {
    renderer: Id,
    selector: Option<Id>,
    task_path: String,
    recipe: Value,
    digest: String,
    transaction: String,
}
impl NativeWaitCompletionRegistration {
    pub fn renderer_admission(&self) -> Id {
        self.renderer
    }
    pub fn selector(&self) -> Option<Id> {
        self.selector
    }
    pub fn task_path(&self) -> &str {
        &self.task_path
    }
    pub fn original_recipe(&self) -> &Value {
        &self.recipe
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}

/// Immutable evidence only. Neither this type nor its constituent IDs grant
/// reader access. C must check both root and original wait reader ceilings.
pub struct NativeWaitCompletionEvidence {
    snapshot: Id,
    source: WaitCompletionSource,
}
impl NativeWaitCompletionEvidence {
    pub(super) fn new(snapshot: Id, source: &WaitCompletionSource) -> Self {
        Self {
            snapshot,
            source: source.clone(),
        }
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn workflow(&self) -> Id {
        self.source.workflow
    }
    pub fn original_snapshot(&self) -> Id {
        self.source.original_snapshot
    }
    pub fn outcome(&self) -> WaitCompletionOutcome {
        let wait = original(&self.source).expect("validated wait evidence");
        let values = native_wait_outcome_values(wait).expect("validated wait outcome");
        if values[1] == Value::Bool(true) {
            WaitCompletionOutcome::Failed
        } else if values[2] == Value::Bool(true) {
            WaitCompletionOutcome::TimedOut
        } else {
            WaitCompletionOutcome::Cancelled
        }
    }
    pub fn original_task_path(&self) -> &str {
        &self.source.original.task_path
    }
    pub fn original_task_name(&self) -> &str {
        &original(&self.source)
            .expect("validated wait evidence")
            .task_name
    }
    pub fn original_state(&self) -> &str {
        &original(&self.source)
            .expect("validated wait evidence")
            .state
    }
    pub fn original_projection(&self) -> &Value {
        &original(&self.source)
            .expect("validated wait evidence")
            .raw_projection
    }
    pub fn original_version(&self) -> &Value {
        &original(&self.source)
            .expect("validated wait evidence")
            .source_version
    }
    pub fn original_release_transaction(&self) -> &str {
        &original(&self.source)
            .expect("validated wait evidence")
            .prepared_transaction
    }
    pub fn original_wait(&self) -> Id {
        original(&self.source)
            .expect("validated wait evidence")
            .wait
    }
    pub fn original_metadata(&self) -> NativeSnapshotMetadata {
        metadata(self.source.original_snapshot, &self.source.original)
    }
    pub fn original_authority(&self) -> &SelectedRuntimeAuthority {
        &self.source.original.authority
    }
    pub fn original_renderer_admission(&self) -> Id {
        self.source.original.renderer
    }
    pub fn original_source_admission(&self) -> Id {
        original(&self.source)
            .expect("validated wait evidence")
            .source_admission
    }
    pub fn original_target(&self) -> &BoundRuntimeTarget {
        &original(&self.source)
            .expect("validated wait evidence")
            .target
    }
    pub fn original_read_owner(&self) -> &super::super::runtime_read::RuntimeReadOwner {
        &original(&self.source)
            .expect("validated wait evidence")
            .read_owner
    }
    pub fn original_source_site(&self) -> (&str, Id, &str) {
        let wait = original(&self.source).expect("validated wait evidence");
        // source_field selects the stored row column, not a JSON pointer within
        // its value. Native Wait metadata and custody both address that root.
        (&wait.source_kind, wait.source_id, "")
    }
    pub fn original_source_contract(&self) -> Option<&Value> {
        original(&self.source)
            .expect("validated wait evidence")
            .contract
            .as_ref()
    }
    pub fn original_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        original(&self.source)
            .expect("validated wait evidence")
            .fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn contributor_manifests(
        &self,
    ) -> &[super::super::accepted_input::AcceptedContributorManifest] {
        &original(&self.source)
            .expect("validated wait evidence")
            .contributors
    }
    pub fn original_receipt_edges(&self) -> &[(Id, InputSite)] {
        &original(&self.source)
            .expect("validated wait evidence")
            .parents
    }
    pub fn original_origins(&self) -> &[SecretOrigin] {
        &original(&self.source)
            .expect("validated wait evidence")
            .origins
    }
    pub fn original_parent(&self) -> &Execution {
        &self.source.parent
    }
    pub fn original_workflow(&self) -> &WorkflowExecution {
        &self.source.workflow_original
    }
    pub fn original_output_contract(&self) -> &ActionSchemaBinding {
        &self.source.contract
    }
    pub fn original_config_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        self.source
            .config_fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn original_result_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        self.source
            .result_fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn original_variable_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        self.source
            .variable_fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn original_registration(&self) -> &NativeWaitCompletionRegistration {
        self.source
            .registration
            .as_ref()
            .expect("validated registration")
    }
    pub fn projected_workflow(&self) -> &WorkflowExecution {
        &self.source.projected
    }
    pub fn receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.source.parents
    }
    pub fn controls(&self) -> &[(Id, InputSite)] {
        &self.source.controls
    }
    pub fn issued_values(&self) -> &[(Id, InputSite)] {
        &self.source.values
    }
    pub fn transaction(&self) -> &str {
        &self.source.transaction
    }
    pub fn target(&self) -> &BoundRuntimeTarget {
        &self.source.target
    }
    pub fn read_owner(&self) -> &super::super::runtime_read::RuntimeReadOwner {
        &self.source.read_owner
    }
}

pub(crate) struct PreparedWaitCompletionOutput {
    pub(crate) state: Arc<PreparedNativeWaitCompletion>,
    pub(crate) source: WaitCompletionSource,
    pub(crate) renderer: super::super::runtime_authority::RuntimeAdmissionRecord,
    pub(crate) value: Value,
    pub(crate) fence: ApprovalEffectFence,
    checked: OnceLock<()>,
    pub(crate) recorded: OnceLock<Id>,
}
impl PreparedWaitCompletionOutput {
    pub(crate) fn retain_control(&mut self, receipt: Id, site: &InputSite) -> Result<()> {
        if site.purpose == InputSitePurpose::Value {
            return Err(denied());
        }
        self.source.controls.push((receipt, site.clone()));
        Ok(())
    }
    pub(crate) fn retain_value(&mut self, receipt: Id, site: &InputSite) -> Result<()> {
        if site.purpose != InputSitePurpose::Value {
            return Err(denied());
        }
        self.source.values.push((receipt, site.clone()));
        Ok(())
    }
    pub(crate) fn retain_parent(
        &mut self,
        receipt: Id,
        site: &InputSite,
        origins: &[SecretOrigin],
    ) -> Result<()> {
        let edge = (receipt, site.clone());
        if !self.source.parents.contains(&edge) {
            self.source.parents.push(edge);
        }
        for origin in origins {
            if !self.source.origins.contains(origin) {
                self.source.origins.push(origin.clone());
            }
        }
        if self.source.parents.len() + self.source.controls.len() + self.source.values.len() > 256
            || self.source.origins.len() > 256
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) async fn require_original_rows(&self, conn: &mut PgConnection) -> Result<()> {
        self.state.require_source_current(conn).await?;
        compare_original(conn, &self.source).await?;
        self.checked.set(()).map_err(|_| denied())
    }
    pub(crate) async fn require_target(
        &self,
        conn: &mut PgConnection,
        projected: &WorkflowExecution,
    ) -> Result<()> {
        if self.checked.get().is_none() || self.recorded.get().is_some() {
            return Err(denied());
        }
        self.state.ensure_fresh_on(conn).await?;
        self.fence.ensure_fresh_on(conn).await?;
        self.state.released_observation(conn).await?;
        compare_source_current(conn, original(&self.source)?).await?;
        require_projection(&self.source, projected)?;
        let parent = super::super::execution::ExecutionRepository::find_by_id(
            &mut *conn,
            self.source.execution,
        )
        .await?
        .ok_or_else(denied)?;
        let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id(
            &mut *conn,
            self.source.workflow,
        )
        .await?
        .ok_or_else(denied)?;
        if serde_json::to_value(parent)? != serde_json::to_value(&self.source.parent)?
            || serde_json::to_value(workflow)? != serde_json::to_value(projected)?
        {
            return Err(denied());
        }
        if Schemas::find(conn, BindingOwner::Execution, self.source.execution)
            .await?
            .as_ref()
            != Some(&self.source.contract)
        {
            return Err(denied());
        }
        if load_fields(conn, "execution_config", self.source.execution).await?
            != self.source.config_fields
            || load_fields(conn, "execution_result", self.source.execution).await?
                != self.source.result_fields
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl NativeInputSnapshotRepository {
    pub(crate) async fn prepare_wait_completion_output(
        conn: &mut PgConnection,
        state: Arc<PreparedNativeWaitCompletion>,
        key: &str,
    ) -> Result<PreparedWaitCompletionOutput> {
        state.require_source_current(conn).await?;
        // This private projection is issued ONLY by the genuine J token. It
        // never calls the public pending evidence API with an ID or flag.
        let (body, value, _, _) = load(conn, state.snapshot(), key).await?;
        let Source::Wait(wait) = &body.source else {
            return Err(denied());
        };
        if body.renderer != state.renderer_admission()
            || body.execution != state.execution()
            || body.task_path != state.task_path()
            || wait.wait != state.wait()
            || wait.workflow != state.workflow()
            || wait.task_name != state.task_name()
            || wait.prepared_transaction != transaction(conn).await?
        {
            return Err(denied());
        }
        validate_wait_value(conn, state.snapshot(), wait, &value, key).await?;
        compare_source_current(conn, wait).await?;
        let outcomes = native_wait_outcome_values(wait)?;
        let index = match state.outcome() {
            WaitCompletionOutcome::Failed => 1,
            WaitCompletionOutcome::TimedOut => 2,
            WaitCompletionOutcome::Cancelled => 3,
        };
        if outcomes[index] != Value::Bool(true) {
            return Err(denied());
        }
        let renderer = DeclaredInputRepository::require_source(conn, state.execution()).await?;
        if renderer.id != body.renderer {
            return Err(denied());
        }
        require_task(conn, &renderer, &body.task_path, &body.authority).await?;
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, renderer.id)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::retained_admission_fence(conn, wait.source_admission)
                    .await?,
            )
            .intersect(
                &RuntimeAuthorityRepository::selected_effect_fence(conn, &body.authority).await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?)
            .intersect(&ApprovalEffectFence::from_input_approval(
                state.held_deadline()?,
            )?);
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(state.execution()),
            renderer.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let source = WaitCompletionSource {
            original_snapshot: state.snapshot(),
            original_digest: hash(&serde_json::to_value(&body)?)?,
            execution: state.execution(),
            workflow: state.workflow(),
            transaction: transaction(conn).await?,
            parent: state.original_parent().clone(),
            workflow_original: state.original_workflow().clone(),
            projected: state.original_workflow().clone(),
            config_fields: load_fields(conn, "execution_config", state.execution()).await?,
            result_fields: load_fields(conn, "execution_result", state.execution()).await?,
            variable_fields: load_fields(conn, "workflow_variables", state.execution()).await?,
            registration: None,
            contract: state.original_output_contract().clone(),
            target: bound_target(state.execution(), &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            parents: wait.parents.clone(),
            controls: vec![],
            values: vec![],
            origins: wait.origins.clone(),
            original: Box::new(body),
        };
        let value = candidate(&source)?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedWaitCompletionOutput {
            state,
            source,
            renderer,
            value,
            fence,
            checked: OnceLock::new(),
            recorded: OnceLock::new(),
        })
    }
    pub(crate) async fn publish_wait_completion_output(
        conn: &mut PgConnection,
        prepared: &PreparedWaitCompletionOutput,
        projected: &WorkflowExecution,
        key: &str,
    ) -> Result<(Id, NativeSnapshotMetadata)> {
        prepared.require_target(conn, projected).await?;
        let mut source = prepared.source.clone();
        source.projected = projected.clone();
        source.registration = load_registration(conn, original(&source)?.wait).await?;
        validate_storage(&source)?;
        let body = Body {
            renderer: prepared.renderer.id,
            execution: source.execution,
            task_path: String::new(),
            authority: prepared.renderer.authority.0.clone(),
            approval_deadline: prepared.fence.approval_deadline(),
            source: Source::WaitCompletion(source.clone()),
        };
        let snapshot = save(conn, &body, &prepared.value, key).await?;
        for (receipt, _) in &source.parents {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(receipt).execute(&mut *conn).await?;
        }
        for admission in admissions(&source)? {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)").bind(snapshot).bind(admission).execute(&mut *conn).await?;
        }
        let wait = original(&source)?;
        sqlx::query("INSERT INTO runtime_native_wait_completion_release(snapshot,execution,workflow,wait,original_snapshot,release_transaction) VALUES($1,$2,$3,$4,$5,pg_current_xact_id()::TEXT)")
            .bind(snapshot).bind(source.execution).bind(source.workflow).bind(wait.wait).bind(source.original_snapshot).execute(&mut *conn).await?;
        require_release(conn, snapshot, &source).await?;
        prepared.require_target(conn, projected).await?;
        Ok((snapshot, metadata(snapshot, &body)))
    }
    pub(crate) async fn validate_wait_completion_recipe(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        plaintext: Option<(&Value, &str)>,
    ) -> Result<()> {
        let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
        let Source::WaitCompletion(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer {
            return Err(denied());
        }
        require_release(conn, snapshot, source).await?;
        if let Some((actual, key)) = plaintext {
            let (_, value, _, _) = load(conn, snapshot, key).await?;
            validate_value(conn, source, &value, key).await?;
            if &value != actual {
                return Err(denied());
            }
        }
        Ok(())
    }
}

pub(super) fn original(source: &WaitCompletionSource) -> Result<&WaitSource> {
    match &source.original.source {
        Source::Wait(wait) => Ok(wait),
        _ => Err(denied()),
    }
}
pub(super) fn admissions(source: &WaitCompletionSource) -> Result<Vec<Id>> {
    let mut ids = vec![source.original.renderer, original(source)?.source_admission];
    ids.sort();
    ids.dedup();
    Ok(ids)
}
pub(super) fn origins(snapshot: Id, source: &WaitCompletionSource) -> Vec<SecretOrigin> {
    let mut origins = source.origins.clone();
    origins.push(SecretOrigin::Entity {
        entity_type: "wait_completion_metadata".into(),
        entity_id: snapshot,
        path: String::new(),
    });
    origins
}
pub(super) fn error_message(source: &WaitCompletionSource) -> Result<String> {
    Ok(format!(
        "Workflow failed: 1 task(s) failed: {}",
        original(source)?.task_name
    ))
}
pub(super) fn candidate(source: &WaitCompletionSource) -> Result<Value> {
    // The original scheduler bypasses output_map and success-only schema-warning
    // insertion on failure. Compile the pinned contract, never today's schema.
    source.contract.validate()?;
    if let Some(contract) = &source.contract.output {
        crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
    }
    Ok(json!({"succeeded":false,"error":error_message(source)?}))
}
fn require_projection(source: &WaitCompletionSource, actual: &WorkflowExecution) -> Result<()> {
    let wait = original(source)?;
    let mut expected = source.workflow_original.clone();
    expected
        .current_tasks
        .retain(|name| name != &wait.task_name);
    if !expected.failed_tasks.contains(&wait.task_name) {
        expected.failed_tasks.push(wait.task_name.clone());
    }
    // Only the owning ordered-publication seal authorizes variables and their
    // ciphertext. Do not reconstruct overwrites from a guessed final map.
    expected.variables = actual.variables.clone();
    expected.updated = actual.updated;
    if actual.status != ExecutionStatus::Running
        || serde_json::to_value(expected)? != serde_json::to_value(actual)?
    {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn validate_storage(source: &WaitCompletionSource) -> Result<()> {
    let wait = original(source)?;
    validate_wait_storage(wait)?;
    if wait.source_version.get("projection_digest") != Some(&json!(hash(&wait.raw_projection)?))
        || wait.source_version.get("ciphertext_digest")
            != Some(&json!(hash(&serde_json::to_value(&wait.fields)?)?))
        || wait.source_version.get("contract_digest")
            != Some(&json!(wait.contract.as_ref().map(hash).transpose()?))
    {
        return Err(denied());
    }
    if let Some(contract) = &wait.contract {
        let contract: crate::value_schema::Contract = serde_json::from_value(contract.clone())?;
        crate::value_schema::CompiledContract::new(&contract).map_err(|_| denied())?;
    }
    let outcomes = native_wait_outcome_values(wait)?;
    if let Some(registration) = &source.registration {
        if hash(&registration.recipe)? != registration.digest
            || registration.renderer != source.original.renderer
            || registration.task_path != source.original.task_path
            || registration.transaction.is_empty()
            || registration.recipe.get("workflow") != Some(&json!(source.workflow))
            || registration.recipe.get("target_kind") != Some(&json!(wait.target_kind))
            || registration.recipe.get("target_id") != Some(&json!(wait.target_id))
            || registration.recipe.get("source_admission") != Some(&json!(wait.source_admission))
        {
            return Err(denied());
        }
    } else if !wait.terminal_observation {
        // Ordinary/prospective waits still require their original registration.
        return Err(denied());
    }
    if source.execution <= 0
        || source.workflow <= 0
        || source.original_snapshot <= 0
        || source.transaction.is_empty()
        || source.original.execution != source.execution
        || wait.prepared_transaction != source.transaction
        || wait.workflow != source.workflow
        || source.original.renderer <= 0
        || source.original_digest != hash(&serde_json::to_value(&source.original)?)?
        || outcomes[0] != Value::Bool(false)
        || source.parent.id != source.execution
        || source.workflow_original.id != source.workflow
        || source.workflow_original.execution != source.execution
        || source.parent.status != ExecutionStatus::Running
        || source.workflow_original.status != ExecutionStatus::Running
        || source.workflow_original.paused
        || source
            .workflow_original
            .failed_tasks
            .contains(&wait.task_name)
        || source
            .workflow_original
            .completed_tasks
            .contains(&wait.task_name)
        || source
            .workflow_original
            .current_tasks
            .iter()
            .any(|name| name != &wait.task_name)
        || source.target.id != source.execution
        || source.target.kind != "execution"
        || source.parents.len() + source.controls.len() + source.values.len() > 256
        || source.config_fields.len() + source.result_fields.len() + source.variable_fields.len()
            > 256
        || source.origins.len() > 256
        || source
            .controls
            .iter()
            .any(|edge| edge.1.purpose == InputSitePurpose::Value || !source.parents.contains(edge))
        || source
            .values
            .iter()
            .any(|edge| edge.1.purpose != InputSitePurpose::Value || !source.parents.contains(edge))
        || wait
            .parents
            .iter()
            .any(|edge| !source.parents.contains(edge))
        || wait
            .origins
            .iter()
            .any(|origin| !source.origins.contains(origin))
    {
        return Err(denied());
    }
    require_projection(source, &source.projected)?;
    candidate(source)?;
    Ok(())
}
async fn compare_original(conn: &mut PgConnection, source: &WaitCompletionSource) -> Result<()> {
    let parent =
        super::super::execution::ExecutionRepository::find_by_id(&mut *conn, source.execution)
            .await?
            .ok_or_else(denied)?;
    let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id(
        &mut *conn,
        source.workflow,
    )
    .await?
    .ok_or_else(denied)?;
    if serde_json::to_value(parent)? != serde_json::to_value(&source.parent)?
        || serde_json::to_value(workflow)? != serde_json::to_value(&source.workflow_original)?
    {
        return Err(denied());
    }
    if load_fields(conn, "execution_config", source.execution).await? != source.config_fields
        || load_fields(conn, "execution_result", source.execution).await? != source.result_fields
        || load_fields(conn, "workflow_variables", source.execution).await?
            != source.variable_fields
    {
        return Err(denied());
    }
    compare_source_current(conn, original(source)?).await
}
async fn compare_source_current(conn: &mut PgConnection, source: &WaitSource) -> Result<()> {
    let (updated, raw, contract) = match source.target_kind.as_str() {
        "execution" => {
            let row = super::super::execution::ExecutionRepository::find_by_id(
                &mut *conn,
                source.target_id,
            )
            .await?
            .ok_or_else(denied)?;
            let binding = Schemas::find(conn, BindingOwner::Execution, row.id)
                .await?
                .ok_or_else(denied)?;
            (
                json!(row.updated),
                json!({"id":row.id,"status":row.status,"result":row.result}),
                binding.output.map(serde_json::to_value).transpose()?,
            )
        }
        "inquiry" => {
            let row =
                super::super::inquiry::InquiryRepository::find_by_id(&mut *conn, source.target_id)
                    .await?
                    .ok_or_else(denied)?;
            let binding =
                super::super::inquiry_schema::InquirySchemaRepository::find(&mut *conn, row.id)
                    .await?
                    .ok_or_else(denied)?;
            let raw = json!({"code":if source.state=="timed_out" {"inquiry_timeout"} else {"inquiry_cancelled"},"inquiry_id":row.id,"status":row.status});
            (
                json!(row.updated),
                raw,
                Some(serde_json::to_value(binding.binding.contract)?),
            )
        }
        "work_queue_item" => {
            let row = super::super::work_queue::WorkQueueItemRepository::find_by_id(
                &mut *conn,
                source.target_id,
            )
            .await?
            .ok_or_else(denied)?;
            let result = if matches!(
                row.status,
                crate::models::WorkQueueItemStatus::Completed
                    | crate::models::WorkQueueItemStatus::Skipped
            ) {
                row.ack_summary
            } else {
                row.last_error
            };
            (
                json!(row.updated),
                json!({"id":row.id,"queue_ref":row.queue_ref,"status":row.status,"result":result}),
                None,
            )
        }
        _ => return Err(denied()),
    };
    let fields = if source.raw_projection.get(&source.source_field).is_some() {
        load_fields(conn, &source.source_kind, source.source_id).await?
    } else {
        vec![]
    };
    if source.source_version.get("updated") != Some(&updated)
        || source.raw_projection != raw
        || fields != source.fields
        || contract != source.contract
    {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn require_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &WaitCompletionSource,
) -> Result<()> {
    let wait = original(source)?;
    let matched: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_completion_release WHERE snapshot=$1 AND execution=$2 AND workflow=$3 AND wait=$4 AND original_snapshot=$5 AND release_transaction=$6)")
        .bind(snapshot).bind(source.execution).bind(source.workflow).bind(wait.wait).bind(source.original_snapshot).bind(&source.transaction).fetch_one(&mut *conn).await?;
    if !matched {
        return Err(denied());
    }
    let original_release: Option<String> = sqlx::query_scalar(
        "SELECT release_transaction FROM runtime_native_wait_release WHERE wait=$1 AND snapshot=$2",
    )
    .bind(wait.wait)
    .bind(source.original_snapshot)
    .fetch_optional(&mut *conn)
    .await?;
    if original_release.as_deref() != Some(wait.prepared_transaction.as_str()) {
        return Err(denied());
    }
    if let Some(actual) = load_registration(conn, wait.wait).await? {
        if serde_json::to_value(actual)?
            != serde_json::to_value(source.registration.as_ref().ok_or_else(denied)?)?
        {
            return Err(denied());
        }
    } else if !wait.terminal_observation
        && super::super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(
            &mut *conn,
            wait.workflow,
            &wait.task_name,
        )
        .await?
        .is_some()
    {
        return Err(denied());
    }
    validate_wait_release(conn, source.original_snapshot, wait).await?;
    InheritedInputRepository::require_cache_selector_publications(conn, &source.parents).await?;
    Ok(())
}
async fn load_registration(
    conn: &mut PgConnection,
    wait: Id,
) -> Result<Option<NativeWaitCompletionRegistration>> {
    let row: Option<(Id, Option<Id>, String, Value, String, String)> = sqlx::query_as("SELECT renderer,selector,task_path,recipe,recipe_digest,registration_transaction FROM runtime_native_wait_registration WHERE wait=$1").bind(wait).fetch_optional(conn).await?;
    Ok(row.map(
        |(renderer, selector, task_path, recipe, digest, transaction)| {
            NativeWaitCompletionRegistration {
                renderer,
                selector,
                task_path,
                recipe,
                digest,
                transaction,
            }
        },
    ))
}
pub(super) async fn validate_value(
    conn: &mut PgConnection,
    source: &WaitCompletionSource,
    value: &Value,
    key: &str,
) -> Result<()> {
    let (body, original_value, _, _) = load(conn, source.original_snapshot, key).await?;
    if serde_json::to_value(body)? != serde_json::to_value(&source.original)?
        || *value != candidate(source)?
    {
        return Err(denied());
    }
    validate_wait_value(
        conn,
        source.original_snapshot,
        original(source)?,
        &original_value,
        key,
    )
    .await
}
pub(super) async fn validate_normalized(
    conn: &mut PgConnection,
    snapshot: Id,
    body: &Body,
    source: &WaitCompletionSource,
) -> Result<()> {
    validate_storage(source)?;
    if body.execution != source.execution
        || body.renderer != source.original.renderer
        || !body.task_path.is_empty()
    {
        return Err(denied());
    }
    let parents: Vec<Id> = sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(snapshot).fetch_all(&mut *conn).await?;
    let actual: Vec<Id> = sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(snapshot).fetch_all(&mut *conn).await?;
    // Only query kinds present in this source baseline. Combined integration
    // must add the independently owned 01026/01027/01028 tables once installed.
    let wrong: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_result_view_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_completion_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_member WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1)").bind(snapshot).fetch_one(&mut *conn).await?;
    if wrong
        || actual != admissions(source)?
        || parents
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            != source.parents.iter().map(|(id, _)| *id).collect()
    {
        return Err(denied());
    }
    let (original_body, _, _) =
        Box::pin(load_encrypted_one(conn, source.original_snapshot)).await?;
    if source.original_snapshot >= snapshot
        || serde_json::to_value(original_body)? != serde_json::to_value(&source.original)?
    {
        return Err(denied());
    }
    let wait = original(source)?;
    if load_fields(conn, "native_input_snapshot", source.original_snapshot).await? != wait.fields {
        return Err(denied());
    }
    let key_hash: String = sqlx::query_scalar(
        "SELECT encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1",
    )
    .bind(snapshot)
    .fetch_one(&mut *conn)
    .await?;
    for field in wait
        .fields
        .iter()
        .chain(&source.config_fields)
        .chain(&source.result_fields)
        .chain(&source.variable_fields)
    {
        if field.encryption_key_hash.as_deref() != Some(key_hash.as_str()) {
            return Err(denied());
        }
        field.provenance()?;
    }
    Ok(())
}
pub(super) fn associations(source: &WaitCompletionSource, renderer: Id) -> Value {
    let Ok(wait) = original(source) else {
        return json!({"invalid_wait_completion":true});
    };
    let mut ids = vec![source.original.renderer, wait.source_admission];
    ids.sort();
    ids.dedup();
    json!({"budget":renderer,"admissions":ids,"release":null,"wait_completion_release":[Id::MAX,source.execution,source.workflow,wait.wait,source.original_snapshot,source.transaction]})
}

pub(super) fn charged_bytes(bytes: usize, parents: &[(Id, InputSite)]) -> Result<usize> {
    // Match save's additional normalized-parent and empty archive-field copies.
    // The initial body limit is insufficient: associations were charged later.
    let bytes = bytes
        .checked_add(serde_json::to_vec(parents)?.len())
        .and_then(|bytes| bytes.checked_add(2))
        .ok_or_else(denied)?;
    if bytes > MAX_BYTES {
        return Err(denied());
    }
    Ok(bytes)
}

#[cfg(test)]
mod accounting_tests {
    use super::*;

    #[test]
    fn normalized_wait_output_bytes_include_every_stored_copy() {
        assert_eq!(charged_bytes(MAX_BYTES - 4, &[]).unwrap(), MAX_BYTES);
        assert!(charged_bytes(MAX_BYTES - 3, &[]).is_err());
        assert!(charged_bytes(usize::MAX, &[]).is_err());
        let parents = [(
            1,
            InputSite {
                execution: 2,
                kind: crate::repositories::inherited_input::InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: String::new(),
            },
        )];
        let charge = serde_json::to_vec(&parents).unwrap().len() + 2;
        assert_eq!(
            charged_bytes(MAX_BYTES - charge, &parents).unwrap(),
            MAX_BYTES
        );
        assert!(charged_bytes(MAX_BYTES - charge + 1, &parents).is_err());
    }
}
