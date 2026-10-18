//! A separate constructive recipe for the original empty ordinary list. No
//! execution member, wait, cache frontier or aggregate result Value is invented.
use super::super::{
    native_iteration_materialization::NativeEmptyIterationSource,
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    FindById,
};
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyIterationArchive {
    pub(super) execution: Id,
    pub(super) workflow: Id,
    definition: Id,
    task_name: String,
    task_path: String,
    transaction: String,
    control: Id,
    control_site: InputSite,
    control_proof: String,
    control_ciphertext: String,
    pub(super) parents: Vec<(Id, InputSite)>,
    schema: ActionSchemaBinding,
    executable: crate::models::ExecutionExecutableSnapshot,
    pub(super) target: BoundRuntimeTarget,
    read_owner: super::super::runtime_read::RuntimeReadOwner,
}

pub struct PreparedNativeEmptyIterationOutcome {
    pub(in crate::repositories) transaction: String,
    pub(in crate::repositories) renderer: Id,
    pub(in crate::repositories) snapshot: Id,
    pub(in crate::repositories) task_path: String,
    pub(in crate::repositories) fence: ApprovalEffectFence,
    archive: EmptyIterationArchive,
    original_parent: crate::models::Execution,
    original_workflow: crate::models::WorkflowExecution,
    original_fields: Vec<crate::secret_values::StoredSecretValue>,
    guarded: std::sync::OnceLock<()>,
    released: std::sync::OnceLock<()>,
}

/// Non-serializable authority to compose this actual empty source before its
/// release. Only its owning preparation token can issue it.
#[derive(Clone)]
pub(in crate::repositories) struct IssuedEmptyRead {
    transaction: String,
    renderer: Id,
    snapshot: Id,
    archive_digest: String,
    fence: ApprovalEffectFence,
}

impl IssuedEmptyRead {
    pub(in crate::repositories) fn same_source(&self, other: &Self) -> bool {
        self.transaction == other.transaction
            && self.renderer == other.renderer
            && self.snapshot == other.snapshot
            && self.archive_digest == other.archive_digest
            && self.fence.approval_deadline() == other.fence.approval_deadline()
    }
    pub(in crate::repositories) fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub(in crate::repositories) async fn require_stage(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<()> {
        if snapshot != self.snapshot
            || renderer != self.renderer
            || transaction(conn).await? != self.transaction
        {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        let (body, value, _, _) = load(conn, snapshot, key).await?;
        let Source::EmptyIteration(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer
            || source.transaction != self.transaction
            || hash(&serde_json::to_value(source)?)? != self.archive_digest
            || !value.is_null()
        {
            return Err(denied());
        }
        let record = RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
        require_empty_identity(conn, &record, &body).await?;
        require_empty_evidence(conn, snapshot, source, true, key).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub(in crate::repositories) async fn evidence(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        path: &str,
        context: &str,
        key: &str,
    ) -> Result<SnapshotEvidence> {
        self.require_stage(conn, snapshot, renderer, key).await?;
        NativeInputSnapshotRepository::evidence(conn, snapshot, renderer, path, context, key, true)
            .await
    }
}

/// Authenticated normalized row, for storage/retention callers without a key.
pub struct NativeEmptyIterationStorageRelease {
    snapshot: Id,
    execution: Id,
    workflow: Id,
    control: Id,
    transaction: String,
}
impl NativeEmptyIterationStorageRelease {
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn execution(&self) -> Id {
        self.execution
    }
    pub fn workflow(&self) -> Id {
        self.workflow
    }
    pub fn control(&self) -> Id {
        self.control
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}
pub(super) fn storage_release(
    snapshot: Id,
    source: &EmptyIterationArchive,
) -> NativeEmptyIterationStorageRelease {
    NativeEmptyIterationStorageRelease {
        snapshot,
        execution: source.execution,
        workflow: source.workflow,
        control: source.control,
        transaction: source.transaction.clone(),
    }
}

impl PreparedNativeEmptyIterationOutcome {
    pub(in crate::repositories) fn issued_read(&self) -> Result<IssuedEmptyRead> {
        self.ensure_fresh()?;
        Ok(IssuedEmptyRead {
            transaction: self.transaction.clone(),
            renderer: self.renderer,
            snapshot: self.snapshot,
            archive_digest: hash(&serde_json::to_value(&self.archive)?)?,
            fence: self.fence.clone(),
        })
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self.fence.approval_deadline())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if transaction(conn).await? != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
    /// Owner-loaded original rows, before FIRST publication. No replacement
    /// baseline or caller-projected workflow can authorize this guard.
    pub async fn guard_original(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let parent = super::super::execution::ExecutionRepository::find_by_id(
            &mut *conn,
            self.archive.execution,
        )
        .await?
        .ok_or_else(denied)?;
        let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id_for_update(
            &mut *conn,
            self.archive.workflow,
        )
        .await?
        .ok_or_else(denied)?;
        if serde_json::to_value(parent)? != serde_json::to_value(&self.original_parent)?
            || serde_json::to_value(workflow)? != serde_json::to_value(&self.original_workflow)?
            || super::super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *conn, crate::secret_values::ENTITY_WORKFLOW_VARIABLES, self.archive.execution).await? != self.original_fields
            || Schemas::find(conn, BindingOwner::Execution, self.archive.execution).await?.as_ref() != Some(&self.archive.schema)
        { return Err(denied()); }
        require_empty_dispatch(conn, &self.archive).await?;
        self.guarded.set(()).map_err(|_| denied())
    }
    /// After the actual upstream publication seal and original Control finalize,
    /// before any logical completion/dispatch effect. One issuing transaction.
    pub(crate) async fn release(
        &self,
        conn: &mut PgConnection,
        source: &NativeEmptyIterationSource<'_>,
    ) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if source.parent().id != self.archive.execution
            || source.workflow().id != self.archive.workflow
            || source.task_path() != self.archive.task_path
            || source.control().origin()
                != (SecretOrigin::Entity {
                    entity_type: self
                        .archive
                        .control_site
                        .kind
                        .origin_kind(self.archive.control_site.purpose),
                    entity_id: self.archive.control,
                    path: self.archive.control_site.path.clone(),
                })
        {
            return Err(denied());
        }
        source.require_projected(conn).await?;
        if Schemas::find(conn, BindingOwner::Execution, self.archive.execution)
            .await?
            .as_ref()
            != Some(&self.archive.schema)
        {
            return Err(denied());
        }
        let parent = super::super::execution::ExecutionRepository::find_by_id(
            &mut *conn,
            self.archive.execution,
        )
        .await?
        .ok_or_else(denied)?;
        super::super::accepted_execution_source::same_execution(&self.original_parent, &parent)?;
        if self.guarded.get().is_none() || self.released.get().is_some() {
            return Err(denied());
        }
        require_empty_dispatch(conn, &self.archive).await?;
        require_empty_control(conn, &self.archive, false).await?;
        sqlx::query("INSERT INTO runtime_native_empty_iteration_release(snapshot,execution,workflow,control,release_transaction) VALUES($1,$2,$3,$4,pg_current_xact_id()::TEXT)")
            .bind(self.snapshot).bind(self.archive.execution).bind(self.archive.workflow).bind(self.archive.control).execute(&mut *conn).await?;
        self.released.set(()).map_err(|_| denied())?;
        self.ensure_fresh_on(conn).await
    }
    pub async fn require_released(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if self.released.get().is_none() {
            return Err(denied());
        }
        require_empty_release(conn, self.snapshot, &self.archive).await
    }
}

impl NativeInputSnapshotRepository {
    pub(crate) async fn require_empty_literal_control_snapshot(
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        path: &str,
    ) -> Result<()> {
        let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
        if body.renderer != renderer
            || !matches!(body.source, Source::EmptyIteration(_))
            || !path.starts_with("/workflow/tasks/")
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) async fn reopen_empty_iteration(
        conn: &mut PgConnection,
        execution: Id,
        task: &str,
        key: &str,
    ) -> Result<Option<ReleasedNativeEmptyIteration>> {
        let ids: Vec<Id> = sqlx::query_scalar("SELECT snapshot.id FROM runtime_native_input_snapshot snapshot JOIN runtime_native_empty_iteration_release released ON released.snapshot=snapshot.id WHERE released.execution=$1 AND snapshot.body#>>'{source,task_name}'=$2 ORDER BY snapshot.id LIMIT 2")
            .bind(execution).bind(task).fetch_all(&mut *conn).await?;
        if ids.is_empty() {
            return Ok(None);
        }
        if ids.len() != 1 {
            return Err(denied());
        }
        let (body, value, _, _) = load(conn, ids[0], key).await?;
        let Source::EmptyIteration(source) = &body.source else {
            return Err(denied());
        };
        if source.execution != execution || source.task_name != task || !value.is_null() {
            return Err(denied());
        }
        let record =
            RuntimeAuthorityRepository::require_retained_admission(conn, body.renderer).await?;
        require_empty_identity(conn, &record, &body).await?;
        require_empty_evidence(conn, ids[0], source, false, key).await?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, body.renderer)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::selected_effect_fence(conn, &body.authority).await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?);
        let mut receipts = Vec::new();
        for (id, site) in &source.parents {
            let receipt =
                InheritedInputRepository::empty_iteration_receipt(conn, *id, site, false, key)
                    .await?;
            fence = fence.intersect(receipt.accepted_fence());
            receipts.push(receipt);
        }
        let graph = &source
            .executable
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let task = graph
            .pointer(
                source
                    .task_path
                    .strip_prefix("/workflow")
                    .ok_or_else(denied)?,
            )
            .ok_or_else(denied)?;
        let expected_conditions: Vec<_> = task
            .get("next")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, transition)| {
                transition
                    .get("when")
                    .and_then(Value::as_str)
                    .is_some_and(|expression| expression.contains("{{"))
            })
            .map(|(index, _)| format!("{}/next/{index}/when", source.task_path))
            .collect();
        let program = InheritedInputRepository::reopen_empty_program_controls(
            conn,
            body.renderer,
            ids[0],
            &source.task_path,
            &expected_conditions,
            key,
        )
        .await?;
        // Recover selection from authenticated original Boolean receipts only.
        // Current variables, Keys and expressions never select replacement work.
        let mut selected_targets = std::collections::BTreeSet::new();
        for (index, transition) in task
            .get("next")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let fires = if transition.get("when").is_some_and(|value| !value.is_null()) {
                let path = format!("{}/next/{index}/when", source.task_path);
                let receipt = program
                    .iter()
                    .find(|receipt| {
                        receipt.site().path == path
                            && receipt.site().purpose == InputSitePurpose::Condition
                    })
                    .ok_or_else(denied)?;
                receipt.accepted_value().as_bool().ok_or_else(denied)?
            } else {
                true
            };
            if fires {
                for target in transition
                    .get("do")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    selected_targets.insert(target.as_str().ok_or_else(denied)?.to_owned());
                }
            }
        }
        for receipt in program {
            fence = fence.intersect(receipt.accepted_fence());
            receipts.push(receipt);
        }
        Ok(Some(ReleasedNativeEmptyIteration {
            workflow: source.workflow,
            transaction: transaction(conn).await?,
            fence,
            receipts,
            selected_targets: selected_targets.into_iter().collect(),
        }))
    }
    pub async fn prepare_empty_iteration(
        conn: &mut PgConnection,
        source: &NativeEmptyIterationSource<'_>,
        key: &str,
    ) -> Result<PreparedNativeEmptyIterationOutcome> {
        source.require_source_current(conn).await?;
        let record = DeclaredInputRepository::require_source(conn, source.parent().id).await?;
        let control = source.control_receipt();
        let (control_id, control_site) =
            InheritedInputRepository::receipt_source_site(conn, &control.origin()).await?;
        let mut parents = vec![(control_id, control_site.clone())];
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(&ApprovalEffectFence::from_input_approval(
                source.held_deadline()?,
            )?)
            .intersect(control.accepted_fence());
        for receipt in source.receipts() {
            receipt.ensure_fresh_on(conn).await?;
            fence = fence.intersect(receipt.accepted_fence());
            let parent =
                InheritedInputRepository::receipt_source_site(conn, &receipt.origin()).await?;
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }
        if parents.len() > 256 {
            return Err(denied());
        }
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(source.parent().id),
            record.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let (proof, encrypted): (String, Value) = sqlx::query_as(
            "SELECT proof_digest,encrypted_value FROM runtime_input_receipt WHERE id=$1",
        )
        .bind(control_id)
        .fetch_one(&mut *conn)
        .await?;
        let archive = EmptyIterationArchive {
            execution: source.parent().id,
            workflow: source.workflow().id,
            definition: source.workflow().workflow_def,
            task_name: source.task_name().into(),
            task_path: source.task_path().into(),
            transaction: transaction(conn).await?,
            control: control_id,
            control_site,
            control_proof: proof,
            control_ciphertext: hash(&encrypted)?,
            parents,
            schema: Schemas::find(conn, BindingOwner::Execution, source.parent().id)
                .await?
                .ok_or_else(denied)?,
            executable: source
                .parent()
                .executable_snapshot
                .clone()
                .ok_or_else(denied)?,
            target: bound_target(source.parent().id, &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
        };
        require_empty_control(conn, &archive, true).await?;
        let body = Body {
            renderer: record.id,
            execution: archive.execution,
            task_path: archive.task_path.clone(),
            authority: record.authority.0.clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::EmptyIteration(archive.clone()),
        };
        let snapshot = save(conn, &body, &Value::Null, key).await?;
        for (receipt, _) in &archive.parents {
            sqlx::query(
                "INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2)",
            )
            .bind(snapshot)
            .bind(receipt)
            .execute(&mut *conn)
            .await?;
        }
        Ok(PreparedNativeEmptyIterationOutcome {
            transaction: archive.transaction.clone(), renderer: record.id, snapshot, task_path: archive.task_path.clone(), fence, archive,
            original_parent: source.parent().clone(), original_workflow: source.workflow().clone(),
            original_fields: super::super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *conn, crate::secret_values::ENTITY_WORKFLOW_VARIABLES, source.parent().id).await?,
            guarded: std::sync::OnceLock::new(), released: std::sync::OnceLock::new(),
        })
    }
}

pub(crate) struct ReleasedNativeEmptyIteration {
    selected_targets: Vec<String>,
    workflow: Id,
    transaction: String,
    fence: ApprovalEffectFence,
    receipts: Vec<super::super::inherited_input::ReceiptContext>,
}
impl ReleasedNativeEmptyIteration {
    pub(crate) fn selected_targets(&self) -> &[String] {
        &self.selected_targets
    }
    pub(crate) fn held_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        std::iter::once(self.fence.approval_deadline())
            .chain(
                self.receipts
                    .iter()
                    .map(super::super::inherited_input::ReceiptContext::approval_deadline),
            )
            .flatten()
            .min()
    }
    pub(crate) fn workflow(&self) -> Id {
        self.workflow
    }
    pub(crate) fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if transaction(conn).await? != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}

async fn require_empty_dispatch(
    conn: &mut PgConnection,
    source: &EmptyIterationArchive,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2",
    )
    .bind(source.workflow)
    .bind(&source.task_name)
    .fetch_one(&mut *conn)
    .await?;
    let children: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution WHERE parent=$1 AND workflow_task->>'task_name'=$2",
    )
    .bind(source.execution)
    .bind(&source.task_name)
    .fetch_one(&mut *conn)
    .await?;
    if count != 0 || children != 0 {
        return Err(denied());
    }
    Ok(())
}

async fn require_empty_control(
    conn: &mut PgConnection,
    source: &EmptyIterationArchive,
    pending: bool,
) -> Result<()> {
    if source.control_site.execution != source.execution
        || source.control_site.path != format!("{}/with_items", source.task_path)
        || source.control_site.purpose != InputSitePurpose::Control
        || source.control_site.kind
            != super::super::inherited_input::InputEntityKind::WorkflowVariables
        || !source
            .parents
            .contains(&(source.control, source.control_site.clone()))
        || source
            .executable
            .executable
            .workflow_definition
            .as_ref()
            .is_none_or(|definition| definition.id != source.definition)
    {
        return Err(denied());
    }
    let row: Option<(Id, Id, Id, String, String, Option<String>, Value, String, Value)> = sqlx::query_as(
        "SELECT binding.execution,binding.workflow,binding.workflow_definition,binding.task_path,binding.prepared_transaction,binding.publication_transaction,receipt.proof,receipt.proof_digest,receipt.encrypted_value FROM runtime_native_iteration_control binding JOIN runtime_input_receipt receipt ON receipt.id=binding.receipt WHERE binding.receipt=$1")
        .bind(source.control).fetch_optional(&mut *conn).await?;
    let (
        execution,
        workflow,
        definition,
        path,
        prepared,
        published,
        proof,
        proof_digest,
        encrypted,
    ) = row.ok_or_else(denied)?;
    if (execution, workflow, definition) != (source.execution, source.workflow, source.definition)
        || path != source.control_site.path
        || prepared != source.transaction
        || hash(&proof)? != proof_digest
        || proof_digest != source.control_proof
        || hash(&encrypted)? != source.control_ciphertext
        || (!pending && published.as_deref() != Some(source.transaction.as_str()))
        || (pending && published.is_some())
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn require_empty_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &EmptyIterationArchive,
) -> Result<()> {
    require_empty_control(conn, source, false).await?;
    let row: Option<(Id, Id, Id, String)> = sqlx::query_as("SELECT execution,workflow,control,release_transaction FROM runtime_native_empty_iteration_release WHERE snapshot=$1")
        .bind(snapshot).fetch_optional(&mut *conn).await?;
    if row
        != Some((
            source.execution,
            source.workflow,
            source.control,
            source.transaction.clone(),
        ))
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn require_empty_normalized(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &EmptyIterationArchive,
) -> Result<()> {
    let parents: Vec<Id> = sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt")
        .bind(snapshot).fetch_all(&mut *conn).await?;
    let expected: std::collections::BTreeSet<_> =
        source.parents.iter().map(|(id, _)| *id).collect();
    if expected.len() != source.parents.len()
        || parents
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            != expected
    {
        return Err(denied());
    }
    let wrong_recipe: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_member WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_release WHERE snapshot=$1)")
        .bind(snapshot).fetch_one(&mut *conn).await?;
    if wrong_recipe {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn empty_metadata(
    snapshot: Id,
    source: &EmptyIterationArchive,
) -> NativeSnapshotMetadata {
    NativeSnapshotMetadata {
        result_contract_kind: None,
        result_mappings: None,
        snapshot,
        source_kind: "empty_iteration_outcome".into(),
        target: source.target.clone(),
        read_owner: source.read_owner.clone(),
        generation: None,
        source_site: None,
        read_version: json!({"workflow":source.workflow,"definition":source.definition,"task":source.task_path,"control":source.control,"transaction":source.transaction}),
    }
}

pub(super) fn empty_associations(renderer: Id, source: &EmptyIterationArchive) -> Value {
    json!({"budget":renderer,"admissions":[],"release":null,
        "empty_iteration_release":[Id::MAX,source.execution,source.workflow,source.control,source.transaction]})
}

pub(super) async fn require_empty_identity(
    conn: &mut PgConnection,
    record: &super::super::runtime_authority::RuntimeAdmissionRecord,
    body: &Body,
) -> Result<()> {
    let Source::EmptyIteration(source) = &body.source else {
        return Err(denied());
    };
    if record.owner_kind != "execution"
        || record.owner_id != source.execution
        || body.renderer != record.id
        || body.execution != source.execution
        || body.task_path != source.task_path
        || body.authority != record.authority.0
        || record.authority.target_release_id != source.executable.release.id
        || record.authority.target_release_digest != source.executable.release.digest
    {
        return Err(denied());
    }
    let task = task_definition(conn, record, &source.task_path).await?;
    if task.get("name").and_then(Value::as_str) != Some(source.task_name.as_str())
        || task.get("with_items").is_none_or(Value::is_null)
        || source.control_site.path != format!("{}/with_items", source.task_path)
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn empty_selection(
    source: &EmptyIterationArchive,
    declaration: &str,
    context: &str,
) -> Result<()> {
    if context == "$control" && declaration.starts_with("/workflow/tasks/") {
        return Ok(());
    }
    if !wait_outcome_path(&source.task_path, declaration)
        || !["succeeded", "failed", "timed_out", "cancelled"]
            .iter()
            .any(|name| context == format!("/$outcome/{name}"))
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn require_empty_evidence(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &EmptyIterationArchive,
    pending: bool,
    key: &str,
) -> Result<()> {
    let released: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_native_empty_iteration_release WHERE snapshot=$1)",
    )
    .bind(snapshot)
    .fetch_one(&mut *conn)
    .await?;
    if released {
        require_empty_release(conn, snapshot, source).await?;
    } else if pending && transaction(conn).await? == source.transaction {
        require_empty_control(conn, source, true).await?;
        require_empty_dispatch(conn, source).await?;
    } else {
        return Err(denied());
    }
    // The receipt/native graph reader traverses every parent returned by this
    // evidence, including source.control. Reopening that graph here resets its
    // accounting and recurses back into native evidence. Authenticate only the
    // exact original control ciphertext at this boundary; the owning walker
    // still validates its canonical proof, admissions, origins and deadlines.
    let (encrypted, key_hash): (Value, String) = sqlx::query_as(
        "SELECT encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1",
    )
    .bind(source.control)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(denied)?;
    bounded(&encrypted)?;
    if key_hash != crate::crypto::hash_encryption_key(key)
        || hash(&encrypted)? != source.control_ciphertext
        || crate::crypto::decrypt_json(&encrypted, key)? != json!([])
    {
        return Err(denied());
    }
    Ok(())
}
