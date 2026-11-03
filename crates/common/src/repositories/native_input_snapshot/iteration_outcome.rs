//! A finite observation of D's complete nonempty ordinary sibling set.
//! This archive authenticates lifecycle CONTROL, never a child result VALUE.
use super::super::{
    native_iteration_materialization::PreparedNativeIterationCompletion,
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    FindById,
};
use super::*;
use crate::models::ExecutionExecutableSnapshot;
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIterationObservation {
    execution: Id,
    admission: Id,
    original_execution: Option<Id>,
    ordinal: i32,
    task_batch: Option<i32>,
    retry_count: i32,
    metadata_retry_count: i32,
    created: chrono::DateTime<chrono::Utc>,
    status: String,
    timed_out: bool,
    schema: ActionSchemaBinding,
    executable: ExecutionExecutableSnapshot,
    target: BoundRuntimeTarget,
    read_owner: super::super::runtime_read::RuntimeReadOwner,
    contributors: Vec<super::super::accepted_input::AcceptedContributorManifest>,
    input_fields: Vec<ArchivedField>,
    input_receipts: Vec<(Id, InputSite)>,
}
impl NativeIterationObservation {
    pub(super) fn storage_nodes(&self) -> usize {
        self.contributors.len() + self.input_fields.len() + self.input_receipts.len() + 1
    }
    pub fn original_input_fields(&self) -> Vec<crate::secret_values::StoredSecretValue> {
        self.input_fields
            .iter()
            .map(ArchivedField::stored)
            .collect()
    }
    pub fn input_receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.input_receipts
    }
    pub fn execution(&self) -> Id {
        self.execution
    }
    pub fn admission(&self) -> Id {
        self.admission
    }
    pub fn original_execution(&self) -> Option<Id> {
        self.original_execution
    }
    pub fn ordinal(&self) -> i32 {
        self.ordinal
    }
    pub fn task_batch(&self) -> Option<i32> {
        self.task_batch
    }
    pub fn retry_count(&self) -> i32 {
        self.retry_count
    }
    pub fn metadata_retry_count(&self) -> i32 {
        self.metadata_retry_count
    }
    pub fn created(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.created
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }
    pub fn original_schema(&self) -> &ActionSchemaBinding {
        &self.schema
    }
    pub fn original_executable(&self) -> &ExecutionExecutableSnapshot {
        &self.executable
    }
    pub fn target(&self) -> &BoundRuntimeTarget {
        &self.target
    }
    pub fn read_owner(&self) -> &super::super::runtime_read::RuntimeReadOwner {
        &self.read_owner
    }
    pub fn contributor_manifests(
        &self,
    ) -> &[super::super::accepted_input::AcceptedContributorManifest] {
        &self.contributors
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIterationOutcomeMember {
    original: NativeIterationObservation,
    latest: NativeIterationObservation,
}
impl NativeIterationOutcomeMember {
    pub fn original(&self) -> &NativeIterationObservation {
        &self.original
    }
    pub fn latest(&self) -> &NativeIterationObservation {
        &self.latest
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IterationOutcomeSource {
    execution: Id,
    workflow: Id,
    workflow_definition: Id,
    task_name: String,
    task_path: String,
    transaction: String,
    status: String,
    target: BoundRuntimeTarget,
    read_owner: super::super::runtime_read::RuntimeReadOwner,
    executable: ExecutionExecutableSnapshot,
    schema: ActionSchemaBinding,
    parents: Vec<(Id, InputSite)>,
    list_controls: Vec<(Id, InputSite)>,
    members: Vec<NativeIterationOutcomeMember>,
    members_digest: String,
}

pub(super) fn convert_encryption_metadata(
    source: &mut IterationOutcomeSource,
    conversion: &crate::repositories::encryption_conversion::VerifierConversion<'_>,
) -> Result<()> {
    validate_storage(source)?;
    for member in &mut source.members {
        for observation in [&mut member.original, &mut member.latest] {
            for contributor in &observation.contributors {
                contributor.validate()?;
            }
            super::encryption_conversion::fields(&mut observation.input_fields, conversion)?;
        }
    }
    source.members_digest = hash(&serde_json::to_value(&source.members)?)?;
    validate_storage(source)
}

/// Authenticated frozen observation for C/F. Its manifests are independent
/// ceilings. Neither metadata nor these read-only getters confer reader rights.
pub struct NativeIterationOutcomeEvidence {
    source: IterationOutcomeSource,
}
impl NativeIterationOutcomeEvidence {
    pub(super) fn new(source: &IterationOutcomeSource) -> Self {
        Self {
            source: source.clone(),
        }
    }
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn workflow(&self) -> Id {
        self.source.workflow
    }
    pub fn workflow_definition(&self) -> Id {
        self.source.workflow_definition
    }
    pub fn task_name(&self) -> &str {
        &self.source.task_name
    }
    pub fn task_path(&self) -> &str {
        &self.source.task_path
    }
    pub fn status(&self) -> &str {
        &self.source.status
    }
    pub fn transaction(&self) -> &str {
        &self.source.transaction
    }
    pub fn members(&self) -> &[NativeIterationOutcomeMember] {
        &self.source.members
    }
    pub fn receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.source.parents
    }
    pub fn list_controls(&self) -> &[(Id, InputSite)] {
        &self.source.list_controls
    }
    pub fn original_schema(&self) -> &ActionSchemaBinding {
        &self.source.schema
    }
    pub fn original_executable(&self) -> &ExecutionExecutableSnapshot {
        &self.source.executable
    }
}

/// Owns a shared borrow of the genuine D source. No public constructor, serde,
/// detached status constructor, synthetic execution or zero-member witness.
pub struct PreparedNativeIterationOutcome {
    source: Arc<PreparedNativeIterationCompletion>,
    archive: IterationOutcomeSource,
    input_buffers: std::collections::BTreeMap<Id, Option<Value>>,
    pub(crate) transaction: String,
    pub(crate) renderer: Id,
    pub(crate) snapshot: Id,
    pub(crate) task_path: String,
    pub(crate) status: String,
    pub(crate) fence: ApprovalEffectFence,
}
impl PreparedNativeIterationOutcome {
    pub(in crate::repositories) fn held_deadline(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self
            .source
            .held_deadline()?
            .into_iter()
            .chain(self.fence.approval_deadline())
            .min())
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.source.ensure_fresh()?;
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.source.ensure_fresh_on(conn).await?;
        self.compare_inputs(conn).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    async fn compare_inputs(&self, conn: &mut PgConnection) -> Result<()> {
        // Buffer comparison only. In particular, retain the selected retry's
        // inputs too, even when D's original-sibling input set does not include
        // that later execution. Do not reopen acceptance or source IAM here.
        let mut visited = std::collections::BTreeSet::new();
        for member in &self.archive.members {
            for observed in [&member.original, &member.latest] {
                if !visited.insert(observed.execution) {
                    continue;
                }
                let row = super::super::execution::ExecutionRepository::find_by_id(
                    &mut *conn,
                    observed.execution,
                )
                .await?
                .ok_or_else(denied)?;
                if self.input_buffers.get(&observed.execution) != Some(&row.config)
                    || serde_json::to_value(&row.executable_snapshot)?
                        != serde_json::to_value(Some(&observed.executable))?
                    || Schemas::find(conn, BindingOwner::Execution, observed.execution)
                        .await?
                        .as_ref()
                        != Some(&observed.schema)
                    || load_fields(conn, "execution_config", observed.execution).await?
                        != observed.input_fields
                {
                    return Err(denied());
                }
            }
        }
        Ok(())
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        self.source.require_source_current(conn).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub async fn require_members_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        self.source.require_members_current(conn).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn require_original_rows(
        &self,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
    ) -> Result<()> {
        self.ensure_fresh()?;
        if serde_json::to_value(parent)? != serde_json::to_value(self.source.original_parent())?
            || serde_json::to_value(workflow)? != serde_json::to_value(self.source.workflow())?
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl NativeInputSnapshotRepository {
    pub async fn prepare_native_iteration_outcome(
        conn: &mut PgConnection,
        source: &Arc<PreparedNativeIterationCompletion>,
        key: &str,
    ) -> Result<PreparedNativeIterationOutcome> {
        source.require_source_current(conn).await?;
        let count = source.ordinal_count();
        if count == 0
            || count > 256
            || source.original_siblings().len() != count
            || source.members().len() != count
        {
            return Err(denied());
        }
        let record = DeclaredInputRepository::require_source(conn, source.renderer()).await?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let mut parents = Vec::new();
        let mut list_controls = Vec::new();
        let list_path = format!("{}/with_items", source.task_path());
        for receipt in source.input_receipts().iter().chain(source.controls()) {
            receipt.ensure_fresh_on(conn).await?;
            fence = fence.intersect(receipt.accepted_fence());
            let parent =
                InheritedInputRepository::receipt_source_site(conn, &receipt.origin()).await?;
            if !parents.contains(&parent) {
                parents.push(parent.clone());
            }
            if is_list_control(&parent.1, source.renderer(), &list_path)
                && !list_controls.contains(&parent)
            {
                list_controls.push(parent);
            }
            for dependency in receipt.source_receipts() {
                if is_list_control(dependency.site(), source.renderer(), &list_path) {
                    let parent = (dependency.receipt(), dependency.site().clone());
                    if !parents.contains(&parent) {
                        parents.push(parent.clone());
                    }
                    if !list_controls.contains(&parent) {
                        list_controls.push(parent);
                    }
                }
            }
        }
        // Recovered sources must carry the original sealed list lineage. Never
        // reevaluate with_items or reinterpret missing dispatch as empty success.
        if list_controls.is_empty() || parents.len() > 256 {
            return Err(denied());
        }
        for (id, site) in &parents {
            let origin = SecretOrigin::Entity {
                entity_type: site.kind.origin_kind(site.purpose),
                entity_id: *id,
                path: site.path.clone(),
            };
            let receipt =
                InheritedInputRepository::receipt_context(conn, site, &origin, key).await?;
            if list_controls.contains(&(*id, site.clone()))
                && InheritedInputRepository::original_iteration_cardinality(conn, &record, &receipt)
                    .await?
                    != count
            {
                return Err(denied());
            }
            fence = fence.intersect(receipt.accepted_fence());
        }
        let dispatches:Vec<(i32,Id)>=sqlx::query_as("SELECT task_index,execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index IS NOT NULL ORDER BY task_index LIMIT 257").bind(source.workflow().id).bind(source.task_name()).fetch_all(&mut *conn).await?;
        if dispatches.len() != count {
            return Err(denied());
        }
        let mut members = Vec::new();
        let mut member_bytes = 0usize;
        let mut input_buffers = std::collections::BTreeMap::new();
        for (index, member) in source.members().enumerate() {
            let original = &source.original_siblings()[index];
            let ordinal = i32::try_from(index).map_err(|_| denied())?;
            let outcome = member.outcome();
            let member_status = status(member.status())?;
            let transaction = transaction(conn).await?;
            let (outcome_admission, outcome_timed_out, outcome_fence) = outcome
                .require_iteration_observation(
                    &transaction,
                    record.id,
                    member.execution(),
                    source.task_name(),
                    &member_status,
                )?;
            if member.ordinal() != ordinal
                || dispatches[index] != (ordinal, original.id)
                || member.original_execution() != original.id
                || original.original_execution.is_some()
            {
                return Err(denied());
            }
            fence = fence.intersect(outcome_fence);
            let latest = super::super::execution::ExecutionRepository::find_by_id(
                &mut *conn,
                member.execution(),
            )
            .await?
            .ok_or_else(denied)?;
            if latest.created != *member.created() || latest.retry_count != member.retry_count() {
                return Err(denied());
            }
            for row in [original, &latest] {
                if input_buffers
                    .insert(row.id, row.config.clone())
                    .is_some_and(|previous| previous != row.config)
                {
                    return Err(denied());
                }
            }
            let (original, original_fence) = observe(conn, original, source, ordinal, key).await?;
            let (latest, latest_fence) = observe(conn, &latest, source, ordinal, key).await?;
            for receipt in original.input_receipts.iter().chain(&latest.input_receipts) {
                if !parents.contains(receipt) {
                    parents.push(receipt.clone());
                }
            }
            if latest.admission != outcome_admission.id
                || latest.status != member_status
                || latest.timed_out != outcome_timed_out
                || latest.task_batch != member.task_batch()
                || latest.metadata_retry_count != member.metadata_retry_count()
                || latest.original_execution.unwrap_or(latest.execution) != original.execution
            {
                return Err(denied());
            }
            fence = fence.intersect(&original_fence).intersect(&latest_fence);
            let member = NativeIterationOutcomeMember { original, latest };
            // Reserve both immutable copies while assembling the source, not
            // after accumulating an unbounded set of contracts/ciphertexts.
            let charge = serde_json::to_vec(&member)?
                .len()
                .checked_mul(2)
                .ok_or_else(denied)?;
            member_bytes = member_bytes.checked_add(charge).ok_or_else(denied)?;
            if member_bytes > MAX_BYTES {
                return Err(denied());
            }
            members.push(member);
        }
        let parent = source.original_parent();
        let schema = Schemas::find(conn, BindingOwner::Execution, parent.id)
            .await?
            .ok_or_else(denied)?;
        let executable = parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?
            .clone();
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(parent.id),
            record.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let archive = IterationOutcomeSource {
            execution: parent.id,
            workflow: source.workflow().id,
            workflow_definition: source.workflow().workflow_def,
            task_name: source.task_name().into(),
            task_path: source.task_path().into(),
            transaction: transaction(conn).await?,
            status: status(source.status())?,
            target: bound_target(parent.id, &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            members_digest: hash(&serde_json::to_value(&members)?)?,
            executable,
            schema,
            parents,
            list_controls,
            members,
        };
        validate_storage(&archive)?;
        let body = Body {
            renderer: record.id,
            execution: parent.id,
            task_path: archive.task_path.clone(),
            authority: record.authority.0.clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::IterationOutcome(archive.clone()),
        };
        validate_body(&body, &archive)?;
        let snapshot = save(conn, &body, &values(&archive)?, key).await?;
        for (receipt, _) in &archive.parents {
            sqlx::query(
                "INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2)",
            )
            .bind(snapshot)
            .bind(receipt)
            .execute(&mut *conn)
            .await?;
        }
        for admission in admissions(&archive) {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)").bind(snapshot).bind(admission).execute(&mut *conn).await?;
        }
        for member in &archive.members {
            sqlx::query("INSERT INTO runtime_native_iteration_outcome_member(snapshot,ordinal,member) VALUES($1,$2,$3)").bind(snapshot).bind(member.original.ordinal).bind(serde_json::to_value(member)?).execute(&mut *conn).await?;
        }
        source.require_source_current(conn).await?;
        // This seals an already complete persisted observation, not a projected
        // workflow mutation or pending native capability of another kind.
        sqlx::query("INSERT INTO runtime_native_iteration_outcome_release(snapshot,execution,workflow,release_transaction) VALUES($1,$2,$3,pg_current_xact_id()::TEXT)").bind(snapshot).bind(parent.id).bind(archive.workflow).execute(&mut *conn).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeIterationOutcome {
            source: Arc::clone(source),
            input_buffers,
            transaction: archive.transaction.clone(),
            renderer: record.id,
            snapshot,
            task_path: archive.task_path.clone(),
            status: archive.status.clone(),
            fence,
            archive,
        })
    }
}

fn is_list_control(site: &InputSite, execution: Id, path: &str) -> bool {
    site.execution == execution
        && site.kind == super::super::inherited_input::InputEntityKind::WorkflowVariables
        && site.purpose == InputSitePurpose::Control
        && site.path == path
}
fn status(value: crate::models::ExecutionStatus) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(denied)
}
async fn observe(
    conn: &mut PgConnection,
    row: &crate::models::Execution,
    source: &PreparedNativeIterationCompletion,
    ordinal: i32,
    key: &str,
) -> Result<(NativeIterationObservation, ApprovalEffectFence)> {
    let metadata = row.workflow_task.as_ref().ok_or_else(denied)?;
    if row.parent != Some(source.renderer())
        || metadata.workflow_execution != source.workflow().id
        || metadata.task_name != source.task_name()
        || metadata.task_index != Some(ordinal)
    {
        return Err(denied());
    }
    let admission = RuntimeAuthorityRepository::execution_admission(conn, row.id)
        .await?
        .ok_or_else(denied)?;
    let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?;
    let schema = Schemas::find(conn, BindingOwner::Execution, row.id)
        .await?
        .ok_or_else(denied)?;
    let executable = row.executable_snapshot.as_ref().ok_or_else(denied)?.clone();
    let root = source
        .original_parent()
        .executable_snapshot
        .as_ref()
        .ok_or_else(denied)?;
    let InvocationBinding::WorkflowTask {
        workflow_ref,
        task_path,
        action_ref,
    } = &admission.authority.binding
    else {
        return Err(denied());
    };
    let graph = &root
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    if workflow_ref != &root.executable.action.r#ref
        || action_ref != &row.action_ref
        || canonical_task_path(graph, task_path)
            .map(|path| format!("/workflow{path}"))
            .as_deref()
            != Some(source.task_path())
        || row.action != Some(executable.executable.action.id)
        || row.action_ref != executable.executable.action.r#ref
        || row.pack_release != Some(executable.release.id)
        || row.pack_release_digest.as_deref() != Some(executable.release.digest.as_str())
        || admission.authority.target_release_id != executable.release.id
        || admission.authority.target_release_digest != executable.release.digest
    {
        return Err(denied());
    }
    let (_, contributors) =
        super::super::accepted_input::AcceptedInputRepository::source_contributors(conn, row.id)
            .await?;
    let resolved = RuntimeTargetRepository::resolve(
        conn,
        RuntimeTarget::Execution(row.id),
        admission.authority.owner.sponsor_identity_id,
    )
    .await?
    .ok_or_else(denied)?;
    // Existing canonical observation only. Never delivery to a terminal process,
    // creation/repair of acceptance, or recapture of a current human authority.
    let accepted = super::super::accepted_input::AcceptedInputRepository::revalidate_execution(
        conn, row.id, key,
    )
    .await?;
    let fence = fence.intersect(&accepted);
    let (fields, receipts) =
        super::super::accepted_execution_source::prepare_observation(conn, row, &schema, key)
            .await?;
    let mut input_receipts = Vec::new();
    let mut fence = fence;
    for receipt in &receipts {
        receipt.ensure_fresh_on(conn).await?;
        fence = fence.intersect(receipt.accepted_fence());
        let edge = InheritedInputRepository::receipt_source_site(conn, &receipt.origin()).await?;
        if !input_receipts.contains(&edge) {
            input_receipts.push(edge);
        }
    }
    let input_fields = fields
        .into_iter()
        .map(|field| ArchivedField {
            json_path: field.json_path,
            encrypted_value: field.encrypted_value,
            encryption_key_hash: field.encryption_key_hash,
            source_kind: field.source_kind,
            source_ref: field.source_ref,
        })
        .collect();
    Ok((
        NativeIterationObservation {
            execution: row.id,
            admission: admission.id,
            original_execution: row.original_execution,
            ordinal,
            task_batch: metadata.task_batch,
            retry_count: row.retry_count,
            metadata_retry_count: metadata.retry_count,
            created: row.created,
            status: status(row.status)?,
            timed_out: metadata.timed_out,
            schema,
            executable,
            target: bound_target(row.id, &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            contributors,
            input_fields,
            input_receipts,
        },
        fence,
    ))
}

pub(super) fn admissions(source: &IterationOutcomeSource) -> Vec<Id> {
    source
        .members
        .iter()
        .flat_map(|m| [m.original.admission, m.latest.admission])
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub(super) fn parents(source: &IterationOutcomeSource) -> &[(Id, InputSite)] {
    &source.parents
}
pub(super) fn origins(snapshot: Id) -> Vec<SecretOrigin> {
    // A distinct archived lifecycle source. Never claim a child result field;
    // readers that do not implement this typed source must fail closed.
    vec![SecretOrigin::Entity {
        entity_type: "ordinary_iteration_outcome".into(),
        entity_id: snapshot,
        path: String::new(),
    }]
}
pub(super) fn evidence(source: &IterationOutcomeSource) -> NativeIterationOutcomeEvidence {
    NativeIterationOutcomeEvidence::new(source)
}
pub(super) fn target(
    source: &IterationOutcomeSource,
) -> (
    &BoundRuntimeTarget,
    &super::super::runtime_read::RuntimeReadOwner,
) {
    (&source.target, &source.read_owner)
}
pub(super) fn read_version(source: &IterationOutcomeSource) -> Value {
    json!({"workflow":source.workflow,"workflow_definition":source.workflow_definition,"task_path":source.task_path,"task_name":source.task_name,"status":source.status,"members_digest":source.members_digest})
}
pub(super) fn selection_scope(
    source: &IterationOutcomeSource,
    path: &str,
    context: &str,
) -> Result<()> {
    if context == "$control" {
        return Ok(());
    }
    let suffix = path
        .strip_prefix(&format!("{}/next/", source.task_path))
        .ok_or_else(denied)?;
    let tokens = suffix.split('/').collect::<Vec<_>>();
    if tokens.len() != 2
        || tokens[0].parse::<usize>().is_err()
        || tokens[1] != "when"
        || !["succeeded", "failed", "timed_out", "cancelled"]
            .iter()
            .any(|name| context == format!("/$outcome/{name}"))
    {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn values(source: &IterationOutcomeSource) -> Result<Value> {
    function_values(&source.status)
}
fn function_values(status: &str) -> Result<Value> {
    let completed = status == "completed";
    if !completed && status != "failed" {
        return Err(denied());
    }
    Ok(json!({"succeeded":completed,"failed":!completed,"timed_out":false,"cancelled":false}))
}
fn lifecycle_status<'a>(statuses: impl IntoIterator<Item = &'a str>) -> Result<&'static str> {
    let mut nonempty = false;
    let mut completed = true;
    for status in statuses {
        if !matches!(
            status,
            "completed" | "failed" | "timeout" | "cancelled" | "abandoned"
        ) {
            return Err(denied());
        }
        nonempty = true;
        completed &= status == "completed";
    }
    if !nonempty {
        return Err(denied());
    }
    Ok(if completed { "completed" } else { "failed" })
}
pub(super) fn validate_storage(source: &IterationOutcomeSource) -> Result<()> {
    if hash(&serde_json::to_value(&source.members)?)? != source.members_digest {
        return Err(denied());
    }
    if source.execution <= 0
        || source.workflow <= 0
        || source.workflow_definition <= 0
        || source.transaction.is_empty()
        || source.members.is_empty()
        || source.members.len() > 256
        || source.parents.len() > 256
        || source.list_controls.is_empty()
        || source.list_controls.len() > 256
        || source.target.id != source.execution
        || source.target.kind != "execution"
    {
        return Err(denied());
    }
    source.schema.validate()?;
    if source.schema.input.source
        != (crate::value_schema::SourceIdentity::Released {
            component_ref: source.executable.executable.action.r#ref.clone(),
            release_id: source.executable.release.id,
            release_digest: source.executable.release.digest.clone(),
        })
    {
        return Err(denied());
    }
    if source
        .executable
        .executable
        .workflow_definition
        .as_ref()
        .is_none_or(|graph| graph.id != source.workflow_definition)
    {
        return Err(denied());
    }
    let graph = &source
        .executable
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    let pointer = source
        .task_path
        .strip_prefix("/workflow")
        .ok_or_else(denied)?;
    let task = graph.pointer(pointer).ok_or_else(denied)?;
    let (container, token) = pointer.rsplit_once('/').ok_or_else(denied)?;
    let name = if graph.pointer(container).is_some_and(Value::is_object) {
        token.replace("~1", "/").replace("~0", "~")
    } else {
        task.get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned()
    };
    if name != source.task_name
        || name.is_empty()
        || task.get("with_items").is_none_or(Value::is_null)
        || task.get("iterate_cache").is_some_and(|v| !v.is_null())
    {
        return Err(denied());
    }
    for control in &source.list_controls {
        if !source.parents.contains(control)
            || !is_list_control(
                &control.1,
                source.execution,
                &format!("{}/with_items", source.task_path),
            )
        {
            return Err(denied());
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut latest_ids = std::collections::BTreeSet::new();
    for (index, member) in source.members.iter().enumerate() {
        let ordinal = i32::try_from(index).map_err(|_| denied())?;
        if member.original.ordinal != ordinal
            || member.latest.ordinal != ordinal
            || member.original.original_execution.is_some()
            || member.original.retry_count != 0
            || member.original.metadata_retry_count != 0
            || member.latest.retry_count != member.latest.metadata_retry_count
            || member.latest.task_batch != member.original.task_batch
            || (member.latest.execution == member.original.execution)
                != (member.latest.retry_count == 0)
            || member
                .latest
                .original_execution
                .unwrap_or(member.latest.execution)
                != member.original.execution
            || !ids.insert(member.original.execution)
            || !latest_ids.insert(member.latest.execution)
        {
            return Err(denied());
        }
        if member.latest.execution == member.original.execution
            && serde_json::to_value(&member.latest)? != serde_json::to_value(&member.original)?
        {
            return Err(denied());
        }
        for row in [&member.original, &member.latest] {
            if row.execution <= 0
                || row.admission <= 0
                || row.retry_count < 0
                || row.metadata_retry_count < 0
                || row.target.id != row.execution
                || row.target.kind != "execution"
                || !matches!(
                    row.status.as_str(),
                    "completed" | "failed" | "timeout" | "cancelled" | "abandoned"
                )
            {
                return Err(denied());
            }
            row.schema.validate()?;
            if row.input_fields.len() > 256 || row.input_receipts.len() > 256 {
                return Err(denied());
            }
            for field in &row.input_fields {
                if field.source_kind != "provenance" || field.provenance()?.origins.is_empty() {
                    return Err(denied());
                }
            }
            for receipt in &row.input_receipts {
                if !source.parents.contains(receipt) {
                    return Err(denied());
                }
            }
            if row.schema.input.source
                != (crate::value_schema::SourceIdentity::Released {
                    component_ref: row.executable.executable.action.r#ref.clone(),
                    release_id: row.executable.release.id,
                    release_digest: row.executable.release.digest.clone(),
                })
            {
                return Err(denied());
            }
            for contributor in &row.contributors {
                contributor.validate()?;
            }
        }
    }
    let expected = lifecycle_status(source.members.iter().map(|m| m.latest.status.as_str()))?;
    if source.status != expected {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn validate_body(body: &Body, source: &IterationOutcomeSource) -> Result<()> {
    if body.execution != source.execution
        || body.task_path != source.task_path
        || body.authority.binding.action_ref() != source.executable.executable.action.r#ref
        || body.authority.target_release_id != source.executable.release.id
        || body.authority.target_release_digest != source.executable.release.digest
    {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn require_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &IterationOutcomeSource,
) -> Result<()> {
    let matched:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_release WHERE snapshot=$1 AND execution=$2 AND workflow=$3 AND release_transaction=$4)").bind(snapshot).bind(source.execution).bind(source.workflow).bind(&source.transaction).fetch_one(conn).await?;
    if !matched {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn validate_normalized(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &IterationOutcomeSource,
) -> Result<()> {
    validate_storage(source)?;
    let rows:Vec<(i32,Value)>=sqlx::query_as("SELECT ordinal,member FROM runtime_native_iteration_outcome_member WHERE snapshot=$1 ORDER BY ordinal LIMIT 257").bind(snapshot).fetch_all(conn).await?;
    if rows.len() != source.members.len() {
        return Err(denied());
    }
    for ((ordinal, actual), member) in rows.into_iter().zip(&source.members) {
        if ordinal != member.original.ordinal || actual != serde_json::to_value(member)? {
            return Err(denied());
        }
    }
    Ok(())
}
pub(super) fn associations(source: &IterationOutcomeSource, renderer: Id) -> Value {
    json!({"budget":renderer,"admissions":admissions(source),"release":null,
        "iteration_outcome_members":source.members.iter().map(|m|json!([Id::MAX,m.original.ordinal,m])).collect::<Vec<_>>(),
        "iteration_outcome_release":[Id::MAX,source.execution,source.workflow,source.transaction]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonempty_group_lifecycle_does_not_use_the_last_successful_child() {
        assert_eq!(lifecycle_status(["failed", "completed"]).unwrap(), "failed");
        assert_eq!(
            lifecycle_status(["completed", "completed"]).unwrap(),
            "completed"
        );
        for failure in ["timeout", "cancelled", "abandoned"] {
            assert_eq!(lifecycle_status([failure, "completed"]).unwrap(), "failed");
        }
        assert!(lifecycle_status([]).is_err());
        assert!(lifecycle_status(["running"]).is_err());
    }
    #[test]
    fn group_functions_preserve_the_finite_collapsed_lifecycle_contract() {
        assert_eq!(
            function_values("failed").unwrap(),
            json!({"succeeded":false,"failed":true,"timed_out":false,"cancelled":false})
        );
        assert_eq!(
            function_values("completed").unwrap(),
            json!({"succeeded":true,"failed":false,"timed_out":false,"cancelled":false})
        );
        assert!(function_values("timeout").is_err());
    }
    #[test]
    fn aggregate_selection_has_no_caller_status_or_child_result_identity() {
        use super::super::super::inherited_input::SelectionSource;
        let source = SelectionSource::NativeIterationOutcome { snapshot: 7 };
        assert_eq!(
            serde_json::to_value(source).unwrap(),
            json!({"purpose":"native_iteration_outcome","snapshot":7})
        );
        assert!(serde_json::from_value::<SelectionSource>(json!({"purpose":"native_iteration_outcome","snapshot":7,"execution":1,"status":"completed"})).is_err());
    }
}
