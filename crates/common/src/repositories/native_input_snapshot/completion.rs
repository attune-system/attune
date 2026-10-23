//! Finite ordinary-workflow failure recipe. H alone projects scheduler state;
//! this factory borrows H's opaque observation and freezes its actual sources.
use super::super::{
    native_failure_materialization::PreparedNativeFailure,
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository},
    FindById,
};
use super::*;

/// Each child retains its own ceiling and actual original field paths. This is
/// not a synthetic child snapshot or a union of independent contributor grants.
pub struct NativeCompletionSource {
    source: ResultViewSource,
}
impl NativeCompletionSource {
    pub(super) fn new(source: ResultViewSource) -> Self {
        Self { source }
    }
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn admission(&self) -> Id {
        self.source.admission
    }
    pub fn task_name(&self) -> &str {
        &self.source.task_name
    }
    pub fn status(&self) -> &str {
        &self.source.status
    }
    pub fn timed_out(&self) -> bool {
        self.source.timed_out
    }
    pub fn target(&self) -> &BoundRuntimeTarget {
        &self.source.target
    }
    pub fn read_owner(&self) -> &super::super::runtime_read::RuntimeReadOwner {
        &self.source.read_owner
    }
    pub fn original_contract(&self) -> Option<&Value> {
        self.source.contract.as_ref()
    }
    pub fn contract_kind(&self) -> NativeResultContractKind {
        self.source.contract_kind
    }
    pub fn original_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        self.source
            .fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn original_version(&self) -> &Value {
        &self.source.version
    }
    pub fn contributor_manifests(
        &self,
    ) -> &[super::super::accepted_input::AcceptedContributorManifest] {
        &self.source.contributors
    }
    pub fn receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.source.parents
    }
    pub fn origins(&self) -> &[SecretOrigin] {
        &self.source.origins
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletionSource {
    pub(super) execution: Id,
    pub(super) workflow: Id,
    pub(super) completed: Id,
    pub(super) transaction: String,
    pub(super) parent_original: Value,
    pub(super) workflow_original: Value,
    pub(super) contract: ActionSchemaBinding,
    pub(super) parent_digest: String,
    pub(super) workflow_digest: String,
    pub(super) contract_digest: String,
    pub(super) target: BoundRuntimeTarget,
    pub(super) read_owner: super::super::runtime_read::RuntimeReadOwner,
    pub(super) children: Vec<ResultViewSource>,
    pub(super) failed: Vec<(String, Id)>,
    pub(super) parents: Vec<(Id, InputSite)>,
    pub(super) controls: Vec<(Id, InputSite)>,
    pub(super) fields: Vec<ArchivedField>,
    pub(super) origins: Vec<SecretOrigin>,
}

pub struct PreparedNativeCompletionMetadata {
    pub(crate) transaction: String,
    pub(crate) renderer: Id,
    pub(crate) snapshot: Id,
    pub(crate) value: Value,
    pub(crate) fence: ApprovalEffectFence,
    source: CompletionSource,
    original_checked: std::sync::OnceLock<()>,
}
impl PreparedNativeCompletionMetadata {
    /// Failure status/summary depends on real child observations and independent
    /// decisions. Protect the complete native payload conservatively, including
    /// invalid-schema fallback and any originally annotated generated field.
    pub fn secret_path_sources(
        &self,
        receipt: &ReceiptContext,
    ) -> Result<Vec<crate::secret_values::SecretPathSource>> {
        if receipt.site().execution != self.source.execution
            || receipt.site().kind
                != super::super::inherited_input::InputEntityKind::ExecutionResult
            || receipt.site().purpose != InputSitePurpose::Value
            || !receipt.site().path.is_empty()
            || receipt.accepted_value() != &self.value
            || !receipt
                .native_source_metadata()
                .iter()
                .any(|metadata| metadata.snapshot() == self.snapshot)
        {
            return Err(denied());
        }
        Ok(vec![crate::secret_values::SecretPathSource {
            path: String::new(),
            source: crate::secret_values::SecretSource::Bound(receipt.provenance()),
        }])
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn require_original_rows(
        &self,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
    ) -> Result<()> {
        if serde_json::to_value(parent)? != self.source.parent_original
            || serde_json::to_value(workflow)? != self.source.workflow_original
        {
            return Err(denied());
        }
        self.original_checked.get_or_init(|| ());
        Ok(())
    }
    /// A final target fact, not an input to the source recipe or H's algorithm.
    pub fn require_projected_failure(&self, actual: &[String]) -> Result<()> {
        if actual.iter().map(String::as_str).ne(self
            .source
            .failed
            .iter()
            .map(|(name, _)| name.as_str()))
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        if self.original_checked.get().is_none() {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await?;
        for source in &self.source.children {
            result_view::compare_current(conn, source).await?;
        }
        let binding =
            SchemaContractRepository::find(conn, BindingOwner::Execution, self.source.execution)
                .await?
                .ok_or_else(denied)?;
        if binding != self.source.contract {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }
    /// Call after the pure original-row guard, and after B has produced its
    /// actual final failed set, before the output/terminal mutation.
    pub async fn finalize(
        &self,
        conn: &mut PgConnection,
        actual_failed_tasks: &[String],
    ) -> Result<()> {
        self.require_projected_failure(actual_failed_tasks)?;
        self.require_current(conn).await?;
        InheritedInputRepository::require_cache_selector_publications(conn, &self.source.parents)
            .await?;
        let changed=sqlx::query("INSERT INTO runtime_native_completion_release(snapshot,execution,completed_execution,release_transaction) VALUES($1,$2,$3,pg_current_xact_id()::TEXT) ON CONFLICT DO NOTHING").bind(self.snapshot).bind(self.source.execution).bind(self.source.completed).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }
}

impl NativeInputSnapshotRepository {
    pub async fn prepare_native_completion_metadata(
        conn: &mut PgConnection,
        state: &PreparedNativeFailure,
        controls: &[&PreparedInputSource],
        key: &str,
    ) -> Result<PreparedNativeCompletionMetadata> {
        state.ensure_fresh_on(conn).await?;
        let record = DeclaredInputRepository::require_source(conn, state.execution()).await?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let mut children = Vec::new();
        let mut parents = Vec::new();
        let mut origins = Vec::new();
        for actual in state.source_executions() {
            let (source, _, child_fence) =
                Self::prepare_result_source(conn, &record, actual.id, key).await?;
            let row =
                super::super::execution::ExecutionRepository::find_by_id(&mut *conn, actual.id)
                    .await?
                    .ok_or_else(denied)?;
            let binding = SchemaContractRepository::find(conn, BindingOwner::Execution, actual.id)
                .await?
                .ok_or_else(denied)?;
            state.require_original_child(
                &row,
                &binding,
                &source
                    .fields
                    .iter()
                    .map(ArchivedField::stored)
                    .collect::<Vec<_>>(),
            )?;
            if source.workflow != state.workflow().id
                || source.transaction != transaction(conn).await?
            {
                return Err(denied());
            }
            fence = fence.intersect(&child_fence);
            for parent in &source.parents {
                if !parents.contains(parent) {
                    parents.push(parent.clone());
                }
            }
            for origin in &source.origins {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
            children.push(source);
        }
        let mut held_controls = Vec::new();
        for control in controls {
            for (receipt, site, control_fence) in
                InheritedInputRepository::native_control_dependencies(conn, record.id, control)
                    .await?
            {
                fence = fence.intersect(&control_fence);
                let parent = (receipt, site);
                if !parents.contains(&parent) {
                    parents.push(parent.clone());
                }
                if !held_controls.contains(&parent) {
                    held_controls.push(parent);
                }
            }
        }
        let mut failed = Vec::new();
        for name in state.failed_tasks() {
            let child = children
                .iter()
                .find(|source| {
                    source.task_name == *name
                        && matches!(source.status.as_str(), "failed" | "timeout")
                })
                .ok_or_else(denied)?;
            failed.push((name.clone(), child.execution));
        }
        let parent =
            super::super::execution::ExecutionRepository::find_by_id(&mut *conn, state.execution())
                .await?
                .ok_or_else(denied)?;
        let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id(
            &mut *conn,
            state.workflow().id,
        )
        .await?
        .ok_or_else(denied)?;
        state.require_current_rows(&parent, &workflow)?;
        let contract =
            SchemaContractRepository::find(conn, BindingOwner::Execution, state.execution())
                .await?
                .ok_or_else(denied)?;
        if &contract != state.output_contract() {
            return Err(denied());
        }
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(state.execution()),
            record.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let source = CompletionSource {
            execution: state.execution(),
            workflow: state.workflow().id,
            completed: state.completed_execution(),
            transaction: transaction(conn).await?,
            parent_digest: hash(&serde_json::to_value(&parent)?)?,
            workflow_digest: hash(&serde_json::to_value(&workflow)?)?,
            contract_digest: hash(&serde_json::to_value(&contract)?)?,
            parent_original: serde_json::to_value(&parent)?,
            workflow_original: serde_json::to_value(&workflow)?,
            contract,
            target: bound_target(state.execution(), &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            fields: archive_fields(&children)?,
            children,
            failed,
            parents,
            controls: held_controls,
            origins,
        };
        validate_storage(&source)?;
        let value = value(&source)?;
        state.require_metadata(&value)?;
        let body = Body {
            renderer: record.id,
            execution: state.execution(),
            task_path: String::new(),
            authority: record.authority.0,
            approval_deadline: fence.approval_deadline(),
            source: Source::Completion(source.clone()),
        };
        let snapshot = save(conn, &body, &value, key).await?;
        for (receipt, _) in &source.parents {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(receipt).execute(&mut *conn).await?;
        }
        for admission in admissions(&source) {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)").bind(snapshot).bind(admission).execute(&mut *conn).await?;
        }
        archive_wait(conn, snapshot, &source.fields).await?;
        state.ensure_fresh_on(conn).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeCompletionMetadata {
            transaction: source.transaction.clone(),
            renderer: record.id,
            snapshot,
            value,
            fence,
            source,
            original_checked: std::sync::OnceLock::new(),
        })
    }

    pub(crate) async fn validate_native_completion_recipe(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        plaintext: Option<(&Value, &str)>,
        pending: bool,
    ) -> Result<()> {
        let body = if let Some((expected, key)) = plaintext {
            let (body, value, _, _) = load(conn, snapshot, key).await?;
            if value != *expected {
                return Err(denied());
            }
            let Source::Completion(source) = &body.source else {
                return Err(denied());
            };
            validate_value(conn, snapshot, source, &value, key).await?;
            body
        } else {
            let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
            let Source::Completion(source) = &body.source else {
                return Err(denied());
            };
            validate_storage(source)?;
            if load_fields(conn, "native_input_snapshot", snapshot).await? != source.fields {
                return Err(denied());
            }
            body
        };
        let Source::Completion(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer || source.execution != body.execution {
            return Err(denied());
        }
        if !pending || source.transaction != transaction(conn).await? {
            require_release(conn, snapshot, source).await?;
        }
        Ok(())
    }
}

pub(super) fn value(source: &CompletionSource) -> Result<Value> {
    let names = source
        .failed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    let value = json!({"succeeded":false,"error":format!("Workflow failed: {} task(s) failed: {}",names.len(),names.join(", "))});
    bounded(&value)?;
    Ok(value)
}
pub(super) fn admissions(source: &CompletionSource) -> Vec<Id> {
    source
        .children
        .iter()
        .map(|source| source.admission)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub(super) fn archive_fields(children: &[ResultViewSource]) -> Result<Vec<ArchivedField>> {
    let mut fields = Vec::new();
    for (index, child) in children.iter().enumerate() {
        for original in &child.fields {
            let mut field = original.clone();
            field.json_path = format!("/children/{index}{}", original.json_path);
            field.source_kind = "provenance".into();
            field.source_ref = Some(serde_json::to_string(&original.provenance()?)?);
            fields.push(field);
        }
    }
    Ok(fields)
}
pub(super) fn validate_storage(source: &CompletionSource) -> Result<()> {
    if hash(&source.parent_original)? != source.parent_digest
        || hash(&source.workflow_original)? != source.workflow_digest
        || hash(&serde_json::to_value(&source.contract)?)? != source.contract_digest
    {
        return Err(denied());
    }
    if source.execution <= 0
        || source.workflow <= 0
        || source.completed <= 0
        || source.transaction.is_empty()
        || source.children.is_empty()
        || source.children.len() > 256
        || source.parents.len() > 256
        || source.fields.len() > 256
        || source.origins.len() > 256
        || source.failed.is_empty()
        || source.failed.len() > 256
        || source.target.id != source.execution
        || source.target.kind != "execution"
    {
        return Err(denied());
    }
    source.contract.validate()?;
    if let Some(contract) = &source.contract.output {
        crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
    }
    if source.parent_original.get("id").and_then(Value::as_i64) != Some(source.execution)
        || source.workflow_original.get("id").and_then(Value::as_i64) != Some(source.workflow)
        || source
            .workflow_original
            .get("execution")
            .and_then(Value::as_i64)
            != Some(source.execution)
        || archive_fields(&source.children)? != source.fields
    {
        return Err(denied());
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for child in &source.children {
        result_view::validate_storage(child)?;
        if child.workflow != source.workflow
            || child.transaction != source.transaction
            || !identities.insert(child.execution)
            || !names.insert(&child.task_name)
            || !matches!(child.status.as_str(), "completed" | "failed" | "timeout")
        {
            return Err(denied());
        }
        for parent in &child.parents {
            if !source.parents.contains(parent) {
                return Err(denied());
            }
        }
        for origin in &child.origins {
            if !source.origins.contains(origin) {
                return Err(denied());
            }
        }
    }
    if !identities.contains(&source.completed) {
        return Err(denied());
    }
    let mut selected = std::collections::BTreeSet::new();
    for (name, execution) in &source.failed {
        if !selected.insert(name)
            || !source.children.iter().any(|child| {
                child.execution == *execution
                    && child.task_name == *name
                    && matches!(child.status.as_str(), "failed" | "timeout")
            })
        {
            return Err(denied());
        }
    }
    for control in &source.controls {
        if !source.parents.contains(control) {
            return Err(denied());
        }
    }
    Ok(())
}
pub(super) async fn require_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &CompletionSource,
) -> Result<()> {
    let matched:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_completion_release WHERE snapshot=$1 AND execution=$2 AND completed_execution=$3 AND release_transaction=$4)").bind(snapshot).bind(source.execution).bind(source.completed).bind(&source.transaction).fetch_one(conn).await?;
    if !matched {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn validate_value(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &CompletionSource,
    actual: &Value,
    key: &str,
) -> Result<()> {
    validate_storage(source)?;
    for child in &source.children {
        result_view::original_value(child, key)?;
    }
    if value(source)? != *actual
        || load_fields(conn, "native_input_snapshot", snapshot).await? != source.fields
    {
        return Err(denied());
    }
    Ok(())
}
