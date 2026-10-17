//! Purpose-qualified receipt dataflow. Sessions, evaluated contexts and receipt
//! contexts have private fields and cannot be deserialized from API claims.
use super::native_input_snapshot::IssuedWaitRead;
use super::{
    declared_input::{DeclaredInputPermit, DeclaredInputRepository},
    runtime_authority::{ApprovalEffectFence, RuntimeAdmissionRecord, RuntimeAuthorityRepository},
    runtime_target::{RuntimeTarget, RuntimeTargetRepository},
};
use crate::{
    models::Id,
    secret_provenance::{SecretOrigin, SecretProvenance},
    value_schema::digest,
    workflow::expression::{
        eval_selected, eval_selected_async, parse_expression, AsyncEvalContext, EvalContext,
        EvalError, EvalResult, SelectedField,
    },
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgConnection;
mod cache_control;
mod iteration_control;
mod iteration_outcome;
pub use iteration_control::PreparedIterationControl;
mod native_result_view;
mod native_values;
mod success_metadata;
pub use success_metadata::PreparedNativeSuccessDestination;
mod pending_result;
pub(crate) use cache_control::CacheControlPublication;
pub use native_result_view::{NativeResultViewBinding, PreparedNativeValueDestination};
use native_values::NativeValueRecipe;
pub use native_values::PreparedNativeRecipeDestination;
pub use pending_result::{
    NativeResultStageBinding, PreparedNativeResultCondition, PreparedNativeResultStage,
};
use std::{collections::BTreeSet, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEntityKind {
    ExecutionConfig,
    ExecutionResult,
    WorkflowVariables,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSitePurpose {
    Value,
    Condition,
    Control,
}
impl InputSitePurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Condition => "condition",
            Self::Control => "control",
        }
    }
}
impl InputEntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExecutionConfig => "execution_config",
            Self::ExecutionResult => "execution_result",
            Self::WorkflowVariables => "workflow_variables",
        }
    }
    pub(crate) fn origin_kind(self, purpose: InputSitePurpose) -> String {
        format!("declared_input:{}:{}", self.as_str(), purpose.as_str())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSite {
    pub execution: Id,
    pub kind: InputEntityKind,
    pub purpose: InputSitePurpose,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionSource {
    NativeIterationOutcome {
        snapshot: Id,
    },
    NativeSnapshot {
        snapshot: Id,
    },
    Iteration {
        receipt: Id,
        site: InputSite,
        ordinal: usize,
        batch_size: Option<usize>,
    },
    System {
        admission: Id,
        execution: Id,
        created: chrono::DateTime<chrono::Utc>,
    },
    Control {
        receipt: Id,
        site: InputSite,
    },
    NativeOutcome {
        admission: Id,
        execution: Id,
        status: String,
        timed_out: bool,
    },
    Authored {
        origins: Vec<SecretOrigin>,
        target: Option<crate::runtime_authority::BoundRuntimeTarget>,
        input_authority: Option<Id>,
    },
    Inherited {
        receipt: Id,
        site: InputSite,
    },
    PublicParameter {
        site: InputSite,
        admission: Id,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSelection {
    pub context_path: String,
    pub source: SelectionSource,
}

struct BoundValue {
    path: String,
    value: Value,
    source: SelectionSource,
}
pub struct RenderSession {
    result_reads: Vec<super::native_input_snapshot::IssuedResultRead>,
    wait_reads: Vec<IssuedWaitRead>,
    transaction: String,
    permit: DeclaredInputPermit,
    values: Vec<BoundValue>,
    fence: ApprovalEffectFence,
    purpose: InputSitePurpose,
    controls: Vec<ReadSelection>,
}
pub struct RenderedReceiptContext {
    result_reads: Vec<super::native_input_snapshot::IssuedResultRead>,
    wait_reads: Vec<IssuedWaitRead>,
    native: Option<NativeValueRecipe>,
    transaction: String,
    renderer: RuntimeAdmissionRecord,
    declaration: Option<Id>,
    literal: Option<LiteralLeaf>,
    value: Value,
    selections: Vec<ReadSelection>,
    fence: ApprovalEffectFence,
    purpose: InputSitePurpose,
}
impl RenderedReceiptContext {
    /// Read-only summary of this actual render's held fence, not authority to
    /// issue a proof from a supplied timestamp or to omit independent owners.
    pub fn approval_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.fence.approval_deadline()
    }
    pub(crate) fn issued_wait_reads(&self) -> &[IssuedWaitRead] {
        &self.wait_reads
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn selections(&self) -> &[ReadSelection] {
        &self.selections
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
}
pub struct ReceiptContext {
    // Issued only by the authenticated graph reader in this exact transaction.
    // Accounting consumes these facts, not another authorization traversal.
    graph_accounting: Option<std::sync::Arc<ReceiptGraphAccounting>>,
    result_reads: Vec<super::native_input_snapshot::IssuedResultRead>,
    wait_reads: Vec<IssuedWaitRead>,
    source_receipts: Vec<ReceiptSourceDescriptor>,
    native_sources: Vec<super::native_input_snapshot::NativeSnapshotMetadata>,
    newly_recorded: bool,
    transaction: String,
    id: Id,
    site: InputSite,
    value: Value,
    origins: Vec<SecretOrigin>,
    public_sources: Vec<InputSite>,
    outcomes: Vec<OutcomeMetadata>,
    fence: ApprovalEffectFence,
}
struct ReceiptGraphAccounting {
    bytes: usize,
    nodes: BTreeSet<(bool, Id)>,
    key_hash: String,
}
/// An original source reached by the authenticated receipt graph. This is not
/// a current destination-field claim, a reader permission or an API capability.
#[derive(Clone)]
pub struct ReceiptSourceDescriptor {
    transaction: String,
    retained_by: Id,
    retaining_site: InputSite,
    receipt: Id,
    site: InputSite,
    fence: ApprovalEffectFence,
}
impl ReceiptSourceDescriptor {
    pub fn receipt(&self) -> Id {
        self.receipt
    }
    pub fn retained_by(&self) -> Id {
        self.retained_by
    }
    pub fn site(&self) -> &InputSite {
        &self.site
    }
    pub fn origin(&self) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: self.site.kind.origin_kind(self.site.purpose),
            entity_id: self.receipt,
            path: self.site.path.clone(),
        }
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
}
pub struct PreparedInputSource {
    renderer: Id,
    receipt: ReceiptContext,
    controls: Vec<ReceiptContext>,
}
/// Prospective same-advance variable value, issued only from a canonical render.
/// It cannot become a downstream receipt until the actual variable is published.
pub struct PreparedStagedValue {
    receipt: ReceiptContext,
}
impl PreparedStagedValue {
    pub fn value(&self) -> &Value {
        &self.receipt.value
    }
    pub fn provenance(&self) -> SecretProvenance {
        self.receipt.provenance()
    }
    pub fn site(&self) -> &InputSite {
        &self.receipt.site
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.receipt.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.receipt.ensure_fresh()
    }
}
pub struct PreparedWorkflowOutcome {
    transaction: String,
    renderer: Id,
    source: RuntimeAdmissionRecord,
    status: String,
    timed_out: bool,
    task_name: String,
    fence: ApprovalEffectFence,
}
pub struct PreparedSystemSnapshot {
    transaction: String,
    admission: Id,
    execution: Id,
    created: chrono::DateTime<chrono::Utc>,
    value: Value,
    fence: ApprovalEffectFence,
}
pub struct PreparedIterationItem {
    renderer: Id,
    receipt: ReceiptContext,
    ordinal: usize,
    batch_size: Option<usize>,
    value: Value,
}
impl PreparedIterationItem {
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn index(&self) -> usize {
        self.ordinal
    }
    pub fn source_site(&self) -> &InputSite {
        &self.receipt.site
    }
}
impl PreparedSystemSnapshot {
    pub fn value(&self) -> &Value {
        &self.value
    }
}
impl PreparedWorkflowOutcome {
    pub(crate) fn approval_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.fence.approval_deadline()
    }

    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if self.transaction != transaction(conn).await? {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub(crate) fn require_iteration_observation(
        &self,
        transaction: &str,
        renderer: Id,
        execution: Id,
        task: &str,
        status: &str,
    ) -> Result<(&RuntimeAdmissionRecord, bool, &ApprovalEffectFence)> {
        self.fence.ensure_fresh()?;
        if self.transaction != transaction
            || self.renderer != renderer
            || self.source.owner_kind != "execution"
            || self.source.owner_id != execution
            || self.task_name != task
            || self.status != status
        {
            return Err(denied());
        }
        Ok((&self.source, self.timed_out, &self.fence))
    }
    pub fn execution(&self) -> Id {
        self.source.owner_id
    }
    pub fn task_name(&self) -> &str {
        &self.task_name
    }
    pub fn value(&self, function: &str) -> Option<Value> {
        outcome_value(&self.status, self.timed_out, function)
    }
}
pub struct PreparedReceiptDestination {
    transaction: String,
    result_reads: Vec<super::native_input_snapshot::IssuedResultRead>,
    wait_reads: Vec<IssuedWaitRead>,
    source_receipts: Vec<ReceiptSourceDescriptor>,
    snapshot_parents: Vec<Id>,
    native_sources: Vec<super::native_input_snapshot::NativeSnapshotMetadata>,
    renderer: Id,
    render_digest: String,
    site: InputSite,
    admission: Id,
    origins: Vec<SecretOrigin>,
    public_sources: Vec<InputSite>,
    outcomes: Vec<OutcomeMetadata>,
    fence: ApprovalEffectFence,
}
impl PreparedReceiptDestination {
    /// Complete retained destination closure, not merely its rendered value or
    /// one receipt date. No source/transaction/field authority is reissued.
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        check_site(&self.site)?;
        if self.transaction.is_empty()
            || self.renderer <= 0
            || self.admission <= 0
            || self.render_digest.is_empty()
            || self
                .source_receipts
                .iter()
                .any(|source| source.transaction != self.transaction)
        {
            return Err(denied());
        }
        self.fence.ensure_fresh()?;
        for source in &self.source_receipts {
            source.ensure_fresh()?;
        }
        // All source/capability fences were intersected by the canonical source
        // walker. Retain every independently held receipt fence as well.
        Ok(combined_approval_deadline(
            &self.fence,
            self.source_receipts.iter().map(|source| &source.fence),
        ))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeMetadata {
    execution: Id,
    admission: Id,
    status: String,
    timed_out: bool,
}
impl OutcomeMetadata {
    pub fn execution(&self) -> Id {
        self.execution
    }
    pub fn admission(&self) -> Id {
        self.admission
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }
}
pub struct InputReceiptParents {
    native_snapshots: Vec<Id>,
    policies: Vec<Id>,
    approvals: Vec<Id>,
    receipts: Vec<Id>,
    admissions: Vec<Id>,
    accepted_inputs: Vec<Id>,
    origins: Vec<SecretOrigin>,
    bytes: usize,
}
impl InputReceiptParents {
    pub fn native_snapshots(&self) -> &[Id] {
        &self.native_snapshots
    }
    pub fn policies(&self) -> &[Id] {
        &self.policies
    }
    pub fn approvals(&self) -> &[Id] {
        &self.approvals
    }
    pub fn receipts(&self) -> &[Id] {
        &self.receipts
    }
    pub fn admissions(&self) -> &[Id] {
        &self.admissions
    }
    pub fn accepted_inputs(&self) -> &[Id] {
        &self.accepted_inputs
    }
    pub fn origins(&self) -> &[SecretOrigin] {
        &self.origins
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}
impl PreparedInputSource {
    /// Check every held decision/source fence without reopening receipt data.
    pub fn ensure_fresh(&self) -> Result<()> {
        self.receipt.ensure_fresh()?;
        for control in &self.controls {
            control.ensure_fresh()?;
        }
        Ok(())
    }
    /// Same-transaction deadline checks only, including standalone decisions.
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.receipt.ensure_fresh_on(conn).await?;
        for control in &self.controls {
            control.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    fn issued_result_reads(&self) -> Result<Vec<super::native_input_snapshot::IssuedResultRead>> {
        let mut reads = self.receipt.result_reads.clone();
        for control in &self.controls {
            merge_result_reads(&mut reads, &control.result_reads)?;
        }
        Ok(reads)
    }
    /// Includes every independently attached decision, even false/no-output
    /// controls. The opaque source remains required; this is not a constructor.
    pub fn approval_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        combined_approval_deadline(
            &self.receipt.fence,
            self.controls.iter().map(|control| &control.fence),
        )
    }
    pub(crate) fn issued_wait_reads(&self) -> Result<Vec<IssuedWaitRead>> {
        let mut reads = self.receipt.wait_reads.clone();
        for control in &self.controls {
            merge_wait_reads(&mut reads, &control.wait_reads)?;
        }
        Ok(reads)
    }
    pub fn origin(&self) -> SecretOrigin {
        self.receipt.origin()
    }
    pub fn value(&self) -> &Value {
        &self.receipt.value
    }
    pub fn site(&self) -> &InputSite {
        &self.receipt.site
    }
}
impl ReceiptContext {
    pub(crate) fn issued_wait_reads(&self) -> &[IssuedWaitRead] {
        &self.wait_reads
    }
    pub fn source_receipts(&self) -> &[ReceiptSourceDescriptor] {
        &self.source_receipts
    }
    pub fn source_receipt(&self, origin: &SecretOrigin) -> Result<&ReceiptSourceDescriptor> {
        self.source_receipts
            .iter()
            .find(|source| source.origin() == *origin)
            .ok_or_else(denied)
    }
    fn held_source_descriptor(&self) -> ReceiptSourceDescriptor {
        ReceiptSourceDescriptor {
            transaction: self.transaction.clone(),
            retained_by: self.id,
            retaining_site: self.site.clone(),
            receipt: self.id,
            site: self.site.clone(),
            fence: self.fence.clone(),
        }
    }
    pub(crate) async fn require_declared_control(
        &self,
        conn: &mut PgConnection,
        renderer: Id,
        path: &str,
    ) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        if self.site.kind != InputEntityKind::WorkflowVariables
            || self.site.purpose != InputSitePurpose::Control
            || self.site.path != path
        {
            return Err(denied());
        }
        let (body, expected): (Value, String) =
            sqlx::query_as("SELECT proof,proof_digest FROM runtime_input_receipt WHERE id=$1")
                .bind(self.id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(denied)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let proof: RenderProof = serde_json::from_value(body)?;
        if proof.renderer != renderer
            || proof.site != self.site
            || proof.literal.is_some()
            || proof.accepted.is_some()
        {
            return Err(denied());
        }
        let (body, expected): (Value, String) = sqlx::query_as(
            "SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1",
        )
        .bind(proof.declaration.ok_or_else(denied)?)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let declaration: super::declared_input::DeclaredInputBinding =
            serde_json::from_value(body)?;
        if declaration.source_path != path {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
    fn copy_held(&self) -> Self {
        Self {
            graph_accounting: self.graph_accounting.clone(),
            wait_reads: self.wait_reads.clone(),
            result_reads: self.result_reads.clone(),
            source_receipts: self.source_receipts.clone(),
            native_sources: self.native_sources.clone(),
            newly_recorded: false,
            transaction: self.transaction.clone(),
            id: self.id,
            site: self.site.clone(),
            value: self.value.clone(),
            origins: self.origins.clone(),
            public_sources: self.public_sources.clone(),
            outcomes: self.outcomes.clone(),
            fence: self.fence.clone(),
        }
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub fn public_source_sites(&self) -> &[InputSite] {
        &self.public_sources
    }
    pub fn outcome_metadata(&self) -> &[OutcomeMetadata] {
        &self.outcomes
    }
    pub fn native_source_metadata(
        &self,
    ) -> &[super::native_input_snapshot::NativeSnapshotMetadata] {
        &self.native_sources
    }
    pub(crate) fn accepted_value(&self) -> &Value {
        &self.value
    }
    pub(crate) async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let mut sources = self.source_receipts.clone();
        sources.push(self.held_source_descriptor());
        require_published_sources(conn, &sources).await
    }
    pub(crate) fn accepted_fence(&self) -> &ApprovalEffectFence {
        &self.fence
    }
    pub fn origin(&self) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: self.site.kind.origin_kind(self.site.purpose),
            entity_id: self.id,
            path: self.site.path.clone(),
        }
    }
    pub fn site(&self) -> &InputSite {
        &self.site
    }
    /// Underlying origins are for C's complete disclosure closure, not general
    /// Key rights. Worker input delivery consumes this purpose-qualified proof.
    pub fn source_origins(&self) -> &[SecretOrigin] {
        &self.origins
    }
    pub fn provenance(&self) -> SecretProvenance {
        SecretProvenance {
            origins: vec![self.origin()],
            templates: vec![],
        }
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderProof {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native: Option<NativeValueRecipe>,
    site: InputSite,
    renderer: Id,
    declaration: Option<Id>,
    literal: Option<LiteralLeaf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted: Option<Id>,
    selections: Vec<ReadSelection>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiteralLeaf {
    revision: String,
    path: String,
}

pub struct InheritedInputRepository;
impl InheritedInputRepository {
    /// Source phase only: retain a successful canonical Condition as CONTROL.
    /// Pending VALUE parents remain private preparation evidence, not published
    /// values. The returned opaque source can only be attached as control.
    pub async fn prepare_condition_control(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedInputSource> {
        if rendered.value != Value::Bool(true) {
            return Err(denied());
        }
        Self::prepare_condition_decision(conn, rendered, site, key).await
    }

    /// Retain the actual canonical Boolean decision, including a skipped branch.
    /// False is CONTROL evidence, never authority to evaluate that branch or a
    /// claim that a Boolean workflow variable was published. Keep this token's
    /// fence through the final effect even if no later value carries the decision.
    pub async fn prepare_condition_decision(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedInputSource> {
        require_transaction(conn, &rendered.transaction).await?;
        if rendered.purpose != InputSitePurpose::Condition
            || !rendered.value.is_boolean()
            || site.kind != InputEntityKind::WorkflowVariables
            || site.purpose != InputSitePurpose::Condition
            || site.execution != rendered.renderer.owner_id
            || !transition_condition(&site.path)
        {
            return Err(denied());
        }
        let (body, expected): (Value, String) = sqlx::query_as(
            "SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2",
        )
        .bind(rendered.declaration.ok_or_else(denied)?)
        .bind(rendered.renderer.policy_revision)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let declaration: super::declared_input::DeclaredInputBinding =
            serde_json::from_value(body)?;
        DeclaredInputRepository::validate_declaration(conn, &rendered.renderer, &declaration)
            .await?;
        if declaration.source_path != site.path {
            return Err(denied());
        }
        let prepared = Self::prepare_destination(conn, rendered, site, key).await?;
        let receipt = Self::record_prepared_inner(
            conn,
            rendered,
            &prepared,
            rendered.value(),
            rendered.selections(),
            key,
            true,
        )
        .await?;
        Ok(PreparedInputSource {
            renderer: rendered.renderer.id,
            receipt,
            controls: vec![],
        })
    }
    /// Source phase, before outer workflow/iteration targets. No replacement
    /// value or selected proof can be supplied by the caller.
    pub async fn prepare_staged_value(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedStagedValue> {
        if rendered.purpose != InputSitePurpose::Value
            || site.kind != InputEntityKind::WorkflowVariables
            || site.purpose != InputSitePurpose::Value
            || site.execution != rendered.renderer.owner_id
        {
            return Err(denied());
        }
        let prepared = Self::prepare_destination(conn, rendered, site, key).await?;
        let receipt = Self::record_prepared_inner(
            conn,
            rendered,
            &prepared,
            rendered.value(),
            rendered.selections(),
            key,
            true,
        )
        .await?;
        sqlx::query("INSERT INTO runtime_input_receipt_staged(receipt) VALUES($1)")
            .bind(receipt.id)
            .execute(&mut *conn)
            .await?;
        Ok(PreparedStagedValue { receipt })
    }
    /// Pure binding from the opaque prior render. Never reparse its returned
    /// value or reload the not-yet-published workflow variable.
    pub fn bind_staged_value(
        session: &mut RenderSession,
        path: impl Into<String>,
        staged: &PreparedStagedValue,
    ) -> Result<()> {
        let path = path.into();
        if protected_namespace(&path) {
            return Err(denied());
        }
        let source = &staged.receipt;
        if !source.newly_recorded
            || source.transaction != session.transaction
            || source.site.execution != session.permit.source.owner_id
            || source.site.kind != InputEntityKind::WorkflowVariables
            || source.site.purpose != InputSitePurpose::Value
        {
            return Err(denied());
        }
        source.ensure_fresh()?;
        session.fence = session.fence.intersect(&source.fence);
        merge_wait_reads(&mut session.wait_reads, &source.wait_reads)?;
        merge_result_reads(&mut session.result_reads, &source.result_reads)?;
        insert(
            session,
            path,
            source.value.clone(),
            SelectionSource::Inherited {
                receipt: source.id,
                site: source.site.clone(),
            },
        )
    }
    /// Target phase after writing the ACTUAL workflow variable and its receipts.
    /// Only buffer verification occurs here; no new source authority is acquired.
    pub async fn finalize_staged_value(
        conn: &mut PgConnection,
        staged: &PreparedStagedValue,
        key: &str,
    ) -> Result<()> {
        let source = &staged.receipt;
        require_transaction(conn, &source.transaction).await?;
        source.ensure_fresh_on(conn).await?;
        require_published_sources(conn, &source.source_receipts).await?;
        Self::require_finalized_native_sources(conn, &source.native_sources).await?;
        let (raw, fields) = stored_field(conn, &source.site).await?.ok_or_else(denied)?;
        if (!source.wait_reads.is_empty() || !source.result_reads.is_empty())
            && raw
                .pointer(&source.site.path)
                .is_none_or(|value| !crate::secret_values::is_redaction_marker(value))
        {
            return Err(denied());
        }
        if restore_field(raw, &fields, &source.site, key)? != source.value {
            return Err(denied());
        }
        let mut published = false;
        for field in fields
            .iter()
            .filter(|field| overlaps(&field.json_path, &source.site.path))
        {
            if field.source_kind != "provenance" {
                return Err(denied());
            }
            let provenance: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
            if !source.result_reads.is_empty() && provenance != source.provenance() {
                return Err(denied());
            }
            if !provenance.origins.contains(&source.origin()) {
                return Err(denied());
            }
            published |= field.json_path == source.site.path;
        }
        if !published {
            return Err(denied());
        }
        let updated=sqlx::query("UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL").bind(source.id).execute(&mut *conn).await?;
        if updated.rows_affected() != 1 {
            return Err(denied());
        }
        source.ensure_fresh_on(conn).await
    }
    /// Source entry only. The original persisted receipt is validated at its
    /// authenticated original site. Never rediscover it through the live field,
    /// which may now contain a different first-delivery receipt after sealing.
    pub async fn prepare_receipt_source(
        conn: &mut PgConnection,
        source: &ReceiptSourceDescriptor,
        encryption_key: &str,
    ) -> Result<ReceiptContext> {
        source.ensure_fresh_on(conn).await?;
        let retaining_origin = SecretOrigin::Entity {
            entity_type: source
                .retaining_site
                .kind
                .origin_kind(source.retaining_site.purpose),
            entity_id: source.retained_by,
            path: source.retaining_site.path.clone(),
        };
        let retaining = Self::receipt_context(
            conn,
            &source.retaining_site,
            &retaining_origin,
            encryption_key,
        )
        .await?;
        let reachable = retaining.source_receipt(&source.origin())?;
        if reachable.site != source.site {
            return Err(denied());
        }
        let mut context =
            Self::receipt_context(conn, &source.site, &source.origin(), encryption_key).await?;
        context.fence = context
            .fence
            .intersect(&source.fence)
            .intersect(&retaining.fence);
        for parent in &mut context.source_receipts {
            parent.fence = parent.fence.intersect(&context.fence);
            parent.retained_by = source.retained_by;
            parent.retaining_site = source.retaining_site.clone();
        }
        context.fence.ensure_fresh_on(conn).await?;
        Ok(context)
    }
    pub(crate) async fn receipt_source_site(
        conn: &mut PgConnection,
        origin: &SecretOrigin,
    ) -> Result<(Id, InputSite)> {
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = origin
        else {
            return Err(denied());
        };
        let row: Option<(Value, String)> =
            sqlx::query_as("SELECT proof,proof_digest FROM runtime_input_receipt WHERE id = $1")
                .bind(entity_id)
                .fetch_optional(&mut *conn)
                .await?;
        let (body, expected) = row.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let proof: RenderProof = serde_json::from_value(body)?;
        if entity_type != &proof.site.kind.origin_kind(proof.site.purpose)
            || path != &proof.site.path
        {
            return Err(denied());
        }
        Ok((*entity_id, proof.site))
    }
    /// Direct immutable dependencies for C/F's bounded retention traversal. This
    /// grants no read/delivery authority and does not replace live validation.
    pub async fn retention_parents(
        conn: &mut PgConnection,
        receipt: Id,
    ) -> Result<InputReceiptParents> {
        let row:Option<(Id,Id,Option<Id>,Value,String,Value)>=sqlx::query_as("SELECT destination_admission,renderer_admission,declaration,proof,proof_digest,encrypted_value FROM runtime_input_receipt WHERE id = $1").bind(receipt).fetch_optional(&mut *conn).await?;
        let (destination, renderer, declaration, body, expected, encrypted) =
            row.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let mut bytes = serde_json::to_vec(&body)?.len()
            + serde_json::to_vec(&encrypted)?.len()
            + cache_control::staged_storage_bytes(conn, receipt).await?;
        let proof: RenderProof = serde_json::from_value(body)?;
        check_site(&proof.site)?;
        if proof.renderer != renderer || proof.declaration != declaration {
            return Err(denied());
        }
        if let Some(native) = &proof.native {
            if proof.declaration.is_some() || proof.literal.is_some() || proof.accepted.is_some() {
                return Err(denied());
            }
            let record =
                RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
            native_values::validate_recipe(
                conn,
                &record,
                native,
                &proof.site,
                &proof.selections,
                None,
                false,
            )
            .await?;
        }
        let mut receipts = BTreeSet::new();
        let mut direct = BTreeSet::new();
        let mut origins = Vec::new();
        let mut accepted_inputs = vec![];
        let mut native_snapshots = BTreeSet::new();
        let mut snapshot_kinds = std::collections::BTreeMap::new();
        let mut native_admissions = BTreeSet::new();
        let mut policies = BTreeSet::new();
        let mut approvals = BTreeSet::new();
        for selected in proof.selections {
            let group = matches!(
                &selected.source,
                SelectionSource::NativeIterationOutcome { .. }
            );
            match selected.source {
                SelectionSource::NativeSnapshot { snapshot }
                | SelectionSource::NativeIterationOutcome { snapshot } => {
                    if snapshot_kinds
                        .insert(snapshot, group)
                        .is_some_and(|previous| previous != group)
                    {
                        return Err(denied());
                    }
                    if native_snapshots.insert(snapshot) {
                        let evidence =
                            super::native_input_snapshot::NativeInputSnapshotRepository::retention(
                                conn, snapshot,
                            )
                            .await?;
                        if evidence.renderer != renderer {
                            return Err(denied());
                        }
                        if (evidence.source_kind == "ordinary_iteration_outcome") != group {
                            return Err(denied());
                        }
                        policies.insert(evidence.policy);
                        native_admissions.extend(evidence.admissions);
                        approvals.extend(evidence.approvals);
                        origins.extend(evidence.origins);
                        receipts.extend(evidence.parents);
                        bytes += evidence.bytes;
                    }
                }
                SelectionSource::System {
                    admission,
                    execution,
                    created: _,
                } => {
                    if admission != renderer {
                        return Err(denied());
                    }
                    let owner: Option<(String, Id)> = sqlx::query_as(
                        "SELECT owner_kind,owner_id FROM runtime_authority_admission WHERE id=$1",
                    )
                    .bind(admission)
                    .fetch_optional(&mut *conn)
                    .await?;
                    if owner != Some(("execution".into(), execution)) {
                        return Err(denied());
                    }
                    direct.insert(admission);
                }
                SelectionSource::Authored {
                    origins: source, ..
                } => {
                    for origin in source {
                        if !origins.contains(&origin) {
                            origins.push(origin);
                        }
                    }
                }
                SelectionSource::Inherited {
                    receipt: source,
                    site,
                }
                | SelectionSource::Control {
                    receipt: source,
                    site,
                }
                | SelectionSource::Iteration {
                    receipt: source,
                    site,
                    ..
                } => {
                    if source >= receipt {
                        return Err(denied());
                    }
                    receipts.insert(source);
                    if site.purpose == InputSitePurpose::Value {
                        origins.push(SecretOrigin::Entity {
                            entity_type: site.kind.as_str().into(),
                            entity_id: site.execution,
                            path: site.path,
                        });
                    }
                }
                SelectionSource::PublicParameter { site, admission } => {
                    direct.insert(admission);
                    origins.push(SecretOrigin::Entity {
                        entity_type: site.kind.as_str().into(),
                        entity_id: site.execution,
                        path: site.path,
                    });
                }
                SelectionSource::NativeOutcome {
                    admission,
                    execution,
                    ..
                } => {
                    direct.insert(admission);
                    origins.push(SecretOrigin::Entity {
                        entity_type: InputEntityKind::ExecutionResult.as_str().into(),
                        entity_id: execution,
                        path: String::new(),
                    });
                }
            }
        }
        let actual:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_input_receipt_dependency WHERE receipt = $1 ORDER BY admission").bind(receipt).fetch_all(&mut *conn).await?;
        if actual.into_iter().collect::<BTreeSet<_>>() != direct {
            return Err(denied());
        }
        let accepted: Option<Id> = sqlx::query_scalar(
            "SELECT accepted FROM runtime_input_receipt_accepted WHERE receipt = $1",
        )
        .bind(receipt)
        .fetch_optional(&mut *conn)
        .await?;
        if accepted != proof.accepted {
            return Err(denied());
        }
        if let Some(accepted) = accepted {
            if declaration.is_some()
                || proof.literal.is_some()
                || !receipts.is_empty()
                || !direct.is_empty()
                || destination != renderer
            {
                return Err(denied());
            }
            let (admission, parents, charge, source_receipts) =
                super::accepted_input::AcceptedInputRepository::retention_evidence(
                    conn,
                    accepted,
                    &proof.site,
                )
                .await?;
            if admission != destination {
                return Err(denied());
            }
            for source in source_receipts {
                if source >= receipt {
                    return Err(denied());
                }
                receipts.insert(source);
            }
            origins.extend(parents);
            bytes += charge;
            accepted_inputs.push(accepted);
        }
        let actual:Vec<Id>=sqlx::query_scalar("SELECT source_receipt FROM runtime_input_receipt_source WHERE receipt = $1 ORDER BY source_receipt").bind(receipt).fetch_all(&mut *conn).await?;
        let saved:Vec<Id>=sqlx::query_scalar("SELECT snapshot FROM runtime_input_receipt_snapshot WHERE receipt=$1 ORDER BY snapshot").bind(receipt).fetch_all(&mut *conn).await?;
        if saved.into_iter().collect::<BTreeSet<_>>() != native_snapshots {
            return Err(denied());
        }
        if actual.into_iter().collect::<BTreeSet<_>>() != receipts {
            return Err(denied());
        }
        direct.insert(destination);
        direct.insert(renderer);
        direct.extend(native_admissions);
        bytes += serde_json::to_vec(
            &json!({"receipts":receipts,"admissions":direct,"accepted_inputs":accepted_inputs,"snapshots":native_snapshots,"policies":policies,"approvals":approvals}),
        )?
        .len();
        if receipts.len()
            + direct.len()
            + origins.len()
            + accepted_inputs.len()
            + native_snapshots.len()
            + policies.len()
            + approvals.len()
            > 256
            || bytes > 1024 * 1024
        {
            return Err(denied());
        }
        Ok(InputReceiptParents {
            native_snapshots: native_snapshots.into_iter().collect(),
            policies: policies.into_iter().collect(),
            approvals: approvals.into_iter().collect(),
            receipts: receipts.into_iter().collect(),
            admissions: direct.into_iter().collect(),
            accepted_inputs,
            origins,
            bytes,
        })
    }
    pub(crate) async fn record_accepted(
        conn: &mut PgConnection,
        site: &InputSite,
        evidence: super::accepted_input::AcceptedFieldEvidence,
        key: &str,
    ) -> Result<ReceiptContext> {
        check_site(site)?;
        if site.kind != InputEntityKind::ExecutionConfig
            || site.purpose != InputSitePurpose::Value
            || evidence.admission.owner_kind != "execution"
            || evidence.admission.owner_id != site.execution
        {
            return Err(denied());
        }
        let proof = RenderProof {
            native: None,
            site: site.clone(),
            renderer: evidence.admission.id,
            declaration: None,
            literal: None,
            accepted: Some(evidence.accepted),
            selections: vec![],
        };
        let body = serde_json::to_value(&proof)?;
        bounded(&body)?;
        let expected = hash(&body)?;
        let existing:Option<Id>=sqlx::query_scalar("SELECT receipt.id FROM runtime_input_receipt receipt JOIN runtime_input_receipt_accepted source ON source.receipt = receipt.id WHERE source.accepted = $1 AND receipt.execution = $2 AND receipt.entity_kind = $3 AND receipt.destination_purpose = $4 AND receipt.destination_path = $5 AND receipt.proof_digest = $6 ORDER BY receipt.id LIMIT 1")
            .bind(evidence.accepted).bind(site.execution).bind(site.kind.as_str()).bind(site.purpose.as_str()).bind(&site.path).bind(&expected).fetch_optional(&mut *conn).await?;
        let id = if let Some(id) = existing {
            id
        } else {
            let encrypted = crate::crypto::encrypt_json(&evidence.value, key)?;
            let id:Id=sqlx::query_scalar("INSERT INTO runtime_input_receipt(execution,entity_kind,destination_purpose,destination_path,destination_admission,renderer_admission,declaration,proof,proof_digest,encrypted_value,encryption_key_hash) VALUES ($1,$2,$3,$4,$5,$5,NULL,$6,$7,$8,$9) RETURNING id")
                .bind(site.execution).bind(site.kind.as_str()).bind(site.purpose.as_str()).bind(&site.path).bind(evidence.admission.id).bind(body).bind(expected).bind(encrypted).bind(crate::crypto::hash_encryption_key(key)).fetch_one(&mut *conn).await?;
            sqlx::query(
                "INSERT INTO runtime_input_receipt_accepted(receipt,accepted) VALUES ($1,$2)",
            )
            .bind(id)
            .bind(evidence.accepted)
            .execute(&mut *conn)
            .await?;
            for (parent, _) in &evidence.parents {
                if *parent >= id {
                    return Err(denied());
                }
                sqlx::query("INSERT INTO runtime_input_receipt_source(receipt,source_receipt) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(id).bind(parent).execute(&mut *conn).await?;
            }
            id
        };
        evidence.fence.ensure_fresh_on(conn).await?;
        // Reuse the bounded traversal for accepted parents as well. Their
        // approval deadlines and classifications are part of this source.
        Self::receipt_context(
            conn,
            site,
            &SecretOrigin::Entity {
                entity_type: site.kind.origin_kind(site.purpose),
                entity_id: id,
                path: site.path.clone(),
            },
            key,
        )
        .await
    }
    /// Read native outcome from an actual completed workflow child. No caller
    /// supplied boolean or last-task snapshot can establish this evidence.
    pub async fn prepare_outcome(
        conn: &mut PgConnection,
        renderer: Id,
        child: Id,
    ) -> Result<PreparedWorkflowOutcome> {
        let record = RuntimeAuthorityRepository::execution_admission(conn, renderer)
            .await?
            .ok_or_else(denied)?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let source = RuntimeAuthorityRepository::execution_admission(conn, child)
            .await?
            .ok_or_else(denied)?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, source.id).await?,
        );
        if !related(conn, record.id, source.id).await? {
            return Err(denied());
        }
        let row: Option<(Option<Id>, String, Option<Value>)> = sqlx::query_as(
            "SELECT parent,status::TEXT,workflow_task FROM execution WHERE id = $1 FOR SHARE",
        )
        .bind(child)
        .fetch_optional(&mut *conn)
        .await?;
        let (parent, status, metadata) = row.ok_or_else(denied)?;
        if parent != Some(renderer) || !terminal_status(&status) {
            return Err(denied());
        }
        let metadata: crate::models::WorkflowTaskMetadata =
            serde_json::from_value(metadata.ok_or_else(denied)?)?;
        if !matches!(&source.authority.binding,crate::runtime_authority::InvocationBinding::WorkflowTask {workflow_ref,..} if workflow_ref==record.authority.binding.action_ref())
        {
            return Err(denied());
        }
        let belongs: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE id = $1 AND execution = $2)",
        )
        .bind(metadata.workflow_execution)
        .bind(renderer)
        .fetch_one(&mut *conn)
        .await?;
        if !belongs {
            return Err(denied());
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedWorkflowOutcome {
            transaction: transaction(conn).await?,
            renderer: record.id,
            source,
            status,
            timed_out: metadata.timed_out,
            task_name: metadata.task_name,
            fence,
        })
    }

    pub fn bind_outcome(
        session: &mut RenderSession,
        outcome: &PreparedWorkflowOutcome,
    ) -> Result<()> {
        if session.transaction != outcome.transaction
            || session.permit.source.id != outcome.renderer
        {
            return Err(denied());
        }
        outcome.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&outcome.fence);
        for function in ["succeeded", "failed", "timed_out", "cancelled"] {
            insert(
                session,
                format!("/$outcome/{function}"),
                outcome.value(function).ok_or_else(denied)?,
                SelectionSource::NativeOutcome {
                    admission: outcome.source.id,
                    execution: outcome.source.owner_id,
                    status: outcome.status.clone(),
                    timed_out: outcome.timed_out,
                },
            )?;
        }
        Ok(())
    }

    /// Public fields still come from their actual typed persisted entity. Secret
    /// values use prepare_source/bind_prepared instead. This covers task/result
    /// namespaces and variable aliases without accepting caller-asserted values.
    pub async fn bind_public_field(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        context_path: &str,
        site: &InputSite,
    ) -> Result<()> {
        require_transaction(conn, &session.transaction).await?;
        if site.purpose != InputSitePurpose::Value || protected_namespace(context_path) {
            return Err(denied());
        }
        let source = RuntimeAuthorityRepository::execution_admission(conn, site.execution)
            .await?
            .ok_or_else(denied)?;
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, source.id).await?;
        if !related(conn, source.id, session.permit.source.id).await?
            && !(site.kind == InputEntityKind::ExecutionResult
                && related(conn, session.permit.source.id, source.id).await?)
        {
            return Err(denied());
        }
        let (raw, secrets) = stored_field(conn, site).await?.ok_or_else(denied)?;
        public_schema_site(conn, site, &raw).await?;
        let value = raw.pointer(&site.path).cloned().ok_or_else(denied)?;
        if !secrets.is_empty() || !crate::secret_values::redacted_paths(&value).is_empty() {
            return Err(denied());
        }
        session.fence = session.fence.intersect(&fence);
        insert(
            session,
            context_path.into(),
            value,
            SelectionSource::PublicParameter {
                site: site.clone(),
                admission: source.id,
            },
        )
    }
    /// Independent, explicit consent for ONE retained declaration. This issues a
    /// fixed approval using the issuer's control-plane authority, not the selected
    /// SDK refs. Later reads never inspect the issuer's identity or assignments.
    pub async fn approve_external_input(
        conn: &mut PgConnection,
        renderer: Id,
        revision: &str,
        path: &str,
        expression: &str,
        issuer: Id,
        grants: Vec<crate::rbac::Grant>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Id> {
        let permit =
            DeclaredInputRepository::permit(conn, renderer, revision, path, expression).await?;
        Self::issue_input_approval(
            conn,
            permit.binding_id,
            permit.declaration.pack,
            issuer,
            grants,
            expires_at,
        )
        .await
    }

    pub async fn approve_external_declaration(
        conn: &mut PgConnection,
        policy: Id,
        binding: &crate::runtime_authority::InvocationBinding,
        revision: &str,
        path: &str,
        expression: &str,
        issuer: Id,
        grants: Vec<crate::rbac::Grant>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Id> {
        let (id, declaration) = DeclaredInputRepository::declaration_for_policy(
            conn, policy, binding, revision, path, expression,
        )
        .await?;
        Self::issue_input_approval(conn, id, declaration.pack, issuer, grants, expires_at).await
    }

    async fn issue_input_approval(
        conn: &mut PgConnection,
        declaration: Id,
        pack: Id,
        issuer: Id,
        grants: Vec<crate::rbac::Grant>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Id> {
        if grants.is_empty()
            || grants.iter().any(|grant| {
                !matches!(
                    grant.resource,
                    crate::rbac::Resource::Keys | crate::rbac::Resource::Packs
                ) || grant.actions.is_empty()
                    || grant.actions.iter().any(|action| {
                        !matches!(
                            action,
                            crate::rbac::Action::Read | crate::rbac::Action::Decrypt
                        )
                    })
            })
        {
            return Err(denied());
        }
        let approval =
            RuntimeAuthorityRepository::issue_approval(conn, pack, issuer, grants, expires_at)
                .await?;
        sqlx::query("INSERT INTO runtime_input_only_approval(approval) VALUES ($1)")
            .bind(approval.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO runtime_declared_input_approval(declaration,approval) VALUES ($1,$2)",
        )
        .bind(declaration)
        .bind(approval.id)
        .execute(&mut *conn)
        .await?;
        Ok(approval.id)
    }
    pub async fn begin_render(
        conn: &mut PgConnection,
        renderer: Id,
        revision: &str,
        path: &str,
        expression: &str,
        purpose: InputSitePurpose,
    ) -> Result<RenderSession> {
        let permit =
            DeclaredInputRepository::permit(conn, renderer, revision, path, expression).await?;
        let condition = transition_condition(path);
        if condition != (purpose == InputSitePurpose::Condition) {
            return Err(denied());
        }
        let fence = permit.effect_fence.clone();
        Ok(RenderSession {
            result_reads: Vec::new(),
            wait_reads: Vec::new(),
            transaction: transaction(conn).await?,
            permit,
            values: vec![],
            fence,
            purpose,
            controls: vec![],
        })
    }

    /// Canonical system time is the original execution creation time, never
    /// a fresh wall clock or a caller-provided system map.
    pub async fn prepare_system(
        conn: &mut PgConnection,
        renderer: Id,
    ) -> Result<PreparedSystemSnapshot> {
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let created: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT created FROM execution WHERE id=$1 FOR SHARE")
                .bind(renderer)
                .fetch_one(&mut *conn)
                .await?;
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedSystemSnapshot {
            transaction: transaction(conn).await?,
            admission: record.id,
            execution: renderer,
            created,
            value: json!({"workflow_start":created}),
            fence,
        })
    }

    pub fn bind_prepared_system(
        session: &mut RenderSession,
        system: &PreparedSystemSnapshot,
    ) -> Result<()> {
        if session.transaction != system.transaction
            || session.permit.source.id != system.admission
            || session.permit.source.owner_id != system.execution
        {
            return Err(denied());
        }
        system.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&system.fence);
        insert(
            session,
            "/system".into(),
            system.value.clone(),
            SelectionSource::System {
                admission: system.admission,
                execution: system.execution,
                created: system.created,
            },
        )
    }

    /// Select only from the canonical evaluator's already-produced with_items
    /// array. The complete array receipt retains the original declaration and
    /// every selected contributor/control proof. No expression is evaluated here.
    pub async fn prepare_iteration_item(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        ordinal: usize,
        batch_size: Option<usize>,
        key: &str,
    ) -> Result<PreparedIterationItem> {
        require_transaction(conn, &rendered.transaction).await?;
        if rendered.purpose != InputSitePurpose::Control && rendered.literal.is_none() {
            return Err(denied());
        }
        let path = if let Some(id) = rendered.declaration {
            let (body,expected):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2").bind(id).bind(rendered.renderer.policy_revision).fetch_one(&mut *conn).await?;
            bounded(&body)?;
            if hash(&body)? != expected {
                return Err(denied());
            }
            let declaration: super::declared_input::DeclaredInputBinding =
                serde_json::from_value(body)?;
            declaration.source_path
        } else if let Some(literal) = &rendered.literal {
            literal.path.clone()
        } else {
            return Err(denied());
        };
        if !path.starts_with("/workflow/tasks/") || !path.ends_with("/with_items") {
            return Err(denied());
        }
        if iteration_batch_size(conn, &rendered.renderer, &path).await? != batch_size {
            return Err(denied());
        }
        let value = iteration_value(&rendered.value, ordinal, batch_size)?;
        let site = InputSite {
            execution: rendered.renderer.owner_id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path,
        };
        let source = RenderedReceiptContext {
            result_reads: rendered.result_reads.clone(),
            wait_reads: rendered.wait_reads.clone(),
            native: rendered.native.clone(),
            transaction: rendered.transaction.clone(),
            renderer: rendered.renderer.clone(),
            declaration: rendered.declaration,
            literal: rendered.literal.clone(),
            value: rendered.value.clone(),
            selections: rendered.selections.clone(),
            fence: rendered.fence.clone(),
            purpose: InputSitePurpose::Control,
        };
        let receipt =
            Self::record_render(conn, &source, &site, &source.value, &source.selections, key)
                .await?;
        Ok(PreparedIterationItem {
            renderer: rendered.renderer.id,
            receipt,
            ordinal,
            batch_size,
            value,
        })
    }

    pub fn bind_iteration_item(
        session: &mut RenderSession,
        item: &PreparedIterationItem,
    ) -> Result<()> {
        if session.transaction != item.receipt.transaction
            || session.permit.source.id != item.renderer
            || !iteration_task_scope(
                &item.receipt.site.path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        item.receipt.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&item.receipt.fence);
        merge_wait_reads(&mut session.wait_reads, &item.receipt.wait_reads)?;
        merge_result_reads(&mut session.result_reads, &item.receipt.result_reads)?;
        let source = SelectionSource::Iteration {
            receipt: item.receipt.id,
            site: item.receipt.site.clone(),
            ordinal: item.ordinal,
            batch_size: item.batch_size,
        };
        insert(session, "/item".into(), item.value.clone(), source.clone())?;
        insert(session, "/index".into(), json!(item.ordinal), source)
    }

    pub fn bind_cache_input(
        session: &mut RenderSession,
        cache: &super::native_input_snapshot::PreparedCacheInput,
    ) -> Result<()> {
        if cache.count() == 0
            || session.transaction != cache.transaction
            || session.permit.source.id != cache.renderer
            || !super::native_input_snapshot::in_task(
                &cache.task_path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        cache.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&cache.fence);
        let source = SelectionSource::NativeSnapshot {
            snapshot: cache.snapshot,
        };
        insert(session, "/item".into(), cache.value.clone(), source.clone())?;
        insert(session, "/index".into(), json!(cache.index()), source)
    }

    pub fn bind_wait_input(
        session: &mut RenderSession,
        wait: &super::native_input_snapshot::ReleasedWaitInput,
    ) -> Result<()> {
        let (transaction, renderer, snapshot, task_path, context_path, value, fence) =
            wait.binding();
        if session.transaction != transaction
            || session.permit.source.id != renderer
            || !super::native_input_snapshot::in_task(
                task_path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        fence.ensure_fresh()?;
        session.fence = session.fence.intersect(fence);
        insert(
            session,
            context_path.into(),
            value.clone(),
            SelectionSource::NativeSnapshot { snapshot },
        )
    }
    /// Authenticate outcome functions from the actual prepared native wait.
    /// Publication still requires release_wait in the matching transaction.
    pub fn bind_prepared_wait_outcome(
        session: &mut RenderSession,
        wait: &super::native_input_snapshot::PreparedWaitInput,
    ) -> Result<()> {
        Self::bind_native_wait_outcome(session, wait.outcome_binding()?)?;
        merge_wait_reads(&mut session.wait_reads, &wait.issued_reads()?)
    }

    /// Replay uses the original immutable classification and source edges.
    pub fn bind_wait_outcome(
        session: &mut RenderSession,
        wait: &super::native_input_snapshot::ReleasedWaitInput,
    ) -> Result<()> {
        Self::bind_native_wait_outcome(session, wait.outcome_binding()?)
    }

    fn bind_native_wait_outcome(
        session: &mut RenderSession,
        binding: super::native_input_snapshot::NativeWaitOutcomeBinding<'_>,
    ) -> Result<()> {
        if session.purpose != InputSitePurpose::Condition
            || session.transaction != binding.transaction
            || session.permit.source.id != binding.renderer
            || !super::native_input_snapshot::wait_outcome_path(
                binding.task_path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        binding.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(binding.fence);
        for (function, value) in ["succeeded", "failed", "timed_out", "cancelled"]
            .into_iter()
            .zip(binding.values)
        {
            insert(
                session,
                format!("/$outcome/{function}"),
                value,
                SelectionSource::NativeSnapshot {
                    snapshot: binding.snapshot,
                },
            )?;
        }
        Ok(())
    }

    pub fn bind_prepared_wait_input(
        session: &mut RenderSession,
        wait: &super::native_input_snapshot::PreparedWaitInput,
    ) -> Result<()> {
        let (transaction, renderer, snapshot, task_path, context_path, value, fence) =
            wait.binding();
        if session.transaction != transaction
            || session.permit.source.id != renderer
            || !super::native_input_snapshot::in_task(
                task_path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        fence.ensure_fresh()?;
        session.fence = session.fence.intersect(fence);
        merge_wait_reads(&mut session.wait_reads, &wait.issued_reads()?)?;
        insert(
            session,
            context_path.into(),
            value.clone(),
            SelectionSource::NativeSnapshot { snapshot },
        )
    }

    /// Read the exact Key once. The selected immutable owner/origin and value
    /// remain in this session; persistence and propagation never resolve it again.
    pub async fn bind_key(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        reference: &str,
        encryption_key: &str,
    ) -> Result<()> {
        require_transaction(conn, &session.transaction).await?;
        if !session.permit.declaration.uses_keystore() {
            return Err(denied());
        }
        let key = super::key::KeyRepository::find_by_ref_for_share(conn, reference)
            .await?
            .ok_or_else(denied)?;
        let origin = crate::key_access::key_origin(&key);
        let target = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Key(key.id),
            session.permit.source.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let input_authority = if session
            .permit
            .allows_target(&target, crate::rbac::Action::Read)
            && (!key.encrypted
                || session
                    .permit
                    .allows_target(&target, crate::rbac::Action::Decrypt))
        {
            None
        } else {
            let approvals:Vec<Id>=sqlx::query_scalar("SELECT binding.approval FROM runtime_declared_input_approval binding JOIN runtime_external_approval approval ON approval.id = binding.approval WHERE binding.declaration = $1 AND approval.revoked_at IS NULL AND (approval.expires_at IS NULL OR approval.expires_at > clock_timestamp()) ORDER BY binding.approval").bind(session.permit.binding_id).fetch_all(&mut *conn).await?;
            let mut selected = None;
            for approval in approvals {
                if let Some(fence) = input_approval(
                    conn,
                    session.permit.binding_id,
                    session.permit.declaration.pack,
                    approval,
                    &target,
                    key.id,
                    key.encrypted,
                )
                .await?
                {
                    session.fence = session.fence.intersect(&fence);
                    selected = Some(approval);
                    break;
                }
            }
            let selected = match selected {
                Some(approval) => approval,
                None => {
                    // Keep the expired-candidate denial distinguishable from
                    // an expression/receipt-shape failure. No authority fallback.
                    let all_expired: bool = sqlx::query_scalar(
                        "SELECT count(*) > 0 AND bool_and(approval.expires_at IS NOT NULL AND approval.expires_at <= clock_timestamp()) FROM runtime_declared_input_approval binding JOIN runtime_external_approval approval ON approval.id=binding.approval WHERE binding.declaration=$1 AND approval.revoked_at IS NULL",
                    )
                    .bind(session.permit.binding_id)
                    .fetch_one(&mut *conn)
                    .await?;
                    return Err(if all_expired {
                        Error::PermissionDenied(
                            "Declared-input Key approvals expired during source preparation".into(),
                        )
                    } else {
                        denied()
                    });
                }
            };
            Some(selected)
        };
        let target = crate::runtime_authority::BoundRuntimeTarget {
            kind: "key".into(),
            id: key.id,
            pack_id: target.pack_id(),
            owner_type: Some(
                serde_json::to_value(key.owner_type)?
                    .as_str()
                    .ok_or_else(denied)?
                    .into(),
            ),
            owner_id: target.owner_id(),
        };
        if let Some(approval) = input_authority {
            let origin_body = serde_json::to_value(SecretOrigin::Key(origin.clone()))?;
            sqlx::query("INSERT INTO runtime_declared_input_key_read(declaration,approval,origin_digest,origin,target) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                .bind(session.permit.binding_id).bind(approval).bind(hash(&origin_body)?).bind(origin_body).bind(serde_json::to_value(&target)?).execute(&mut *conn).await?;
        }
        let value = if key.encrypted {
            crate::crypto::decrypt_json(&key.value, encryption_key)?
        } else {
            key.value
        };
        insert(
            session,
            format!("/keystore/{}", escape(reference)),
            value,
            SelectionSource::Authored {
                origins: vec![SecretOrigin::Key(origin)],
                target: Some(target),
                input_authority,
            },
        )
    }

    /// Captures the actual immutable configuration revision at read time. Both
    /// canonical workflow `config` and rule-style `pack.config` share that value.
    pub async fn bind_configuration(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        encryption_key: &str,
    ) -> Result<Id> {
        require_transaction(conn, &session.transaction).await?;
        let (value, revision) = DeclaredInputRepository::pack_configuration(
            conn,
            &session.permit,
            session.permit.declaration.pack,
            Some(encryption_key),
        )
        .await?;
        let source = SelectionSource::Authored {
            origins: vec![SecretOrigin::Entity {
                entity_type: super::component_configuration::ENTITY_COMPONENT_CONFIGURATION.into(),
                entity_id: revision,
                path: String::new(),
            }],
            target: Some(crate::runtime_authority::BoundRuntimeTarget {
                kind: "pack".into(),
                id: session.permit.declaration.pack,
                pack_id: Some(session.permit.declaration.pack),
                owner_type: None,
                owner_id: None,
            }),
            input_authority: None,
        };
        insert(session, "/config".into(), value.clone(), source.clone())?;
        insert(session, "/pack/config".into(), value, source)?;
        Ok(revision)
    }

    /// Supplied PUBLIC parameters are loaded from the renderer's real config.
    /// Protected fields must use bind_source or B's retained contributor path;
    /// arbitrary plaintext/source maps cannot be asserted as authored authority.
    pub async fn bind_public_parameter(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        path: &str,
    ) -> Result<()> {
        require_transaction(conn, &session.transaction).await?;
        let site = InputSite {
            execution: session.permit.source.owner_id,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: path.into(),
        };
        let (raw, secrets) = stored_field(conn, &site).await?.ok_or_else(denied)?;
        public_schema_site(conn, &site, &raw).await?;
        let raw = raw.pointer(&site.path).cloned().ok_or_else(denied)?;
        if !secrets.is_empty() || !crate::secret_values::redacted_paths(&raw).is_empty() {
            return Err(denied());
        }
        let admission = session.permit.source.id;
        insert(
            session,
            format!("/parameters{path}"),
            raw,
            SelectionSource::PublicParameter { site, admission },
        )
    }

    /// The original exact typed field receipt is validated, not a whole-execution
    /// permission. The alias is the actual evaluation context slot; the evaluator
    /// below computes the selected path map itself, including dynamic selectors.
    pub async fn bind_source(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        context_path: &str,
        site: &InputSite,
        proof: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<()> {
        require_transaction(conn, &session.transaction).await?;
        if site.purpose != InputSitePurpose::Value {
            return Err(denied());
        }
        if protected_namespace(context_path) {
            return Err(denied());
        }
        let source = Self::receipt_context(conn, site, proof, encryption_key).await?;
        let source_admission =
            RuntimeAuthorityRepository::execution_admission(conn, site.execution)
                .await?
                .ok_or_else(denied)?;
        // Inputs/variables flow down admitted lineage. A task's explicit Result
        // can flow back to its containing renderer; its receipt keeps that task's
        // independent fence without making cyclic native admission dependencies.
        if !related(conn, source_admission.id, session.permit.source.id).await?
            && !(site.kind == InputEntityKind::ExecutionResult
                && related(conn, session.permit.source.id, source_admission.id).await?)
        {
            return Err(denied());
        }
        if let Some((actual, secrets)) = stored_field(conn, site).await? {
            let restored = restore_field(actual, &secrets, site, encryption_key)?;
            if restored != source.value {
                return Err(denied());
            }
            if !secrets.is_empty()
                && !secrets.iter().any(|secret| {
                    stored_provenance(secret)
                        .is_ok_and(|provenance| provenance.origins.contains(proof))
                })
                && !accepted_receipt(conn, source.id).await?
            {
                return Err(denied());
            }
        }
        session.fence = session.fence.intersect(&source.fence);
        insert(
            session,
            context_path.into(),
            source.value,
            SelectionSource::Inherited {
                receipt: source.id,
                site: site.clone(),
            },
        )
    }

    /// Prepare before target/workflow/capture locks. Later binding is pure and
    /// compares the renderer loader's actual selected value to this proof.
    pub async fn prepare_source(
        conn: &mut PgConnection,
        renderer: Id,
        site: &InputSite,
        proof: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<PreparedInputSource> {
        let record = RuntimeAuthorityRepository::execution_admission(conn, renderer)
            .await?
            .ok_or_else(denied)?;
        RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?;
        let receipt = Self::receipt_context(conn, site, proof, encryption_key).await?;
        let source: Id = sqlx::query_scalar(
            "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
        )
        .bind(receipt.id)
        .fetch_one(&mut *conn)
        .await?;
        if !related(conn, source, record.id).await?
            && !(site.kind == InputEntityKind::ExecutionResult
                && related(conn, record.id, source).await?)
        {
            return Err(denied());
        }
        if site.purpose == InputSitePurpose::Value {
            if let Some((actual, secrets)) = stored_field(conn, site).await? {
                if restore_field(actual, &secrets, site, encryption_key)? != receipt.value {
                    return Err(denied());
                }
                if !secrets.is_empty()
                    && !secrets.iter().any(|secret| {
                        stored_provenance(secret).is_ok_and(|value| value.origins.contains(proof))
                    })
                    && !accepted_receipt(conn, receipt.id).await?
                {
                    return Err(denied());
                }
            }
        }
        Ok(PreparedInputSource {
            renderer: record.id,
            receipt,
            controls: vec![],
        })
    }

    /// Same-transaction self publication only. The opaque receipt must have
    /// been returned by record_prepared, and the actual protected destination
    /// field must now contain that receipt. This takes no source/graph locks.
    pub async fn prepare_recorded_source(
        conn: &mut PgConnection,
        renderer: Id,
        receipt: &ReceiptContext,
        encryption_key: &str,
    ) -> Result<PreparedInputSource> {
        Self::prepare_recorded_source_with_contexts(conn, renderer, receipt, &[], encryption_key)
            .await
    }

    pub async fn prepare_recorded_source_with_contexts(
        conn: &mut PgConnection,
        renderer: Id,
        receipt: &ReceiptContext,
        additional: &[&ReceiptContext],
        encryption_key: &str,
    ) -> Result<PreparedInputSource> {
        require_transaction(conn, &receipt.transaction).await?;
        if !receipt.newly_recorded || receipt.site.execution != renderer {
            return Err(denied());
        }
        if additional.len() > 256 {
            return Err(denied());
        }
        let mut permitted = receipt.origins.clone();
        permitted.push(receipt.origin());
        let mut fence = receipt.fence.clone();
        for source in additional {
            if source.transaction != receipt.transaction || source.site.execution != renderer {
                return Err(denied());
            }
            source.fence.ensure_fresh()?;
            fence = fence.intersect(&source.fence);
            permitted.push(source.origin());
            permitted.extend(source.origins.iter().cloned());
        }
        let row:Option<(Id,Value,String,Value,String)>=sqlx::query_as("SELECT destination_admission,proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id = $1")
            .bind(receipt.id).fetch_optional(&mut *conn).await?;
        let (admission, body, expected, encrypted, key_hash) = row.ok_or_else(denied)?;
        if hash(&body)? != expected
            || key_hash != crate::crypto::hash_encryption_key(encryption_key)
            || crate::crypto::decrypt_json(&encrypted, encryption_key)? != receipt.value
        {
            return Err(denied());
        }
        let proof: RenderProof = serde_json::from_value(body)?;
        if proof.site != receipt.site {
            return Err(denied());
        }
        if receipt.site.purpose == InputSitePurpose::Value {
            let (actual, secrets) = stored_field(conn, &receipt.site)
                .await?
                .ok_or_else(denied)?;
            if restore_field(actual, &secrets, &receipt.site, encryption_key)? != receipt.value {
                return Err(denied());
            }
            let origin = receipt.origin();
            let mut found = false;
            for secret in &secrets {
                let provenance = stored_provenance(secret)?;
                found |= provenance.origins.contains(&origin);
                if provenance
                    .origins
                    .iter()
                    .any(|source| !permitted.contains(source))
                {
                    return Err(denied());
                }
            }
            if !found {
                return Err(denied());
            }
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedInputSource {
            renderer: admission,
            controls: additional.iter().map(|source| source.copy_held()).collect(),
            receipt: ReceiptContext {
                graph_accounting: receipt.graph_accounting.clone(),
                result_reads: receipt.result_reads.clone(),
                wait_reads: receipt.wait_reads.clone(),
                source_receipts: receipt.source_receipts.clone(),
                native_sources: receipt.native_sources.clone(),
                newly_recorded: false,
                transaction: receipt.transaction.clone(),
                id: receipt.id,
                site: receipt.site.clone(),
                value: receipt.value.clone(),
                origins: receipt.origins.clone(),
                public_sources: receipt.public_sources.clone(),
                outcomes: receipt.outcomes.clone(),
                fence: receipt.fence.clone(),
            },
        })
    }

    pub fn bind_prepared(
        session: &mut RenderSession,
        context_path: &str,
        prepared: &PreparedInputSource,
        actual: &Value,
    ) -> Result<()> {
        if prepared.renderer != session.permit.source.id
            || prepared.receipt.transaction != session.transaction
            || prepared.receipt.site.purpose != InputSitePurpose::Value
            || actual != &prepared.receipt.value
            || protected_namespace(context_path)
        {
            return Err(denied());
        }
        prepared.receipt.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&prepared.receipt.fence);
        merge_wait_reads(&mut session.wait_reads, &prepared.issued_wait_reads()?)?;
        merge_result_reads(&mut session.result_reads, &prepared.issued_result_reads()?)?;
        for source in &prepared.controls {
            source.fence.ensure_fresh()?;
            session.fence = session.fence.intersect(&source.fence);
            session.controls.push(ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::Control {
                    receipt: source.id,
                    site: source.site.clone(),
                },
            });
        }
        if session.controls.len() > 256 {
            return Err(denied());
        }
        insert(
            session,
            context_path.into(),
            actual.clone(),
            SelectionSource::Inherited {
                receipt: prepared.receipt.id,
                site: prepared.receipt.site.clone(),
            },
        )
    }

    pub fn attach_prepared_control(
        session: &mut RenderSession,
        prepared: &PreparedInputSource,
    ) -> Result<()> {
        if prepared.renderer != session.permit.source.id
            || prepared.receipt.transaction != session.transaction
        {
            return Err(denied());
        }
        prepared.receipt.fence.ensure_fresh()?;
        session.fence = session.fence.intersect(&prepared.receipt.fence);
        merge_wait_reads(&mut session.wait_reads, &prepared.issued_wait_reads()?)?;
        merge_result_reads(&mut session.result_reads, &prepared.issued_result_reads()?)?;
        session.controls.push(ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::Control {
                receipt: prepared.receipt.id,
                site: prepared.receipt.site.clone(),
            },
        });
        for source in &prepared.controls {
            source.fence.ensure_fresh()?;
            session.fence = session.fence.intersect(&source.fence);
            session.controls.push(ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::Control {
                    receipt: source.id,
                    site: source.site.clone(),
                },
            });
        }
        if session.controls.len() > 256 {
            return Err(denied());
        }
        Ok(())
    }

    /// A condition's proof is carried as CONTROL, not as a claim that its bool
    /// was the published object/string value. The separately evaluated value
    /// session and its destination shape remain unchanged.
    pub async fn attach_control(
        conn: &mut PgConnection,
        session: &mut RenderSession,
        site: &InputSite,
        proof: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<()> {
        require_transaction(conn, &session.transaction).await?;
        let source = Self::receipt_context(conn, site, proof, encryption_key).await?;
        let source_admission: Id = sqlx::query_scalar(
            "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
        )
        .bind(source.id)
        .fetch_one(&mut *conn)
        .await?;
        if !related(conn, source_admission, session.permit.source.id).await? {
            return Err(denied());
        }
        session.fence = session.fence.intersect(&source.fence);
        session.controls.push(ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::Control {
                receipt: source.id,
                site: site.clone(),
            },
        });
        if session.controls.len() > 256 {
            return Err(denied());
        }
        Ok(())
    }

    /// Evaluates the immutable declaration once using only validated bound values.
    /// Values returned by expressions are never recursively interpreted.
    pub async fn evaluate(
        conn: &mut PgConnection,
        session: RenderSession,
    ) -> Result<RenderedReceiptContext> {
        require_transaction(conn, &session.transaction).await?;
        let context = TraceContext {
            values: &session.values,
        };
        let (value, fields) = render(&session.permit.declaration.expression, &context)?;
        Self::finish_render(conn, session, value, fields).await
    }

    /// Apply the workflow condition's coercion to the single canonical evaluation.
    /// No new lookup or second evaluation is performed, including for properties
    /// whose result type is not known until the expression runs.
    pub async fn evaluate_condition(
        conn: &mut PgConnection,
        session: RenderSession,
    ) -> Result<RenderedReceiptContext> {
        if session.purpose != InputSitePurpose::Condition {
            return Err(denied());
        }
        require_transaction(conn, &session.transaction).await?;
        let (value, fields) = render_condition(
            &session.permit.declaration.expression,
            &TraceContext {
                values: &session.values,
            },
        )?;
        Self::finish_render(conn, session, value, fields).await
    }

    /// Key loads, selected-source capture and truthiness share one evaluator pass.
    pub async fn evaluate_condition_with_keys(
        conn: &mut PgConnection,
        mut session: RenderSession,
        encryption_key: &str,
    ) -> Result<RenderedReceiptContext> {
        if session.purpose != InputSitePurpose::Condition {
            return Err(denied());
        }
        require_transaction(conn, &session.transaction).await?;
        let expression = session.permit.declaration.expression.clone();
        let (value, fields) = {
            let context = LoadingContext {
                state: tokio::sync::Mutex::new((&mut *conn, &mut session)),
                source_error: tokio::sync::Mutex::new(None),
                encryption_key,
            };
            let evaluated = render_condition_async(&expression, &context).await;
            context.finish(evaluated)?
        };
        Self::finish_render(conn, session, value, fields).await
    }

    /// Load each selected Key asynchronously at its member access in the SAME
    /// evaluator pass. Call before native target locks. No selector prepass,
    /// source rescan, namespace enumeration or reevaluation is involved.
    pub async fn evaluate_with_keys(
        conn: &mut PgConnection,
        mut session: RenderSession,
        encryption_key: &str,
    ) -> Result<RenderedReceiptContext> {
        require_transaction(conn, &session.transaction).await?;
        let expression = session.permit.declaration.expression.clone();
        let (value, fields) = {
            let context = LoadingContext {
                state: tokio::sync::Mutex::new((&mut *conn, &mut session)),
                source_error: tokio::sync::Mutex::new(None),
                encryption_key,
            };
            let evaluated = render_async(&expression, &context).await;
            context.finish(evaluated)?
        };
        Self::finish_render(conn, session, value, fields).await
    }

    async fn finish_render(
        conn: &mut PgConnection,
        session: RenderSession,
        value: Value,
        fields: BTreeSet<SelectedField>,
    ) -> Result<RenderedReceiptContext> {
        let mut selections = selected_sources(&session.values, &fields)?;
        selections.extend(session.controls);
        bounded(&value)?;
        if selections.len() > 256 {
            return Err(denied());
        }
        session.fence.ensure_fresh_on(conn).await?;
        Ok(RenderedReceiptContext {
            result_reads: session.result_reads,
            wait_reads: session.wait_reads,
            native: None,
            transaction: session.transaction,
            renderer: session.permit.source,
            declaration: Some(session.permit.binding_id),
            literal: None,
            value,
            selections,
            fence: session.fence,
            purpose: session.purpose,
        })
    }

    /// Nonrecursive literal publication controlled by already evaluated condition
    /// proofs. The literal is loaded from immutable code, never supplied value or
    /// a claim that the condition's boolean produced the published object.
    pub async fn controlled_literal(
        conn: &mut PgConnection,
        renderer: Id,
        revision: &str,
        path: &str,
        controls: &[(InputSite, SecretOrigin)],
        encryption_key: &str,
    ) -> Result<RenderedReceiptContext> {
        if controls.len() > 256 {
            return Err(denied());
        }
        if controls.is_empty() {
            return Self::literal(conn, renderer, revision, path).await;
        }
        if transition_condition(path) {
            return Err(denied());
        }
        let source = DeclaredInputRepository::require_source(conn, renderer).await?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, source.id).await?;
        let leaf = LiteralLeaf {
            revision: revision.into(),
            path: path.into(),
        };
        let value = literal_value(conn, &source, &leaf).await?;
        let mut selections = Vec::new();
        for (site, origin) in controls {
            let control = Self::receipt_context(conn, site, origin, encryption_key).await?;
            let admission: Id = sqlx::query_scalar(
                "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
            )
            .bind(control.id)
            .fetch_one(&mut *conn)
            .await?;
            if !related(conn, admission, source.id).await? {
                return Err(denied());
            }
            fence = fence.intersect(&control.fence);
            selections.push(ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::Control {
                    receipt: control.id,
                    site: site.clone(),
                },
            });
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(RenderedReceiptContext {
            result_reads: Vec::new(),
            wait_reads: Vec::new(),
            native: None,
            transaction: transaction(conn).await?,
            renderer: source,
            declaration: None,
            literal: Some(leaf),
            value,
            selections,
            fence,
            purpose: InputSitePurpose::Value,
        })
    }

    /// An authored literal needs exact immutable-site proof even when no
    /// transition condition executed. It is never supplied caller JSON.
    pub async fn literal(
        conn: &mut PgConnection,
        renderer: Id,
        revision: &str,
        path: &str,
    ) -> Result<RenderedReceiptContext> {
        if transition_condition(path) {
            return Err(denied());
        }
        let source = DeclaredInputRepository::require_source(conn, renderer).await?;
        let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, source.id).await?;
        let leaf = LiteralLeaf {
            revision: revision.into(),
            path: path.into(),
        };
        let value = literal_value(conn, &source, &leaf).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(RenderedReceiptContext {
            result_reads: Vec::new(),
            wait_reads: Vec::new(),
            native: None,
            transaction: transaction(conn).await?,
            renderer: source,
            declaration: None,
            literal: Some(leaf),
            value,
            selections: vec![],
            fence,
            purpose: InputSitePurpose::Value,
        })
    }

    /// Attach actual held control dependencies after choosing a prevalidated
    /// literal/value. It never substitutes the control's value for that value.
    pub fn attach_rendered_control(
        rendered: &mut RenderedReceiptContext,
        source: &PreparedInputSource,
    ) -> Result<()> {
        if source.renderer != rendered.renderer.id
            || source.receipt.transaction != rendered.transaction
        {
            return Err(denied());
        }
        for receipt in std::iter::once(&source.receipt).chain(&source.controls) {
            merge_wait_reads(&mut rendered.wait_reads, &receipt.wait_reads)?;
            merge_result_reads(&mut rendered.result_reads, &receipt.result_reads)?;
            receipt.fence.ensure_fresh()?;
            rendered.fence = rendered.fence.intersect(&receipt.fence);
            let selected = ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::Control {
                    receipt: receipt.id,
                    site: receipt.site.clone(),
                },
            };
            if !rendered.selections.contains(&selected) {
                rendered.selections.push(selected);
            }
        }
        if rendered.selections.len() > 256 {
            return Err(denied());
        }
        Ok(())
    }

    /// Add a just-published/held control to an already prepared destination.
    /// Only control evidence changes; its prevalidated value/site cannot change.
    pub fn attach_prepared_destination_control(
        rendered: &mut RenderedReceiptContext,
        destination: &mut PreparedReceiptDestination,
        source: &PreparedInputSource,
    ) -> Result<()> {
        if destination.renderer != rendered.renderer.id
            || destination.render_digest != render_digest(rendered)?
        {
            return Err(denied());
        }
        Self::attach_rendered_control(rendered, source)?;
        for receipt in std::iter::once(&source.receipt).chain(&source.controls) {
            merge_wait_reads(&mut destination.wait_reads, &receipt.wait_reads)?;
            merge_result_reads(&mut destination.result_reads, &receipt.result_reads)?;
            destination.fence = destination.fence.intersect(&receipt.fence);
            collect_source_receipts(receipt, &mut destination.source_receipts)?;
            for origin in &receipt.origins {
                if !destination.origins.contains(origin) {
                    destination.origins.push(origin.clone());
                }
            }
            if receipt.site.purpose == InputSitePurpose::Value {
                let origin = SecretOrigin::Entity {
                    entity_type: receipt.site.kind.as_str().into(),
                    entity_id: receipt.site.execution,
                    path: receipt.site.path.clone(),
                };
                if !destination.origins.contains(&origin) {
                    destination.origins.push(origin);
                }
            }
            for site in &receipt.public_sources {
                if !destination.public_sources.contains(site) {
                    destination.public_sources.push(site.clone());
                }
            }
            for metadata in &receipt.outcomes {
                if !destination.outcomes.contains(metadata) {
                    destination.outcomes.push(metadata.clone());
                }
            }
            for metadata in &receipt.native_sources {
                if !destination.native_sources.contains(metadata) {
                    destination.native_sources.push(metadata.clone());
                }
            }
        }
        if destination.origins.len() > 256
            || destination.public_sources.len() > 256
            || destination.outcomes.len() > 256
            || destination.native_sources.len() > 256
        {
            return Err(denied());
        }
        destination.render_digest = render_digest(rendered)?;
        Ok(())
    }

    /// Call with the actual renderer result/map before encrypting the destination
    /// using the returned purpose-qualified origin. Both must equal the opaque
    /// evaluated context, so omitted or invented mixed-expression origins deny.
    pub async fn record_render(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        destination: &InputSite,
        actual: &Value,
        selected: &[ReadSelection],
        encryption_key: &str,
    ) -> Result<ReceiptContext> {
        let prepared =
            Self::prepare_destination(conn, rendered, destination, encryption_key).await?;
        Self::record_prepared(conn, rendered, &prepared, actual, selected, encryption_key).await
    }

    /// Acquire source/admission proof before native destination/workflow locks.
    /// The final record_prepared call does inserts and deadline checks only.
    pub async fn prepare_destination(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        destination: &InputSite,
        encryption_key: &str,
    ) -> Result<PreparedReceiptDestination> {
        require_transaction(conn, &rendered.transaction).await?;
        if let Some(recipe) = &rendered.native {
            if rendered.declaration.is_some() || rendered.literal.is_some() {
                return Err(denied());
            }
            native_values::validate_recipe(
                conn,
                &rendered.renderer,
                recipe,
                destination,
                &rendered.selections,
                Some((&rendered.value, encryption_key)),
                true,
            )
            .await?;
        }
        check_site(destination)?;
        if destination.purpose != rendered.purpose {
            return Err(denied());
        }
        if destination.purpose == InputSitePurpose::Condition && !rendered.value.is_boolean() {
            return Err(denied());
        }
        let destination_admission =
            RuntimeAuthorityRepository::execution_admission(conn, destination.execution)
                .await?
                .ok_or_else(denied)?;
        let deadline =
            RuntimeAuthorityRepository::retained_admission_fence(conn, destination_admission.id)
                .await?;
        if !related(conn, rendered.renderer.id, destination_admission.id).await? {
            return Err(denied());
        }
        let mut fence = rendered.fence.intersect(&deadline);
        let mut origins = Vec::new();
        let mut public_sources = Vec::new();
        let mut outcomes = Vec::new();
        let mut native_sources = Vec::new();
        let mut snapshot_parents = BTreeSet::new();
        let mut source_receipts = Vec::new();
        let mut admissions = RetainedReceiptAdmissions::default();
        for selected in &rendered.selections {
            let group = matches!(
                &selected.source,
                SelectionSource::NativeIterationOutcome { .. }
            );
            let mut inherited = match &selected.source {
                SelectionSource::NativeSnapshot { snapshot }
                | SelectionSource::NativeIterationOutcome { snapshot } => {
                    let path = if let Some(declaration) = rendered.declaration {
                        let body: Value = sqlx::query_scalar(
                            "SELECT declaration FROM runtime_declared_input_binding WHERE id=$1",
                        )
                        .bind(declaration)
                        .fetch_one(&mut *conn)
                        .await?;
                        let declaration: super::declared_input::DeclaredInputBinding =
                            serde_json::from_value(body)?;
                        declaration.source_path
                    } else if group && rendered.literal.is_some() {
                        rendered.literal.as_ref().ok_or_else(denied)?.path.clone()
                    } else if rendered
                        .native
                        .as_ref()
                        .is_some_and(|native| native.snapshot() == *snapshot)
                    {
                        "$native/value".into()
                    } else {
                        return Err(denied());
                    };
                    let evidence = if let Some(read) = rendered
                        .wait_reads
                        .iter()
                        .find(|read| read.snapshot() == *snapshot)
                    {
                        read.evidence(
                            conn,
                            *snapshot,
                            rendered.renderer.id,
                            &path,
                            &selected.context_path,
                            encryption_key,
                        )
                        .await?
                    } else if let Some(read) = rendered
                        .result_reads
                        .iter()
                        .find(|read| read.snapshot() == *snapshot)
                    {
                        read.evidence(
                            conn,
                            *snapshot,
                            rendered.renderer.id,
                            &path,
                            &selected.context_path,
                            encryption_key,
                        )
                        .await?
                    } else {
                        super::native_input_snapshot::NativeInputSnapshotRepository::evidence(
                            conn,
                            *snapshot,
                            rendered.renderer.id,
                            &path,
                            &selected.context_path,
                            encryption_key,
                            true,
                        )
                        .await?
                    };
                    if (evidence.metadata.source_kind() == "ordinary_iteration_outcome") != group {
                        return Err(denied());
                    }
                    if group
                        && selected.context_path != "$control"
                        && destination.purpose != InputSitePurpose::Condition
                    {
                        return Err(denied());
                    }
                    fence = fence.intersect(&evidence.fence);
                    for (parent, site) in &evidence.parents {
                        let origin = SecretOrigin::Entity {
                            entity_type: site.kind.origin_kind(site.purpose),
                            entity_id: *parent,
                            path: site.path.clone(),
                        };
                        let context = Self::preparation_receipt_context_with_admissions(
                            conn,
                            site,
                            &origin,
                            encryption_key,
                            &rendered.wait_reads,
                            &rendered.result_reads,
                            &mut admissions,
                        )
                        .await?;
                        collect_source_receipts(&context, &mut source_receipts)?;
                        fence = fence.intersect(&context.fence);
                        snapshot_parents.insert(*parent);
                        for origin in context.origins {
                            if !origins.contains(&origin) {
                                origins.push(origin);
                            }
                        }
                        for site in context.public_sources {
                            if !public_sources.contains(&site) {
                                public_sources.push(site);
                            }
                        }
                        for outcome in context.outcomes {
                            if !outcomes.contains(&outcome) {
                                outcomes.push(outcome);
                            }
                        }
                        for native in context.native_sources {
                            if !native_sources.contains(&native) {
                                native_sources.push(native);
                            }
                        }
                    }
                    if !native_sources.contains(&evidence.metadata) {
                        native_sources.push(evidence.metadata);
                    }
                    evidence.origins
                }
                SelectionSource::System {
                    admission,
                    execution,
                    created,
                } => {
                    fence = fence.intersect(
                        &validate_system_source(
                            conn,
                            rendered.renderer.id,
                            *admission,
                            *execution,
                            *created,
                        )
                        .await?,
                    );
                    vec![]
                }
                SelectionSource::NativeOutcome {
                    admission,
                    execution,
                    status,
                    timed_out,
                } => {
                    fence = fence.intersect(
                        &validate_outcome_source(
                            conn,
                            rendered.renderer.id,
                            *admission,
                            *execution,
                            status,
                        )
                        .await?,
                    );
                    let metadata = OutcomeMetadata {
                        execution: *execution,
                        admission: *admission,
                        status: status.clone(),
                        timed_out: *timed_out,
                    };
                    if !outcomes.contains(&metadata) {
                        outcomes.push(metadata);
                    }
                    vec![SecretOrigin::Entity {
                        entity_type: InputEntityKind::ExecutionResult.as_str().into(),
                        entity_id: *execution,
                        path: String::new(),
                    }]
                }
                SelectionSource::Authored { origins, .. } => origins.clone(),
                SelectionSource::Inherited { receipt, site }
                | SelectionSource::Control { receipt, site }
                | SelectionSource::Iteration { receipt, site, .. } => {
                    let proof = SecretOrigin::Entity {
                        entity_type: site.kind.origin_kind(site.purpose),
                        entity_id: *receipt,
                        path: site.path.clone(),
                    };
                    let source = Self::preparation_receipt_context_with_admissions(
                        conn,
                        site,
                        &proof,
                        encryption_key,
                        &rendered.wait_reads,
                        &rendered.result_reads,
                        &mut admissions,
                    )
                    .await?;
                    collect_source_receipts(&source, &mut source_receipts)?;
                    fence = fence.intersect(&source.fence);
                    for site in source.public_sources {
                        if !public_sources.contains(&site) {
                            public_sources.push(site);
                        }
                    }
                    for outcome in source.outcomes {
                        if !outcomes.contains(&outcome) {
                            outcomes.push(outcome);
                        }
                    }
                    for native in source.native_sources {
                        if !native_sources.contains(&native) {
                            native_sources.push(native);
                        }
                    }
                    let mut origins = source.origins;
                    if site.purpose == InputSitePurpose::Value {
                        origins.push(SecretOrigin::Entity {
                            entity_type: site.kind.as_str().into(),
                            entity_id: site.execution,
                            path: site.path.clone(),
                        });
                    }
                    origins
                }
                SelectionSource::PublicParameter { site, .. } => {
                    if !public_sources.contains(site) {
                        public_sources.push(site.clone());
                    }
                    vec![SecretOrigin::Entity {
                        entity_type: site.kind.as_str().into(),
                        entity_id: site.execution,
                        path: site.path.clone(),
                    }]
                }
            };
            for origin in inherited.drain(..) {
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
        }
        if origins.len() > 256
            || public_sources.len() > 256
            || outcomes.len() > 256
            || native_sources.len() > 256
            || snapshot_parents.len() > 256
        {
            return Err(denied());
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedReceiptDestination {
            transaction: rendered.transaction.clone(),
            result_reads: rendered
                .result_reads
                .iter()
                .filter(|read| {
                    native_sources
                        .iter()
                        .any(|source| source.snapshot() == read.snapshot())
                })
                .cloned()
                .collect(),
            wait_reads: rendered
                .issued_wait_reads()
                .iter()
                .filter(|read| {
                    native_sources
                        .iter()
                        .any(|source| source.snapshot() == read.snapshot())
                })
                .cloned()
                .collect(),
            source_receipts,
            snapshot_parents: snapshot_parents.into_iter().collect(),
            native_sources,
            renderer: rendered.renderer.id,
            render_digest: render_digest(rendered)?,
            site: destination.clone(),
            admission: destination_admission.id,
            origins,
            public_sources,
            outcomes,
            fence,
        })
    }

    pub async fn record_prepared(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        prepared: &PreparedReceiptDestination,
        actual: &Value,
        selected: &[ReadSelection],
        encryption_key: &str,
    ) -> Result<ReceiptContext> {
        Self::record_prepared_inner(
            conn,
            rendered,
            prepared,
            actual,
            selected,
            encryption_key,
            false,
        )
        .await
    }
    async fn record_prepared_inner(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        prepared: &PreparedReceiptDestination,
        actual: &Value,
        selected: &[ReadSelection],
        encryption_key: &str,
        staging: bool,
    ) -> Result<ReceiptContext> {
        require_transaction(conn, &rendered.transaction).await?;
        let destination = &prepared.site;
        if prepared.renderer != rendered.renderer.id
            || prepared.transaction != rendered.transaction
            || prepared.render_digest != render_digest(rendered)?
            || actual != &rendered.value
            || selected != rendered.selections
        {
            return Err(denied());
        }
        if !staging {
            require_published_sources(conn, &prepared.source_receipts).await?;
        }
        for native in &prepared.native_sources {
            if staging {
                if let Some(read) = prepared
                    .wait_reads
                    .iter()
                    .find(|read| read.snapshot() == native.snapshot())
                {
                    read.require_stage(
                        conn,
                        native.snapshot(),
                        rendered.renderer.id,
                        encryption_key,
                    )
                    .await?;
                    continue;
                }
                if let Some(read) = prepared
                    .result_reads
                    .iter()
                    .find(|read| read.snapshot() == native.snapshot())
                {
                    read.require_stage(
                        conn,
                        native.snapshot(),
                        rendered.renderer.id,
                        encryption_key,
                    )
                    .await?;
                    continue;
                }
            }
            // Includes transitive native sources reached through Conditions and
            // staged VALUES. Only independently issued Wait or ResultView reads
            // permit private staging; no pending-kind or ID-only exception.
            super::native_input_snapshot::NativeInputSnapshotRepository::require_finalized(
                conn,
                native.snapshot(),
            )
            .await?;
        }
        prepared.fence.ensure_fresh_on(conn).await?;
        rendered.fence.ensure_fresh_on(conn).await?;
        let proof = RenderProof {
            native: rendered.native.clone(),
            site: destination.clone(),
            renderer: rendered.renderer.id,
            declaration: rendered.declaration,
            literal: rendered.literal.clone(),
            accepted: None,
            selections: rendered.selections.clone(),
        };
        let body = serde_json::to_value(&proof)?;
        bounded(&body)?;
        let encrypted = crate::crypto::encrypt_json(actual, encryption_key)?;
        let id:Id = sqlx::query_scalar("INSERT INTO runtime_input_receipt(execution,entity_kind,destination_purpose,destination_path,destination_admission,renderer_admission,declaration,proof,proof_digest,encrypted_value,encryption_key_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id")
            .bind(destination.execution).bind(destination.kind.as_str()).bind(destination.purpose.as_str()).bind(&destination.path).bind(prepared.admission).bind(proof.renderer).bind(proof.declaration).bind(&body).bind(hash(&body)?).bind(encrypted).bind(crate::crypto::hash_encryption_key(encryption_key)).fetch_one(&mut *conn).await?;
        for parent in &prepared.snapshot_parents {
            if *parent >= id {
                return Err(denied());
            }
            sqlx::query("INSERT INTO runtime_input_receipt_source(receipt,source_receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(id).bind(parent).execute(&mut *conn).await?;
        }
        for source in selected {
            if let SelectionSource::NativeSnapshot { snapshot }
            | SelectionSource::NativeIterationOutcome { snapshot } = &source.source
            {
                sqlx::query("INSERT INTO runtime_input_receipt_snapshot(receipt,snapshot) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(id).bind(snapshot).execute(&mut *conn).await?;
            }
            let admission = match &source.source {
                SelectionSource::NativeOutcome { admission, .. }
                | SelectionSource::System { admission, .. }
                | SelectionSource::PublicParameter { admission, .. } => Some(*admission),
                _ => None,
            };
            if let Some(admission) = admission {
                sqlx::query("INSERT INTO runtime_input_receipt_dependency(receipt,admission) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(id).bind(admission).execute(&mut *conn).await?;
            }
            if let SelectionSource::Inherited { receipt, .. }
            | SelectionSource::Control { receipt, .. }
            | SelectionSource::Iteration { receipt, .. } = &source.source
            {
                if *receipt >= id {
                    return Err(denied());
                }
                sqlx::query("INSERT INTO runtime_input_receipt_source(receipt,source_receipt) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                    .bind(id).bind(receipt).execute(&mut *conn).await?;
            }
        }
        for origin in &prepared.origins {
            if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                ..
            } = origin
            {
                if entity_type == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION {
                    sqlx::query("INSERT INTO runtime_input_configuration_reference(receipt,revision) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                        .bind(id).bind(entity_id).execute(&mut *conn).await?;
                }
            }
        }
        let fence = rendered.fence.intersect(&prepared.fence);
        fence.ensure_fresh_on(conn).await?;
        Ok(ReceiptContext {
            graph_accounting: None,
            result_reads: prepared.result_reads.clone(),
            wait_reads: prepared.wait_reads.clone(),
            source_receipts: retained_source_receipts(
                &prepared.source_receipts,
                id,
                destination,
                &fence,
            ),
            native_sources: prepared.native_sources.clone(),
            newly_recorded: true,
            transaction: rendered.transaction.clone(),
            id,
            site: destination.clone(),
            value: actual.clone(),
            origins: prepared.origins.clone(),
            public_sources: prepared.public_sources.clone(),
            outcomes: prepared.outcomes.clone(),
            fence,
        })
    }

    /// C worker/native consumers must pass the ACTUAL entity kind and path. A
    /// freely supplied Entity origin is merely an ID to validate, not authority.
    pub async fn delivery_allowed(
        conn: &mut PgConnection,
        destination: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<bool> {
        Ok(
            Self::prepare_delivery(conn, destination, origin, encryption_key)
                .await?
                .is_some(),
        )
    }

    /// Return the same context that authorized actual stored-value delivery.
    /// Consumers retain its original transaction and fences without reopening
    /// the graph immediately after a successful Boolean authorization check.
    pub(crate) async fn prepare_delivery(
        conn: &mut PgConnection,
        destination: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<Option<ReceiptContext>> {
        let SecretOrigin::Entity { entity_type, .. } = origin else {
            return Ok(None);
        };
        if *entity_type != destination.kind.origin_kind(destination.purpose) {
            return Ok(None);
        }
        let receipt = Self::receipt_context(conn, destination, origin, encryption_key).await?;
        if destination.purpose == InputSitePurpose::Value {
            let (actual, secrets) = stored_field(conn, destination).await?.ok_or_else(denied)?;
            if restore_field(actual, &secrets, destination, encryption_key)? != receipt.value {
                return Err(denied());
            }
        }
        receipt.ensure_fresh_on(conn).await?;
        Ok(Some(receipt))
    }

    pub async fn receipt_context(
        conn: &mut PgConnection,
        site: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<ReceiptContext> {
        let receipt = Self::preparation_receipt_context(conn, site, origin, encryption_key).await?;
        receipt.require_published(conn).await?;
        Ok(receipt)
    }

    // The one authenticated graph reader is shared internally, not a public
    // allow-unpublished switch. Only source-phase composition may read pending
    // ancestry, and the traversal below still requires its issuing transaction.
    // Preserve J's private/public split. An ID-only preparation reader has NO
    // authority to read pending native waits. Only opaque carried reads add it.
    async fn preparation_receipt_context(
        conn: &mut PgConnection,
        site: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
    ) -> Result<ReceiptContext> {
        Self::preparation_receipt_context_with_wait_reads(conn, site, origin, encryption_key, &[])
            .await
    }
    async fn preparation_receipt_context_with_wait_reads(
        conn: &mut PgConnection,
        site: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
        wait_reads: &[IssuedWaitRead],
    ) -> Result<ReceiptContext> {
        Self::preparation_receipt_context_with_admissions(
            conn,
            site,
            origin,
            encryption_key,
            wait_reads,
            &[],
            &mut RetainedReceiptAdmissions::default(),
        )
        .await
    }
    async fn preparation_receipt_context_with_admissions(
        conn: &mut PgConnection,
        site: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
        wait_reads: &[IssuedWaitRead],
        result_reads: &[super::native_input_snapshot::IssuedResultRead],
        admissions: &mut RetainedReceiptAdmissions,
    ) -> Result<ReceiptContext> {
        Self::counted_preparation_receipt_context(
            conn,
            site,
            origin,
            encryption_key,
            wait_reads,
            result_reads,
            admissions,
        )
        .await
        .map(|(context, _, _)| context)
    }

    /// The same authenticated graph traversal also supplies its byte charge.
    /// Pending native sources still require the original opaque issued reads.
    async fn counted_preparation_receipt_context(
        conn: &mut PgConnection,
        site: &InputSite,
        origin: &SecretOrigin,
        encryption_key: &str,
        wait_reads: &[IssuedWaitRead],
        result_reads: &[super::native_input_snapshot::IssuedResultRead],
        admissions: &mut RetainedReceiptAdmissions,
    ) -> Result<(ReceiptContext, usize, BTreeSet<(bool, Id)>)> {
        check_site(site)?;
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = origin
        else {
            return Err(denied());
        };
        if *entity_type != site.kind.origin_kind(site.purpose) || *path != site.path {
            return Err(denied());
        }
        let transaction = transaction(conn).await?;
        admissions.bind_transaction(&transaction)?;
        let mut pending = vec![(*entity_id, Some(site.clone()))];
        let mut visited = std::collections::BTreeMap::new();
        let mut origins = Vec::new();
        let mut public_sources = Vec::new();
        let mut outcomes = Vec::new();
        let mut native_sources = Vec::new();
        let mut snapshot_seen = BTreeSet::new();
        let mut accepted_seen = BTreeSet::new();
        let mut root_value = None;
        let mut fence: Option<ApprovalEffectFence> = None;
        let mut bytes = 0usize;
        while let Some((id, expected_site)) = pending.pop() {
            // Check every incoming site's identity even for an already verified
            // node, but do not reacquire its staged proof for every graph edge.
            // This walk does not mutate receipts or publication markers.
            if let Some(actual) = visited.get(&id) {
                if expected_site
                    .as_ref()
                    .is_some_and(|expected| expected != actual)
                {
                    return Err(denied());
                }
                continue;
            }
            if visited.len() >= 256 {
                return Err(denied());
            }
            let staged:Option<(String,Option<String>)>=sqlx::query_as("SELECT prepared_transaction,publication_transaction FROM runtime_input_receipt_staged WHERE receipt=$1").bind(id).fetch_optional(&mut *conn).await?;
            if let Some((prepared, published)) = staged {
                if published.as_deref() != Some(prepared.as_str()) && transaction != prepared {
                    return Err(denied());
                }
            }
            let row:Option<(Id,String,String,String,Id,Id,Option<Id>,Value,String,Value,String)> = sqlx::query_as("SELECT execution,entity_kind,destination_purpose,destination_path,destination_admission,renderer_admission,declaration,proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id = $1")
                .bind(id).fetch_optional(&mut *conn).await?;
            let (
                execution,
                kind,
                purpose,
                destination_path,
                destination,
                renderer,
                declaration,
                body,
                expected,
                encrypted,
                key_hash,
            ) = row.ok_or_else(denied)?;
            let charge = serde_json::to_vec(&body)?.len()
                + serde_json::to_vec(&encrypted)?.len()
                + cache_control::staged_storage_bytes(conn, id).await?;
            bytes = bytes.checked_add(charge).ok_or_else(denied)?;
            if bytes > 1024 * 1024 {
                return Err(denied());
            }
            if hash(&body)? != expected
                || key_hash != crate::crypto::hash_encryption_key(encryption_key)
            {
                return Err(denied());
            }
            let proof: RenderProof = serde_json::from_value(body)?;
            visited.insert(id, proof.site.clone());
            if expected_site
                .as_ref()
                .is_some_and(|site| *site != proof.site)
                || proof.site.execution != execution
                || proof.site.kind.as_str() != kind
                || proof.site.purpose.as_str() != purpose
                || proof.site.path != destination_path
                || proof.renderer != renderer
                || proof.declaration != declaration
            {
                return Err(denied());
            }
            check_site(&proof.site)?;
            let (renderer_record, renderer_fence) = admissions.get(conn, renderer).await?;
            let (destination_record, destination_fence) = admissions.get(conn, destination).await?;
            if renderer_record.owner_kind != "execution"
                || destination_record.owner_kind != "execution"
                || destination_record.owner_id != execution
                || !related(conn, renderer, destination).await?
            {
                return Err(denied());
            }
            let mut accepted_sources = Vec::new();
            let declaration = if let Some(declaration) = declaration {
                if proof.literal.is_some() || proof.accepted.is_some() || proof.native.is_some() {
                    return Err(denied());
                }
                let (body,expected):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id = $1 AND policy_revision = $2").bind(declaration).bind(renderer_record.policy_revision).fetch_one(&mut *conn).await?;
                if hash(&body)? != expected {
                    return Err(denied());
                }
                let declaration = serde_json::from_value(body)?;
                DeclaredInputRepository::validate_declaration(conn, &renderer_record, &declaration)
                    .await?;
                Some(declaration)
            } else if let Some(accepted) = proof.accepted {
                if proof.literal.is_some() || !proof.selections.is_empty() || proof.native.is_some()
                {
                    return Err(denied());
                }
                if accepted_seen.insert(accepted) {
                    let (_, _, charge, _) =
                        super::accepted_input::AcceptedInputRepository::retention_evidence(
                            conn,
                            accepted,
                            &proof.site,
                        )
                        .await?;
                    bytes = bytes.checked_add(charge).ok_or_else(denied)?;
                    if bytes > 1024 * 1024 {
                        return Err(denied());
                    }
                }
                let evidence = super::accepted_input::AcceptedInputRepository::field(
                    conn,
                    &proof.site,
                    encryption_key,
                )
                .await?;
                if evidence.accepted != accepted
                    || evidence.admission.id != renderer
                    || destination != renderer
                    || evidence.value != crate::crypto::decrypt_json(&encrypted, encryption_key)?
                {
                    return Err(denied());
                }
                for origin in evidence.provenance.origins {
                    if !origins.contains(&origin) {
                        origins.push(origin);
                    }
                }
                accepted_sources = evidence.parents;
                fence = Some(fence.map_or(evidence.fence.clone(), |previous| {
                    previous.intersect(&evidence.fence)
                }));
                None
            } else if let Some(native) = &proof.native {
                if proof.literal.is_some() {
                    return Err(denied());
                }
                native_values::validate_recipe(
                    conn,
                    &renderer_record,
                    native,
                    &proof.site,
                    &proof.selections,
                    Some((
                        &crate::crypto::decrypt_json(&encrypted, encryption_key)?,
                        encryption_key,
                    )),
                    result_reads
                        .iter()
                        .any(|read| read.snapshot() == native.snapshot()),
                )
                .await?;
                None
            } else {
                let leaf = proof.literal.as_ref().ok_or_else(denied)?;
                let value = literal_value(conn, &renderer_record, leaf).await?;
                if crate::crypto::decrypt_json(&encrypted, encryption_key)? != value {
                    return Err(denied());
                }
                None
            };
            let accepted: Option<Id> = sqlx::query_scalar(
                "SELECT accepted FROM runtime_input_receipt_accepted WHERE receipt = $1",
            )
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
            if accepted != proof.accepted {
                return Err(denied());
            }
            let deadline = renderer_fence.intersect(&destination_fence);
            fence = Some(fence.map_or_else(
                || deadline.clone(),
                |previous| previous.intersect(&deadline),
            ));
            let mut expected_sources = BTreeSet::new();
            for (source, site) in accepted_sources {
                if source >= id {
                    return Err(denied());
                }
                let admission: Id = sqlx::query_scalar(
                    "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
                )
                .bind(source)
                .fetch_one(&mut *conn)
                .await?;
                if !related(conn, admission, renderer).await?
                    && !(site.kind == InputEntityKind::ExecutionResult
                        && related(conn, renderer, admission).await?)
                {
                    return Err(denied());
                }
                expected_sources.insert(source);
                pending.push((source, Some(site)));
            }
            let mut expected_admissions = BTreeSet::new();
            let mut expected_snapshots = BTreeSet::new();
            for selected in proof.selections {
                let group = matches!(
                    &selected.source,
                    SelectionSource::NativeIterationOutcome { .. }
                );
                match selected.source {
                    SelectionSource::NativeSnapshot { snapshot }
                    | SelectionSource::NativeIterationOutcome { snapshot } => {
                        let declaration_path = declaration
                            .as_ref()
                            .map(|declaration| declaration.source_path.as_str())
                            .or_else(|| {
                                proof
                                    .literal
                                    .as_ref()
                                    .filter(|_| group)
                                    .map(|literal| literal.path.as_str())
                            })
                            .or_else(|| {
                                proof
                                    .native
                                    .as_ref()
                                    .filter(|native| native.snapshot() == snapshot)
                                    .map(|_| "$native/value")
                            })
                            .ok_or_else(denied)?;
                        let evidence = if let Some(read) =
                            wait_reads.iter().find(|read| read.snapshot() == snapshot)
                        {
                            read.evidence(
                                conn,
                                snapshot,
                                renderer,
                                declaration_path,
                                &selected.context_path,
                                encryption_key,
                            )
                            .await?
                        } else if let Some(read) =
                            result_reads.iter().find(|read| read.snapshot() == snapshot)
                        {
                            read.evidence(
                                conn,
                                snapshot,
                                renderer,
                                declaration_path,
                                &selected.context_path,
                                encryption_key,
                            )
                            .await?
                        } else {
                            super::native_input_snapshot::NativeInputSnapshotRepository::evidence(
                                conn,
                                snapshot,
                                renderer,
                                declaration_path,
                                &selected.context_path,
                                encryption_key,
                                false,
                            )
                            .await?
                        };
                        if (evidence.metadata.source_kind() == "ordinary_iteration_outcome")
                            != group
                        {
                            return Err(denied());
                        }
                        if group
                            && selected.context_path != "$control"
                            && proof.site.purpose != InputSitePurpose::Condition
                        {
                            return Err(denied());
                        }
                        fence = Some(fence.ok_or_else(denied)?.intersect(&evidence.fence));
                        if snapshot_seen.insert(snapshot) {
                            bytes += evidence.bytes;
                            if bytes > 1024 * 1024 {
                                return Err(denied());
                            }
                        }
                        for origin in evidence.origins {
                            if !origins.contains(&origin) {
                                origins.push(origin);
                            }
                        }
                        for (parent, site) in evidence.parents {
                            if parent >= id {
                                return Err(denied());
                            }
                            expected_sources.insert(parent);
                            pending.push((parent, Some(site)));
                        }
                        if !native_sources.contains(&evidence.metadata) {
                            native_sources.push(evidence.metadata);
                        }
                        expected_snapshots.insert(snapshot);
                    }
                    SelectionSource::Iteration {
                        receipt,
                        site: source_site,
                        ordinal,
                        batch_size,
                    } => {
                        if !iteration_task_scope(
                            &source_site.path,
                            &declaration.as_ref().ok_or_else(denied)?.source_path,
                        ) {
                            return Err(denied());
                        }
                        if receipt >= id
                            || source_site.kind != InputEntityKind::WorkflowVariables
                            || source_site.purpose != InputSitePurpose::Control
                            || !source_site.path.starts_with("/workflow/tasks/")
                            || !source_site.path.ends_with("/with_items")
                            || !(selected.context_path == "/index"
                                || selected.context_path == "/item"
                                || selected.context_path.starts_with("/item/"))
                        {
                            return Err(denied());
                        }
                        let parent:Option<(Id,Value,String,Value,String)>=sqlx::query_as("SELECT renderer_admission,proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1").bind(receipt).fetch_optional(&mut *conn).await?;
                        let (source_renderer, body, expected, encrypted, key_hash) =
                            parent.ok_or_else(denied)?;
                        bounded(&body)?;
                        bounded(&encrypted)?;
                        if source_renderer != renderer
                            || hash(&body)? != expected
                            || key_hash != crate::crypto::hash_encryption_key(encryption_key)
                        {
                            return Err(denied());
                        }
                        let parent: RenderProof = serde_json::from_value(body)?;
                        if parent.site != source_site {
                            return Err(denied());
                        }
                        if let Some(declaration) = parent.declaration {
                            let (body,expected):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2").bind(declaration).bind(renderer_record.policy_revision).fetch_one(&mut *conn).await?;
                            bounded(&body)?;
                            if hash(&body)? != expected {
                                return Err(denied());
                            }
                            let declaration: super::declared_input::DeclaredInputBinding =
                                serde_json::from_value(body)?;
                            if declaration.source_path != source_site.path {
                                return Err(denied());
                            }
                        } else if parent
                            .literal
                            .as_ref()
                            .is_none_or(|literal| literal.path != source_site.path)
                        {
                            return Err(denied());
                        }
                        if iteration_batch_size(conn, &renderer_record, &source_site.path).await?
                            != batch_size
                        {
                            return Err(denied());
                        }
                        iteration_value(
                            &crate::crypto::decrypt_json(&encrypted, encryption_key)?,
                            ordinal,
                            batch_size,
                        )?;
                        expected_sources.insert(receipt);
                        pending.push((receipt, Some(source_site)));
                    }
                    SelectionSource::System {
                        admission,
                        execution,
                        created,
                    } => {
                        let deadline =
                            validate_system_source(conn, renderer, admission, execution, created)
                                .await?;
                        fence = Some(fence.ok_or_else(denied)?.intersect(&deadline));
                        expected_admissions.insert(admission);
                    }
                    SelectionSource::NativeOutcome {
                        admission,
                        execution,
                        status,
                        timed_out,
                    } => {
                        let deadline =
                            validate_outcome_source(conn, renderer, admission, execution, &status)
                                .await?;
                        fence = Some(fence.ok_or_else(denied)?.intersect(&deadline));
                        expected_admissions.insert(admission);
                        let metadata = OutcomeMetadata {
                            execution,
                            admission,
                            status,
                            timed_out,
                        };
                        if !outcomes.contains(&metadata) {
                            outcomes.push(metadata);
                        }
                        let origin = SecretOrigin::Entity {
                            entity_type: InputEntityKind::ExecutionResult.as_str().into(),
                            entity_id: execution,
                            path: String::new(),
                        };
                        if !origins.contains(&origin) {
                            origins.push(origin);
                        }
                    }
                    SelectionSource::Authored {
                        origins: actual,
                        target,
                        input_authority,
                    } => {
                        for origin in actual {
                            let declaration = declaration.as_ref().ok_or_else(denied)?;
                            let target = target.as_ref().ok_or_else(denied)?;
                            validate_authored_origin(
                                declaration,
                                &origin,
                                target,
                                input_authority,
                            )?;
                            if let Some(approval) = input_authority {
                                let deadline = retained_input_approval(
                                    conn,
                                    proof.declaration.ok_or_else(denied)?,
                                    declaration.pack,
                                    approval,
                                    &origin,
                                    target,
                                )
                                .await?;
                                fence = Some(fence.ok_or_else(denied)?.intersect(&deadline));
                            }
                            if !origins.contains(&origin) {
                                origins.push(origin);
                            }
                        }
                    }
                    SelectionSource::Inherited {
                        receipt,
                        site: source_site,
                    } => {
                        if (selected.context_path == "$control")
                            != (source_site.purpose != InputSitePurpose::Value)
                        {
                            return Err(denied());
                        }
                        if receipt >= id {
                            return Err(denied());
                        }
                        expected_sources.insert(receipt);
                        if source_site.purpose == InputSitePurpose::Value {
                            let origin = SecretOrigin::Entity {
                                entity_type: source_site.kind.as_str().into(),
                                entity_id: source_site.execution,
                                path: source_site.path.clone(),
                            };
                            if !origins.contains(&origin) {
                                origins.push(origin);
                            }
                        }
                        let source_admission: Id = sqlx::query_scalar(
                            "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
                        )
                        .bind(receipt)
                        .fetch_one(&mut *conn)
                        .await?;
                        if !related(conn, source_admission, renderer).await?
                            && !(source_site.kind == InputEntityKind::ExecutionResult
                                && related(conn, renderer, source_admission).await?)
                        {
                            return Err(denied());
                        }
                        pending.push((receipt, Some(source_site)));
                    }
                    SelectionSource::Control {
                        receipt,
                        site: source_site,
                    } => {
                        if selected.context_path != "$control" || receipt >= id {
                            return Err(denied());
                        }
                        let admission: Id = sqlx::query_scalar(
                            "SELECT destination_admission FROM runtime_input_receipt WHERE id = $1",
                        )
                        .bind(receipt)
                        .fetch_one(&mut *conn)
                        .await?;
                        if !related(conn, admission, renderer).await?
                            && !(source_site.kind == InputEntityKind::ExecutionResult
                                && related(conn, renderer, admission).await?)
                        {
                            return Err(denied());
                        }
                        if source_site.purpose == InputSitePurpose::Value {
                            let origin = SecretOrigin::Entity {
                                entity_type: source_site.kind.as_str().into(),
                                entity_id: source_site.execution,
                                path: source_site.path.clone(),
                            };
                            if !origins.contains(&origin) {
                                origins.push(origin);
                            }
                        }
                        expected_sources.insert(receipt);
                        pending.push((receipt, Some(source_site)));
                    }
                    SelectionSource::PublicParameter {
                        site: source_site,
                        admission,
                    } => {
                        expected_admissions.insert(admission);
                        let source =
                            RuntimeAuthorityRepository::require_retained_admission(conn, admission)
                                .await?;
                        if source.owner_kind != "execution"
                            || source.owner_id != source_site.execution
                            || source_site.purpose != InputSitePurpose::Value
                            || (!related(conn, source.id, renderer).await?
                                && !(source_site.kind == InputEntityKind::ExecutionResult
                                    && related(conn, renderer, source.id).await?))
                        {
                            return Err(denied());
                        }
                        let deadline =
                            RuntimeAuthorityRepository::retained_admission_fence(conn, source.id)
                                .await?;
                        fence = Some(fence.ok_or_else(denied)?.intersect(&deadline));
                        if !public_sources.contains(&source_site) {
                            public_sources.push(source_site.clone());
                        }
                        let origin = SecretOrigin::Entity {
                            entity_type: source_site.kind.as_str().into(),
                            entity_id: source_site.execution,
                            path: source_site.path,
                        };
                        if !origins.contains(&origin) {
                            origins.push(origin);
                        }
                    }
                }
            }
            let saved:Vec<Id>=sqlx::query_scalar("SELECT snapshot FROM runtime_input_receipt_snapshot WHERE receipt=$1 ORDER BY snapshot").bind(id).fetch_all(&mut *conn).await?;
            if saved.into_iter().collect::<BTreeSet<_>>() != expected_snapshots {
                return Err(denied());
            }
            if origins.len() > 256
                || public_sources.len() > 256
                || outcomes.len() > 256
                || native_sources.len() > 256
            {
                return Err(denied());
            }
            let actual_sources:Vec<Id> = sqlx::query_scalar("SELECT source_receipt FROM runtime_input_receipt_source WHERE receipt = $1 ORDER BY source_receipt")
                .bind(id).fetch_all(&mut *conn).await?;
            if actual_sources.into_iter().collect::<BTreeSet<_>>() != expected_sources {
                return Err(denied());
            }
            let actual_admissions:Vec<Id>=sqlx::query_scalar("SELECT admission FROM runtime_input_receipt_dependency WHERE receipt = $1 ORDER BY admission").bind(id).fetch_all(&mut *conn).await?;
            if actual_admissions.into_iter().collect::<BTreeSet<_>>() != expected_admissions {
                return Err(denied());
            }
            if id == *entity_id || proof.site.purpose == InputSitePurpose::Condition {
                let value = crate::crypto::decrypt_json(&encrypted, encryption_key)?;
                if proof.site.purpose == InputSitePurpose::Condition && !value.is_boolean() {
                    return Err(denied());
                }
                if id == *entity_id {
                    root_value = Some(value);
                }
            }
        }
        let fence = fence.ok_or_else(denied)?;
        fence.ensure_fresh_on(conn).await?;
        let graph_nodes: BTreeSet<_> = visited
            .keys()
            .map(|id| (false, *id))
            .chain(snapshot_seen.iter().map(|id| (true, *id)))
            .collect();
        let source_receipts = visited
            .into_iter()
            .filter(|(id, _)| id != entity_id)
            .map(|(receipt, source_site)| ReceiptSourceDescriptor {
                transaction: transaction.clone(),
                retained_by: *entity_id,
                retaining_site: site.clone(),
                receipt,
                site: source_site,
                fence: fence.clone(),
            })
            .collect();
        Ok((
            ReceiptContext {
                graph_accounting: Some(std::sync::Arc::new(ReceiptGraphAccounting {
                    bytes,
                    nodes: graph_nodes.clone(),
                    key_hash: crate::crypto::hash_encryption_key(encryption_key),
                })),
                result_reads: result_reads
                    .iter()
                    .filter(|read| snapshot_seen.contains(&read.snapshot()))
                    .cloned()
                    .collect(),
                wait_reads: wait_reads
                    .iter()
                    .filter(|read| snapshot_seen.contains(&read.snapshot()))
                    .cloned()
                    .collect(),
                source_receipts,
                native_sources,
                newly_recorded: false,
                transaction,
                id: *entity_id,
                site: site.clone(),
                value: root_value.ok_or_else(denied)?,
                origins,
                public_sources,
                outcomes,
                fence,
            },
            bytes,
            graph_nodes,
        ))
    }

    pub(crate) async fn require_finalized_native_sources(
        conn: &mut PgConnection,
        sources: &[super::native_input_snapshot::NativeSnapshotMetadata],
    ) -> Result<()> {
        for source in sources {
            super::native_input_snapshot::NativeInputSnapshotRepository::require_finalized(
                conn,
                source.snapshot(),
            )
            .await?;
        }
        Ok(())
    }
}

/// One canonical walk only. The original validation locks remain held by the
/// transaction; cache hits carry the SAME independent fence, never union grants.
/// Full caches fall back to validation rather than widening the graph budget.
#[derive(Default)]
struct RetainedReceiptAdmissions {
    transaction: Option<String>,
    validated: std::collections::BTreeMap<Id, (RuntimeAdmissionRecord, ApprovalEffectFence)>,
    bytes: usize,
}
impl RetainedReceiptAdmissions {
    fn has_capacity(&self, charge: usize) -> bool {
        self.validated.len() < 256 && charge <= 1024 * 1024 - self.bytes
    }

    fn bind_transaction(&mut self, transaction: &str) -> Result<()> {
        if let Some(held) = &self.transaction {
            if held != transaction {
                return Err(denied());
            }
        } else {
            self.transaction = Some(transaction.into());
        }
        Ok(())
    }

    async fn get(
        &mut self,
        conn: &mut PgConnection,
        id: Id,
    ) -> Result<(RuntimeAdmissionRecord, ApprovalEffectFence)> {
        if self.transaction.is_none() {
            return Err(denied());
        }
        if let Some((record, fence)) = self.validated.get(&id) {
            fence.ensure_fresh()?;
            return Ok((record.clone(), fence.clone()));
        }
        let (record, fence) =
            RuntimeAuthorityRepository::retained_admission_with_fence(conn, id).await?;
        let charge = serde_json::to_vec(&record.authority)?.len()
            + record.owner_kind.len()
            + record.authority_digest.len();
        if self.has_capacity(charge) {
            self.bytes += charge;
            self.validated.insert(id, (record.clone(), fence.clone()));
        }
        Ok((record, fence))
    }
}

fn merge_wait_reads(target: &mut Vec<IssuedWaitRead>, sources: &[IssuedWaitRead]) -> Result<()> {
    for source in sources {
        if let Some(saved) = target
            .iter_mut()
            .find(|saved| saved.snapshot() == source.snapshot())
        {
            saved.merge(source)?;
        } else {
            target.push(source.clone());
        }
    }
    if target.len() > 256 {
        return Err(denied());
    }
    Ok(())
}

fn merge_result_reads(
    target: &mut Vec<super::native_input_snapshot::IssuedResultRead>,
    sources: &[super::native_input_snapshot::IssuedResultRead],
) -> Result<()> {
    for read in sources {
        if let Some(held) = target
            .iter_mut()
            .find(|held| held.snapshot() == read.snapshot())
        {
            held.merge(read)?;
        } else {
            target.push(read.clone());
        }
    }
    if target.len() > 256 {
        return Err(denied());
    }
    Ok(())
}

fn combined_approval_deadline<'a>(
    base: &ApprovalEffectFence,
    controls: impl Iterator<Item = &'a ApprovalEffectFence>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    base.approval_deadline()
        .into_iter()
        .chain(controls.filter_map(ApprovalEffectFence::approval_deadline))
        .min()
}

async fn require_published_sources(
    conn: &mut PgConnection,
    sources: &[ReceiptSourceDescriptor],
) -> Result<()> {
    if sources.len() > 256 {
        return Err(denied());
    }
    let ids = sources
        .iter()
        .map(|source| source.receipt)
        .collect::<Vec<_>>();
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_staged WHERE receipt=ANY($1) AND publication_transaction IS NULL)").bind(&ids).fetch_one(&mut *conn).await?;
    if pending {
        return Err(denied());
    }
    Ok(())
}

fn collect_source_receipts(
    context: &ReceiptContext,
    sources: &mut Vec<ReceiptSourceDescriptor>,
) -> Result<()> {
    for source in std::iter::once(context.held_source_descriptor())
        .chain(context.source_receipts.iter().cloned())
    {
        if let Some(saved) = sources
            .iter_mut()
            .find(|saved| saved.receipt == source.receipt)
        {
            if saved.site != source.site || saved.transaction != source.transaction {
                return Err(denied());
            }
            saved.fence = saved.fence.intersect(&source.fence);
        } else {
            sources.push(source);
        }
    }
    if sources.len() >= 256 {
        return Err(denied());
    }
    Ok(())
}
fn retained_source_receipts(
    sources: &[ReceiptSourceDescriptor],
    retained_by: Id,
    site: &InputSite,
    fence: &ApprovalEffectFence,
) -> Vec<ReceiptSourceDescriptor> {
    sources
        .iter()
        .map(|source| {
            let mut source = source.clone();
            source.retained_by = retained_by;
            source.retaining_site = site.clone();
            source.fence = source.fence.intersect(fence);
            source
        })
        .collect()
}

struct TraceContext<'a> {
    values: &'a [BoundValue],
}

struct LoadingContext<'a> {
    state: tokio::sync::Mutex<(&'a mut PgConnection, &'a mut RenderSession)>,
    source_error: tokio::sync::Mutex<Option<Error>>,
    encryption_key: &'a str,
}
impl LoadingContext<'_> {
    fn finish<T>(self, evaluated: Result<T>) -> Result<T> {
        // No second evaluation or source read. Retain the actual repository
        // failure instead of flattening it through EvalError into denied().
        source_evaluation_result(evaluated, self.source_error.into_inner())
    }

    async fn source_failed(&self, error: Error) -> EvalError {
        let mut failure = self.source_error.lock().await;
        if failure.is_none() {
            *failure = Some(error);
        }
        EvalError::TypeError("Selected source input is not authorized".into())
    }
}
fn source_evaluation_result<T>(evaluated: Result<T>, source_error: Option<Error>) -> Result<T> {
    match source_error {
        Some(error) => Err(error),
        None => evaluated,
    }
}
impl AsyncEvalContext for LoadingContext<'_> {
    fn workflow_function_source(&self, name: &str, args: &[Value]) -> Option<SelectedField> {
        TraceContext { values: &[] }.workflow_function_source(name, args)
    }
    fn resolve_variable<'a>(
        &'a self,
        name: &'a str,
    ) -> futures::future::BoxFuture<'a, EvalResult<Value>> {
        Box::pin(async move {
            let state = self.state.lock().await;
            let context = TraceContext {
                values: &state.1.values,
            };
            if name == "keystore"
                && !state
                    .1
                    .values
                    .iter()
                    .any(|value| value.path.starts_with("/keystore/"))
            {
                return Ok(json!({}));
            }
            context.resolve_variable(name)
        })
    }
    fn call_workflow_function<'a>(
        &'a self,
        name: &'a str,
        args: &'a [Value],
    ) -> futures::future::BoxFuture<'a, EvalResult<Option<Value>>> {
        Box::pin(async move {
            let state = self.state.lock().await;
            TraceContext {
                values: &state.1.values,
            }
            .call_workflow_function(name, args)
        })
    }
    fn resolve_member<'a>(
        &'a self,
        source: &'a SelectedField,
        member: &'a str,
    ) -> futures::future::BoxFuture<'a, EvalResult<Option<Value>>> {
        Box::pin(async move {
            if source.namespace != "keystore" || !source.path.is_empty() {
                return Ok(None);
            }
            let mut state = self.state.lock().await;
            let path = format!("/keystore/{}", escape(member));
            if let Some(value) = state.1.values.iter().find(|value| value.path == path) {
                return Ok(Some(value.value.clone()));
            }
            let (conn, session) = &mut *state;
            if let Err(error) =
                InheritedInputRepository::bind_key(conn, session, member, self.encryption_key).await
            {
                return Err(self.source_failed(error).await);
            }
            session
                .values
                .iter()
                .find(|value| value.path == path)
                .map(|value| Some(value.value.clone()))
                .ok_or_else(|| EvalError::VariableNotFound(path))
        })
    }
}
impl EvalContext for TraceContext<'_> {
    fn workflow_function_source(&self, name: &str, args: &[Value]) -> Option<SelectedField> {
        if !args.is_empty() {
            return None;
        }
        if name == "result" {
            return Some(SelectedField {
                namespace: "result".into(),
                path: String::new(),
            });
        }
        matches!(name, "succeeded" | "failed" | "timed_out" | "cancelled").then(|| SelectedField {
            namespace: "$outcome".into(),
            path: format!("/{name}"),
        })
    }
    fn resolve_variable(&self, name: &str) -> EvalResult<Value> {
        let path = format!("/{}", escape(name));
        let mut root = json!({});
        for value in self
            .values
            .iter()
            .filter(|value| overlaps(&value.path, &path))
        {
            set(&mut root, &value.path, value.value.clone())
                .map_err(|error| EvalError::TypeError(error.to_string()))?;
        }
        root.pointer(&path)
            .cloned()
            .ok_or_else(|| EvalError::VariableNotFound(path))
    }
    fn call_workflow_function(&self, name: &str, args: &[Value]) -> EvalResult<Option<Value>> {
        if name == "result" && args.is_empty() {
            return self.resolve_variable("result").map(Some);
        }
        if args.is_empty() && matches!(name, "succeeded" | "failed" | "timed_out" | "cancelled") {
            return self
                .resolve_variable("$outcome")?
                .get(name)
                .cloned()
                .map(Some)
                .ok_or_else(|| EvalError::VariableNotFound(name.into()));
        }
        Ok(None)
    }
}
fn selected_sources(
    values: &[BoundValue],
    fields: &BTreeSet<SelectedField>,
) -> Result<Vec<ReadSelection>> {
    let mut selections = Vec::new();
    for field in fields {
        let path = format!("/{}{}", escape(&field.namespace), field.path);
        let bound = values
            .iter()
            .filter(|value| overlaps(&value.path, &path))
            .collect::<Vec<_>>();
        if bound.is_empty() {
            return Err(denied());
        }
        for value in bound {
            let mut source = value.source.clone();
            if let SelectionSource::Authored { origins, .. } = &mut source {
                for origin in origins {
                    if let SecretOrigin::Entity {
                        entity_type,
                        path: origin_path,
                        ..
                    } = origin
                    {
                        if entity_type
                            == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION
                        {
                            *origin_path = path.strip_prefix(&value.path).unwrap_or("").into();
                        }
                    }
                }
            }
            let selected = ReadSelection {
                context_path: path.clone(),
                source,
            };
            if !selections.contains(&selected) {
                selections.push(selected);
            }
        }
    }
    Ok(selections)
}

fn condition_truth(value: &Value, template: bool) -> bool {
    if !template {
        return crate::workflow::expression::is_truthy(value);
    }
    // WorkflowContext's template fallback first stringifies the evaluated value.
    // In particular "false" is false in a template but truthy as a bare expression.
    match value {
        Value::Null => false,
        Value::String(value) => !matches!(
            value.trim().to_lowercase().as_str(),
            "" | "false" | "0" | "null" | "none"
        ),
        value => !matches!(value.to_string().as_str(), "false" | "0" | "null"),
    }
}

fn render_condition(
    expression: &str,
    context: &dyn EvalContext,
) -> Result<(Value, BTreeSet<SelectedField>)> {
    if let Ok(ast) = parse_expression(expression) {
        let evaluated = eval_selected(&ast, context).map_err(|_| denied())?;
        return Ok((
            json!(condition_truth(&evaluated.value, false)),
            evaluated.fields,
        ));
    }
    let (value, fields) = render(expression, context)?;
    Ok((json!(condition_truth(&value, true)), fields))
}
async fn render_condition_async(
    expression: &str,
    context: &dyn AsyncEvalContext,
) -> Result<(Value, BTreeSet<SelectedField>)> {
    if let Ok(ast) = parse_expression(expression) {
        let evaluated = eval_selected_async(&ast, context)
            .await
            .map_err(|_| denied())?;
        return Ok((
            json!(condition_truth(&evaluated.value, false)),
            evaluated.fields,
        ));
    }
    let (value, fields) = render_async(expression, context).await?;
    Ok((json!(condition_truth(&value, true)), fields))
}

fn render(template: &str, context: &dyn EvalContext) -> Result<(Value, BTreeSet<SelectedField>)> {
    static EXPRESSIONS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\{\{([\s\S]*?)\}\}").expect("receipt expression regex")
    });
    let matches = EXPRESSIONS.captures_iter(template).collect::<Vec<_>>();
    if matches.is_empty() {
        let ast = parse_expression(template).map_err(|_| denied())?;
        let evaluated = eval_selected(&ast, context).map_err(|_| denied())?;
        return Ok((evaluated.value, evaluated.fields));
    }
    if matches.len() == 1
        && matches[0]
            .get(0)
            .is_some_and(|span| span.as_str() == template.trim())
    {
        let ast = parse_expression(matches[0][1].trim()).map_err(|_| denied())?;
        let evaluated = eval_selected(&ast, context).map_err(|_| denied())?;
        return Ok((evaluated.value, evaluated.fields));
    }
    let mut result = String::new();
    let mut end = 0;
    let mut fields = BTreeSet::new();
    for capture in matches {
        let span = capture.get(0).ok_or_else(denied)?;
        result.push_str(&template[end..span.start()]);
        let ast = parse_expression(capture[1].trim()).map_err(|_| denied())?;
        let evaluated = eval_selected(&ast, context).map_err(|_| denied())?;
        fields.extend(evaluated.fields);
        let value = evaluated.value;
        if !value.is_null() {
            result.push_str(
                value
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| value.to_string())
                    .as_str(),
            );
        }
        end = span.end();
    }
    result.push_str(&template[end..]);
    Ok((Value::String(result), fields))
}

async fn render_async(
    template: &str,
    context: &dyn AsyncEvalContext,
) -> Result<(Value, BTreeSet<SelectedField>)> {
    static EXPRESSIONS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\{\{([\s\S]*?)\}\}").expect("receipt expression regex")
    });
    let matches = EXPRESSIONS.captures_iter(template).collect::<Vec<_>>();
    if matches.is_empty() {
        let ast = parse_expression(template).map_err(|_| denied())?;
        let evaluated = eval_selected_async(&ast, context)
            .await
            .map_err(|_| denied())?;
        return Ok((evaluated.value, evaluated.fields));
    }
    if matches.len() == 1
        && matches[0]
            .get(0)
            .is_some_and(|span| span.as_str() == template.trim())
    {
        let ast = parse_expression(matches[0][1].trim()).map_err(|_| denied())?;
        let evaluated = eval_selected_async(&ast, context)
            .await
            .map_err(|_| denied())?;
        return Ok((evaluated.value, evaluated.fields));
    }
    let mut result = String::new();
    let mut end = 0;
    let mut fields = BTreeSet::new();
    for capture in matches {
        let span = capture.get(0).ok_or_else(denied)?;
        result.push_str(&template[end..span.start()]);
        let ast = parse_expression(capture[1].trim()).map_err(|_| denied())?;
        let evaluated = eval_selected_async(&ast, context)
            .await
            .map_err(|_| denied())?;
        fields.extend(evaluated.fields);
        let value = evaluated.value;
        if !value.is_null() {
            result.push_str(
                value
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| value.to_string())
                    .as_str(),
            );
        }
        end = span.end();
    }
    result.push_str(&template[end..]);
    Ok((Value::String(result), fields))
}
fn insert(
    session: &mut RenderSession,
    path: String,
    value: Value,
    source: SelectionSource,
) -> Result<()> {
    check_pointer(&path)?;
    bounded(&value)?;
    if session.values.len() >= 256
        || session
            .values
            .iter()
            .any(|existing| overlaps(&existing.path, &path))
    {
        return Err(denied());
    }
    let bytes = session
        .values
        .iter()
        .map(|bound| serde_json::to_vec(&bound.value).map(|value| value.len()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>();
    if bytes + serde_json::to_vec(&value)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    session.values.push(BoundValue {
        path,
        value,
        source,
    });
    Ok(())
}
fn validate_authored_origin(
    declaration: &super::declared_input::DeclaredInputBinding,
    origin: &SecretOrigin,
    target: &crate::runtime_authority::BoundRuntimeTarget,
    input_authority: Option<Id>,
) -> Result<()> {
    if input_authority.is_none() && target.pack_id != Some(declaration.pack) {
        return Err(denied());
    }
    match origin {
        SecretOrigin::Key(key)
            if declaration.uses_keystore()
                && target.kind == "key"
                && target.id == key.key_id
                && (input_authority.is_some()
                    || matches!(
                        key.owner_type,
                        crate::models::OwnerType::Pack
                            | crate::models::OwnerType::Action
                            | crate::models::OwnerType::Sensor
                    )) =>
        {
            Ok(())
        }
        SecretOrigin::Entity { entity_type, .. }
            if entity_type == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION
                && declaration.uses_pack_configuration() =>
        {
            Ok(())
        }
        _ => Err(denied()),
    }
}
fn transition_condition(path: &str) -> bool {
    let Some(path) = path.strip_prefix("/workflow/") else {
        return false;
    };
    let tokens = path.split('/').collect::<Vec<_>>();
    if tokens.len() < 5
        || tokens[tokens.len() - 3] != "next"
        || tokens[tokens.len() - 2].parse::<usize>().is_err()
        || tokens[tokens.len() - 1] != "when"
    {
        return false;
    }
    let task = &tokens[..tokens.len() - 3];
    task.len() % 2 == 0 && task.chunks_exact(2).all(|pair| pair[0] == "tasks")
}
async fn literal_value(
    conn: &mut PgConnection,
    source: &RuntimeAdmissionRecord,
    leaf: &LiteralLeaf,
) -> Result<Value> {
    if !literal_site(&leaf.path) {
        return Err(denied());
    }
    let policy = RuntimeAuthorityRepository::require_policy(conn, source.policy_revision).await?;
    let invocation = policy
        .policy
        .invocations
        .iter()
        .find(|invocation| invocation.binding == source.authority.binding)
        .ok_or_else(denied)?;
    let snapshot = super::managed_executable::ManagedExecutableRepository::invocation_snapshot(
        conn, invocation,
    )
    .await?;
    if hash(&serde_json::to_value(&snapshot.executable)?)? != leaf.revision {
        return Err(denied());
    }
    let graph = snapshot.executable.workflow_definition.ok_or_else(denied)?;
    let pointer = leaf.path.strip_prefix("/workflow").ok_or_else(denied)?;
    let value = graph
        .definition
        .pointer(pointer)
        .cloned()
        .ok_or_else(denied)?;
    let mut pending = vec![(&value, 0)];
    let mut nodes = 0;
    while let Some((node, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 {
            return Err(denied());
        }
        match node {
            Value::String(value) if value.contains("{{") => return Err(denied()),
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    bounded(&value)?;
    Ok(value)
}
async fn iteration_batch_size(
    conn: &mut PgConnection,
    source: &RuntimeAdmissionRecord,
    path: &str,
) -> Result<Option<usize>> {
    Ok(iteration_shape(conn, source, path).await?.1)
}
async fn iteration_shape(
    conn: &mut PgConnection,
    source: &RuntimeAdmissionRecord,
    path: &str,
) -> Result<(Id, Option<usize>)> {
    if !iteration_control::with_items_path(path) {
        return Err(denied());
    }
    let policy = RuntimeAuthorityRepository::find_policy(conn, source.policy_revision)
        .await?
        .ok_or_else(denied)?;
    let invocation = policy
        .policy
        .invocations
        .iter()
        .find(|invocation| invocation.binding == source.authority.binding)
        .ok_or_else(denied)?;
    let snapshot = super::managed_executable::ManagedExecutableRepository::invocation_snapshot(
        conn, invocation,
    )
    .await?;
    let graph = snapshot.executable.workflow_definition.ok_or_else(denied)?;
    let pointer = path
        .strip_prefix("/workflow")
        .and_then(|path| path.strip_suffix("/with_items"))
        .ok_or_else(denied)?;
    let task = graph
        .definition
        .pointer(pointer)
        .and_then(Value::as_object)
        .ok_or_else(denied)?;
    let batch = match task.get("batch_size") {
        None | Some(Value::Null) => None,
        Some(size) => {
            let size = usize::try_from(size.as_u64().ok_or_else(denied)?).map_err(|_| denied())?;
            if size == 0 {
                return Err(denied());
            }
            Some(size)
        }
    };
    Ok((graph.id, batch))
}
fn literal_site(path: &str) -> bool {
    let Some(path) = path.strip_prefix("/workflow/") else {
        return false;
    };
    let tokens = path.split('/').collect::<Vec<_>>();
    if matches!(tokens.first(), Some(&"vars" | &"output_map")) {
        return tokens.len() >= 2;
    }
    let mut offset = 0;
    while tokens.get(offset) == Some(&"tasks") && tokens.get(offset + 1).is_some() {
        offset += 2;
    }
    match tokens.get(offset) {
        Some(&"input" | &"params" | &"with_items") => tokens.len() > offset,
        Some(&"next") => {
            tokens
                .get(offset + 1)
                .is_some_and(|index| index.parse::<usize>().is_ok())
                && tokens.get(offset + 2) == Some(&"publish")
                && tokens.len() > offset + 3
        }
        _ => false,
    }
}
async fn related(conn: &mut PgConnection, ancestor: Id, descendant: Id) -> Result<bool> {
    if ancestor == descendant {
        return Ok(true);
    }
    Ok(sqlx::query_scalar("WITH RECURSIVE lineage(id) AS (SELECT dependency FROM runtime_admission_dependency WHERE admission = $1 UNION SELECT edge.dependency FROM runtime_admission_dependency edge JOIN lineage ON edge.admission = lineage.id) SELECT EXISTS(SELECT 1 FROM lineage WHERE id = $2)")
        .bind(descendant).bind(ancestor).fetch_one(conn).await?)
}
async fn stored_field(
    conn: &mut PgConnection,
    site: &InputSite,
) -> Result<Option<(Value, Vec<crate::secret_values::StoredSecretValue>)>> {
    check_site(site)?;
    let sql = match site.kind {
        InputEntityKind::ExecutionConfig => "SELECT config FROM execution WHERE id = $1",
        InputEntityKind::ExecutionResult => "SELECT result FROM execution WHERE id = $1",
        InputEntityKind::WorkflowVariables => {
            "SELECT variables FROM workflow_execution WHERE execution = $1"
        }
    };
    let raw: Option<Option<Value>> = sqlx::query_scalar(sql)
        .bind(site.execution)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(Some(raw)) = raw else {
        return Ok(None);
    };
    let secrets =
        super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            site.kind.as_str(),
            site.execution,
        )
        .await?
        .into_iter()
        .filter(|secret| overlaps(&secret.json_path, &site.path))
        .collect();
    Ok(Some((raw, secrets)))
}
fn restore_field(
    raw: Value,
    secrets: &[crate::secret_values::StoredSecretValue],
    site: &InputSite,
    key: &str,
) -> Result<Value> {
    let restored = crate::secret_values::restore_secret_values(raw, secrets, key)?;
    restored.pointer(&site.path).cloned().ok_or_else(denied)
}
fn stored_provenance(secret: &crate::secret_values::StoredSecretValue) -> Result<SecretProvenance> {
    if secret.source_kind != "provenance" {
        return Err(denied());
    }
    serde_json::from_str(secret.source_ref.as_deref().ok_or_else(denied)?).map_err(|_| denied())
}
fn check_site(site: &InputSite) -> Result<()> {
    if site.execution <= 0 {
        return Err(denied());
    }
    check_pointer(&site.path)
}
fn check_pointer(path: &str) -> Result<()> {
    if path.len() > 4096
        || (!path.is_empty() && !path.starts_with('/'))
        || path.split('/').count() > 33
    {
        return Err(denied());
    }
    let bytes = path.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'~' && !matches!(bytes.get(index + 1), Some(b'0' | b'1')) {
            return Err(denied());
        }
    }
    Ok(())
}
fn overlaps(a: &str, b: &str) -> bool {
    a == b
        || a.is_empty()
        || b.is_empty()
        || a.strip_prefix(b)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || b.strip_prefix(a)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
fn set(root: &mut Value, path: &str, value: Value) -> Result<()> {
    if path.is_empty() {
        *root = value;
        return Ok(());
    }
    let tokens = path
        .strip_prefix('/')
        .ok_or_else(denied)?
        .split('/')
        .map(|value| value.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>();
    let mut node = root;
    for (index, token) in tokens.iter().enumerate() {
        if !node.is_object() {
            return Err(denied());
        }
        let map = node.as_object_mut().ok_or_else(denied)?;
        if index + 1 == tokens.len() {
            map.insert(token.clone(), value);
            return Ok(());
        }
        node = map.entry(token.clone()).or_insert_with(|| json!({}));
    }
    Err(denied())
}
fn bounded(value: &Value) -> Result<()> {
    if serde_json::to_vec(value)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    let mut pending = vec![(value, 0usize)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 {
            return Err(denied());
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    Ok(())
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| denied())
}
fn iteration_task_scope(source: &str, target: &str) -> bool {
    source.strip_suffix("/with_items").is_some_and(|task| {
        target
            .strip_prefix(task)
            .is_some_and(|suffix| suffix.starts_with('/'))
    })
}
fn iteration_value(array: &Value, ordinal: usize, batch_size: Option<usize>) -> Result<Value> {
    let array = array
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(array));
    match batch_size {
        None | Some(1) => array.get(ordinal).cloned().ok_or_else(denied),
        Some(0) => Err(denied()),
        Some(size) => {
            let first = ordinal.checked_mul(size).ok_or_else(denied)?;
            if first >= array.len() {
                return Err(denied());
            }
            let end = first.saturating_add(size).min(array.len());
            Ok(Value::Array(array[first..end].to_vec()))
        }
    }
}

fn denied() -> Error {
    Error::PermissionDenied(
        "Purpose input receipt, selection, value shape or lineage does not match".into(),
    )
}
async fn accepted_receipt(conn: &mut PgConnection, receipt: Id) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_accepted WHERE receipt = $1)",
    )
    .bind(receipt)
    .fetch_one(conn)
    .await?)
}
fn protected_namespace(path: &str) -> bool {
    [
        "/keystore",
        "/config",
        "/pack/config",
        "/$outcome",
        "/system",
        "/index",
        "/inquiry",
        "/execution",
        "/work_queue_item",
    ]
    .iter()
    .any(|reserved| overlaps(path, reserved))
}
async fn validate_system_source(
    conn: &mut PgConnection,
    renderer: Id,
    admission: Id,
    execution: Id,
    created: chrono::DateTime<chrono::Utc>,
) -> Result<ApprovalEffectFence> {
    let source = RuntimeAuthorityRepository::require_retained_admission(conn, admission).await?;
    if admission != renderer || source.owner_kind != "execution" || source.owner_id != execution {
        return Err(denied());
    }
    let actual: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT created FROM execution WHERE id=$1 FOR SHARE")
            .bind(execution)
            .fetch_optional(&mut *conn)
            .await?;
    if actual.is_some_and(|actual| actual != created) {
        return Err(denied());
    }
    RuntimeAuthorityRepository::retained_admission_fence(conn, admission).await
}

async fn public_schema_site(conn: &mut PgConnection, site: &InputSite, raw: &Value) -> Result<()> {
    let binding = super::schema_contract::SchemaContractRepository::find(
        conn,
        super::schema_contract::BindingOwner::Execution,
        site.execution,
    )
    .await?
    .ok_or_else(denied)?;
    binding.validate()?;
    let contract = match site.kind {
        InputEntityKind::ExecutionConfig => Some(&binding.input),
        InputEntityKind::ExecutionResult => binding.output.as_ref(),
        InputEntityKind::WorkflowVariables => None,
    };
    if let Some(contract) = contract {
        let compiled =
            crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
        if compiled
            .secret_paths(raw)
            .map_err(|_| denied())?
            .iter()
            .any(|path| overlaps(path, &site.path))
        {
            return Err(denied());
        }
    }
    Ok(())
}
fn terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "timeout" | "abandoned"
    )
}
fn outcome_value(status: &str, timed_out: bool, function: &str) -> Option<Value> {
    let timeout = timed_out || status == "timeout";
    Some(json!(match function {
        "succeeded" => status == "completed" && !timeout,
        "failed" => matches!(status, "failed" | "abandoned") && !timeout,
        "timed_out" => timeout,
        "cancelled" => status == "cancelled",
        _ => return None,
    }))
}
async fn validate_outcome_source(
    conn: &mut PgConnection,
    renderer: Id,
    admission: Id,
    execution: Id,
    status: &str,
) -> Result<ApprovalEffectFence> {
    let source = RuntimeAuthorityRepository::require_retained_admission(conn, admission).await?;
    if source.owner_kind != "execution"
        || source.owner_id != execution
        || !terminal_status(status)
        || !related(conn, renderer, admission).await?
    {
        return Err(denied());
    }
    RuntimeAuthorityRepository::retained_admission_fence(conn, admission).await
}
fn render_digest(rendered: &RenderedReceiptContext) -> Result<String> {
    hash(
        &json!({"transaction":rendered.transaction,"renderer":rendered.renderer.id,"declaration":rendered.declaration,"literal":rendered.literal,"native":rendered.native,"value":rendered.value,"selections":rendered.selections,"purpose":rendered.purpose}),
    )
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

#[cfg(test)]
mod tests {
    #[test]
    fn held_deadline_summary_keeps_independent_control_expiry() {
        let now = chrono::Utc::now();
        let unlimited = ApprovalEffectFence::from_input_approval(None).unwrap();
        let early =
            ApprovalEffectFence::from_input_approval(Some(now + chrono::Duration::hours(1)))
                .unwrap();
        let late = ApprovalEffectFence::from_input_approval(Some(now + chrono::Duration::hours(2)))
            .unwrap();
        assert_eq!(
            combined_approval_deadline(&late, [&unlimited, &early].into_iter()),
            early.approval_deadline()
        );
        assert_eq!(
            combined_approval_deadline(&early, [&late].into_iter()),
            early.approval_deadline()
        );
        assert_eq!(
            combined_approval_deadline(&unlimited, [&unlimited].into_iter()),
            None
        );
    }
    #[test]
    fn receipt_admission_reuse_is_scoped_to_the_issuing_transaction() {
        let mut cache = super::RetainedReceiptAdmissions::default();
        cache.bind_transaction("issuing-transaction").unwrap();
        cache.bind_transaction("issuing-transaction").unwrap();
        assert!(cache.bind_transaction("other-transaction").is_err());
        assert_eq!(cache.transaction.as_deref(), Some("issuing-transaction"));
    }

    #[test]
    fn receipt_admission_reuse_has_a_bounded_budget_without_rejecting_the_graph() {
        let mut cache = super::RetainedReceiptAdmissions::default();
        assert!(cache.has_capacity(1024 * 1024));
        assert!(!cache.has_capacity(1024 * 1024 + 1));
        cache.bytes = 1024 * 1024;
        assert!(!cache.has_capacity(1));
        // The cache declines to store another proof. It does not authorize or
        // reject that source; get() still performs the original full validation.
        assert!(cache.validated.is_empty());
    }

    #[test]
    fn source_evaluation_preserves_expiry_instead_of_generic_lineage_denial() {
        let actual = super::source_evaluation_result::<()>(
            Err(super::denied()),
            Some(crate::Error::PermissionDenied(
                "Declared-input Key approvals expired during source preparation".into(),
            )),
        );
        assert!(matches!(actual, Err(crate::Error::PermissionDenied(reason))
            if reason == "Declared-input Key approvals expired during source preparation"));
    }

    #[test]
    fn source_evaluation_cannot_swallow_a_failed_source_read() {
        let actual = super::source_evaluation_result(Ok(true), Some(super::denied()));
        assert!(actual.is_err());
        assert!(super::source_evaluation_result(Ok(true), None).unwrap());
    }

    #[test]
    fn source_evaluation_leaves_non_source_errors_unchanged() {
        let actual = super::source_evaluation_result::<()>(
            Err(crate::Error::PermissionDenied("expression failure".into())),
            None,
        );
        assert!(matches!(actual, Err(crate::Error::PermissionDenied(reason))
            if reason == "expression failure"));
    }
    use super::*;

    struct CountedConditionContext {
        value: Value,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl EvalContext for CountedConditionContext {
        fn resolve_variable(&self, name: &str) -> EvalResult<Value> {
            assert_eq!(name, "parameters");
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"unknown":self.value}))
        }
        fn call_workflow_function(
            &self,
            _name: &str,
            _args: &[Value],
        ) -> EvalResult<Option<Value>> {
            Ok(None)
        }
    }
    impl AsyncEvalContext for CountedConditionContext {
        fn resolve_variable<'a>(
            &'a self,
            name: &'a str,
        ) -> futures::future::BoxFuture<'a, EvalResult<Value>> {
            Box::pin(async move { EvalContext::resolve_variable(self, name) })
        }
        fn call_workflow_function<'a>(
            &'a self,
            _name: &'a str,
            _args: &'a [Value],
        ) -> futures::future::BoxFuture<'a, EvalResult<Option<Value>>> {
            Box::pin(async { Ok(None) })
        }
    }
    #[tokio::test]
    async fn condition_coercion_preserves_canonical_truthiness_and_one_selected_pass() {
        for value in [
            json!(null),
            json!(false),
            json!(true),
            json!(0),
            json!(0.0),
            json!(-1),
            json!(""),
            json!("false"),
            json!(" NONE "),
            json!([]),
            json!({}),
            json!([false]),
            json!("{{ keystore['never'] }}"),
        ] {
            for expression in [
                "parameters.unknown",
                "parameters.unknown == false",
                "not parameters.unknown",
                "true or parameters.unknown",
            ] {
                let context = CountedConditionContext {
                    value: value.clone(),
                    reads: Default::default(),
                };
                let canonical =
                    crate::workflow::expression::eval_expression(expression, &context).unwrap();
                context.reads.store(0, std::sync::atomic::Ordering::SeqCst);
                let (actual, fields) = render_condition_async(expression, &context).await.unwrap();
                assert_eq!(
                    actual,
                    json!(crate::workflow::expression::is_truthy(&canonical)),
                    "{expression}: {value}"
                );
                let skipped = expression.starts_with("true or");
                assert_eq!(
                    context.reads.load(std::sync::atomic::Ordering::SeqCst),
                    usize::from(!skipped),
                    "coercion must not reevaluate unknown-type properties"
                );
                assert_eq!(
                    fields,
                    if skipped {
                        BTreeSet::new()
                    } else {
                        BTreeSet::from([SelectedField {
                            namespace: "parameters".into(),
                            path: "/unknown".into(),
                        }])
                    }
                );
                let (sync, sync_fields) = render_condition(expression, &context).unwrap();
                assert_eq!((sync, sync_fields), (actual, fields));
            }
        }
        for (value, expected) in [
            (json!("false"), false),
            (json!(" NONE "), false),
            (json!(false), false),
            (json!(0), false),
            (json!(null), false),
            (json!([]), true),
            (json!({}), true),
            (json!("{{ keystore['never'] }}"), true),
        ] {
            let context = CountedConditionContext {
                value,
                reads: Default::default(),
            };
            let (actual, fields) = render_condition_async("{{ parameters.unknown }}", &context)
                .await
                .unwrap();
            assert_eq!(actual, json!(expected));
            assert_eq!(context.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                fields,
                BTreeSet::from([SelectedField {
                    namespace: "parameters".into(),
                    path: "/unknown".into()
                }])
            );
        }
    }

    #[test]
    fn outcome_functions_select_native_metadata_not_a_fabricated_value_receipt() {
        let source = SelectionSource::NativeOutcome {
            admission: 10,
            execution: 11,
            status: "completed".into(),
            timed_out: false,
        };
        let values = vec![BoundValue {
            path: "/$outcome/succeeded".into(),
            value: json!(true),
            source: source.clone(),
        }];
        let (value, fields) = render("succeeded()", &TraceContext { values: &values }).unwrap();
        assert_eq!(value, json!(true));
        assert_eq!(
            selected_sources(&values, &fields).unwrap(),
            vec![ReadSelection {
                context_path: "/$outcome/succeeded".into(),
                source
            }]
        );
        assert_eq!(
            outcome_value("timeout", false, "timed_out"),
            Some(json!(true))
        );
        assert_eq!(outcome_value("failed", true, "failed"), Some(json!(false)));
        assert!(protected_namespace("/$outcome/succeeded"));
    }

    #[test]
    fn nested_receipt_field_restores_an_encrypted_ancestor_before_selection() {
        let key = "nested-receipt-test-owned-encryption-key";
        let site = InputSite {
            execution: 9,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: "/data/items/0/token".into(),
        };
        let secret = crate::secret_values::StoredSecretValue {
            json_path: "/data".into(),
            encrypted_value: crate::crypto::encrypt_json(
                &json!({"items":[{"token":"retained"}]}),
                key,
            )
            .unwrap(),
            encryption_key_hash: Some(crate::crypto::hash_encryption_key(key)),
            source_kind: "provenance".into(),
            source_ref: Some("{\"origins\":[],\"templates\":[]}".into()),
        };
        assert_eq!(
            restore_field(
                json!({"data":crate::secret_values::redaction_marker()}),
                &[secret],
                &site,
                key
            )
            .unwrap(),
            json!("retained")
        );
    }

    fn parameter(path: &str, value: Value) -> BoundValue {
        BoundValue {
            path: format!("/parameters{path}"),
            value,
            source: SelectionSource::PublicParameter {
                admission: 7,
                site: InputSite {
                    execution: 7,
                    kind: InputEntityKind::ExecutionConfig,
                    purpose: InputSitePurpose::Value,
                    path: path.into(),
                },
            },
        }
    }

    #[test]
    fn supplied_template_text_is_a_literal_and_never_selects_a_key() {
        let literal = json!("{{ keystore['system.not_authored'] }}");
        let values = vec![parameter("/supplied", literal.clone())];
        let (value, fields) = render(
            "{{ parameters.supplied }}",
            &TraceContext { values: &values },
        )
        .unwrap();
        assert_eq!(value, literal);
        assert_eq!(
            fields,
            BTreeSet::from([SelectedField {
                namespace: "parameters".into(),
                path: "/supplied".into()
            }])
        );
        assert_eq!(selected_sources(&values, &fields).unwrap().len(), 1);
    }

    #[test]
    fn selected_map_comes_from_evaluated_dynamic_and_mixed_ast_fields() {
        let values = vec![
            parameter("/selector", json!("a/b")),
            parameter("/map", json!({"a/b":"one","ignored":"two"})),
            parameter("/suffix", json!("!")),
        ];
        let (value, fields) = render(
            "{{ upper(parameters.map[parameters.selector]) + parameters.suffix }}",
            &TraceContext { values: &values },
        )
        .unwrap();
        assert_eq!(value, json!("ONE!"));
        assert!(fields.contains(&SelectedField {
            namespace: "parameters".into(),
            path: "/map/a~1b".into()
        }));
        assert!(fields.contains(&SelectedField {
            namespace: "parameters".into(),
            path: "/selector".into()
        }));
        assert_eq!(selected_sources(&values, &fields).unwrap().len(), 3);
        assert!(!fields.iter().any(|field| field.path.contains("ignored")));
    }

    #[test]
    fn short_circuit_and_negative_array_index_use_only_actual_sources() {
        let values = vec![parameter("/items", json!(["a", "b"]))];
        let (value, fields) = render(
            "{{ true or parameters.unavailable }}",
            &TraceContext { values: &values },
        )
        .unwrap();
        assert_eq!(value, json!(true));
        assert!(fields.is_empty());
        let (value, fields) = render(
            "{{ parameters.items[-1] }}",
            &TraceContext { values: &values },
        )
        .unwrap();
        assert_eq!(value, json!("b"));
        assert_eq!(
            fields,
            BTreeSet::from([SelectedField {
                namespace: "parameters".into(),
                path: "/items/1".into()
            }])
        );
    }

    #[test]
    fn same_pointer_different_entity_or_purpose_has_different_proof_identity() {
        let kinds = [
            InputEntityKind::ExecutionConfig,
            InputEntityKind::ExecutionResult,
            InputEntityKind::WorkflowVariables,
        ];
        let names = kinds
            .into_iter()
            .flat_map(|kind| {
                [
                    InputSitePurpose::Value,
                    InputSitePurpose::Condition,
                    InputSitePurpose::Control,
                ]
                .map(|purpose| kind.origin_kind(purpose))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), 9);
        assert!(check_pointer("/a~1b/~0").is_ok());
        assert!(check_pointer("/invalid~2").is_err());
        assert!(check_pointer("/invalid~").is_err());
    }

    #[test]
    fn prospective_item_projection_is_bounded_and_does_not_interpolate_values() {
        assert!(iteration_task_scope(
            "/workflow/tasks/1/with_items",
            "/workflow/tasks/1/params/token"
        ));
        assert!(!iteration_task_scope(
            "/workflow/tasks/1/with_items",
            "/workflow/tasks/10/params/token"
        ));
        assert!(!iteration_task_scope(
            "/workflow/tasks/1/with_items",
            "/workflow/vars/token"
        ));
        let array = json!(["{{ keystore['system.not_a_lookup'] }}",{"token":"second"},"third"]);
        assert_eq!(iteration_value(&array, 0, None).unwrap(), array[0]);
        assert_eq!(
            iteration_value(&array, 1, Some(2)).unwrap(),
            json!(["third"])
        );
        assert!(iteration_value(&array, 2, Some(2)).is_err());
        assert!(iteration_value(&array, 0, Some(0)).is_err());
        assert!(iteration_value(&array, usize::MAX, Some(2)).is_err());
        let scalar = json!("{{ opaque scalar }}");
        assert_eq!(iteration_value(&scalar, 0, None).unwrap(), scalar);
        assert_eq!(iteration_value(&scalar, 0, Some(1)).unwrap(), scalar);
        assert_eq!(
            iteration_value(&scalar, 0, Some(2)).unwrap(),
            json!([scalar])
        );
        assert_eq!(iteration_value(&array, 1, Some(1)).unwrap(), array[1]);
        assert_eq!(iteration_value(&Value::Null, 0, None).unwrap(), Value::Null);
        assert!(iteration_value(&scalar, 1, None).is_err());
        let source = SelectionSource::Iteration {
            receipt: 11,
            site: InputSite {
                execution: 5,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Control,
                path: "/workflow/tasks/0/with_items".into(),
            },
            ordinal: 1,
            batch_size: None,
        };
        let values = vec![
            BoundValue {
                path: "/item".into(),
                value: iteration_value(&array, 1, None).unwrap(),
                source: source.clone(),
            },
            BoundValue {
                path: "/index".into(),
                value: json!(1),
                source,
            },
        ];
        let (value, fields) = render(
            "{{ item.token + string(index) }}",
            &TraceContext { values: &values },
        )
        .unwrap();
        assert_eq!(value, json!("second1"));
        assert_eq!(selected_sources(&values, &fields).unwrap().len(), 2);
    }

    #[test]
    fn bare_transition_site_is_not_an_arbitrary_data_field_named_when() {
        assert!(transition_condition("/workflow/tasks/task/next/0/when"));
        assert!(transition_condition(
            "/workflow/tasks/task/tasks/nested/next/1/when"
        ));
        assert!(!transition_condition(
            "/workflow/tasks/task/input/next/0/when"
        ));
        assert!(!transition_condition("/workflow/vars/next/0/when"));
    }
}

async fn locked_approval(
    conn: &mut PgConnection,
    declaration: Id,
    pack: Id,
    approval: Id,
) -> Result<super::runtime_authority::ExternalApprovalRecord> {
    let exists:Option<Id>=sqlx::query_scalar("SELECT a.id FROM runtime_external_approval a JOIN runtime_declared_input_approval binding ON binding.approval = a.id WHERE binding.declaration = $1 AND a.id = $2 FOR SHARE OF a")
        .bind(declaration).bind(approval).fetch_optional(&mut *conn).await?;
    if exists.is_none() {
        return Err(denied());
    }
    let approval = RuntimeAuthorityRepository::find_approval(conn, approval)
        .await?
        .ok_or_else(denied)?;
    if approval.subject_pack != pack || !approval.is_active(chrono::Utc::now()) {
        return Err(denied());
    }
    approval.approval()?;
    Ok(approval)
}

async fn input_approval(
    conn: &mut PgConnection,
    declaration: Id,
    pack: Id,
    approval: Id,
    target: &super::runtime_target::ResolvedRuntimeTarget,
    key: Id,
    encrypted: bool,
) -> Result<Option<ApprovalEffectFence>> {
    let approval = locked_approval(conn, declaration, pack, approval).await?;
    let mut required = vec![crate::rbac::Action::Read];
    if encrypted {
        required.push(crate::rbac::Action::Decrypt);
    }
    for action in required {
        let mut allowed = false;
        for grant in approval.grants.iter() {
            if !grant.allows(crate::rbac::Resource::Keys, action, target.context()) {
                continue;
            }
            let binding:Option<sqlx::types::Json<crate::runtime_authority::GrantTargetBindings>>=sqlx::query_scalar("SELECT binding FROM runtime_external_approval_target_binding WHERE approval = $1 AND grant_digest = $2")
                .bind(approval.id).bind(hash(&serde_json::to_value(grant)?)?).fetch_optional(&mut *conn).await?;
            if binding
                .as_ref()
                .is_some_and(|binding| super::runtime_target_binding::matches(&binding.0, target))
            {
                allowed = true;
                break;
            }
        }
        if !allowed {
            return Ok(None);
        }
    }
    if target.resource() != crate::rbac::Resource::Keys || key <= 0 {
        return Err(denied());
    }
    Ok(Some(ApprovalEffectFence::from_input_approval(
        approval.expires_at,
    )?))
}

async fn retained_input_approval(
    conn: &mut PgConnection,
    declaration: Id,
    pack: Id,
    approval: Id,
    origin: &SecretOrigin,
    target: &crate::runtime_authority::BoundRuntimeTarget,
) -> Result<ApprovalEffectFence> {
    let approval = locked_approval(conn, declaration, pack, approval).await?;
    let body = serde_json::to_value(origin)?;
    let proof:Option<(Value,Value)>=sqlx::query_as("SELECT origin,target FROM runtime_declared_input_key_read WHERE declaration = $1 AND approval = $2 AND origin_digest = $3")
        .bind(declaration).bind(approval.id).bind(hash(&body)?).fetch_optional(&mut *conn).await?;
    let (saved_origin, saved_target) = proof.ok_or_else(denied)?;
    if saved_origin != body || saved_target != serde_json::to_value(target)? {
        return Err(denied());
    }
    ApprovalEffectFence::from_input_approval(approval.expires_at)
}
