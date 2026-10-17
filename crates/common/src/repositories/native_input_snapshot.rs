//! Repository-owned native source observations. Public model rows and supplied
//! JSON cannot construct these transaction-bound input proofs.
use super::{
    cache::CacheEntryRepository,
    declared_input::DeclaredInputRepository,
    execution_runtime_admission::{ExecutionAdmission, ExecutionAdmissionRepository},
    inherited_input::{
        CacheControlPublication, InheritedInputRepository, InputSite, InputSitePurpose,
        PreparedInputSource, ReceiptContext,
    },
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository},
    runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    workflow_cache_iteration::WorkflowCacheIterationRepository,
};
use crate::{
    models::{Id, WorkflowCacheIteration},
    runtime_authority::{
        BoundRuntimeTarget, InvocationBinding, RequestAttribution, SelectedRuntimeAuthority,
    },
    secret_provenance::SecretOrigin,
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgConnection;
mod completion;
mod success_metadata;
use success_metadata::SuccessSource;
pub use success_metadata::{
    NativeSuccessMetadataEvidence, NativeSuccessOutputLeaf, PreparedNativeSuccessMetadata,
};
mod iteration_outcome;
mod pending_result;
mod pending_wait;
pub(crate) use pending_result::IssuedResultRead;
mod result_view;
use completion::CompletionSource;
pub use completion::{NativeCompletionSource, PreparedNativeCompletionMetadata};
use iteration_outcome::IterationOutcomeSource;
pub use iteration_outcome::{
    NativeIterationObservation, NativeIterationOutcomeEvidence, NativeIterationOutcomeMember,
    PreparedNativeIterationOutcome,
};
pub(crate) use pending_wait::IssuedWaitRead;
use result_view::ResultViewSource;
pub use result_view::{
    NativeResultContractKind, NativeResultPointerMapping, PreparedNativeResultView,
};

const MAX_BYTES: usize = 1024 * 1024;
mod prospective_wait;
pub use prospective_wait::{PreparedWaitRegistration, RegisteredInitialWait};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheSource {
    iteration: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frontier: Option<CacheFrontierPlan>,
    workflow: Id,
    task_name: String,
    namespace: Id,
    generation: Id,
    target: BoundRuntimeTarget,
    read_owner: super::runtime_read::RuntimeReadOwner,
    before: Option<String>,
    after: Option<String>,
    ordinal: i64,
    page_size: i32,
    batch_size: i32,
    entries: Vec<(Id, String, Option<String>)>,
    exhausted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parents: Vec<(Id, InputSite)>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheFrontierPlan {
    prepared_transaction: String,
    predecessor: Option<Id>,
    initial: bool,
    concurrency: i32,
    scanned_before: i64,
    active_generation: bool,
    fresh_until: Option<chrono::DateTime<chrono::Utc>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    SuccessMetadata(SuccessSource),
    IterationOutcome(IterationOutcomeSource),
    Cache(CacheSource),
    Wait(WaitSource),
    ResultView(ResultViewSource),
    Completion(CompletionSource),
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ArchivedField {
    json_path: String,
    encrypted_value: Value,
    encryption_key_hash: Option<String>,
    source_kind: String,
    source_ref: Option<String>,
}
impl ArchivedField {
    fn from(field: &crate::secret_values::StoredSecretValue) -> Self {
        Self {
            json_path: field.json_path.clone(),
            encrypted_value: field.encrypted_value.clone(),
            encryption_key_hash: field.encryption_key_hash.clone(),
            source_kind: field.source_kind.clone(),
            source_ref: field.source_ref.clone(),
        }
    }

    fn stored(&self) -> crate::secret_values::StoredSecretValue {
        crate::secret_values::StoredSecretValue {
            json_path: self.json_path.clone(),
            encrypted_value: self.encrypted_value.clone(),
            encryption_key_hash: self.encryption_key_hash.clone(),
            source_kind: self.source_kind.clone(),
            source_ref: self.source_ref.clone(),
        }
    }
    fn provenance(&self) -> Result<crate::secret_provenance::SecretProvenance> {
        if self.source_kind == "provenance" {
            return Ok(serde_json::from_str(
                self.source_ref.as_deref().ok_or_else(denied)?,
            )?);
        }
        if self.source_kind == "parameter_schema"
            && self.source_ref.as_deref() == Some(self.json_path.as_str())
        {
            return Ok(crate::secret_values::SecretSource::ParameterSchema {
                path: self.json_path.clone(),
            }
            .provenance());
        }
        Err(denied())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitSource {
    wait: Id,
    workflow: Id,
    task_name: String,
    target: BoundRuntimeTarget,
    read_owner: super::runtime_read::RuntimeReadOwner,
    prepared_transaction: String,
    target_kind: String,
    target_id: Id,
    state: String,
    context_path: String,
    source_kind: String,
    source_id: Id,
    source_field: String,
    source_admission: Id,
    contributors: Vec<super::accepted_input::AcceptedContributorManifest>,
    raw_projection: Value,
    source_version: Value,
    contract: Option<Value>,
    fields: Vec<ArchivedField>,
    parents: Vec<(Id, InputSite)>,
    origins: Vec<SecretOrigin>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    renderer: Id,
    execution: Id,
    task_path: String,
    authority: SelectedRuntimeAuthority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
    source: Source,
}

/// Authenticated classification, not a reader grant or a public-field exception.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSnapshotMetadata {
    snapshot: Id,
    source_kind: String,
    target: BoundRuntimeTarget,
    read_owner: super::runtime_read::RuntimeReadOwner,
    generation: Option<Id>,
    read_version: Value,
    source_site: Option<(String, Id, String)>,
    result_mappings: Option<Vec<NativeResultPointerMapping>>,
    result_contract_kind: Option<NativeResultContractKind>,
}
impl NativeSnapshotMetadata {
    pub fn result_contract_kind(&self) -> Option<NativeResultContractKind> {
        self.result_contract_kind
    }
    pub fn result_pointer_mappings(&self) -> Option<&[NativeResultPointerMapping]> {
        self.result_mappings.as_deref()
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn source_kind(&self) -> &str {
        &self.source_kind
    }
    pub fn target(&self) -> &BoundRuntimeTarget {
        &self.target
    }
    pub fn read_owner(&self) -> &super::runtime_read::RuntimeReadOwner {
        &self.read_owner
    }
    pub fn generation(&self) -> Option<Id> {
        self.generation
    }
    pub fn read_version(&self) -> &Value {
        &self.read_version
    }
    pub fn source_site(&self) -> Option<(&str, Id, &str)> {
        self.source_site
            .as_ref()
            .map(|(kind, id, path)| (kind.as_str(), *id, path.as_str()))
    }
}
pub struct PreparedCacheInput {
    pub(crate) transaction: String,
    pub(crate) renderer: Id,
    pub(crate) snapshot: Id,
    pub(crate) task_path: String,
    pub(crate) value: Value,
    pub(crate) fence: ApprovalEffectFence,
    source: CacheSource,
    selector_publications: Vec<CacheControlPublication>,
    selector_bytes: usize,
    fresh_until: Option<chrono::DateTime<chrono::Utc>>,
}
/// Mixed committed/issued selector inputs, never caller data or receipt IDs.
pub struct CacheSelectorControl<'a> {
    source: CacheSelectorControlSource<'a>,
}
enum CacheSelectorControlSource<'a> {
    Recorded(&'a ReceiptContext),
    Prepared(&'a PreparedInputSource),
}
impl<'a> CacheSelectorControl<'a> {
    pub fn recorded(source: &'a ReceiptContext) -> Self {
        Self {
            source: CacheSelectorControlSource::Recorded(source),
        }
    }
    pub fn prepared(source: &'a PreparedInputSource) -> Self {
        Self {
            source: CacheSelectorControlSource::Prepared(source),
        }
    }
}
pub struct PreparedWaitInput {
    read_dependencies: Vec<IssuedWaitRead>,
    transaction: String,
    renderer: Id,
    snapshot: Id,
    task_path: String,
    value: Value,
    fence: ApprovalEffectFence,
    source: WaitSource,
}
pub struct ReleasedWaitInput {
    prepared: PreparedWaitInput,
}
/// Frozen source parents for C/F, never current reader permission. Cache readers
/// additionally retain the original task's source-freshness policy through IO.
pub struct NativeSourceEvidence {
    success_metadata: Option<NativeSuccessMetadataEvidence>,
    iteration_outcome: Option<NativeIterationOutcomeEvidence>,
    completion_sources: Vec<NativeCompletionSource>,
    contributors: Vec<super::accepted_input::AcceptedContributorManifest>,
    renderer: Id,
    policy: Id,
    approvals: Vec<Id>,
    admissions: Vec<Id>,
    fields: Vec<crate::secret_values::StoredSecretValue>,
    origins: Vec<SecretOrigin>,
    parents: Vec<Id>,
    contract: Option<crate::value_schema::Contract>,
    bytes: usize,
    cache_read_policy: Option<NativeCacheReadPolicy>,
}
/// Source-phase proof of the original task's cache freshness contract. The
/// generation row is held FOR SHARE by the caller's transaction when required.
/// Retain this token through every target/IO wait and check it after capture
/// commit immediately before byte, HEAD or EOF handoff. It grants no read right.
pub struct NativeCacheReadPolicy {
    require_fresh: bool,
    fresh_until: Option<chrono::DateTime<chrono::Utc>>,
}
impl NativeCacheReadPolicy {
    pub fn require_fresh(&self) -> bool {
        self.require_fresh
    }
    pub fn fresh_until(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.fresh_until
    }
    /// Pure final deadline check, with no source lookup or pointer selection.
    pub fn ensure_fresh(&self) -> Result<()> {
        self.check_at(chrono::Utc::now())
    }
    fn check_at(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        check_cache_freshness_deadline(self.fresh_until, now)
    }
}
/// Physical identities from an authenticated native body, not reader capabilities.
/// Cache entries are generation-qualified. Never recover these IDs from a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeStorageParent {
    Execution {
        execution: Id,
    },
    WorkflowExecution {
        workflow: Id,
    },
    WorkflowTaskWait {
        wait: Id,
    },
    CacheNamespace {
        namespace: Id,
    },
    CacheGeneration {
        generation: Id,
    },
    CacheIteration {
        iteration: Id,
    },
    CacheEntry {
        generation: Id,
        entry: Id,
    },
    Inquiry {
        inquiry: Id,
    },
    QueueItem {
        item: Id,
    },
    Pack {
        pack: Id,
    },
    Identity {
        identity: Id,
    },
    Action {
        action: Id,
    },
    Sensor {
        sensor: Id,
    },
    Event {
        event: Id,
        created: chrono::DateTime<chrono::Utc>,
    },
    Receipt {
        receipt: Id,
    },
    Admission {
        admission: Id,
    },
    Policy {
        policy: Id,
    },
    Approval {
        approval: Id,
    },
    NativeArchive {
        snapshot: Id,
    },
    NativeBudget {
        renderer_admission: Id,
    },
    NativeSnapshot {
        snapshot: Id,
    },
}

// Project C's canonical frozen owner once in A. F never parses its private JSON
// or looks up today's ownership. Unknown future owner fields fail closed here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenOwnerInventory {
    managing_pack: Option<Id>,
    pack: Option<Id>,
    owner: Option<Id>,
    scope: Option<crate::models::OwnerType>,
    owner_ref: Option<String>,
    owner_identity: Option<Id>,
    reference: Option<String>,
}
pub struct NativeStorageOwner {
    inventory: FrozenOwnerInventory,
}
impl NativeStorageOwner {
    pub fn managing_pack(&self) -> Option<Id> {
        self.inventory.managing_pack
    }
    pub fn pack(&self) -> Option<Id> {
        self.inventory.pack
    }
    pub fn owner(&self) -> Option<Id> {
        self.inventory.owner
    }
    pub fn scope(&self) -> Option<crate::models::OwnerType> {
        self.inventory.scope
    }
    pub fn owner_ref(&self) -> Option<&str> {
        self.inventory.owner_ref.as_deref()
    }
    pub fn owner_identity(&self) -> Option<Id> {
        self.inventory.owner_identity
    }
    pub fn reference(&self) -> Option<&str> {
        self.inventory.reference.as_deref()
    }
}
pub struct NativeCacheStorageEntry {
    id: Id,
    external_id: String,
    source_checksum: Option<String>,
}
impl NativeCacheStorageEntry {
    pub fn id(&self) -> Id {
        self.id
    }
    pub fn external_id(&self) -> &str {
        &self.external_id
    }
    pub fn source_checksum(&self) -> Option<&str> {
        self.source_checksum.as_deref()
    }
}
pub struct NativeCacheStorageParents {
    source: CacheSource,
    iteration: Id,
    entries: Vec<NativeCacheStorageEntry>,
}
impl NativeCacheStorageParents {
    pub fn namespace(&self) -> Id {
        self.source.namespace
    }
    pub fn generation(&self) -> Id {
        self.source.generation
    }
    pub fn iteration(&self) -> Id {
        self.iteration
    }
    pub fn workflow(&self) -> Id {
        self.source.workflow
    }
    pub fn entries(&self) -> &[NativeCacheStorageEntry] {
        &self.entries
    }
    pub fn ordinal(&self) -> i64 {
        self.source.ordinal
    }
    pub fn page_size(&self) -> i32 {
        self.source.page_size
    }
    pub fn batch_size(&self) -> i32 {
        self.source.batch_size
    }
    pub fn before(&self) -> Option<&str> {
        self.source.before.as_deref()
    }
    pub fn after(&self) -> Option<&str> {
        self.source.after.as_deref()
    }
    pub fn exhausted(&self) -> bool {
        self.source.exhausted
    }
}
pub struct NativeStorageWaitRelease {
    wait: Id,
    snapshot: Id,
    transaction: String,
}
pub struct NativeStorageResultViewRelease {
    snapshot: Id,
    execution: Id,
    admission: Id,
    transaction: String,
}
pub struct NativeStorageCompletionRelease {
    snapshot: Id,
    execution: Id,
    completed: Id,
    transaction: String,
}
impl NativeStorageCompletionRelease {
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn execution(&self) -> Id {
        self.execution
    }
    pub fn completed_execution(&self) -> Id {
        self.completed
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}
impl NativeStorageResultViewRelease {
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn execution(&self) -> Id {
        self.execution
    }
    pub fn admission(&self) -> Id {
        self.admission
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}
impl NativeStorageWaitRelease {
    pub fn wait(&self) -> Id {
        self.wait
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}
/// Storage authentication only. Private fields/no serde; no key or current IAM.
/// C's separate source_evidence path still owns current-reader authorization.
pub struct NativeSnapshotStorageEvidence {
    success_metadata: Option<NativeSuccessMetadataEvidence>,
    iteration_outcome: Option<NativeIterationOutcomeEvidence>,
    completion_sources: Vec<NativeCompletionSource>,
    metadata: NativeSnapshotMetadata,
    physical_parents: Vec<NativeStorageParent>,
    cache: Option<NativeCacheStorageParents>,
    frozen_owner: NativeStorageOwner,
    contributors: Vec<super::accepted_input::AcceptedContributorManifest>,
    contract: Option<crate::value_schema::Contract>,
    archive_fields: Vec<crate::secret_values::StoredSecretValue>,
    encrypted: Value,
    key_hash: String,
    receipts: Vec<(Id, InputSite)>,
    admissions: Vec<Id>,
    release: Option<NativeStorageWaitRelease>,
    result_release: Option<NativeStorageResultViewRelease>,
    completion_release: Option<NativeStorageCompletionRelease>,
    renderer: Id,
    policy: Id,
    approvals: Vec<Id>,
    origins: Vec<SecretOrigin>,
    bytes: usize,
}
impl NativeSnapshotStorageEvidence {
    pub fn success_metadata(&self) -> Option<&NativeSuccessMetadataEvidence> {
        self.success_metadata.as_ref()
    }
    pub fn iteration_outcome(&self) -> Option<&NativeIterationOutcomeEvidence> {
        self.iteration_outcome.as_ref()
    }
    pub fn completion_sources(&self) -> &[NativeCompletionSource] {
        &self.completion_sources
    }
    pub fn metadata(&self) -> &NativeSnapshotMetadata {
        &self.metadata
    }
    pub fn physical_parents(&self) -> &[NativeStorageParent] {
        &self.physical_parents
    }
    pub fn cache(&self) -> Option<&NativeCacheStorageParents> {
        self.cache.as_ref()
    }
    pub fn frozen_owner(&self) -> &NativeStorageOwner {
        &self.frozen_owner
    }
    pub fn contributor_manifests(&self) -> &[super::accepted_input::AcceptedContributorManifest] {
        &self.contributors
    }
    pub fn contract(&self) -> Option<&crate::value_schema::Contract> {
        self.contract.as_ref()
    }
    pub fn archive_fields(&self) -> &[crate::secret_values::StoredSecretValue] {
        &self.archive_fields
    }
    pub fn archive_owner(&self) -> (&'static str, Id) {
        ("native_input_snapshot", self.metadata.snapshot)
    }
    pub fn encrypted_projection(&self) -> &Value {
        &self.encrypted
    }
    pub fn encryption_key_hash(&self) -> &str {
        &self.key_hash
    }
    pub fn receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.receipts
    }
    pub fn admission_edges(&self) -> &[Id] {
        &self.admissions
    }
    pub fn wait_release(&self) -> Option<&NativeStorageWaitRelease> {
        self.release.as_ref()
    }
    pub fn result_view_release(&self) -> Option<&NativeStorageResultViewRelease> {
        self.result_release.as_ref()
    }
    pub fn completion_release(&self) -> Option<&NativeStorageCompletionRelease> {
        self.completion_release.as_ref()
    }
    pub fn renderer_admission(&self) -> Id {
        self.renderer
    }
    pub fn policy_revision(&self) -> Id {
        self.policy
    }
    pub fn approval_ids(&self) -> &[Id] {
        &self.approvals
    }
    pub fn origins(&self) -> &[SecretOrigin] {
        &self.origins
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}
impl NativeSourceEvidence {
    pub fn success_metadata(&self) -> Option<&NativeSuccessMetadataEvidence> {
        self.success_metadata.as_ref()
    }
    pub fn iteration_outcome(&self) -> Option<&NativeIterationOutcomeEvidence> {
        self.iteration_outcome.as_ref()
    }
    pub fn completion_sources(&self) -> &[NativeCompletionSource] {
        &self.completion_sources
    }
    pub fn cache_read_policy(&self) -> Option<&NativeCacheReadPolicy> {
        self.cache_read_policy.as_ref()
    }
    pub fn contributor_manifests(&self) -> &[super::accepted_input::AcceptedContributorManifest] {
        &self.contributors
    }
    pub fn renderer_admission(&self) -> Id {
        self.renderer
    }
    pub fn policy_revision(&self) -> Id {
        self.policy
    }
    pub fn approval_ids(&self) -> &[Id] {
        &self.approvals
    }
    pub fn source_admissions(&self) -> &[Id] {
        &self.admissions
    }
    pub fn fields(&self) -> &[crate::secret_values::StoredSecretValue] {
        &self.fields
    }
    pub fn origins(&self) -> &[SecretOrigin] {
        &self.origins
    }
    pub fn receipts(&self) -> &[Id] {
        &self.parents
    }
    pub fn contract(&self) -> Option<&crate::value_schema::Contract> {
        self.contract.as_ref()
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}
impl PreparedWaitInput {
    pub(crate) fn outcome_binding(&self) -> Result<NativeWaitOutcomeBinding<'_>> {
        Ok(NativeWaitOutcomeBinding {
            transaction: &self.transaction,
            renderer: self.renderer,
            snapshot: self.snapshot,
            task_path: &self.task_path,
            fence: &self.fence,
            values: native_wait_outcome_values(&self.source)?,
        })
    }
    pub(crate) fn binding(&self) -> (&str, Id, Id, &str, &str, &Value, &ApprovalEffectFence) {
        (
            &self.transaction,
            self.renderer,
            self.snapshot,
            &self.task_path,
            &self.source.context_path,
            &self.value,
            &self.fence,
        )
    }
}
pub(crate) struct NativeWaitOutcomeBinding<'a> {
    pub transaction: &'a str,
    pub renderer: Id,
    pub snapshot: Id,
    pub task_path: &'a str,
    pub fence: &'a ApprovalEffectFence,
    pub values: [Value; 4],
}
impl ReleasedWaitInput {
    pub(crate) fn outcome_binding(&self) -> Result<NativeWaitOutcomeBinding<'_>> {
        self.prepared.outcome_binding()
    }
    pub fn value(&self) -> &Value {
        &self.prepared.value
    }
    pub fn state(&self) -> &str {
        &self.prepared.source.state
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.prepared.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.prepared.transaction).await?;
        self.prepared.fence.ensure_fresh_on(conn).await
    }
    pub(crate) fn binding(&self) -> (&str, Id, Id, &str, &str, &Value, &ApprovalEffectFence) {
        (
            &self.prepared.transaction,
            self.prepared.renderer,
            self.prepared.snapshot,
            &self.prepared.task_path,
            &self.prepared.source.context_path,
            &self.prepared.value,
            &self.prepared.fence,
        )
    }
}
impl PreparedCacheInput {
    pub fn initial_create_input(
        &self,
    ) -> Result<super::workflow_cache_iteration::CreateWorkflowCacheIterationInput> {
        let plan = self.source.frontier.as_ref().ok_or_else(denied)?;
        if !plan.initial || plan.predecessor.is_some() || self.source.iteration.is_some() {
            return Err(denied());
        }
        self.ensure_fresh()?;
        Ok(
            super::workflow_cache_iteration::CreateWorkflowCacheIterationInput {
                workflow_execution: self.source.workflow,
                task_name: self.source.task_name.clone(),
                namespace: self.source.namespace,
                generation: self.source.generation,
                page_size: self.source.page_size,
                batch_size: self.source.batch_size,
                concurrency: plan.concurrency,
            },
        )
    }
    pub fn value(&self) -> Option<&Value> {
        if self.source.entries.is_empty() {
            None
        } else {
            Some(&self.value)
        }
    }
    pub fn index(&self) -> i64 {
        self.source.ordinal
    }
    pub fn after_external_id(&self) -> Option<&str> {
        self.source.after.as_deref()
    }
    pub fn exhausted(&self) -> bool {
        self.source.exhausted
    }
    pub fn count(&self) -> usize {
        self.source.entries.len()
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.check_cache_deadline(chrono::Utc::now())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await?;
        let now = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(conn)
            .await?;
        self.check_cache_deadline(now)
    }
    /// Original absolute deadline, also retained by linked pages and EOF. This
    /// getter is not a caller-provided date or a require_fresh policy inference.
    pub fn fresh_until(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.fresh_until
    }
    fn check_cache_deadline(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        check_cache_freshness_deadline(self.fresh_until, now)
    }
    /// Compare the actual target-locked owning row before publishing or advancing
    /// progress. Only the row identity/cursor is consumed; no supplied item JSON.
    pub async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        if self.source.frontier.is_some() {
            return NativeInputSnapshotRepository::bind_cache_frontier(
                conn,
                self,
                self.source.iteration.ok_or_else(denied)?,
            )
            .await;
        }
        let row = WorkflowCacheIterationRepository::find_by_id_for_update(
            &mut *conn,
            self.source.iteration.ok_or_else(denied)?,
        )
        .await?
        .ok_or_else(denied)?;
        if !cache_frontier_matches(&self.source, &row) {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
}
pub(crate) struct SnapshotEvidence {
    pub(crate) fence: ApprovalEffectFence,
    pub(crate) origins: Vec<SecretOrigin>,
    pub(crate) parents: Vec<(Id, InputSite)>,
    pub(crate) metadata: NativeSnapshotMetadata,
    pub(crate) bytes: usize,
}
pub(crate) struct SnapshotRetention {
    pub(crate) source_kind: String,
    pub(crate) admissions: Vec<Id>,
    pub(crate) renderer: Id,
    pub(crate) policy: Id,
    pub(crate) approvals: Vec<Id>,
    pub(crate) origins: Vec<SecretOrigin>,
    pub(crate) parents: Vec<Id>,
    pub(crate) bytes: usize,
}
pub struct NativeInputSnapshotRepository;
impl NativeInputSnapshotRepository {
    /// Compose actual selector Controls with pending VALUE ancestry, before all
    /// outer targets. The ordinary committed-context constructor remains intact.
    pub async fn prepare_cache_initial_from_controls(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        workflow: Id,
        max_bytes: usize,
        controls: &[CacheSelectorControl<'_>],
        key: &str,
    ) -> Result<PreparedCacheInput> {
        if controls.len() > 4 {
            return Err(denied());
        }
        // Reject stale/cross-TX tokens before touching any source/catalog gate.
        for control in controls {
            match &control.source {
                CacheSelectorControlSource::Recorded(source) => {
                    source.ensure_fresh_on(conn).await?
                }
                CacheSelectorControlSource::Prepared(source) => {
                    source.ensure_fresh_on(conn).await?
                }
            }
        }
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let mut issued = Vec::new();
        let mut publications = Vec::new();
        for (index, control) in controls.iter().enumerate() {
            match &control.source {
                CacheSelectorControlSource::Recorded(source) => {
                    source.require_published(conn).await?
                }
                CacheSelectorControlSource::Prepared(source) => {
                    let (context, publication) = snapshot_storage_step(
                        source.cache_selector_context(conn, record.id).await,
                        "cache-selector-context",
                    )?;
                    issued.push((index, context));
                    publications.push(publication);
                }
            }
        }
        let contexts = controls
            .iter()
            .enumerate()
            .map(|(index, control)| match &control.source {
                CacheSelectorControlSource::Recorded(source) => Ok(*source),
                CacheSelectorControlSource::Prepared(_) => issued
                    .iter()
                    .find(|(slot, _)| *slot == index)
                    .map(|(_, context)| context)
                    .ok_or_else(denied),
            })
            .collect::<Result<Vec<_>>>()?;
        let selector_bytes = snapshot_storage_step(
            InheritedInputRepository::require_cache_selector_bounds(conn, &contexts, key).await,
            "cache-selector-bounds",
        )?;
        let mut prepared = snapshot_storage_step(
            Self::prepare_cache_initial(
                conn, renderer, task_path, admission, workflow, max_bytes, &contexts, key,
            )
            .await,
            "cache-initial",
        )?;
        let (_, _, snapshot_bytes, _) = snapshot_storage_step(
            load_encrypted(conn, prepared.snapshot).await,
            "cache-initial-reload",
        )?;
        if selector_bytes
            .checked_add(snapshot_bytes)
            .is_none_or(|bytes| bytes > MAX_BYTES)
        {
            return Err(denied());
        }
        prepared.selector_publications = publications;
        prepared.selector_bytes = selector_bytes;
        Ok(prepared)
    }
    /// Source phase of atomic first activation. No iteration exists yet.
    pub async fn prepare_cache_initial(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        workflow: Id,
        max_bytes: usize,
        controls: &[&ReceiptContext],
        key: &str,
    ) -> Result<PreparedCacheInput> {
        use super::FindById;
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        require_task(conn, &record, task_path, admission.authority()).await?;
        let task = task_definition(conn, &record, task_path).await?;
        let task_name = task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let belongs: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE id=$1 AND execution=$2)",
        )
        .bind(workflow)
        .bind(renderer)
        .fetch_one(&mut *conn)
        .await?;
        if !belongs
            || WorkflowCacheIterationRepository::find_by_workflow_task(
                &mut *conn, workflow, &task_name,
            )
            .await?
            .is_some()
        {
            return Err(denied());
        }
        let config: crate::workflow::parser::IterateCacheConfig =
            serde_json::from_value(task.get("iterate_cache").cloned().ok_or_else(denied)?)?;
        let page_size = i32::try_from(config.page_size).map_err(|_| denied())?;
        let batch_size = task
            .get("batch_size")
            .filter(|value| !value.is_null())
            .map(|value| value.as_i64().ok_or_else(denied))
            .transpose()?
            .unwrap_or(1);
        let concurrency = task
            .get("concurrency")
            .filter(|value| !value.is_null())
            .map(|value| value.as_i64().ok_or_else(denied))
            .transpose()?
            .unwrap_or(1);
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(admission.effect_fence());
        let (selectors, parents) = cache_plan_selectors(
            conn, record.id, renderer, task_path, &config, controls, &mut fence,
        )
        .await?;
        let root = super::execution::ExecutionRepository::find_by_id(&mut *conn, renderer)
            .await?
            .ok_or_else(denied)?;
        let snapshot = root.executable_snapshot.as_ref().ok_or_else(denied)?;
        let scope = cache_plan_owner(
            conn,
            &selectors,
            snapshot,
            record.authority.owner.sponsor_identity_id,
        )
        .await?;
        let namespace_name = cache_selector_text(selectors.get("namespace").ok_or_else(denied)?);
        let namespace =
            super::cache::CacheNamespaceRepository::resolve(&mut *conn, &scope, &namespace_name)
                .await?
                .ok_or_else(denied)?;
        if namespace.mode != crate::models::CacheMode::Snapshot
            || !ExecutionAdmissionRepository::authorize_workflow_cache_read(
                conn,
                admission,
                namespace.id,
            )
            .await?
        {
            return Err(denied());
        }
        let selector = cache_selector_text(selectors.get("generation").ok_or_else(denied)?);
        let active_generation = selector == "active";
        let generation = if active_generation {
            namespace.active_generation.ok_or_else(denied)?
        } else {
            selector.parse::<Id>().map_err(|_| denied())?
        };
        let generation_row =
            super::cache::CacheGenerationRepository::find_by_id_for_share(conn, generation)
                .await?
                .ok_or_else(denied)?;
        if generation_row.namespace != namespace.id
            || !(generation_row.state == crate::models::CacheGenerationState::Active
                || generation_row.state == crate::models::CacheGenerationState::Retired
                    && generation_row
                        .readable_until
                        .is_some_and(|until| until > chrono::Utc::now()))
        {
            return Err(denied());
        }
        let fresh_until = cache_freshness_deadline(
            &generation_row,
            namespace.freshness_target_seconds,
            config.require_fresh,
        )?;
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::CacheNamespace(namespace.id),
            admission.authority().owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::selected_effect_fence(conn, admission.authority()).await?,
        );
        let mut source = CacheSource {
            iteration: None,
            frontier: Some(CacheFrontierPlan {
                prepared_transaction: transaction(conn).await?,
                predecessor: None,
                initial: true,
                concurrency: i32::try_from(concurrency).map_err(|_| denied())?,
                scanned_before: 0,
                active_generation,
                fresh_until,
            }),
            workflow,
            task_name,
            namespace: namespace.id,
            generation,
            target: bound_target(namespace.id, &resolved)?,
            read_owner: super::runtime_read::RuntimeReadOwner::of(&resolved),
            before: None,
            after: None,
            ordinal: 0,
            page_size,
            batch_size: i32::try_from(batch_size).map_err(|_| denied())?,
            entries: vec![],
            exhausted: false,
            parents,
        };
        let value = scan_cache_frontier(conn, &mut source, max_bytes, &fence).await?;
        let body = Body {
            renderer: record.id,
            execution: renderer,
            task_path: task_path.into(),
            authority: admission.authority().clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::Cache(source.clone()),
        };
        persist_cache_frontier(conn, body, source, value, fence, key).await
    }
    /// Source phase only. Cursor/index/settings derive solely from the opaque
    /// predecessor; no supplied page, cursor, ordinal or second selector pass.
    pub async fn prepare_cache_next(
        conn: &mut PgConnection,
        previous: &PreparedCacheInput,
        max_bytes: usize,
        key: &str,
    ) -> Result<PreparedCacheInput> {
        previous.ensure_fresh_on(conn).await?;
        if previous.source.entries.is_empty() {
            return Err(denied());
        }
        let (mut body, _, _, _) = load_encrypted(conn, previous.snapshot).await?;
        let Source::Cache(saved) = &body.source else {
            return Err(denied());
        };
        if serde_json::to_value(saved)? != serde_json::to_value(&previous.source)?
            || body.renderer != previous.renderer
            || body.task_path != previous.task_path
        {
            return Err(denied());
        }
        let key_hash: String = sqlx::query_scalar(
            "SELECT encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1",
        )
        .bind(previous.snapshot)
        .fetch_one(&mut *conn)
        .await?;
        if key_hash != crate::crypto::hash_encryption_key(key) {
            return Err(denied());
        }
        let prior = previous.source.frontier.as_ref().ok_or_else(denied)?;
        let mut source = previous.source.clone();
        source.before = source.after.clone();
        source.ordinal = source.ordinal.checked_add(1).ok_or_else(denied)?;
        source.entries.clear();
        source.frontier = Some(CacheFrontierPlan {
            prepared_transaction: previous.transaction.clone(),
            predecessor: Some(previous.snapshot),
            initial: false,
            concurrency: prior.concurrency,
            scanned_before: prior
                .scanned_before
                .checked_add(i64::try_from(previous.source.entries.len()).map_err(|_| denied())?)
                .ok_or_else(denied)?,
            active_generation: false,
            fresh_until: None,
        });
        let value = scan_cache_frontier(conn, &mut source, max_bytes, &previous.fence).await?;
        body.approval_deadline = previous.fence.approval_deadline();
        body.source = Source::Cache(source.clone());
        let mut prepared =
            persist_cache_frontier(conn, body, source, value, previous.fence.clone(), key).await?;
        let (_, _, snapshot_bytes, _) = load_encrypted(conn, prepared.snapshot).await?;
        if previous
            .selector_bytes
            .checked_add(snapshot_bytes)
            .is_none_or(|bytes| bytes > MAX_BYTES)
        {
            return Err(denied());
        }
        prepared.selector_publications = previous.selector_publications.clone();
        prepared.selector_bytes = previous.selector_bytes;
        prepared.fresh_until = previous.fresh_until;
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }
    /// Target phase only: compare already-held source facts against the actual
    /// locked row, consume each frontier once, then permit publication/progress.
    pub async fn bind_cache_frontier(
        conn: &mut PgConnection,
        prepared: &PreparedCacheInput,
        iteration: Id,
    ) -> Result<()> {
        prepared.ensure_fresh_on(conn).await?;
        for selector in &prepared.selector_publications {
            selector.require_ready(conn).await?;
        }
        let source = &prepared.source;
        let plan = source.frontier.as_ref().ok_or_else(denied)?;
        if plan.prepared_transaction != prepared.transaction
            || plan
                .fresh_until
                .is_some_and(|until| until <= chrono::Utc::now())
        {
            return Err(denied());
        }
        let row = WorkflowCacheIterationRepository::find_by_id_for_update(&mut *conn, iteration)
            .await?
            .ok_or_else(denied)?;
        // The row lookup may have waited past the original generation deadline.
        // Deny before writing any frontier or selector publication association.
        prepared.ensure_fresh_on(conn).await?;
        if source.iteration.is_some_and(|id| id != iteration)
            || row.workflow_execution != source.workflow
            || row.task_name != source.task_name
            || row.namespace != source.namespace
            || row.generation != source.generation
            || row.page_size != source.page_size
            || row.batch_size != source.batch_size
            || row.concurrency != plan.concurrency
            || row.last_external_id != source.before
            || row.next_batch_index != source.ordinal
            || row.scanned_count != plan.scanned_before
            || row.state != crate::models::WorkflowCacheIterationState::Scanning
        {
            return Err(denied());
        }
        if plan.initial {
            let created:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_cache_iteration_epoch WHERE iteration=$1 AND creation_transaction=pg_current_xact_id()::TEXT)").bind(iteration).fetch_one(&mut *conn).await?;
            if !created {
                return Err(denied());
            }
            if plan.active_generation {
                let current: Option<Id> =
                    sqlx::query_scalar("SELECT active_generation FROM cache_namespace WHERE id=$1")
                        .bind(source.namespace)
                        .fetch_one(&mut *conn)
                        .await?;
                if current != Some(source.generation) {
                    return Err(denied());
                }
            }
        }
        if let Some(predecessor) = plan.predecessor {
            let prior:Option<Id>=sqlx::query_scalar("SELECT bound_iteration FROM runtime_native_cache_frontier WHERE snapshot=$1 AND bound_transaction=pg_current_xact_id()::TEXT").bind(predecessor).fetch_optional(&mut *conn).await?.flatten();
            if prior != Some(iteration) {
                return Err(denied());
            }
        }
        let consumed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE bound_iteration=$1 AND ordinal=$2 AND bound_transaction=pg_current_xact_id()::TEXT)").bind(iteration).bind(source.ordinal).fetch_one(&mut *conn).await?;
        if consumed {
            return Err(denied());
        }
        let changed=sqlx::query("UPDATE runtime_native_cache_frontier SET bound_iteration=$2,bound_transaction=pg_current_xact_id()::TEXT WHERE snapshot=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND bound_iteration IS NULL").bind(prepared.snapshot).bind(iteration).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        for selector in &prepared.selector_publications {
            selector.publish(conn, prepared.snapshot).await?;
        }
        prepared.ensure_fresh_on(conn).await
    }
    /// Source preparation only. Target identity comes from the actual owning
    /// wait row. A dynamic wait selector additionally needs its exact opaque
    /// evaluated Control receipt, never a replacement ID/JSON assertion.
    pub async fn prepare_wait(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        wait: Id,
        target_control: Option<&ReceiptContext>,
        key: &str,
    ) -> Result<PreparedWaitInput> {
        Self::prepare_wait_inner(
            conn,
            renderer,
            task_path,
            admission,
            wait,
            target_control,
            key,
            None,
        )
        .await
    }

    async fn prepare_wait_inner(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        wait: Id,
        target_control: Option<&ReceiptContext>,
        key: &str,
        prospective: Option<&PreparedWaitRegistration>,
    ) -> Result<PreparedWaitInput> {
        use super::{workflow_task_wait::WorkflowTaskWaitRepository as Waits, FindById};
        use crate::models::{WorkflowTaskWaitState, WorkflowTaskWaitTarget};
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        require_task(conn, &record, task_path, admission.authority()).await?;
        let (workflow, task_name, target) = if let Some(initial) = prospective {
            initial
                .identity(conn, renderer, task_path, admission, wait)
                .await?
        } else {
            let (workflow, task_name): (Id, String) = sqlx::query_as(
                "SELECT workflow_execution,task_name FROM workflow_task_wait WHERE id=$1",
            )
            .bind(wait)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(denied)?;
            let row = Waits::find_by_workflow_task(&mut *conn, workflow, &task_name)
                .await?
                .ok_or_else(denied)?;
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE id=$1 AND execution=$2)",
            )
            .bind(workflow)
            .bind(renderer)
            .fetch_one(&mut *conn)
            .await?;
            if !belongs || row.id != wait || row.state != WorkflowTaskWaitState::Waiting {
                return Err(denied());
            }
            (workflow, task_name, row.target()?)
        };
        let task = task_definition(conn, &record, task_path).await?;
        if task.get("name").and_then(Value::as_str) != Some(task_name.as_str()) {
            return Err(denied());
        }
        let (kind, id, runtime_target, source_kind, source_field) = match target {
            WorkflowTaskWaitTarget::Inquiry(id) => (
                "inquiry",
                id,
                RuntimeTarget::Inquiry(id),
                "inquiry_response",
                "response",
            ),
            WorkflowTaskWaitTarget::Execution(id) => (
                "execution",
                id,
                RuntimeTarget::Execution(id),
                "execution_result",
                "result",
            ),
            WorkflowTaskWaitTarget::WorkQueueItem(id) => (
                "work_queue_item",
                id,
                RuntimeTarget::QueueItem(id),
                "queue_item_result",
                "result",
            ),
        };
        let wait_for: crate::workflow::TaskWaitFor =
            serde_json::from_value(task.get("wait_for").cloned().ok_or_else(denied)?)?;
        let (authored_kind, selector) = wait_for.target();
        if authored_kind != kind {
            return Err(denied());
        }
        let mut parents = Vec::new();
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(admission.effect_fence());
        if selector.as_i64() != Some(id) {
            if !selector
                .as_str()
                .is_some_and(|selector| selector.contains("{{"))
            {
                return Err(denied());
            }
            let control = target_control.ok_or_else(denied)?;
            control
                .require_declared_control(conn, record.id, &format!("{task_path}/wait_for/{kind}"))
                .await?;
            control.ensure_fresh_on(conn).await?;
            if control.site().execution != renderer
                || control.site().purpose != InputSitePurpose::Control
                || control.site().path != format!("{task_path}/wait_for/{kind}")
                || control.accepted_value() != &json!(id)
            {
                return Err(denied());
            }
            parents.push(
                InheritedInputRepository::receipt_source_site(conn, &control.origin()).await?,
            );
            fence = fence.intersect(control.accepted_fence());
        } else if target_control.is_some() {
            return Err(denied());
        }
        // Authorize the actual read before source row/ciphertext locks. Selected
        // task capabilities do not become SDK tokens or contributor rights.
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            runtime_target,
            admission.authority().owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        require_selected_read(
            conn,
            admission.authority(),
            &resolved,
            crate::rbac::Action::Read,
        )
        .await?;
        let target_binding = bound_target(id, &resolved)?;
        let (raw, state, contract, source_execution, source_version) = match target {
            WorkflowTaskWaitTarget::Inquiry(id) => {
                sqlx::query("SELECT id FROM inquiry WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source = super::inquiry::InquiryRepository::find_by_id(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                if source.workflow_execution != Some(workflow) {
                    return Err(denied());
                }
                let stored = super::inquiry_schema::InquirySchemaRepository::find(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                let state = match source.status {
                    crate::models::InquiryStatus::Responded => "released",
                    crate::models::InquiryStatus::Timeout => "timed_out",
                    crate::models::InquiryStatus::Cancelled => "failed",
                    _ => return Err(denied()),
                };
                let raw = if state == "released" {
                    json!({"id":source.id,"status":source.status,"response":source.response,"assigned_to":source.assigned_to,"responded_by":source.responded_by,"responded_at":source.responded_at,"timeout_at":source.timeout_at})
                } else {
                    json!({"code":if state=="timed_out" {"inquiry_timeout"}else {"inquiry_cancelled"},"inquiry_id":id,"status":source.status})
                };
                (
                    raw,
                    state.to_owned(),
                    Some(serde_json::to_value(&stored.binding.contract)?),
                    source.created_by_execution,
                    json!(source.updated),
                )
            }
            WorkflowTaskWaitTarget::Execution(id) => {
                sqlx::query("SELECT id FROM execution WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source = super::execution::ExecutionRepository::find_by_id(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                if !super::execution::ExecutionRepository::is_in_execution_tree(
                    &mut *conn, renderer, id, false,
                )
                .await?
                {
                    return Err(denied());
                }
                let state = match source.status {
                    crate::models::ExecutionStatus::Completed => "released",
                    crate::models::ExecutionStatus::Timeout => "timed_out",
                    crate::models::ExecutionStatus::Failed
                    | crate::models::ExecutionStatus::Cancelled
                    | crate::models::ExecutionStatus::Abandoned => "failed",
                    _ => return Err(denied()),
                };
                let contract = super::schema_contract::SchemaContractRepository::find(
                    conn,
                    super::schema_contract::BindingOwner::Execution,
                    id,
                )
                .await?
                .and_then(|binding| binding.output)
                .map(|output| serde_json::to_value(&output))
                .transpose()?;
                (
                    json!({"id":source.id,"status":source.status,"result":source.result}),
                    state.to_owned(),
                    contract,
                    id,
                    json!(source.updated),
                )
            }
            WorkflowTaskWaitTarget::WorkQueueItem(id) => {
                sqlx::query("SELECT id FROM work_queue_item WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source = super::work_queue::WorkQueueItemRepository::find_by_id(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                let requester = source.requested_by_execution.ok_or_else(denied)?;
                if !super::execution::ExecutionRepository::is_in_execution_tree(
                    &mut *conn, renderer, requester, true,
                )
                .await?
                {
                    return Err(denied());
                }
                let state = match source.status {
                    crate::models::WorkQueueItemStatus::Completed => "released",
                    crate::models::WorkQueueItemStatus::Failed
                    | crate::models::WorkQueueItemStatus::Cancelled
                    | crate::models::WorkQueueItemStatus::Skipped => "failed",
                    _ => return Err(denied()),
                };
                let result = if matches!(
                    source.status,
                    crate::models::WorkQueueItemStatus::Completed
                        | crate::models::WorkQueueItemStatus::Skipped
                ) {
                    source.ack_summary
                } else {
                    source.last_error
                };
                (
                    json!({"id":source.id,"queue_ref":source.queue_ref,"status":source.status,"result":result}),
                    state.to_owned(),
                    None,
                    requester,
                    json!(source.updated),
                )
            }
        };
        let fields = if raw.get(source_field).is_some() {
            load_fields(conn, source_kind, id).await?
        } else {
            vec![]
        };
        let source_version = json!({"updated":source_version,"projection_digest":hash(&raw)?,"ciphertext_digest":hash(&serde_json::to_value(&fields)?)?,"contract_digest":contract.as_ref().map(hash).transpose()?});
        let source_record = RuntimeAuthorityRepository::execution_admission(conn, source_execution)
            .await?
            .ok_or_else(denied)?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, source_record.id).await?,
        );
        if !fields.is_empty() {
            require_selected_read(
                conn,
                admission.authority(),
                &resolved,
                crate::rbac::Action::Decrypt,
            )
            .await?;
        }
        let mut contributors = Vec::new();
        let mut value = raw.clone();
        if let Some(raw_field) = raw.get(source_field) {
            let restored = crate::secret_values::restore_secret_values(
                raw_field.clone(),
                &fields.iter().map(ArchivedField::stored).collect::<Vec<_>>(),
                key,
            )?;
            value[source_field] = restored.clone();
            let secret_paths = if let Some(contract) = &contract {
                let contract: crate::value_schema::Contract =
                    serde_json::from_value(contract.clone())?;
                let compiled =
                    crate::value_schema::CompiledContract::new(&contract).map_err(|_| denied())?;
                compiled.validate(&restored).map_err(|_| denied())?;
                compiled.secret_paths(&restored).map_err(|_| denied())?
            } else {
                vec![]
            };
            let mut ordinary = Vec::new();
            for field in &fields {
                let mut value_source = false;
                let provenance = field.provenance()?;
                if provenance.origins.is_empty() {
                    return Err(denied());
                }
                for origin in provenance.origins {
                    if super::immutable_input_origin::is_input_receipt(&origin) {
                        let parent =
                            InheritedInputRepository::receipt_source_site(conn, &origin).await?;
                        let context = InheritedInputRepository::receipt_context(
                            conn, &parent.1, &origin, key,
                        )
                        .await?;
                        if parent.1.purpose == InputSitePurpose::Value {
                            if Some(context.accepted_value()) != restored.pointer(&field.json_path)
                            {
                                return Err(denied());
                            }
                            value_source = true;
                        }
                        if !parents.contains(&parent) {
                            parents.push(parent);
                        }
                        fence = fence.intersect(context.accepted_fence());
                    } else if matches!(&origin,SecretOrigin::Local {source_kind,source_ref} if source_kind=="parameter_schema"&&source_ref.as_deref()==Some(field.json_path.as_str())&&secret_paths.iter().any(|path|path==&field.json_path))
                    {
                        value_source = true;
                    } else {
                        if matches!(origin, SecretOrigin::Local { .. }) {
                            return Err(denied());
                        }
                        value_source = true;
                        ordinary.push(origin);
                    }
                }
                if !value_source {
                    return Err(denied());
                }
            }
            if !ordinary.is_empty() {
                let (authorities, manifests) =
                    super::accepted_input::AcceptedInputRepository::source_contributors(
                        conn,
                        source_execution,
                    )
                    .await?;
                super::immutable_input_origin::ImmutableInputOriginRepository::require(
                    conn,
                    &authorities,
                    &ordinary,
                )
                .await?;
                contributors = manifests;
            }
        }
        bounded(&raw)?;
        bounded(&value)?;
        let context_path = format!(
            "/{}/{}",
            kind,
            task_name.replace('~', "~0").replace('/', "~1")
        );
        let mut origins = vec![SecretOrigin::Entity {
            entity_type: source_kind.into(),
            entity_id: id,
            path: String::new(),
        }];
        for field in &fields {
            for origin in field.provenance()?.origins {
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
        }
        if parents.len() + origins.len() + fields.len() > 256 {
            return Err(denied());
        }
        let source = WaitSource {
            wait,
            workflow,
            task_name,
            target: target_binding,
            read_owner: super::runtime_read::RuntimeReadOwner::of(&resolved),
            prepared_transaction: transaction(conn).await?,
            target_kind: kind.into(),
            target_id: id,
            state,
            context_path,
            source_kind: source_kind.into(),
            source_id: id,
            source_field: source_field.into(),
            source_admission: source_record.id,
            contributors,
            raw_projection: raw,
            source_version,
            contract,
            fields,
            parents,
            origins,
        };
        let body = Body {
            renderer: record.id,
            execution: renderer,
            task_path: task_path.into(),
            authority: admission.authority().clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::Wait(source.clone()),
        };
        let snapshot = save(conn, &body, &value, key).await?;
        for (parent, _) in &source.parents {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(parent).execute(&mut *conn).await?;
        }
        sqlx::query(
            "INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)",
        )
        .bind(snapshot)
        .bind(source.source_admission)
        .execute(&mut *conn)
        .await?;
        archive_wait(conn, snapshot, &source.fields).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedWaitInput {
            read_dependencies: target_control
                .map(|control| control.issued_wait_reads().to_vec())
                .unwrap_or_default(),
            transaction: transaction(conn).await?,
            renderer: record.id,
            snapshot,
            task_path: task_path.into(),
            value,
            fence,
            source,
        })
    }

    /// Target phase. It consumes only already-held proof/deadlines and writes
    /// the exact prepared redacted response in the actual release transaction.
    pub async fn release_wait(
        conn: &mut PgConnection,
        prepared: PreparedWaitInput,
    ) -> Result<ReleasedWaitInput> {
        use super::workflow_task_wait::WorkflowTaskWaitRepository as Waits;
        require_transaction(conn, &prepared.transaction).await?;
        let row = Waits::find_by_workflow_task_for_update(
            conn,
            prepared.source.workflow,
            &prepared.source.task_name,
        )
        .await?
        .ok_or_else(denied)?;
        if row.id != prepared.source.wait
            || row.state != crate::models::WorkflowTaskWaitState::Waiting
            || row.target()?.id() != prepared.source.target_id
            || serde_json::to_value(row.target()?.kind())?.as_str()
                != Some(prepared.source.target_kind.as_str())
        {
            return Err(denied());
        }
        prepared.fence.ensure_fresh_on(conn).await?;
        let state = wait_state(&prepared.source.state)?;
        Waits::transition_waiting(
            &mut *conn,
            row.id,
            state,
            Some(prepared.source.raw_projection.clone()),
        )
        .await?
        .ok_or_else(denied)?;
        sqlx::query("INSERT INTO runtime_native_wait_release(wait,snapshot,release_transaction) VALUES($1,$2,pg_current_xact_id()::TEXT)").bind(row.id).bind(prepared.snapshot).execute(&mut *conn).await?;
        prepared.fence.ensure_fresh_on(conn).await?;
        Ok(ReleasedWaitInput { prepared })
    }

    pub async fn prepare_released_wait(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        wait: Id,
        key: &str,
    ) -> Result<ReleasedWaitInput> {
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let snapshot: Id =
            sqlx::query_scalar("SELECT snapshot FROM runtime_native_wait_release WHERE wait=$1")
                .bind(wait)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(denied)?;
        let (body, value, _, _) = load(conn, snapshot, key).await?;
        let Source::Wait(source) = body.source else {
            return Err(denied());
        };
        if body.renderer != record.id
            || body.execution != renderer
            || body.task_path != task_path
            || source.wait != wait
        {
            return Err(denied());
        }
        require_task(conn, &record, task_path, &body.authority).await?;
        validate_wait_release(conn, snapshot, &source).await?;
        validate_wait_value(conn, snapshot, &source, &value, key).await?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::selected_effect_fence(conn, &body.authority).await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?);
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, source.source_admission)
                .await?,
        );
        for (parent, site) in &source.parents {
            let origin = SecretOrigin::Entity {
                entity_type: format!(
                    "declared_input:{}:{}",
                    site.kind.as_str(),
                    match site.purpose {
                        InputSitePurpose::Value => "value",
                        InputSitePurpose::Condition => "condition",
                        InputSitePurpose::Control => "control",
                    }
                ),
                entity_id: *parent,
                path: site.path.clone(),
            };
            let context =
                InheritedInputRepository::receipt_context(conn, site, &origin, key).await?;
            fence = fence.intersect(context.accepted_fence());
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(ReleasedWaitInput {
            prepared: PreparedWaitInput {
                read_dependencies: Vec::new(),
                transaction: transaction(conn).await?,
                renderer: record.id,
                snapshot,
                task_path: task_path.into(),
                value,
                fence,
                source,
            },
        })
    }
    /// Source entry, before iteration/workflow target locks. The caller supplies
    /// only an opaque selected task authority and the actual iteration ID.
    pub async fn prepare_cache(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        iteration: Id,
        max_bytes: usize,
        key: &str,
    ) -> Result<PreparedCacheInput> {
        Self::prepare_cache_with_controls(
            conn,
            renderer,
            task_path,
            admission,
            iteration,
            max_bytes,
            &[],
            key,
        )
        .await
    }
    pub async fn prepare_cache_with_controls(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        iteration: Id,
        max_bytes: usize,
        controls: &[&ReceiptContext],
        key: &str,
    ) -> Result<PreparedCacheInput> {
        if max_bytes == 0 || max_bytes > MAX_BYTES {
            return Err(denied());
        }
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let authority = admission.authority();
        require_task(conn, &record, task_path, authority).await?;
        let (workflow, task_name): (Id, String) = sqlx::query_as(
            "SELECT workflow_execution,task_name FROM workflow_cache_iteration WHERE id=$1",
        )
        .bind(iteration)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        let row = WorkflowCacheIterationRepository::find_by_workflow_task(
            &mut *conn, workflow, &task_name,
        )
        .await?
        .ok_or_else(denied)?;
        let belongs: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE id=$1 AND execution=$2)",
        )
        .bind(workflow)
        .bind(renderer)
        .fetch_one(&mut *conn)
        .await?;
        let task = task_definition(conn, &record, task_path).await?;
        if !belongs
            || row.id != iteration
            || row.state != crate::models::WorkflowCacheIterationState::Scanning
            || task.get("name").and_then(Value::as_str) != Some(task_name.as_str())
            || task.get("iterate_cache").is_none()
            || row.batch_size <= 0
            || row.page_size <= 0
            || row.next_batch_index < 0
        {
            return Err(denied());
        }
        let batch = match task.get("batch_size") {
            None | Some(Value::Null) => 1,
            Some(size) => size.as_i64().filter(|size| *size > 0).ok_or_else(denied)?,
        };
        let config: crate::workflow::parser::IterateCacheConfig =
            serde_json::from_value(task.get("iterate_cache").cloned().ok_or_else(denied)?)?;
        let page = i64::try_from(config.page_size).map_err(|_| denied())?;
        let concurrency = task
            .get("concurrency")
            .filter(|value| !value.is_null())
            .map(|value| value.as_i64().ok_or_else(denied))
            .transpose()?
            .unwrap_or(1);
        if i64::from(row.batch_size) != batch
            || i64::from(row.page_size) != page
            || i64::from(row.concurrency) != concurrency
        {
            return Err(denied());
        }
        if !ExecutionAdmissionRepository::authorize_workflow_cache_read(
            conn,
            admission,
            row.namespace,
        )
        .await?
        {
            return Err(denied());
        }
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::CacheNamespace(row.namespace),
            authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let target = bound_target(row.namespace, &resolved)?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(admission.effect_fence());
        use super::FindById;
        let namespace =
            super::cache::CacheNamespaceRepository::find_by_id(&mut *conn, row.namespace)
                .await?
                .ok_or_else(denied)?;
        let selectors = [
            (
                "namespace",
                Some(config.namespace.as_str()),
                json!(namespace.namespace),
            ),
            (
                "generation",
                Some(config.generation.as_str()),
                json!(row.generation),
            ),
            (
                "owner_type",
                config.owner_type.as_deref(),
                serde_json::to_value(resolved.context().owner_type)?,
            ),
            (
                "owner_ref",
                config.owner_ref.as_deref(),
                json!(resolved.context().owner_ref),
            ),
        ];
        let mut parents = Vec::new();
        let mut consumed = std::collections::BTreeSet::new();
        if controls.len() > 4 {
            return Err(denied());
        }
        for (field, selector, actual) in selectors {
            let Some(selector) = selector else {
                continue;
            };
            let dynamic = selector.contains("{{");
            let value = if dynamic {
                let path = format!("{task_path}/iterate_cache/{field}");
                let mut matching = controls
                    .iter()
                    .enumerate()
                    .filter(|(_, control)| control.site().path == path);
                let (index, control) = matching.next().ok_or_else(denied)?;
                if matching.next().is_some() {
                    return Err(denied());
                }
                control
                    .require_declared_control(conn, record.id, &path)
                    .await?;
                if control.site().execution != renderer {
                    return Err(denied());
                }
                consumed.insert(index);
                parents.push(
                    InheritedInputRepository::receipt_source_site(conn, &control.origin()).await?,
                );
                fence = fence.intersect(control.accepted_fence());
                control.accepted_value().clone()
            } else {
                json!(selector)
            };
            let matches = if field == "generation" {
                value.as_str() == Some("active")
                    || value.as_i64() == Some(row.generation)
                    || value.as_str().and_then(|value| value.parse::<Id>().ok())
                        == Some(row.generation)
            } else if field == "owner_type" {
                value.as_str().map(str::to_ascii_lowercase) == actual.as_str().map(str::to_owned)
            } else {
                value == actual
            };
            if !matches {
                return Err(denied());
            }
        }
        if consumed.len() != controls.len() {
            return Err(denied());
        }
        // Record the original source acquisition deadline for persisted-iteration
        // pages too. Reader validation never substitutes current namespace policy.
        let generation =
            super::cache::CacheGenerationRepository::find_by_id_for_share(conn, row.generation)
                .await?
                .ok_or_else(denied)?;
        if generation.namespace != row.namespace {
            return Err(denied());
        }
        let fresh_until = cache_freshness_deadline(
            &generation,
            namespace.freshness_target_seconds,
            config.require_fresh,
        )?;
        let mut source = CacheSource {
            iteration: Some(iteration),
            frontier: Some(CacheFrontierPlan {
                prepared_transaction: transaction(conn).await?,
                predecessor: None,
                initial: false,
                concurrency: row.concurrency,
                scanned_before: row.scanned_count,
                active_generation: false,
                fresh_until,
            }),
            workflow,
            task_name,
            namespace: row.namespace,
            generation: row.generation,
            target,
            read_owner: super::runtime_read::RuntimeReadOwner::of(&resolved),
            before: row.last_external_id.clone(),
            after: row.last_external_id.clone(),
            ordinal: row.next_batch_index,
            page_size: row.page_size,
            batch_size: row.batch_size,
            entries: vec![],
            exhausted: false,
            parents,
        };
        let value = scan_cache_frontier(conn, &mut source, max_bytes, &fence).await?;
        let body = Body {
            renderer: record.id,
            execution: renderer,
            task_path: task_path.into(),
            authority: authority.clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::Cache(source.clone()),
        };
        let snapshot = save(conn, &body, &value, key).await?;
        for (parent, _) in &source.parents {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(parent).execute(&mut *conn).await?;
        }
        save_cache_frontier(conn, snapshot, &source).await?;
        fence = fence
            .intersect(&RuntimeAuthorityRepository::selected_effect_fence(conn, authority).await?);
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedCacheInput {
            fresh_until: source.frontier.as_ref().and_then(|plan| plan.fresh_until),
            selector_publications: vec![],
            selector_bytes: 0,
            transaction: transaction(conn).await?,
            renderer: record.id,
            snapshot,
            task_path: task_path.into(),
            value,
            fence,
            source,
        })
    }
    pub(crate) async fn evidence(
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        declaration_path: &str,
        context_path: &str,
        key: &str,
        allow_pending: bool,
    ) -> Result<SnapshotEvidence> {
        let (body, value, bytes, _) = load(conn, snapshot, key).await?;
        if body.renderer != renderer
            || (!matches!(
                &body.source,
                Source::ResultView(_)
                    | Source::SuccessMetadata(_)
                    | Source::Completion(_)
                    | Source::IterationOutcome(_)
            ) && !in_task(&body.task_path, declaration_path))
        {
            return Err(denied());
        }
        let record = RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
        if matches!(
            &body.source,
            Source::ResultView(_)
                | Source::SuccessMetadata(_)
                | Source::Completion(_)
                | Source::IterationOutcome(_)
        ) {
            if body.authority != record.authority.0 || body.execution != record.owner_id {
                return Err(denied());
            }
        } else {
            require_task(conn, &record, &body.task_path, &body.authority).await?;
        }
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, renderer)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::selected_effect_fence(conn, &body.authority).await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?);
        let (origins, parents) = match &body.source {
            Source::SuccessMetadata(source) => {
                if declaration_path != "$native/value" || context_path != "$native/value" {
                    return Err(denied());
                }
                success_metadata::require_release(conn, snapshot, source).await?;
                success_metadata::validate_value(conn, snapshot, source, &value, key).await?;
                for admission in success_metadata::admissions(source, body.renderer) {
                    fence = fence.intersect(
                        &RuntimeAuthorityRepository::retained_admission_fence(conn, admission)
                            .await?,
                    );
                }
                (success_metadata::origins(snapshot), source.parents.clone())
            }
            Source::IterationOutcome(source) => {
                iteration_outcome::selection_scope(source, declaration_path, context_path)?;
                iteration_outcome::require_release(conn, snapshot, source).await?;
                if value != iteration_outcome::values(source)? {
                    return Err(denied());
                }
                for admission in iteration_outcome::admissions(source) {
                    fence = fence.intersect(
                        &RuntimeAuthorityRepository::retained_admission_fence(conn, admission)
                            .await?,
                    );
                }
                (
                    iteration_outcome::origins(snapshot),
                    iteration_outcome::parents(source).to_vec(),
                )
            }
            Source::Completion(source) => {
                if context_path != "$native/value" || declaration_path != "$native/value" {
                    return Err(denied());
                }
                if !allow_pending || source.transaction != transaction(conn).await? {
                    completion::require_release(conn, snapshot, source).await?;
                }
                completion::validate_value(conn, snapshot, source, &value, key).await?;
                for admission in completion::admissions(source) {
                    fence = fence.intersect(
                        &RuntimeAuthorityRepository::retained_admission_fence(conn, admission)
                            .await?,
                    );
                }
                (source.origins.clone(), source.parents.clone())
            }
            Source::ResultView(source) => {
                if !result_view::context_path(source, context_path) {
                    return Err(denied());
                }
                fence = fence.intersect(
                    &RuntimeAuthorityRepository::retained_admission_fence(conn, source.admission)
                        .await?,
                );
                if !allow_pending || source.transaction != transaction(conn).await? {
                    result_view::require_release(conn, snapshot, source).await?;
                }
                result_view::validate_value(conn, snapshot, source, &value, key).await?;
                (source.origins.clone(), source.parents.clone())
            }
            Source::Cache(source) => {
                if !(context_path == "/index"
                    || context_path == "/item"
                    || context_path.starts_with("/item/"))
                {
                    return Err(denied());
                }
                cache_binding(conn, snapshot, source, !allow_pending).await?;
                validate_cache(source, &value)?;
                (vec![cache_origin(source)], source.parents.clone())
            }
            Source::Wait(source) => {
                fence = fence.intersect(
                    &RuntimeAuthorityRepository::retained_admission_fence(
                        conn,
                        source.source_admission,
                    )
                    .await?,
                );
                let outcome = ["succeeded", "failed", "timed_out", "cancelled"]
                    .iter()
                    .any(|function| context_path == format!("/$outcome/{function}"));
                if !outcome
                    && context_path != source.context_path
                    && !context_path
                        .strip_prefix(&source.context_path)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                {
                    return Err(denied());
                }
                if outcome {
                    if !wait_outcome_path(&body.task_path, declaration_path) {
                        return Err(denied());
                    }
                    native_wait_outcome_values(source)?;
                }
                if allow_pending && source.prepared_transaction == transaction(conn).await? {
                    let finalized:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1 AND wait=$2)").bind(snapshot).bind(source.wait).fetch_one(&mut *conn).await?;
                    if finalized {
                        validate_wait_release(conn, snapshot, source).await?;
                    } else {
                        let wait=super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(&mut *conn,source.workflow,&source.task_name).await?.ok_or_else(denied)?;
                        if wait.id != source.wait
                            || wait.state != crate::models::WorkflowTaskWaitState::Waiting
                            || wait.target()?.id() != source.target_id
                            || serde_json::to_value(wait.target()?.kind())?.as_str()
                                != Some(source.target_kind.as_str())
                        {
                            return Err(denied());
                        }
                    }
                } else {
                    validate_wait_release(conn, snapshot, source).await?;
                }
                validate_wait_value(conn, snapshot, source, &value, key).await?;
                (source.origins.clone(), source.parents.clone())
            }
        };
        Ok(SnapshotEvidence {
            fence,
            origins,
            parents,
            metadata: metadata(snapshot, &body),
            bytes,
        })
    }
    pub async fn source_evidence(
        conn: &mut PgConnection,
        metadata: &NativeSnapshotMetadata,
        key: &str,
    ) -> Result<NativeSourceEvidence> {
        let (body, value, bytes, fresh_until) = load(conn, metadata.snapshot, key).await?;
        if &self::metadata(metadata.snapshot, &body) != metadata {
            return Err(denied());
        }
        match &body.source {
            Source::SuccessMetadata(source) => {
                success_metadata::require_release(conn, metadata.snapshot, source).await?;
                success_metadata::validate_value(conn, metadata.snapshot, source, &value, key)
                    .await?;
                ApprovalEffectFence::from_input_approval(body.approval_deadline)?
                    .ensure_fresh_on(conn)
                    .await?;
                for admission in success_metadata::admissions(source, body.renderer) {
                    RuntimeAuthorityRepository::retained_admission_fence(conn, admission)
                        .await?
                        .ensure_fresh_on(conn)
                        .await?;
                }
                Ok(NativeSourceEvidence {
                    success_metadata: Some(NativeSuccessMetadataEvidence::new(
                        metadata.snapshot,
                        source,
                    )),
                    iteration_outcome: None,
                    completion_sources: Vec::new(),
                    cache_read_policy: None,
                    contributors: source.root_contributors.clone(),
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: success_metadata::admissions(source, body.renderer),
                    fields: Vec::new(),
                    origins: success_metadata::origins(metadata.snapshot),
                    parents: source.parents.iter().map(|(id, _)| *id).collect(),
                    contract: source.contract.output.clone(),
                    bytes,
                })
            }
            Source::IterationOutcome(source) => {
                iteration_outcome::require_release(conn, metadata.snapshot, source).await?;
                if value != iteration_outcome::values(source)? {
                    return Err(denied());
                }
                for admission in iteration_outcome::admissions(source) {
                    RuntimeAuthorityRepository::retained_admission_fence(conn, admission)
                        .await?
                        .ensure_fresh_on(conn)
                        .await?;
                }
                Ok(NativeSourceEvidence {
                    success_metadata: None,
                    iteration_outcome: Some(iteration_outcome::evidence(source)),
                    completion_sources: Vec::new(),
                    cache_read_policy: None,
                    contributors: Vec::new(),
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: iteration_outcome::admissions(source),
                    fields: Vec::new(),
                    origins: iteration_outcome::origins(metadata.snapshot),
                    parents: iteration_outcome::parents(source)
                        .iter()
                        .map(|(id, _)| *id)
                        .collect(),
                    contract: None,
                    bytes,
                })
            }
            Source::Completion(source) => {
                completion::require_release(conn, metadata.snapshot, source).await?;
                completion::validate_value(conn, metadata.snapshot, source, &value, key).await?;
                Ok(NativeSourceEvidence {
                    success_metadata: None,
                    iteration_outcome: None,
                    completion_sources: source
                        .children
                        .iter()
                        .cloned()
                        .map(NativeCompletionSource::new)
                        .collect(),
                    cache_read_policy: None,
                    contributors: source
                        .children
                        .iter()
                        .flat_map(|child| child.contributors.iter().cloned())
                        .collect(),
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: completion::admissions(source),
                    fields: source.fields.iter().map(ArchivedField::stored).collect(),
                    origins: source.origins.clone(),
                    parents: source.parents.iter().map(|(id, _)| *id).collect(),
                    contract: source.contract.output.clone(),
                    bytes,
                })
            }
            Source::ResultView(source) => {
                result_view::require_release(conn, metadata.snapshot, source).await?;
                result_view::validate_value(conn, metadata.snapshot, source, &value, key).await?;
                RuntimeAuthorityRepository::retained_admission_fence(conn, source.admission)
                    .await?
                    .ensure_fresh_on(conn)
                    .await?;
                Ok(NativeSourceEvidence {
                    success_metadata: None,
                    iteration_outcome: None,
                    completion_sources: Vec::new(),
                    cache_read_policy: None,
                    contributors: source.contributors.clone(),
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: vec![source.admission],
                    fields: source.fields.iter().map(ArchivedField::stored).collect(),
                    origins: source.origins.clone(),
                    parents: source.parents.iter().map(|(id, _)| *id).collect(),
                    contract: source
                        .contract
                        .as_ref()
                        .map(|contract| serde_json::from_value(contract.clone()))
                        .transpose()?,
                    bytes,
                })
            }
            Source::Cache(source) => {
                cache_binding(conn, metadata.snapshot, source, true).await?;
                validate_cache(source, &value)?;
                let cache_read_policy =
                    cache_reader_policy(conn, &body, source, fresh_until).await?;
                Ok(NativeSourceEvidence {
                    success_metadata: None,
                    iteration_outcome: None,
                    completion_sources: Vec::new(),
                    cache_read_policy: Some(cache_read_policy),
                    contributors: vec![],
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: vec![],
                    fields: vec![],
                    origins: vec![cache_origin(source)],
                    parents: source.parents.iter().map(|(id, _)| *id).collect(),
                    contract: None,
                    bytes,
                })
            }
            Source::Wait(source) => {
                validate_wait_release(conn, metadata.snapshot, source).await?;
                validate_wait_value(conn, metadata.snapshot, source, &value, key).await?;
                Ok(NativeSourceEvidence {
                    success_metadata: None,
                    iteration_outcome: None,
                    completion_sources: Vec::new(),
                    cache_read_policy: None,
                    contributors: source.contributors.clone(),
                    renderer: body.renderer,
                    policy: body.authority.policy_id,
                    approvals: body.authority.approval_ids().into_iter().collect(),
                    admissions: vec![source.source_admission],
                    fields: source.fields.iter().map(ArchivedField::stored).collect(),
                    origins: source.origins.clone(),
                    parents: source.parents.iter().map(|(id, _)| *id).collect(),
                    contract: source
                        .contract
                        .as_ref()
                        .map(|contract| serde_json::from_value(contract.clone()))
                        .transpose()?,
                    bytes,
                })
            }
        }
    }
    pub(crate) async fn retention(
        conn: &mut PgConnection,
        snapshot: Id,
    ) -> Result<SnapshotRetention> {
        let storage = Self::storage_evidence(conn, snapshot).await?;
        Ok(SnapshotRetention {
            source_kind: storage.metadata.source_kind.clone(),
            admissions: storage.admissions,
            renderer: storage.renderer,
            policy: storage.policy,
            approvals: storage.approvals,
            origins: storage.origins,
            parents: storage.receipts.iter().map(|(id, _)| *id).collect(),
            bytes: storage.bytes,
        })
    }
    /// Keyless storage inventory, under the caller's existing graph/capture gate.
    /// No source authorization, target lock, decryption or live-owner resolution.
    pub async fn storage_evidence(
        conn: &mut PgConnection,
        snapshot: Id,
    ) -> Result<NativeSnapshotStorageEvidence> {
        let (body, encrypted, bytes, _) = load_encrypted(conn, snapshot).await?;
        bounded(&encrypted)?;
        let budget:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_budget WHERE renderer_admission=$1)").bind(body.renderer).fetch_one(&mut *conn).await?;
        if !budget {
            return Err(denied());
        }
        let key_hash: String = sqlx::query_scalar(
            "SELECT encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1",
        )
        .bind(snapshot)
        .fetch_one(&mut *conn)
        .await?;
        let metadata = metadata(snapshot, &body);
        let frozen_owner = storage_owner(metadata.read_owner())?;
        let mut physical_body = body.clone();
        if let Source::Cache(source) = &mut physical_body.source {
            source.iteration = Some(
                cache_binding(conn, snapshot, source, true)
                    .await?
                    .ok_or_else(denied)?,
            );
        }
        let physical_parents = storage_physical_parents(snapshot, &physical_body, &frozen_owner)?;
        let approvals = body
            .authority
            .approval_ids()
            .into_iter()
            .collect::<Vec<_>>();
        let archived = load_fields(conn, "native_input_snapshot", snapshot).await?;
        let (cache, contributors, contract, receipts, admissions, release, origins) = match &body
            .source
        {
            Source::SuccessMetadata(source) => {
                success_metadata::require_release(conn, snapshot, source).await?;
                success_metadata::validate_normalized(conn, snapshot, source).await?;
                if !archived.is_empty() {
                    return Err(denied());
                }
                (
                    None,
                    source.root_contributors.clone(),
                    source.contract.output.clone(),
                    source.parents.clone(),
                    success_metadata::admissions(source, body.renderer),
                    None,
                    success_metadata::origins(snapshot),
                )
            }
            Source::IterationOutcome(source) => {
                iteration_outcome::require_release(conn, snapshot, source).await?;
                iteration_outcome::validate_normalized(conn, snapshot, source).await?;
                if !archived.is_empty() {
                    return Err(denied());
                }
                (
                    None,
                    Vec::new(),
                    None,
                    iteration_outcome::parents(source).to_vec(),
                    iteration_outcome::admissions(source),
                    None,
                    iteration_outcome::origins(snapshot),
                )
            }
            Source::Completion(source) => {
                completion::require_release(conn, snapshot, source).await?;
                completion::validate_storage(source)?;
                if archived != source.fields {
                    return Err(denied());
                }
                for field in &archived {
                    if field.encryption_key_hash.as_deref() != Some(key_hash.as_str()) {
                        return Err(denied());
                    }
                    field.provenance()?;
                }
                (
                    None,
                    source
                        .children
                        .iter()
                        .flat_map(|child| child.contributors.iter().cloned())
                        .collect(),
                    source.contract.output.clone(),
                    source.parents.clone(),
                    completion::admissions(source),
                    None,
                    source.origins.clone(),
                )
            }
            Source::ResultView(source) => {
                result_view::require_release(conn, snapshot, source).await?;
                result_view::validate_storage(source)?;
                if archived != source.fields {
                    return Err(denied());
                }
                for field in &archived {
                    if field.encryption_key_hash.as_deref() != Some(key_hash.as_str()) {
                        return Err(denied());
                    }
                    field.provenance()?;
                }
                (
                    None,
                    source.contributors.clone(),
                    source
                        .contract
                        .as_ref()
                        .map(|value| serde_json::from_value(value.clone()))
                        .transpose()?,
                    source.parents.clone(),
                    vec![source.admission],
                    None,
                    source.origins.clone(),
                )
            }
            Source::Cache(source) => {
                if !archived.is_empty() {
                    return Err(denied());
                }
                let entries = source
                    .entries
                    .iter()
                    .map(|(id, external_id, checksum)| NativeCacheStorageEntry {
                        id: *id,
                        external_id: external_id.clone(),
                        source_checksum: checksum.clone(),
                    })
                    .collect();
                (
                    Some(NativeCacheStorageParents {
                        source: source.clone(),
                        iteration: cache_binding(conn, snapshot, source, true)
                            .await?
                            .ok_or_else(denied)?,
                        entries,
                    }),
                    vec![],
                    None,
                    source.parents.clone(),
                    vec![],
                    None,
                    vec![cache_origin(source)],
                )
            }
            Source::Wait(source) => {
                if archived != source.fields {
                    return Err(denied());
                }
                for field in &archived {
                    if field.encryption_key_hash.as_deref() != Some(key_hash.as_str()) {
                        return Err(denied());
                    }
                    field.provenance()?;
                }
                let transaction:String=sqlx::query_scalar("SELECT release_transaction FROM runtime_native_wait_release WHERE snapshot=$1 AND wait=$2").bind(snapshot).bind(source.wait).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
                if transaction != source.prepared_transaction {
                    return Err(denied());
                }
                let release = NativeStorageWaitRelease {
                    wait: source.wait,
                    snapshot,
                    transaction,
                };
                let contract = source
                    .contract
                    .as_ref()
                    .map(|contract| {
                        serde_json::from_value::<crate::value_schema::Contract>(contract.clone())
                    })
                    .transpose()?;
                if let Some(contract) = &contract {
                    crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
                }
                (
                    None,
                    source.contributors.clone(),
                    contract,
                    source.parents.clone(),
                    vec![source.source_admission],
                    Some(release),
                    source.origins.clone(),
                )
            }
        };
        Ok(NativeSnapshotStorageEvidence {
            success_metadata: match &body.source {
                Source::SuccessMetadata(source) => {
                    Some(NativeSuccessMetadataEvidence::new(snapshot, source))
                }
                _ => None,
            },
            iteration_outcome: match &body.source {
                Source::IterationOutcome(source) => Some(iteration_outcome::evidence(source)),
                _ => None,
            },
            completion_release: match &body.source {
                Source::Completion(source) => Some(NativeStorageCompletionRelease {
                    snapshot,
                    execution: source.execution,
                    completed: source.completed,
                    transaction: source.transaction.clone(),
                }),
                _ => None,
            },
            completion_sources: match &body.source {
                Source::Completion(source) => source
                    .children
                    .iter()
                    .cloned()
                    .map(NativeCompletionSource::new)
                    .collect(),
                _ => Vec::new(),
            },
            result_release: match &body.source {
                Source::ResultView(source) => Some(NativeStorageResultViewRelease {
                    snapshot,
                    execution: source.execution,
                    admission: source.admission,
                    transaction: source.transaction.clone(),
                }),
                _ => None,
            },
            metadata,
            physical_parents,
            cache,
            frozen_owner,
            contributors,
            contract,
            archive_fields: archived.iter().map(ArchivedField::stored).collect(),
            encrypted,
            key_hash,
            receipts,
            admissions,
            release,
            renderer: body.renderer,
            policy: body.authority.policy_id,
            approvals,
            origins,
            bytes,
        })
    }
    /// No source/graph acquisition. Called behind targets only for a render
    /// whose immutable source proof was already prepared under held fences.
    pub(crate) async fn require_finalized(conn: &mut PgConnection, snapshot: Id) -> Result<()> {
        let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
        if let Source::SuccessMetadata(source) = &body.source {
            success_metadata::require_release(conn, snapshot, source).await?;
        }
        if let Source::IterationOutcome(source) = &body.source {
            iteration_outcome::require_release(conn, snapshot, source).await?;
        }
        if let Source::Completion(source) = &body.source {
            completion::require_release(conn, snapshot, source).await?;
        }
        if let Source::ResultView(source) = &body.source {
            result_view::require_release(conn, snapshot, source).await?;
        }
        if let Source::Cache(source) = &body.source {
            cache_binding(conn, snapshot, source, true).await?;
        }
        if let Source::Wait(source) = body.source {
            let saved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1 AND wait=$2)").bind(snapshot).bind(source.wait).fetch_one(conn).await?;
            if !saved {
                return Err(denied());
            }
        }
        Ok(())
    }
}
async fn require_selected_read(
    conn: &mut PgConnection,
    authority: &SelectedRuntimeAuthority,
    target: &super::runtime_target::ResolvedRuntimeTarget,
    action: crate::rbac::Action,
) -> Result<()> {
    let policy = RuntimeAuthorityRepository::find_policy(conn, authority.policy_id)
        .await?
        .ok_or_else(denied)?;
    for grant in &authority.grants {
        let key = super::runtime_target_binding::grant_key(grant)?;
        if policy
            .policy
            .concrete_targets
            .get(&key)
            .is_some_and(|binding| super::runtime_target_binding::matches(binding, target))
            && grant.allows(
                &authority.approval_ids(),
                target.pack_id(),
                target.resource(),
                action,
                target.context(),
            )
        {
            return Ok(());
        }
    }
    Err(denied())
}
fn wait_state(state: &str) -> Result<crate::models::WorkflowTaskWaitState> {
    match state {
        "released" => Ok(crate::models::WorkflowTaskWaitState::Released),
        "failed" => Ok(crate::models::WorkflowTaskWaitState::Failed),
        "timed_out" => Ok(crate::models::WorkflowTaskWaitState::TimedOut),
        _ => Err(denied()),
    }
}
// This status is the native source row's sealed top-level classification, never
// a field inside its response/result payload or the current mutable wait JSON.
fn native_wait_outcome_values(source: &WaitSource) -> Result<[Value; 4]> {
    let status = source
        .raw_projection
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let outcome = match (source.target_kind.as_str(), source.state.as_str(), status) {
        ("inquiry", "released", "responded")
        | ("execution" | "work_queue_item", "released", "completed") => 0,
        ("execution", "failed", "failed" | "abandoned")
        | ("work_queue_item", "failed", "failed" | "skipped") => 1,
        ("inquiry" | "execution", "timed_out", "timeout") => 2,
        ("inquiry" | "execution" | "work_queue_item", "failed", "cancelled") => 3,
        _ => return Err(denied()),
    };
    Ok(std::array::from_fn(|index| json!(index == outcome)))
}
async fn validate_wait_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &WaitSource,
) -> Result<()> {
    let transaction: Option<String> = sqlx::query_scalar(
        "SELECT release_transaction FROM runtime_native_wait_release WHERE wait=$1 AND snapshot=$2",
    )
    .bind(source.wait)
    .bind(snapshot)
    .fetch_optional(&mut *conn)
    .await?;
    if transaction.as_deref().is_none_or(str::is_empty) {
        return Err(denied());
    }
    let row = super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(
        &mut *conn,
        source.workflow,
        &source.task_name,
    )
    .await?;
    if let Some(row) = row {
        let mut raw = row.result.clone().ok_or_else(denied)?;
        if let Some(object) = raw.as_object_mut() {
            object.remove("_delivery_complete");
        }
        if row.id != source.wait
            || row.state != wait_state(&source.state)?
            || row.target()?.id() != source.target_id
            || serde_json::to_value(row.target()?.kind())?.as_str()
                != Some(source.target_kind.as_str())
            || raw != source.raw_projection
        {
            return Err(denied());
        }
    }
    Ok(())
}
async fn load_fields(conn: &mut PgConnection, kind: &str, id: Id) -> Result<Vec<ArchivedField>> {
    let rows =
        super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn, kind, id,
        )
        .await?;
    if rows.len() > 256 {
        return Err(denied());
    }
    let fields = rows
        .into_iter()
        .map(|field| ArchivedField {
            json_path: field.json_path,
            encrypted_value: field.encrypted_value,
            encryption_key_hash: field.encryption_key_hash,
            source_kind: field.source_kind,
            source_ref: field.source_ref,
        })
        .collect::<Vec<_>>();
    bounded(&serde_json::to_value(&fields)?)?;
    Ok(fields)
}
async fn archive_wait(
    conn: &mut PgConnection,
    snapshot: Id,
    fields: &[ArchivedField],
) -> Result<()> {
    let archived = fields
        .iter()
        .map(|field| {
            Ok(crate::secret_values::PreparedSecretValue {
                json_path: field.json_path.clone(),
                encrypted_value: field.encrypted_value.clone(),
                encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(denied)?,
                source_kind: field.source_kind.clone(),
                source_ref: field.source_ref.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    super::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(
        conn,
        "native_input_snapshot",
        snapshot,
        &archived,
    )
    .await
}
async fn validate_wait_value(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &WaitSource,
    value: &Value,
    key: &str,
) -> Result<()> {
    native_wait_outcome_values(source)?;
    if source.contributors.len() > 256 {
        return Err(denied());
    }
    for contributor in &source.contributors {
        contributor.validate()?;
    }
    let expected_site = match source.target_kind.as_str() {
        "inquiry" => ("inquiry_response", "response"),
        "execution" => ("execution_result", "result"),
        "work_queue_item" => ("queue_item_result", "result"),
        _ => return Err(denied()),
    };
    if source.target.id != source.target_id
        || source.target.kind != source.target_kind
        || source.source_kind != expected_site.0
        || source.source_field != expected_site.1
        || source.context_path
            != format!(
                "/{}/{}",
                source.target_kind,
                source.task_name.replace('~', "~0").replace('/', "~1")
            )
    {
        return Err(denied());
    }
    if source.source_version.get("projection_digest") != Some(&json!(hash(&source.raw_projection)?))
        || source.source_version.get("ciphertext_digest")
            != Some(&json!(hash(&serde_json::to_value(&source.fields)?)?))
        || source.source_version.get("contract_digest")
            != Some(&json!(source.contract.as_ref().map(hash).transpose()?))
    {
        return Err(denied());
    }
    if source.wait <= 0
        || source.target_id <= 0
        || source.source_id != source.target_id
        || source.parents.len() + source.origins.len() + source.fields.len() > 256
        || source.source_version.is_null()
        || load_fields(conn, "native_input_snapshot", snapshot).await? != source.fields
    {
        return Err(denied());
    }
    let mut expected = source.raw_projection.clone();
    if let Some(raw) = source.raw_projection.get(&source.source_field) {
        let restored = crate::secret_values::restore_secret_values(
            raw.clone(),
            &source
                .fields
                .iter()
                .map(ArchivedField::stored)
                .collect::<Vec<_>>(),
            key,
        )?;
        if let Some(contract) = &source.contract {
            let contract: crate::value_schema::Contract = serde_json::from_value(contract.clone())?;
            crate::value_schema::CompiledContract::new(&contract)
                .map_err(|_| denied())?
                .validate(&restored)
                .map_err(|_| denied())?;
        }
        expected[&source.source_field] = restored;
    } else if !source.fields.is_empty() {
        return Err(denied());
    }
    if &expected != value {
        return Err(denied());
    }
    Ok(())
}
fn cache_frontier_matches(source: &CacheSource, row: &WorkflowCacheIteration) -> bool {
    Some(row.id) == source.iteration
        && row.workflow_execution == source.workflow
        && row.task_name == source.task_name
        && row.namespace == source.namespace
        && row.generation == source.generation
        && row.last_external_id == source.before
        && row.next_batch_index == source.ordinal
        && row.page_size == source.page_size
        && row.batch_size == source.batch_size
        && row.state == crate::models::WorkflowCacheIterationState::Scanning
}
fn validate_cache(source: &CacheSource, value: &Value) -> Result<()> {
    validate_cache_storage(source)?;
    if source.entries.is_empty() {
        return if source.exhausted && source.after == source.before && value.is_null() {
            Ok(())
        } else {
            Err(denied())
        };
    }
    let values = if source.batch_size == 1 {
        vec![value]
    } else {
        value.as_array().ok_or_else(denied)?.iter().collect()
    };
    if values.len() != source.entries.len() {
        return Err(denied());
    }
    for ((_, external, checksum), value) in source.entries.iter().zip(values) {
        if value.get("external_id") != Some(&json!(external))
            || value.get("source_checksum") != Some(&json!(checksum))
        {
            return Err(denied());
        }
    }
    Ok(())
}
fn storage_owner(owner: &super::runtime_read::RuntimeReadOwner) -> Result<NativeStorageOwner> {
    let inventory: FrozenOwnerInventory = serde_json::from_value(serde_json::to_value(owner)?)?;
    if [
        inventory.managing_pack,
        inventory.pack,
        inventory.owner,
        inventory.owner_identity,
    ]
    .into_iter()
    .flatten()
    .any(|id| id <= 0)
    {
        return Err(denied());
    }
    if inventory.owner.is_some() && inventory.scope.is_none() {
        return Err(denied());
    }
    Ok(NativeStorageOwner { inventory })
}
fn storage_owners(owner: &NativeStorageOwner) -> Result<Vec<NativeStorageParent>> {
    let mut result = Vec::new();
    for pack in [owner.managing_pack(), owner.pack()].into_iter().flatten() {
        result.push(NativeStorageParent::Pack { pack });
    }
    if let Some(identity) = owner.owner_identity() {
        result.push(NativeStorageParent::Identity { identity });
    }
    if let Some(id) = owner.owner() {
        result.push(match owner.scope().ok_or_else(denied)? {
            crate::models::OwnerType::Pack => NativeStorageParent::Pack { pack: id },
            crate::models::OwnerType::Identity => NativeStorageParent::Identity { identity: id },
            crate::models::OwnerType::Action => NativeStorageParent::Action { action: id },
            crate::models::OwnerType::Sensor => NativeStorageParent::Sensor { sensor: id },
            crate::models::OwnerType::System => return Err(denied()),
        });
    }
    Ok(result)
}
fn validate_cache_storage(source: &CacheSource) -> Result<()> {
    if source.iteration.is_some_and(|id| id <= 0)
        || (source.iteration.is_none() && source.frontier.is_none())
        || source.workflow <= 0
        || source.namespace <= 0
        || source.generation <= 0
        || source.target.id != source.namespace
        || source.target.kind != "cache_namespace"
        || source.task_name.is_empty()
        || source.page_size <= 0
        || source.ordinal < 0
        || source.batch_size <= 0
        || source.entries.len() > 256
        || source.entries.len() > source.batch_size as usize
        || source.parents.len() > 256
    {
        return Err(denied());
    }
    let mut before = source.before.as_deref();
    let mut ids = std::collections::BTreeSet::new();
    for (id, external, _) in &source.entries {
        if *id <= 0
            || !ids.insert(*id)
            || before.is_some_and(|previous| external.as_bytes() <= previous.as_bytes())
        {
            return Err(denied());
        }
        before = Some(external);
    }
    if before != source.after.as_deref() || (source.entries.is_empty() && !source.exhausted) {
        return Err(denied());
    }
    Ok(())
}
fn validate_wait_storage(source: &WaitSource) -> Result<()> {
    native_wait_outcome_values(source)?;
    let kind = match source.target_kind.as_str() {
        "execution" => "execution_result",
        "inquiry" => "inquiry_response",
        "work_queue_item" => "queue_item_result",
        _ => return Err(denied()),
    };
    let field = if source.target_kind == "inquiry" {
        "response"
    } else {
        "result"
    };
    if source.wait <= 0
        || source.workflow <= 0
        || source.source_admission <= 0
        || source.target_id <= 0
        || source.target.id != source.target_id
        || source.source_id != source.target_id
        || source.target.kind != source.target_kind
        || source.source_kind != kind
        || source.source_field != field
        || source.prepared_transaction.is_empty()
        || source.contributors.len() > 256
        || source.fields.len() > 256
        || source.parents.len() > 256
        || source.origins.len() > 256
    {
        return Err(denied());
    }
    wait_state(&source.state)?;
    for contributor in &source.contributors {
        contributor.validate()?;
    }
    Ok(())
}
fn storage_physical_parents(
    snapshot: Id,
    body: &Body,
    owner: &NativeStorageOwner,
) -> Result<Vec<NativeStorageParent>> {
    let mut result = snapshot_storage_step(storage_owners(owner), "owner-parents")?;
    if snapshot <= 0 || body.renderer <= 0 || body.execution <= 0 || body.authority.policy_id <= 0 {
        return Err(denied());
    }
    result.extend([
        NativeStorageParent::Execution {
            execution: body.execution,
        },
        NativeStorageParent::Admission {
            admission: body.renderer,
        },
        NativeStorageParent::Policy {
            policy: body.authority.policy_id,
        },
        NativeStorageParent::NativeArchive { snapshot },
        NativeStorageParent::NativeBudget {
            renderer_admission: body.renderer,
        },
    ]);
    result.extend(
        body.authority
            .approval_ids()
            .into_iter()
            .map(|approval| NativeStorageParent::Approval { approval }),
    );
    let (target, parents, extra_nodes): (&BoundRuntimeTarget, &[(Id, InputSite)], usize) =
        match &body.source {
            Source::SuccessMetadata(source) => {
                snapshot_storage_step(
                    success_metadata::validate_storage(source),
                    "success-storage-shape",
                )?;
                result.extend(snapshot_storage_step(
                    success_metadata::physical_parents(source),
                    "success-source-parents",
                )?);
                (
                    &source.target,
                    &source.parents,
                    success_metadata::nodes(source),
                )
            }
            Source::IterationOutcome(source) => {
                iteration_outcome::validate_storage(source)?;
                let frozen = iteration_outcome::evidence(source);
                result.push(NativeStorageParent::WorkflowExecution {
                    workflow: frozen.workflow(),
                });
                let mut nodes = 1usize;
                for member in frozen.members() {
                    for observed in [member.original(), member.latest()] {
                        result.extend([
                            NativeStorageParent::Execution {
                                execution: observed.execution(),
                            },
                            NativeStorageParent::Admission {
                                admission: observed.admission(),
                            },
                        ]);
                        result.extend(storage_owners(&storage_owner(observed.read_owner())?)?);
                        if let Some(pack) = observed.target().pack_id {
                            result.push(NativeStorageParent::Pack { pack });
                        }
                        nodes = nodes
                            .checked_add(observed.storage_nodes())
                            .ok_or_else(denied)?;
                    }
                }
                (
                    iteration_outcome::target(source).0,
                    iteration_outcome::parents(source),
                    nodes,
                )
            }
            Source::Completion(source) => {
                completion::validate_storage(source)?;
                result.push(NativeStorageParent::WorkflowExecution {
                    workflow: source.workflow,
                });
                let mut nodes = 1usize;
                for child in &source.children {
                    result.extend([
                        NativeStorageParent::Execution {
                            execution: child.execution,
                        },
                        NativeStorageParent::Admission {
                            admission: child.admission,
                        },
                    ]);
                    result.extend(storage_owners(&storage_owner(&child.read_owner)?)?);
                    if let Some(pack) = child.target.pack_id {
                        result.push(NativeStorageParent::Pack { pack });
                    }
                    nodes = nodes
                        .checked_add(
                            child.fields.len()
                                + child.parents.len()
                                + child.contributors.len()
                                + child.origins.len()
                                + child.mappings.len()
                                + 1,
                        )
                        .ok_or_else(denied)?;
                }
                (&source.target, &source.parents, nodes)
            }
            Source::ResultView(source) => {
                result_view::validate_storage(source)?;
                result.extend([
                    NativeStorageParent::WorkflowExecution {
                        workflow: source.workflow,
                    },
                    NativeStorageParent::Execution {
                        execution: source.execution,
                    },
                    NativeStorageParent::Admission {
                        admission: source.admission,
                    },
                ]);
                (
                    &source.target,
                    &source.parents,
                    source.fields.len()
                        + source.contributors.len()
                        + source.origins.len()
                        + source.mappings.len()
                        + 1,
                )
            }
            Source::Cache(source) => {
                validate_cache_storage(source)?;
                result.extend([
                    NativeStorageParent::WorkflowExecution {
                        workflow: source.workflow,
                    },
                    NativeStorageParent::CacheNamespace {
                        namespace: source.namespace,
                    },
                    NativeStorageParent::CacheGeneration {
                        generation: source.generation,
                    },
                ]);
                if let Some(iteration) = source.iteration {
                    result.push(NativeStorageParent::CacheIteration { iteration });
                }
                if let Some(plan) = &source.frontier {
                    if plan.concurrency <= 0
                        || plan.scanned_before < 0
                        || plan.prepared_transaction.is_empty()
                    {
                        return Err(denied());
                    }
                    if let Some(snapshot) = plan.predecessor {
                        result.push(NativeStorageParent::NativeSnapshot { snapshot });
                    }
                }
                result.extend(source.entries.iter().map(|(entry, _, _)| {
                    NativeStorageParent::CacheEntry {
                        generation: source.generation,
                        entry: *entry,
                    }
                }));
                (
                    &source.target,
                    &source.parents,
                    1 + usize::from(source.iteration.is_none()),
                )
            }
            Source::Wait(source) => {
                validate_wait_storage(source)?;
                result.extend([
                    NativeStorageParent::WorkflowExecution {
                        workflow: source.workflow,
                    },
                    NativeStorageParent::WorkflowTaskWait { wait: source.wait },
                    NativeStorageParent::Admission {
                        admission: source.source_admission,
                    },
                ]);
                result.push(match source.target_kind.as_str() {
                    "execution" => NativeStorageParent::Execution {
                        execution: source.target_id,
                    },
                    "inquiry" => NativeStorageParent::Inquiry {
                        inquiry: source.target_id,
                    },
                    "work_queue_item" => NativeStorageParent::QueueItem {
                        item: source.target_id,
                    },
                    _ => return Err(denied()),
                });
                (
                    &source.target,
                    &source.parents,
                    source.contributors.len() + source.fields.len() + source.origins.len() + 1,
                )
            }
        };
    let scope = owner.scope().map(|scope| match scope {
        crate::models::OwnerType::System => "system",
        crate::models::OwnerType::Identity => "identity",
        crate::models::OwnerType::Pack => "pack",
        crate::models::OwnerType::Action => "action",
        crate::models::OwnerType::Sensor => "sensor",
    });
    if target.pack_id != owner.pack()
        || target.owner_id != owner.owner()
        || target.owner_type.as_deref() != scope
    {
        return Err(Error::PermissionDenied(
            "Native snapshot target ownership mismatch".into(),
        ));
    }
    if let Some(pack) = target.pack_id {
        if pack <= 0 {
            return Err(denied());
        }
        result.push(NativeStorageParent::Pack { pack });
    }
    if parents
        .iter()
        .any(|(id, site)| *id <= 0 || site.execution <= 0)
    {
        return Err(denied());
    }
    result.extend(
        parents
            .iter()
            .map(|(receipt, _)| NativeStorageParent::Receipt { receipt: *receipt }),
    );
    result.sort();
    result.dedup();
    if result.len() + parents.len() + extra_nodes > 256 {
        return Err(Error::PermissionDenied(
            "Native snapshot physical parent budget exceeded".into(),
        ));
    }
    Ok(result)
}
// Reserve the normalized wait association before release, using the maximum
// snapshot ID width. Bodies already include manifests/contracts; archive fields
// and receipt sites are additionally charged by save/load for their stored copies.
fn storage_associations(body: &Body) -> Value {
    match &body.source {
        Source::SuccessMetadata(source) => success_metadata::associations(source, body.renderer),
        Source::IterationOutcome(source) => iteration_outcome::associations(source, body.renderer),
        Source::Completion(source) => {
            json!({"budget":body.renderer,"admissions":completion::admissions(source),"release":null,"completion_release":[Id::MAX,source.execution,source.completed,source.transaction]})
        }
        Source::ResultView(source) => {
            json!({"budget":body.renderer,"admissions":[source.admission],"release":null,"result_view_release":[Id::MAX,source.admission,source.execution,source.transaction]})
        }
        Source::Cache(source) => {
            let mut associations = json!({"budget":body.renderer,"admissions":[],"release":null});
            if let Some(plan) = &source.frontier {
                associations["cache_frontier"] = json!([
                    Id::MAX,
                    plan.predecessor,
                    plan.prepared_transaction,
                    source.ordinal,
                    Id::MAX,
                    plan.prepared_transaction
                ]);
            }
            associations
        }
        Source::Wait(source) => {
            json!({"budget":body.renderer,"admissions":[source.source_admission],"release":[source.wait,Id::MAX,source.prepared_transaction]})
        }
    }
}
fn metadata(snapshot: Id, body: &Body) -> NativeSnapshotMetadata {
    match &body.source {
        Source::SuccessMetadata(source) => NativeSnapshotMetadata {
            snapshot,
            source_kind: "ordinary_success_metadata".into(),
            target: source.target.clone(),
            read_owner: source.read_owner.clone(),
            generation: None,
            read_version: success_metadata::read_version(source),
            source_site: Some(("execution_result".into(), source.execution, String::new())),
            result_mappings: None,
            result_contract_kind: None,
        },
        Source::IterationOutcome(source) => NativeSnapshotMetadata {
            snapshot,
            source_kind: "ordinary_iteration_outcome".into(),
            target: iteration_outcome::target(source).0.clone(),
            read_owner: iteration_outcome::target(source).1.clone(),
            generation: None,
            read_version: iteration_outcome::read_version(source),
            source_site: None,
            result_mappings: None,
            result_contract_kind: None,
        },
        Source::Completion(source) => NativeSnapshotMetadata {
            result_contract_kind: None,
            snapshot,
            source_kind: "workflow_failure_metadata".into(),
            target: source.target.clone(),
            read_owner: source.read_owner.clone(),
            generation: None,
            read_version: json!({"completed":source.completed,"parent_digest":source.parent_digest,"workflow_digest":source.workflow_digest,"contract_digest":source.contract_digest,"children":source.children.iter().map(|child|json!({"execution":child.execution,"admission":child.admission,"version":child.version,"binding_digest":child.binding_digest})).collect::<Vec<_>>(),"failed":source.failed}),
            source_site: Some(("execution_result".into(), source.execution, String::new())),
            result_mappings: None,
        },
        Source::ResultView(source) => NativeSnapshotMetadata {
            result_contract_kind: Some(source.contract_kind),
            snapshot,
            source_kind: "execution_result_view".into(),
            target: source.target.clone(),
            read_owner: source.read_owner.clone(),
            generation: None,
            read_version: json!({"source_version":source.version,"binding_digest":source.binding_digest,"mappings":source.mappings}),
            source_site: Some(("execution_result".into(), source.execution, String::new())),
            result_mappings: Some(source.mappings.clone()),
        },
        Source::Cache(source) => NativeSnapshotMetadata {
            result_contract_kind: None,
            result_mappings: None,
            snapshot,
            source_kind: "cache".into(),
            target: source.target.clone(),
            read_owner: source.read_owner.clone(),
            generation: Some(source.generation),
            read_version: json!({"generation":source.generation,"entries":source.entries,"before":source.before,"after":source.after,"ordinal":source.ordinal}),
            source_site: None,
        },
        Source::Wait(source) => NativeSnapshotMetadata {
            result_contract_kind: None,
            result_mappings: None,
            snapshot,
            source_kind: source.source_kind.clone(),
            target: source.target.clone(),
            read_owner: source.read_owner.clone(),
            generation: None,
            read_version: source.source_version.clone(),
            source_site: Some((source.source_kind.clone(), source.source_id, String::new())),
        },
    }
}
fn cache_origin(source: &CacheSource) -> SecretOrigin {
    let path = if let Some(iteration) = source.iteration {
        format!(
            "/generation/{}/iteration/{}/{}",
            source.generation, iteration, source.ordinal
        )
    } else {
        format!(
            "/generation/{}/planned-frontier/{}",
            source.generation, source.ordinal
        )
    };
    SecretOrigin::Entity {
        entity_type: "cache_namespace".into(),
        entity_id: source.namespace,
        path,
    }
}
pub(crate) fn in_task(task: &str, path: &str) -> bool {
    path.strip_prefix(task)
        .is_some_and(|suffix| suffix.starts_with('/'))
}
pub(crate) fn wait_outcome_path(task: &str, path: &str) -> bool {
    path.strip_prefix(task)
        .and_then(|suffix| suffix.strip_prefix("/next/"))
        .and_then(|suffix| suffix.split_once('/'))
        .is_some_and(|(index, field)| field == "when" && index.parse::<usize>().is_ok())
}
async fn require_task(
    conn: &mut PgConnection,
    record: &super::runtime_authority::RuntimeAdmissionRecord,
    path: &str,
    authority: &SelectedRuntimeAuthority,
) -> Result<()> {
    let InvocationBinding::WorkflowTask {
        workflow_ref,
        task_path,
        ..
    } = &authority.binding
    else {
        return Err(denied());
    };
    if record.owner_kind != "execution"
        || workflow_ref != record.authority.binding.action_ref()
        || authority.attribution
            != (RequestAttribution::ParentExecution {
                execution_id: record.owner_id,
            })
    {
        return Err(denied());
    }
    let policy = RuntimeAuthorityRepository::find_policy(conn, record.policy_revision)
        .await?
        .ok_or_else(denied)?;
    let invocation = policy
        .policy
        .invocations
        .iter()
        .find(|invocation| invocation.binding == record.authority.binding)
        .ok_or_else(denied)?;
    let expected = if let Some(pin) = &invocation.foreign_workflow_policy {
        let publisher = RuntimeAuthorityRepository::find_policy(conn, pin.revision)
            .await?
            .ok_or_else(denied)?;
        pin.verify(&publisher)?;
        publisher.id
    } else {
        policy.id
    };
    if authority.policy_id != expected {
        return Err(denied());
    }
    let definition = workflow_definition(conn, record).await?;
    let actual = canonical_task_path(
        &definition,
        path.strip_prefix("/workflow").ok_or_else(denied)?,
    )
    .ok_or_else(denied)?;
    if canonical_task_path(&definition, task_path).as_deref() != Some(actual.as_str()) {
        return Err(denied());
    }
    let task = task_definition(conn, record, path).await?;
    let action = task
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    if action != authority.binding.action_ref() {
        return Err(denied());
    }
    RuntimeAuthorityRepository::selected_effect_fence(conn, authority).await?;
    Ok(())
}
async fn task_definition(
    conn: &mut PgConnection,
    record: &super::runtime_authority::RuntimeAdmissionRecord,
    path: &str,
) -> Result<Value> {
    let definition = workflow_definition(conn, record).await?;
    let pointer = path.strip_prefix("/workflow").ok_or_else(denied)?;
    definition
        .pointer(pointer)
        .filter(|task| task.is_object())
        .cloned()
        .ok_or_else(denied)
}
async fn workflow_definition(
    conn: &mut PgConnection,
    record: &super::runtime_authority::RuntimeAdmissionRecord,
) -> Result<Value> {
    let policy = RuntimeAuthorityRepository::find_policy(conn, record.policy_revision)
        .await?
        .ok_or_else(denied)?;
    let invocation = policy
        .policy
        .invocations
        .iter()
        .find(|invocation| invocation.binding == record.authority.binding)
        .ok_or_else(denied)?;
    let snapshot = super::managed_executable::ManagedExecutableRepository::invocation_snapshot(
        conn, invocation,
    )
    .await?;
    let graph = snapshot.executable.workflow_definition.ok_or_else(denied)?;
    Ok(graph.definition)
}
fn canonical_task_path(definition: &Value, path: &str) -> Option<String> {
    let tokens = path.strip_prefix('/')?.split('/').collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() % 2 != 0 {
        return None;
    }
    let mut node = definition;
    let mut canonical = String::new();
    for pair in tokens.chunks_exact(2) {
        if pair[0] != "tasks" {
            return None;
        }
        let selector = pair[1].replace("~1", "/").replace("~0", "~");
        match node.get("tasks")? {
            Value::Array(tasks) => {
                let mut matching = tasks.iter().enumerate().filter(|(index, task)| {
                    index.to_string() == selector
                        || task.get("name").and_then(Value::as_str) == Some(selector.as_str())
                });
                let (index, task) = matching.next()?;
                if matching.next().is_some() {
                    return None;
                }
                canonical.push_str(&format!("/tasks/{index}"));
                node = task;
            }
            Value::Object(tasks) => {
                node = tasks.get(&selector)?;
                canonical.push_str("/tasks/");
                canonical.push_str(&pair[1]);
            }
            _ => return None,
        }
    }
    Some(canonical)
}
fn bound_target(
    id: Id,
    target: &super::runtime_target::ResolvedRuntimeTarget,
) -> Result<BoundRuntimeTarget> {
    Ok(BoundRuntimeTarget {
        kind: target.identity_kind().into(),
        id,
        pack_id: target.pack_id(),
        owner_type: target
            .context()
            .owner_type
            .map(|kind| serde_json::to_value(kind))
            .transpose()?
            .and_then(|kind| kind.as_str().map(str::to_owned)),
        owner_id: target.owner_id(),
    })
}
fn check_cache_freshness_deadline(
    fresh_until: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    if fresh_until.is_some_and(|until| until <= now) {
        return Err(Error::cache_snapshot_expired(
            "original cache freshness deadline expired before effect or disclosure",
        ));
    }
    Ok(())
}
fn cache_freshness_deadline(
    generation: &crate::models::CacheGeneration,
    freshness_target_seconds: i64,
    require_fresh: bool,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    if !require_fresh {
        return Ok(None);
    }
    if generation.state != crate::models::CacheGenerationState::Active {
        return Err(denied());
    }
    let fresh_until = if freshness_target_seconds > 0 {
        generation
            .activated
            .map(|activated| {
                let seconds = freshness_target_seconds.checked_add(1).ok_or_else(denied)?;
                let duration = chrono::Duration::try_seconds(seconds).ok_or_else(denied)?;
                activated.checked_add_signed(duration).ok_or_else(denied)
            })
            .transpose()?
    } else {
        None
    };
    NativeCacheReadPolicy {
        require_fresh,
        fresh_until,
    }
    .ensure_fresh()?;
    Ok(fresh_until)
}

async fn cache_reader_policy(
    conn: &mut PgConnection,
    body: &Body,
    source: &CacheSource,
    fresh_until: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<NativeCacheReadPolicy> {
    let record =
        RuntimeAuthorityRepository::require_retained_admission(conn, body.renderer).await?;
    if record.owner_id != body.execution {
        return Err(denied());
    }
    require_task(conn, &record, &body.task_path, &body.authority).await?;
    // This graph comes from the retained invocation's immutable executable, not
    // the live namespace, current execution JSON or an origin-path convention.
    let task = task_definition(conn, &record, &body.task_path).await?;
    let config: crate::workflow::parser::IterateCacheConfig =
        serde_json::from_value(task.get("iterate_cache").cloned().ok_or_else(denied)?)?;
    let policy = NativeCacheReadPolicy {
        require_fresh: config.require_fresh,
        fresh_until,
    };
    if policy.require_fresh {
        let generation =
            super::cache::CacheGenerationRepository::find_by_id_for_share(conn, source.generation)
                .await?
                .ok_or_else(denied)?;
        if generation.id != source.generation
            || generation.namespace != source.namespace
            || generation.state != crate::models::CacheGenerationState::Active
        {
            return Err(denied());
        }
    } else if policy.fresh_until.is_some() {
        return Err(denied());
    }
    policy.ensure_fresh()?;
    Ok(policy)
}

fn cache_selector_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}
async fn cache_plan_selectors(
    conn: &mut PgConnection,
    renderer_admission: Id,
    renderer: Id,
    task_path: &str,
    config: &crate::workflow::parser::IterateCacheConfig,
    controls: &[&ReceiptContext],
    fence: &mut ApprovalEffectFence,
) -> Result<(
    std::collections::BTreeMap<&'static str, Value>,
    Vec<(Id, InputSite)>,
)> {
    if controls.len() > 4 {
        return Err(denied());
    }
    let mut values = std::collections::BTreeMap::new();
    let mut parents = Vec::new();
    let mut consumed = std::collections::BTreeSet::new();
    for (field, selector) in [
        ("namespace", Some(config.namespace.as_str())),
        ("generation", Some(config.generation.as_str())),
        ("owner_type", config.owner_type.as_deref()),
        ("owner_ref", config.owner_ref.as_deref()),
    ] {
        let Some(selector) = selector else {
            continue;
        };
        let value = if selector.contains("{{") {
            let path = format!("{task_path}/iterate_cache/{field}");
            let matches = controls
                .iter()
                .enumerate()
                .filter(|(_, control)| control.site().path == path)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(denied());
            }
            let (index, control) = matches[0];
            control
                .require_declared_control(conn, renderer_admission, &path)
                .await?;
            if control.site().execution != renderer {
                return Err(denied());
            }
            consumed.insert(index);
            parents.push(
                InheritedInputRepository::receipt_source_site(conn, &control.origin()).await?,
            );
            *fence = fence.intersect(control.accepted_fence());
            control.accepted_value().clone()
        } else {
            json!(selector)
        };
        values.insert(field, value);
    }
    if consumed.len() != controls.len() {
        return Err(denied());
    }
    Ok((values, parents))
}
async fn cache_plan_owner(
    conn: &mut PgConnection,
    selectors: &std::collections::BTreeMap<&str, Value>,
    snapshot: &crate::models::ExecutionExecutableSnapshot,
    sponsor: Id,
) -> Result<super::cache::CacheOwnerScope> {
    use super::FindByRef;
    let owner = selectors
        .get("owner_type")
        .map(cache_selector_text)
        .unwrap_or_else(|| "pack".into())
        .to_ascii_lowercase();
    let reference = selectors.get("owner_ref").map(cache_selector_text);
    Ok(match owner.as_str() {
        "system" if reference.is_none() => super::cache::CacheOwnerScope::system(),
        "identity" if reference.is_none() => super::cache::CacheOwnerScope::identity(sponsor),
        "pack" => {
            if let Some(reference) = reference {
                let pack = super::pack::PackRepository::find_by_ref(&mut *conn, &reference)
                    .await?
                    .ok_or_else(denied)?;
                super::cache::CacheOwnerScope::pack(pack.id, Some(reference))
            } else {
                super::cache::CacheOwnerScope::pack(
                    snapshot.executable.action.pack,
                    Some(snapshot.executable.action.pack_ref.clone()),
                )
            }
        }
        "action" => {
            let reference = reference.ok_or_else(denied)?;
            let action = super::action::ActionRepository::find_by_ref(&mut *conn, &reference)
                .await?
                .ok_or_else(denied)?;
            super::cache::CacheOwnerScope::action(action.id, Some(reference))
        }
        "sensor" => {
            let reference = reference.ok_or_else(denied)?;
            let sensor = super::trigger::SensorRepository::find_by_ref(&mut *conn, &reference)
                .await?
                .ok_or_else(denied)?;
            super::cache::CacheOwnerScope::sensor(sensor.id, Some(reference))
        }
        _ => return Err(denied()),
    })
}
async fn scan_cache_frontier(
    conn: &mut PgConnection,
    source: &mut CacheSource,
    max_bytes: usize,
    fence: &ApprovalEffectFence,
) -> Result<Value> {
    if max_bytes == 0 || max_bytes > MAX_BYTES || source.batch_size <= 0 || source.page_size <= 0 {
        return Err(denied());
    }
    if source.exhausted {
        source.after = source.before.clone();
        return Ok(Value::Null);
    }
    let mut values = Vec::new();
    let mut bytes = 0usize;
    let mut cursor = source.before.clone();
    while source.entries.len() < usize::try_from(source.batch_size).map_err(|_| denied())?
        && source.entries.len() <= 256
    {
        let remaining = max_bytes.checked_sub(bytes).ok_or_else(denied)?;
        if remaining == 0 {
            break;
        }
        let limit = i64::from(source.page_size)
            .min(i64::from(source.batch_size) - source.entries.len() as i64)
            .min(257 - source.entries.len() as i64);
        let page = CacheEntryRepository::scan_pinned_page_with_budget_conn(
            conn,
            source.namespace,
            source.generation,
            cursor.as_deref(),
            limit,
            i64::try_from(remaining).map_err(|_| denied())?,
        )
        .await?;
        if page.generation.id != source.generation || page.generation.namespace != source.namespace
        {
            return Err(denied());
        }
        if page.entries.is_empty() {
            source.exhausted = true;
            break;
        }
        let mut stopped = false;
        for entry in page.entries {
            let projected = json!({"external_id":entry.external_id,"value":entry.value,"source_updated_at":entry.source_updated_at,"source_checksum":entry.source_checksum,"size_bytes":entry.size_bytes});
            let charge = serde_json::to_vec(&projected)?.len() + 64;
            if charge > max_bytes - bytes {
                if values.is_empty() {
                    return Err(denied());
                }
                stopped = true;
                break;
            }
            if entry.generation != source.generation
                || cursor
                    .as_ref()
                    .is_some_and(|before| entry.external_id.as_bytes() <= before.as_bytes())
            {
                return Err(denied());
            }
            bytes += charge;
            cursor = Some(entry.external_id.clone());
            source
                .entries
                .push((entry.id, entry.external_id, entry.source_checksum));
            values.push(projected);
        }
        fence.ensure_fresh_on(conn).await?;
        if stopped {
            break;
        }
        if !page.has_more {
            source.exhausted = true;
            break;
        }
    }
    source.after = cursor;
    if source.entries.len() > 256 {
        return Err(denied());
    }
    Ok(if values.is_empty() {
        Value::Null
    } else if source.batch_size == 1 {
        values.remove(0)
    } else {
        Value::Array(values)
    })
}
async fn save_cache_frontier(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &CacheSource,
) -> Result<()> {
    if let Some(plan) = &source.frontier {
        sqlx::query("INSERT INTO runtime_native_cache_frontier(snapshot,predecessor,prepared_transaction,ordinal) VALUES($1,$2,$3,$4)").bind(snapshot).bind(plan.predecessor).bind(&plan.prepared_transaction).bind(source.ordinal).execute(&mut *conn).await?;
    }
    Ok(())
}
async fn persist_cache_frontier(
    conn: &mut PgConnection,
    body: Body,
    source: CacheSource,
    value: Value,
    fence: ApprovalEffectFence,
    key: &str,
) -> Result<PreparedCacheInput> {
    let snapshot = save(conn, &body, &value, key).await?;
    for (parent, _) in &source.parents {
        sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(parent).execute(&mut *conn).await?;
    }
    save_cache_frontier(conn, snapshot, &source).await?;
    fence.ensure_fresh_on(conn).await?;
    Ok(PreparedCacheInput {
        fresh_until: source.frontier.as_ref().and_then(|plan| plan.fresh_until),
        selector_publications: vec![],
        selector_bytes: 0,
        transaction: transaction(conn).await?,
        renderer: body.renderer,
        snapshot,
        task_path: body.task_path,
        value,
        fence,
        source,
    })
}
async fn cache_binding(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &CacheSource,
    finalized: bool,
) -> Result<Option<Id>> {
    validate_cache_plan(source)?;
    let Some(plan) = &source.frontier else {
        let unexpected: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1)",
        )
        .bind(snapshot)
        .fetch_one(&mut *conn)
        .await?;
        if unexpected {
            return Err(denied());
        }
        return Ok(source.iteration);
    };
    let row:Option<(Option<Id>,String,Option<Id>,Option<String>,i64)>=sqlx::query_as("SELECT predecessor,prepared_transaction,bound_iteration,bound_transaction,ordinal FROM runtime_native_cache_frontier WHERE snapshot=$1").bind(snapshot).fetch_optional(&mut *conn).await?;
    let (predecessor, prepared, iteration, bound, ordinal) = row.ok_or_else(denied)?;
    if ordinal != source.ordinal
        || predecessor != plan.predecessor
        || prepared != plan.prepared_transaction
        || source
            .iteration
            .is_some_and(|id| iteration.is_some_and(|actual| actual != id))
    {
        return Err(denied());
    }
    match (iteration, bound) {
        (Some(id), Some(bound)) if id > 0 && bound == prepared => {
            if finalized {
                InheritedInputRepository::require_cache_selector_publications(
                    conn,
                    &source.parents,
                )
                .await?;
            }
            Ok(Some(id))
        }
        (None, None) if !finalized && transaction(conn).await? == prepared => Ok(None),
        _ => Err(denied()),
    }
}
fn validate_cache_plan(source: &CacheSource) -> Result<()> {
    if let Some(plan) = &source.frontier {
        if plan.prepared_transaction.is_empty()
            || plan.concurrency <= 0
            || plan.scanned_before < 0
            || plan.predecessor.is_some_and(|id| id <= 0)
        {
            return Err(denied());
        }
        if plan.initial {
            if source.iteration.is_some()
                || plan.predecessor.is_some()
                || source.before.is_some()
                || source.ordinal != 0
                || plan.scanned_before != 0
            {
                return Err(denied());
            }
        } else if source.iteration.is_none() && plan.predecessor.is_none() {
            return Err(denied());
        }
    } else if source.iteration.is_none() {
        return Err(denied());
    }
    Ok(())
}
async fn save(conn: &mut PgConnection, body: &Body, value: &Value, key: &str) -> Result<Id> {
    let body = serde_json::to_value(body)?;
    let encrypted = crate::crypto::encrypt_json(value, key)?;
    snapshot_storage_step(bounded(&body), "body-size")?;
    snapshot_storage_step(bounded(&encrypted), "ciphertext-size")?;
    let typed: Body = serde_json::from_value(body.clone())?;
    if let Source::Cache(source) = &typed.source {
        validate_cache_plan(source)?;
    }
    let owner = snapshot_storage_step(
        storage_owner(metadata(Id::MAX, &typed).read_owner()),
        "owner-inventory",
    )?;
    snapshot_storage_step(
        storage_physical_parents(Id::MAX, &typed, &owner),
        "physical-parents",
    )?;
    let body_digest = snapshot_storage_step(hash(&body), "body-digest")?;
    sqlx::query("INSERT INTO runtime_native_input_snapshot_budget(renderer_admission) VALUES($1) ON CONFLICT DO NOTHING").bind(typed.renderer).execute(&mut *conn).await?;
    sqlx::query("SELECT renderer_admission FROM runtime_native_input_snapshot_budget WHERE renderer_admission=$1 FOR UPDATE").bind(typed.renderer).execute(&mut *conn).await?;
    let (count,bytes):(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(octet_length(body::TEXT)+octet_length(encrypted_value::TEXT)+octet_length(COALESCE(body#>'{source,fields}','[]'::JSONB)::TEXT)+octet_length(COALESCE(body#>'{source,parents}','[]'::JSONB)::TEXT)+octet_length(body_digest)+octet_length(encryption_key_hash)+octet_length(native_snapshot_storage_associations(body,renderer_admission)::TEXT)),0)::BIGINT FROM runtime_native_input_snapshot WHERE renderer_admission=$1").bind(typed.renderer).fetch_one(&mut *conn).await?;
    let charge: i64 = sqlx::query_scalar(
        "SELECT (octet_length($1::JSONB::TEXT)+octet_length($2::JSONB::TEXT)+octet_length(COALESCE($1::JSONB#>'{source,fields}','[]'::JSONB)::TEXT)+octet_length(COALESCE($1::JSONB#>'{source,parents}','[]'::JSONB)::TEXT)+octet_length($3::JSONB::TEXT)+octet_length($4::TEXT)+octet_length($5::TEXT))::BIGINT",
    )
    .bind(&body)
    .bind(&encrypted)
    .bind(storage_associations(&typed))
    .bind(&body_digest)
    .bind(crate::crypto::hash_encryption_key(key))
    .fetch_one(&mut *conn)
    .await?;
    if count >= 128
        || bytes
            .checked_add(charge)
            .is_none_or(|bytes| bytes > MAX_BYTES as i64)
    {
        return Err(Error::PermissionDenied(
            "Native snapshot storage budget exceeded".into(),
        ));
    }
    Ok(sqlx::query_scalar("INSERT INTO runtime_native_input_snapshot(renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash) VALUES($1,$2,$3,$4,$5,$6) RETURNING id")
        .bind(typed.renderer).bind(typed.authority.policy_id).bind(&body).bind(body_digest).bind(encrypted).bind(crate::crypto::hash_encryption_key(key)).fetch_one(conn).await?)
}
fn snapshot_storage_step<T>(result: Result<T>, stage: &'static str) -> Result<T> {
    result.map_err(|error| match error {
        Error::PermissionDenied(message) => Error::PermissionDenied(format!(
            "Native snapshot storage [{stage}] denied: {message}"
        )),
        other => other,
    })
}
async fn load_encrypted(
    conn: &mut PgConnection,
    id: Id,
) -> Result<(Body, Value, usize, Option<chrono::DateTime<chrono::Utc>>)> {
    let (body, encrypted, mut bytes) = load_encrypted_one(conn, id).await?;
    let mut fresh_until = match &body.source {
        Source::Cache(source) => source.frontier.as_ref().and_then(|plan| plan.fresh_until),
        Source::Wait(_)
        | Source::SuccessMetadata(_)
        | Source::ResultView(_)
        | Source::Completion(_)
        | Source::IterationOutcome(_) => None,
    };
    let mut current = id;
    let mut child = body.clone();
    let mut count = 1usize;
    loop {
        let Source::Cache(source) = &child.source else {
            break;
        };
        let Some(plan) = &source.frontier else {
            break;
        };
        let Some(previous) = plan.predecessor else {
            break;
        };
        if previous >= current || count >= 128 {
            return Err(denied());
        }
        let (parent, _, charge) = load_encrypted_one(conn, previous).await?;
        let Source::Cache(prior) = &parent.source else {
            return Err(denied());
        };
        let prior_plan = prior.frontier.as_ref().ok_or_else(denied)?;
        fresh_until = fresh_until.into_iter().chain(prior_plan.fresh_until).min();
        if parent.renderer != child.renderer
            || parent.execution != child.execution
            || parent.task_path != child.task_path
            || parent.authority != child.authority
            || plan.initial
            || source.iteration != prior.iteration
            || source.workflow != prior.workflow
            || source.task_name != prior.task_name
            || source.namespace != prior.namespace
            || source.generation != prior.generation
            || source.page_size != prior.page_size
            || source.batch_size != prior.batch_size
            || source.target != prior.target
            || serde_json::to_value(&source.read_owner)? != serde_json::to_value(&prior.read_owner)?
            || source.parents != prior.parents
            || plan.prepared_transaction != prior_plan.prepared_transaction
            || plan.active_generation
            || plan.fresh_until.is_some()
            || plan.concurrency != prior_plan.concurrency
            || source.before != prior.after
            || source.ordinal != prior.ordinal.checked_add(1).ok_or_else(denied)?
            || plan.scanned_before
                != prior_plan
                    .scanned_before
                    .checked_add(i64::try_from(prior.entries.len()).map_err(|_| denied())?)
                    .ok_or_else(denied)?
            || prior.entries.is_empty()
            || (prior.exhausted && (!source.exhausted || !source.entries.is_empty()))
        {
            return Err(denied());
        }
        if parent
            .approval_deadline
            .is_some_and(|deadline| child.approval_deadline.is_none_or(|child| child > deadline))
        {
            return Err(denied());
        }
        let bound = cache_binding(conn, current, source, false).await?;
        if bound.is_some() && cache_binding(conn, previous, prior, false).await? != bound {
            return Err(denied());
        }
        bytes = bytes.checked_add(charge).ok_or_else(denied)?;
        if bytes > MAX_BYTES {
            return Err(denied());
        }
        child = parent;
        current = previous;
        count += 1;
    }
    Ok((body, encrypted, bytes, fresh_until))
}
async fn load_encrypted_one(conn: &mut PgConnection, id: Id) -> Result<(Body, Value, usize)> {
    let row:Option<(Id,Id,Value,String,Value,String)>=sqlx::query_as("SELECT renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1").bind(id).fetch_optional(&mut *conn).await?;
    let (renderer, policy, body, expected, encrypted, key_hash) = row.ok_or_else(denied)?;
    let mut bytes = serde_json::to_vec(&body)?.len()
        + serde_json::to_vec(&encrypted)?.len()
        + expected.len()
        + key_hash.len();
    if bytes > MAX_BYTES || hash(&body)? != expected {
        return Err(denied());
    }
    let body: Body = serde_json::from_value(body)?;
    if let Source::SuccessMetadata(source) = &body.source {
        success_metadata::validate_body(&body, source)?;
        success_metadata::validate_normalized(conn, id, source).await?;
        let parents: Vec<Id> = sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        let admissions: Vec<Id> = sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        let wrong: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_result_view_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_completion_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_iteration_outcome_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1)").bind(id).fetch_one(&mut *conn).await?;
        bytes = bytes
            .checked_add(serde_json::to_vec(&source.parents)?.len())
            .ok_or_else(denied)?;
        if wrong
            || bytes > MAX_BYTES
            || admissions != success_metadata::admissions(source, body.renderer)
            || parents
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != source.parents.iter().map(|(id, _)| *id).collect()
        {
            return Err(denied());
        }
    } else {
        let unexpected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_success_metadata_observation WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_success_metadata_release WHERE snapshot=$1)").bind(id).fetch_one(&mut *conn).await?;
        if unexpected {
            return Err(denied());
        }
    }
    bytes = bytes
        .checked_add(serde_json::to_vec(&storage_associations(&body))?.len())
        .ok_or_else(denied)?;
    if bytes > MAX_BYTES {
        return Err(denied());
    }
    if let Source::IterationOutcome(source) = &body.source {
        bytes = bytes
            .checked_add(serde_json::to_vec(iteration_outcome::parents(source))?.len() + 2)
            .ok_or_else(denied)?;
        iteration_outcome::validate_body(&body, source)?;
        iteration_outcome::validate_normalized(conn, id, source).await?;
        let parents:Vec<Id>=sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        let admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        let wrong:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_result_view_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_completion_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1)").bind(id).fetch_one(&mut *conn).await?;
        if wrong
            || bytes > MAX_BYTES
            || admissions != iteration_outcome::admissions(source)
            || parents
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != iteration_outcome::parents(source)
                    .iter()
                    .map(|(id, _)| *id)
                    .collect()
        {
            return Err(denied());
        }
    } else if let Source::Completion(source) = &body.source {
        completion::validate_storage(source)?;
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&source.fields)?.len()
                    + serde_json::to_vec(&source.parents)?.len(),
            )
            .ok_or_else(denied)?;
        let parents:Vec<Id>=sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        let admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        let wrong:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_result_view_release WHERE snapshot=$1)").bind(id).fetch_one(&mut *conn).await?;
        if wrong
            || bytes > MAX_BYTES
            || admissions != completion::admissions(source)
            || parents
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != source.parents.iter().map(|(id, _)| *id).collect()
        {
            return Err(denied());
        }
    } else if let Source::ResultView(source) = &body.source {
        result_view::validate_storage(source)?;
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&source.fields)?.len()
                    + serde_json::to_vec(&source.parents)?.len(),
            )
            .ok_or_else(denied)?;
        let parents:Vec<Id>=sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        let admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        let wrong:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE snapshot=$1) OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1)").bind(id).fetch_one(&mut *conn).await?;
        if wrong
            || bytes > MAX_BYTES
            || admissions != vec![source.admission]
            || parents
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != source.parents.iter().map(|(id, _)| *id).collect()
        {
            return Err(denied());
        }
    } else if let Source::Wait(source) = &body.source {
        let unexpected: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_native_cache_frontier WHERE snapshot=$1)",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if unexpected {
            return Err(denied());
        }
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&source.fields)?.len()
                    + serde_json::to_vec(&source.parents)?.len(),
            )
            .ok_or_else(denied)?;
        if bytes > MAX_BYTES {
            return Err(denied());
        }
        let actual:Vec<Id>=sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        if actual
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            != source.parents.iter().map(|(id, _)| *id).collect()
        {
            return Err(denied());
        }
        let actual:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        if actual != vec![source.source_admission] {
            return Err(denied());
        }
    } else if let Source::Cache(source) = &body.source {
        validate_cache_storage(source)?;
        cache_binding(conn, id, source, false).await?;
        bytes = bytes
            .checked_add(serde_json::to_vec(&source.parents)?.len())
            .ok_or_else(denied)?;
        let actual:Vec<Id>=sqlx::query_scalar("SELECT receipt FROM runtime_native_input_snapshot_parent WHERE snapshot=$1 ORDER BY receipt").bind(id).fetch_all(&mut *conn).await?;
        let admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_native_input_snapshot_admission WHERE snapshot=$1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
        if bytes > MAX_BYTES
            || !admissions.is_empty()
            || actual
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != source.parents.iter().map(|(id, _)| *id).collect()
        {
            return Err(denied());
        }
    }
    if body.renderer != renderer || body.authority.policy_id != policy {
        return Err(denied());
    }
    Ok((body, encrypted, bytes))
}
async fn load(
    conn: &mut PgConnection,
    id: Id,
    key: &str,
) -> Result<(Body, Value, usize, Option<chrono::DateTime<chrono::Utc>>)> {
    let expected: String = sqlx::query_scalar(
        "SELECT encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(denied)?;
    if expected != crate::crypto::hash_encryption_key(key) {
        return Err(denied());
    }
    let (body, encrypted, bytes, fresh_until) = load_encrypted(conn, id).await?;
    Ok((
        body,
        crate::crypto::decrypt_json(&encrypted, key)?,
        bytes,
        fresh_until,
    ))
}
async fn transaction(conn: &mut PgConnection) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
        .fetch_one(conn)
        .await?)
}
async fn require_transaction(conn: &mut PgConnection, expected: &str) -> Result<()> {
    if transaction(conn).await? != expected {
        return Err(denied());
    }
    Ok(())
}
fn bounded(value: &Value) -> Result<()> {
    if serde_json::to_vec(value)?.len() > MAX_BYTES {
        return Err(denied());
    }
    Ok(())
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| denied())
}
fn denied() -> Error {
    Error::PermissionDenied(
        "Native snapshot source, authority, task or transaction does not match".into(),
    )
}

#[cfg(test)]
mod cache_reader_policy_tests {
    use super::*;

    #[test]
    fn original_task_flag_distinguishes_unlimited_freshness_from_default_archive_reads() {
        let default: crate::workflow::parser::IterateCacheConfig =
            serde_json::from_value(json!({"namespace":"original"})).unwrap();
        let required: crate::workflow::parser::IterateCacheConfig =
            serde_json::from_value(json!({"namespace":"original","require_fresh":true})).unwrap();
        let archived = NativeCacheReadPolicy {
            require_fresh: default.require_fresh,
            fresh_until: None,
        };
        let unlimited = NativeCacheReadPolicy {
            require_fresh: required.require_fresh,
            fresh_until: None,
        };
        assert!(!archived.require_fresh());
        assert!(unlimited.require_fresh());
        assert_eq!(archived.fresh_until(), unlimited.fresh_until());
        archived.ensure_fresh().unwrap();
        unlimited.ensure_fresh().unwrap();
    }

    #[test]
    fn held_original_deadline_expires_at_final_handoff_without_relookup() {
        let deadline = chrono::Utc::now();
        let held = NativeCacheReadPolicy {
            require_fresh: true,
            fresh_until: Some(deadline),
        };
        held.check_at(deadline - chrono::Duration::nanoseconds(1))
            .unwrap();
        assert!(held.check_at(deadline).is_err());
        assert!(held
            .check_at(deadline + chrono::Duration::seconds(1))
            .is_err());
        assert!(held.ensure_fresh().is_err());
    }
}
