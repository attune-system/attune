//! Source-first native transitions, successor frames and terminal outputs. Publications
//! can consume earlier opaque VALUE stages, including overwrites.
//! True and false decisions retain opaque CONTROL tokens, never Boolean variable
//! values. Owning cache/ordinary/wait writers retain their separate target phases.
use super::native_cache_materialization::profile::{measure, Operation as Op};
mod wait_held;
use super::{
    declared_input::{DeclaredInputBinding, DeclaredInputRepository},
    execution::ExecutionRepository,
    execution_admission::ExecutionAdmissionRepository,
    execution_secret_value::ExecutionSecretValueRepository,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        PreparedInputSource, PreparedReceiptDestination, PreparedStagedValue,
        PreparedSystemSnapshot, PreparedWorkflowOutcome, ReceiptContext, RenderSession,
        RenderedReceiptContext,
    },
    native_failure_materialization::{
        CanonicalCompletionTask, PreparedNativeResult, PreparedResultCondition,
        PreparedResultShorthand, ResultOutputDestination,
    },
    native_workflow_materialization::{
        check_bounds, collect_leaves, contains_template, escape, expression_functions,
        expression_names, public_leaves, reserved,
    },
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository, RuntimeOwner},
    workflow::WorkflowExecutionRepository,
    FindById, Update,
};
use crate::{
    models::{Execution, ExecutionStatus, Id, WorkflowExecution},
    secret_provenance::SecretProvenance,
    secret_values::{PreparedSecretValue, SecretPathSource, SecretSource, StoredSecretValue},
    Error, Result,
};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};

mod frame;
pub use frame::{NativeTransitionRenderFrame, PreparedNativeOrdinaryTransitions};
mod iteration;
pub use iteration::PreparedNativeIterationTransitions;

pub(super) struct Field {
    pub(super) site: InputSite,
    pub(super) paths: Vec<String>,
    pub(super) source: Option<PreparedInputSource>,
}
mod empty_iteration;
mod held;
pub use empty_iteration::PreparedNativeEmptyTransitions;

pub(super) struct Sources {
    empty_iteration:
        Option<std::sync::Arc<super::native_iteration_materialization::PreparedNativeIteration>>,
    transaction: String,
    parent: Execution,
    encryption_key_hash: String,
    pub(super) fields: Vec<Field>,
    pub(super) system: PreparedSystemSnapshot,
    outcome: Option<PreparedWorkflowOutcome>,
    completed: bool,
    fence: ApprovalEffectFence,
    iteration: Option<IterationOutcome>,
    native_result: Option<PreparedNativeResult>,
    child_versions: Vec<(Id, String)>,
    // Structural mapping from the actual ordinary owning factory, never group DATA.
    completion_task: Option<CanonicalCompletionTask>,
    ordinary_task_path: Option<String>,
}
struct IterationOutcome {
    task_path: String,
    proof: std::sync::Arc<super::native_input_snapshot::PreparedNativeIterationOutcome>,
}
struct Output {
    value: Value,
    contract: Option<crate::value_schema::Contract>,
    leaves: Vec<(String, RenderedReceiptContext, ResultOutputDestination)>,
}
struct Publication {
    name: String,
    value: Value,
    leaves: Vec<(String, PublicationProof)>,
}
#[expect(
    clippy::large_enum_variant,
    reason = "Inline variants retain distinct prepared publication proofs without introducing shared ownership"
)]
enum PublicationProof {
    WholeResult(PreparedResultShorthand),
    Staged(PreparedStagedValue),
    Independent(RenderedReceiptContext, PreparedReceiptDestination),
}
struct Transition {
    fires: bool,
    condition: Option<ConditionProof>,
    publications: Vec<Publication>,
}
enum ConditionProof {
    Ordinary(PreparedInputSource),
    Native(PreparedResultCondition),
}
impl ConditionProof {
    fn source(&self) -> &PreparedInputSource {
        match self {
            Self::Ordinary(source) => source,
            Self::Native(condition) => condition.source(),
        }
    }
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        match self {
            Self::Ordinary(source) => source.ensure_fresh_on(conn).await,
            Self::Native(condition) => condition.ensure_fresh_on(conn).await,
        }
    }
    fn ensure_fresh(&self) -> Result<()> {
        match self {
            Self::Ordinary(source) => source.ensure_fresh(),
            Self::Native(condition) => condition.ensure_fresh(),
        }
    }
}

pub struct PreparedNativeTransitions {
    empty_completion: Option<super::native_input_snapshot::PreparedEmptySuccessWitness>,
    workflow: WorkflowExecution,
    task_name: Option<String>,
    // Canonical comparison structure, possibly parser-derived for legacy tasks.
    // Receipt declarations/directives always use their original immutable sites.
    authored: Vec<Value>,
    transitions: Vec<Transition>,
    original_secrets: Vec<StoredSecretValue>,
    recorded: std::sync::OnceLock<Vec<ReceiptContext>>,
    output: Option<Output>,
    output_receipts: std::sync::OnceLock<Vec<ReceiptContext>>,
    inherited_controls: Vec<SecretPathSource>,
    cache_sources: std::sync::Arc<Sources>,
    cache_publication: std::sync::OnceLock<PublishedCacheFrame>,
}

mod wait_successor;
pub use wait_successor::PreparedNativeWaitPredecessor;

struct PublishedCacheFrame {
    variables: Value,
    fields: Vec<PreparedSecretValue>,
}
pub struct RecordedNativeTransitions {
    pub publications: Vec<(usize, String, Value, Vec<SecretPathSource>)>,
    pub variables: Value,
    pub secrets: Vec<PreparedSecretValue>,
}
impl PreparedNativeTransitions {
    /// Read-only observations from the genuine owning source factory. These do
    /// not issue receipts or permit caller replacement of any prepared value.
    pub fn original_parent(&self) -> &Execution {
        &self.cache_sources.parent
    }
    pub fn source_transaction(&self) -> &str {
        &self.cache_sources.transaction
    }
    pub fn completion_outcome(&self) -> Option<&PreparedWorkflowOutcome> {
        self.cache_sources.outcome.as_ref()
    }
    pub fn prepared_output_contract(&self) -> Option<&crate::value_schema::Contract> {
        self.output
            .as_ref()
            .and_then(|output| output.contract.as_ref())
    }
    pub fn prepared_output_leaves(&self) -> impl Iterator<Item = (&str, &RenderedReceiptContext)> {
        self.output.iter().flat_map(|output| {
            output
                .leaves
                .iter()
                .map(|(path, rendered, _)| (path.as_str(), rendered))
        })
    }
    pub fn original_variable_fields(&self) -> &[StoredSecretValue] {
        &self.original_secrets
    }
    pub fn require_source_phase(&self) -> Result<()> {
        if self.recorded.get().is_some()
            || self.output_receipts.get().is_some()
            || self.cache_publication.get().is_some()
        {
            return Err(changed());
        }
        Ok(())
    }
    pub fn require_original_child_rows(&self, actual: &[Execution]) -> Result<()> {
        if child_versions(actual)? != self.cache_sources.child_versions {
            return Err(changed());
        }
        Ok(())
    }
    pub async fn require_original_result_view_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(result) = &self.cache_sources.native_result {
            result.require_current(conn).await?;
        }
        Ok(())
    }
    /// Canonical DATA already prepared by A. Context reconstruction must not
    /// decode this source again or parse any returned string as a template.
    pub fn native_result_view(&self, child: Id) -> Option<&Value> {
        self.cache_sources
            .native_result
            .as_ref()
            .filter(|result| result.execution() == child)
            .map(|result| result.value())
    }
    pub fn uses_native_result_projection(&self) -> bool {
        self.cache_sources
            .native_result
            .as_ref()
            .is_some_and(|result| result.view_only())
            || self
                .transitions
                .iter()
                .flat_map(|transition| &transition.publications)
                .flat_map(|publication| &publication.leaves)
                .any(|(_, proof)| matches!(proof, PublicationProof::WholeResult(_)))
    }
    pub fn needs_native_success_metadata(&self) -> bool {
        self.uses_native_result_projection()
            && self
                .output
                .as_ref()
                .is_none_or(|output| output.value.get("succeeded").is_none())
    }
    /// The same single-pass prepared Conditions used by advancement, including
    /// false decisions. Generated failure output retains them as CONTROL only.
    pub fn completion_decisions(&self) -> Vec<&PreparedInputSource> {
        decision_controls(&self.transitions)
    }
    pub(super) async fn prepared_frame_sources(
        &self,
        conn: &mut PgConnection,
        parent: &Execution,
        workflow: &WorkflowExecution,
        key: &str,
    ) -> Result<std::sync::Arc<Sources>> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        self.require_current(workflow)?;
        if transaction != self.cache_sources.transaction
            || serde_json::to_value(parent)? != serde_json::to_value(&self.cache_sources.parent)?
            || crate::crypto::hash_encryption_key(key) != self.cache_sources.encryption_key_hash
        {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        Ok(self.cache_sources.clone())
    }
    pub(super) async fn bind_prepared_frame(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        binding: &DeclaredInputBinding,
        key: &str,
    ) -> Result<()> {
        bind_evaluation_sources(
            conn,
            session,
            binding,
            &self.cache_sources,
            &decision_controls(&self.transitions),
            &all_publications(&self.transitions, &[]),
            key,
        )
        .await
    }

    pub(super) fn attach_frame_literal(&self, rendered: &mut RenderedReceiptContext) -> Result<()> {
        attach_literal_controls(
            rendered,
            &self.cache_sources,
            &decision_controls(&self.transitions),
        )
    }

    pub(super) async fn require_frame_publication(&self, conn: &mut PgConnection) -> Result<()> {
        let expected = self.cache_publication.get().ok_or_else(changed)?;
        self.ensure_fresh_on(conn).await?;
        let actual = WorkflowExecutionRepository::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(changed)?;
        let fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            self.execution(),
        )
        .await?;
        let mut expected_fields = expected
            .fields
            .iter()
            .map(|field| {
                (
                    field.json_path.clone(),
                    field.encrypted_value.clone(),
                    Some(field.encryption_key_hash.clone()),
                    field.source_kind.clone(),
                    field.source_ref.clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut actual_fields = fields
            .into_iter()
            .map(|field| {
                (
                    field.json_path,
                    field.encrypted_value,
                    field.encryption_key_hash,
                    field.source_kind,
                    field.source_ref,
                )
            })
            .collect::<Vec<_>>();
        expected_fields.sort_by(|a, b| a.0.cmp(&b.0));
        actual_fields.sort_by(|a, b| a.0.cmp(&b.0));
        if actual.variables != expected.variables || actual_fields != expected_fields {
            return Err(changed());
        }
        Ok(())
    }

    pub fn output(&self) -> Option<&Value> {
        self.output.as_ref().map(|output| &output.value)
    }

    /// The owning finalizer passes the actual successful result AFTER schema
    /// diagnostics/status metadata are added, before ciphertext persistence.
    /// Each authored leaf must still match its one evaluated opaque value.
    pub async fn record_output(
        &self,
        conn: &mut PgConnection,
        contract: Option<&crate::value_schema::Contract>,
        actual: &Value,
        key: &str,
    ) -> Result<Vec<SecretPathSource>> {
        check_bounds(actual)?;
        if self
            .transitions
            .iter()
            .any(|transition| !transition.publications.is_empty())
            && self.recorded.get().is_none()
        {
            return Err(changed());
        }
        let output = self.output.as_ref().ok_or_else(changed)?;
        if contract != output.contract.as_ref() {
            return Err(changed());
        }
        let mut expected = output.value.clone();
        expected
            .as_object_mut()
            .ok_or_else(changed)?
            .entry("succeeded".to_owned())
            .or_insert(Value::Bool(true));
        if let Some(contract) = contract {
            let compiled =
                crate::value_schema::CompiledContract::new(contract).map_err(|_| changed())?;
            if let Err(error) = compiled.validate(&expected) {
                expected["output_validation_warning"] =
                    serde_json::json!({"code":error.code,"path":error.path});
            }
        }
        if actual != &expected {
            return Err(changed());
        }
        if self.uses_native_result_projection()
            && expected.get("output_validation_warning").is_some()
        {
            // A's finite success-warning recipe is distinct from authored VALUE
            // leaves and is not implemented by a ResultView or shorthand proof.
            return Err(changed());
        }
        let mut receipts = Vec::new();
        let mut sources = Vec::new();
        for (path, rendered, destination) in &output.leaves {
            let actual = actual.pointer(path).ok_or_else(changed)?;
            let receipt = if let Some(result) = &self.cache_sources.native_result {
                result
                    .record_output(conn, rendered, destination, actual, key)
                    .await?
            } else {
                let ResultOutputDestination::Ordinary(destination) = destination else {
                    return Err(changed());
                };
                Inputs::record_prepared(
                    conn,
                    rendered,
                    destination,
                    actual,
                    rendered.selections(),
                    key,
                )
                .await?
            };
            sources.push(SecretPathSource {
                path: path.clone(),
                source: SecretSource::Bound(receipt.provenance()),
            });
            receipts.push(receipt);
        }
        self.output_receipts.set(receipts).map_err(|_| changed())?;
        self.ensure_fresh_on(conn).await?;
        Ok(sources)
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        &self.workflow
    }
    pub fn execution(&self) -> Id {
        self.workflow.execution
    }
    pub fn fires(&self, index: usize) -> Result<bool> {
        self.transitions
            .get(index)
            .map(|transition| transition.fires)
            .ok_or_else(changed)
    }
    pub fn require_transitions(&self, actual: &Value) -> Result<()> {
        let actual = actual.as_array().ok_or_else(changed)?;
        if let Some(saved) = self.task_name.as_ref().and_then(|name| {
            self.workflow
                .task_graph
                .get("nodes")?
                .get(name)?
                .get("transitions")
        }) {
            if saved.as_array() != Some(actual) {
                return Err(changed());
            }
        }
        if actual.len() != self.authored.len() {
            return Err(changed());
        }
        for (actual, authored) in actual.iter().zip(&self.authored) {
            if actual.get("when").filter(|value| !value.is_null())
                != authored.get("when").filter(|value| !value.is_null())
            {
                return Err(changed());
            }
            // Reuse the SAME canonical ordering check as preparation, including
            // genuine string shorthand. No parallel map-only publication walk.
            let ordered = publication_order(authored, Some(actual))?;
            let published = actual
                .get("publish")
                .and_then(Value::as_array)
                .ok_or_else(changed)?;
            if ordered.len() != published.len()
                || actual.get("do_tasks")
                    != Some(authored.get("do").unwrap_or(&serde_json::json!([])))
            {
                return Err(changed());
            }
        }
        Ok(())
    }
    /// Compare the complete source workflow after acquiring its outer row lock.
    /// No source preparation, selector evaluation or authority lookup occurs here.
    pub fn require_current(&self, actual: &WorkflowExecution) -> Result<()> {
        if serde_json::to_value(actual)? != serde_json::to_value(&self.workflow)? {
            return Err(changed());
        }
        for transition in &self.transitions {
            for publication in &transition.publications {
                for (_, proof) in &publication.leaves {
                    if let PublicationProof::WholeResult(shorthand) = proof {
                        shorthand.require_original_workflow(actual)?;
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(empty) = &self.empty_completion {
            empty.ensure_fresh_on(conn).await?;
        }
        if let Some(group) = &self.cache_sources.iteration {
            group.proof.ensure_fresh_on(conn).await?;
        }
        self.cache_sources.fence.ensure_fresh_on(conn).await?;
        if let Some(result) = &self.cache_sources.native_result {
            result.ensure_fresh_on(conn).await?;
        }
        for source in self
            .cache_sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
        {
            source.ensure_fresh_on(conn).await?;
        }
        if let Some(receipts) = self.recorded.get() {
            for receipt in receipts {
                receipt.ensure_fresh_on(conn).await?;
            }
        }
        if let Some(receipts) = self.output_receipts.get() {
            for receipt in receipts {
                receipt.ensure_fresh_on(conn).await?;
            }
        }
        if let Some(output) = &self.output {
            for (_, rendered, destination) in &output.leaves {
                rendered.ensure_fresh_on(conn).await?;
                destination.ensure_fresh_on(conn).await?;
            }
        }
        for transition in &self.transitions {
            if let Some(condition) = &transition.condition {
                condition.ensure_fresh_on(conn).await?;
            }
            for publication in &transition.publications {
                for (_, proof) in &publication.leaves {
                    match proof {
                        PublicationProof::WholeResult(shorthand) => {
                            shorthand.ensure_fresh_on(conn).await?
                        }
                        PublicationProof::Staged(staged) => staged.ensure_fresh_on(conn).await?,
                        PublicationProof::Independent(rendered, _) => {
                            rendered.ensure_fresh_on(conn).await?
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(empty) = &self.empty_completion {
            empty.ensure_fresh()?;
        }
        if let Some(group) = &self.cache_sources.iteration {
            group.proof.ensure_fresh()?;
        }
        self.cache_sources.fence.ensure_fresh()?;
        if let Some(result) = &self.cache_sources.native_result {
            result.ensure_fresh()?;
        }
        if let Some(output) = &self.output {
            for (_, rendered, destination) in &output.leaves {
                rendered.ensure_fresh()?;
                destination.ensure_fresh()?;
            }
        }
        for source in self
            .cache_sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
        {
            source.ensure_fresh()?;
        }
        if let Some(receipts) = self.recorded.get() {
            for receipt in receipts {
                receipt.ensure_fresh()?;
            }
        }
        if let Some(receipts) = self.output_receipts.get() {
            for receipt in receipts {
                receipt.ensure_fresh()?;
            }
        }
        for transition in &self.transitions {
            if let Some(condition) = &transition.condition {
                condition.ensure_fresh()?;
            }
        }
        for transition in &self.transitions {
            for publication in &transition.publications {
                for (_, proof) in &publication.leaves {
                    match proof {
                        PublicationProof::Staged(staged) => staged.ensure_fresh()?,
                        PublicationProof::WholeResult(shorthand) => shorthand.ensure_fresh()?,
                        PublicationProof::Independent(_, _) => {}
                    }
                }
            }
        }
        Ok(())
    }
    pub fn variable_sources(&self) -> Result<Vec<(String, SecretSource)>> {
        self.original_secrets
            .iter()
            .map(|field| {
                if field.source_kind != "provenance" {
                    return Err(changed());
                }
                Ok((
                    field.json_path.clone(),
                    SecretSource::Bound(serde_json::from_str(
                        field.source_ref.as_deref().ok_or_else(changed)?,
                    )?),
                ))
            })
            .collect()
    }
    /// Persist/finalize terminal stages in order, including actual intermediate
    /// overwrites. Only already-prepared authority is used behind the target lock.
    pub async fn record(
        &self,
        conn: &mut PgConnection,
        actual: &WorkflowExecution,
        key: &str,
    ) -> Result<RecordedNativeTransitions> {
        self.require_current(actual)?;
        if self.recorded.get().is_some() {
            return Err(changed());
        }
        let locked = WorkflowExecutionRepository::find_by_id_for_update(&mut *conn, actual.id)
            .await?
            .ok_or_else(changed)?;
        self.require_current(&locked)?;
        let stored = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            actual.execution,
        )
        .await?;
        if stored != self.original_secrets {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        if let Some(group) = &self.cache_sources.iteration {
            let parent = ExecutionRepository::find_by_id(&mut *conn, actual.execution)
                .await?
                .ok_or_else(changed)?;
            group.proof.require_original_rows(&parent, &locked)?;
            group.proof.require_source_current(conn).await?;
        }
        if let Some(result) = &self.cache_sources.native_result {
            result.finalize(conn).await?;
        }
        let mut variables = actual.variables.clone();
        let mut secrets: Vec<PreparedSecretValue> = self
            .original_secrets
            .iter()
            .map(|field| {
                Ok(PreparedSecretValue {
                    json_path: field.json_path.clone(),
                    encrypted_value: field.encrypted_value.clone(),
                    encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(changed)?,
                    source_kind: field.source_kind.clone(),
                    source_ref: field.source_ref.clone(),
                })
            })
            .collect::<Result<_>>()?;
        let mut publications = Vec::new();
        let mut receipts = Vec::new();
        for (index, transition) in self.transitions.iter().enumerate() {
            if let Some(ConditionProof::Native(condition)) = &transition.condition {
                condition.finalize(conn).await?;
            }
            for publication in &transition.publications {
                let mut sources = Vec::new();
                for (suffix, proof) in &publication.leaves {
                    let provenance = match proof {
                        PublicationProof::WholeResult(shorthand) => {
                            if shorthand.value() != &publication.value {
                                return Err(changed());
                            }
                            shorthand.receipt().provenance()
                        }
                        PublicationProof::Staged(staged) => staged.provenance(),
                        PublicationProof::Independent(rendered, destination) => {
                            let receipt = Inputs::record_prepared(
                                conn,
                                rendered,
                                destination,
                                rendered.value(),
                                rendered.selections(),
                                key,
                            )
                            .await?;
                            let provenance = receipt.provenance();
                            receipts.push(receipt);
                            provenance
                        }
                    };
                    sources.push(SecretPathSource {
                        path: suffix.clone(),
                        source: SecretSource::Bound(provenance),
                    });
                }
                let path = format!("/{}", escape(&publication.name));
                secrets.retain(|field| {
                    crate::secret_values::pointer_suffix(&field.json_path, &path).is_none()
                });
                let (redacted, pending) = crate::secret_values::redact_secret_path_sources(
                    publication.value.clone(),
                    &sources,
                );
                for mut field in crate::secret_values::prepare_secret_values(pending, key)? {
                    field.json_path = crate::secret_values::pointer_join(&path, &field.json_path);
                    secrets.push(field);
                }
                variables
                    .as_object_mut()
                    .ok_or_else(changed)?
                    .insert(publication.name.clone(), redacted);
                check_bounds(&variables)?;
                if secrets.len() > 256 || serde_json::to_vec(&secrets)?.len() > 1024 * 1024 {
                    return Err(changed());
                }
                // Persist each publication in program order, including real
                // intermediate overwrites. Finalize only after the exact value
                // and that stage's own field provenance are actually stored.
                if publication.leaves.iter().any(|(_, proof)| {
                    matches!(
                        proof,
                        PublicationProof::Staged(_) | PublicationProof::WholeResult(_)
                    )
                }) {
                    ExecutionSecretValueRepository::replace_entity_with_conn(
                        conn,
                        crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
                        actual.execution,
                        &secrets,
                    )
                    .await?;
                    WorkflowExecutionRepository::update(
                        &mut *conn,
                        actual.id,
                        super::workflow::UpdateWorkflowExecutionInput {
                            variables: Some(variables.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;
                    for (_, proof) in &publication.leaves {
                        if let PublicationProof::WholeResult(shorthand) = proof {
                            shorthand.finalize(conn, key).await?;
                        }
                        if let PublicationProof::Staged(staged) = proof {
                            Inputs::finalize_staged_value(conn, staged, key).await?;
                        }
                    }
                }
                publications.push((
                    index,
                    publication.name.clone(),
                    publication.value.clone(),
                    sources,
                ));
            }
        }
        check_bounds(&variables)?;
        if secrets.len() > 256 || serde_json::to_vec(&secrets)?.len() > 1024 * 1024 {
            return Err(changed());
        }
        // Conditions remain held by the plan through commit/publication. The
        // separately recorded Value receipts already include their control edges.
        self.recorded.set(receipts).map_err(|_| changed())?;
        self.cache_publication
            .set(PublishedCacheFrame {
                variables: variables.clone(),
                fields: secrets.clone(),
            })
            .map_err(|_| changed())?;
        self.ensure_fresh_on(conn).await?;
        Ok(RecordedNativeTransitions {
            publications,
            variables,
            secrets,
        })
    }
    pub fn control_sources(&self) -> Vec<SecretPathSource> {
        self.inherited_controls
            .iter()
            .cloned()
            .chain(
                self.transitions
                    .iter()
                    .filter_map(|transition| transition.condition.as_ref())
                    .map(|condition| SecretPathSource {
                        path: String::new(),
                        source: SecretSource::Bound(SecretProvenance {
                            origins: vec![condition.source().origin()],
                            templates: vec![],
                        }),
                    }),
            )
            .collect()
    }
}

pub struct NativeTransitionMaterializationRepository;
impl NativeTransitionMaterializationRepository {
    /// Initial variables have already been committed as actual typed fields.
    /// Prepare the zero-task output in its new owning completion transaction,
    /// before capture/workflow target locks. No child or fake outcome is supplied.
    pub async fn prepare_empty_workflow_output(
        conn: &mut PgConnection,
        execution: Id,
        key: &str,
    ) -> Result<Option<PreparedNativeTransitions>> {
        let parent = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(changed)?;
        if ExecutionAdmissionRepository::find(conn, RuntimeOwner::Execution(execution))
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        if !graph.get("tasks").is_some_and(|tasks| match tasks {
            Value::Array(tasks) => tasks.is_empty(),
            Value::Object(tasks) => tasks.is_empty(),
            _ => false,
        }) {
            return Ok(None);
        }
        let Some(authored) = graph
            .get("output_map")
            .filter(|value| value.as_object().is_some_and(|fields| !fields.is_empty()))
        else {
            return Ok(None);
        };
        if !empty_output_context(authored)? {
            return Ok(None);
        }
        if !matches!(
            parent.status,
            ExecutionStatus::Requested
                | ExecutionStatus::Scheduling
                | ExecutionStatus::Scheduled
                | ExecutionStatus::Running
        ) {
            return Err(changed());
        }
        let workflow = WorkflowExecutionRepository::find_by_execution(&mut *conn, execution)
            .await?
            .ok_or_else(changed)?;
        if !workflow
            .task_graph
            .get("nodes")
            .and_then(Value::as_object)
            .is_some_and(|nodes| nodes.is_empty())
            || !matches!(
                workflow.status,
                ExecutionStatus::Requested
                    | ExecutionStatus::Scheduling
                    | ExecutionStatus::Scheduled
                    | ExecutionStatus::Running
            )
        {
            return Err(changed());
        }
        let bindings = DeclaredInputRepository::bindings(conn, execution)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| changed())?;
        let Some(sources) = prepare_sources(conn, &parent, None, &workflow, key).await? else {
            return Ok(None);
        };
        let Some(output) = prepare_output(
            conn,
            execution,
            graph,
            &[],
            &workflow,
            &[],
            &bindings,
            &sources,
            &[],
            &revision,
            key,
        )
        .await?
        else {
            return Ok(None);
        };
        let original_secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            execution,
        )
        .await?;
        let plan = PreparedNativeTransitions {
            empty_completion: None,
            workflow,
            task_name: None,
            authored: Vec::new(),
            transitions: Vec::new(),
            original_secrets,
            recorded: std::sync::OnceLock::new(),
            output: Some(output),
            output_receipts: std::sync::OnceLock::new(),
            inherited_controls: inherited_control_sources(&sources),
            cache_sources: std::sync::Arc::new(sources),
            cache_publication: std::sync::OnceLock::new(),
        };
        plan.ensure_fresh_on(conn).await?;
        Ok(Some(plan))
    }

    /// Called in the owning transaction BEFORE the workflow advisory/row lock or
    /// cache iteration target locks. None means outside this incremental path's
    /// scope, never an authorization failure or permission fallback.
    pub async fn prepare_for_completion(
        conn: &mut PgConnection,
        completed: &Execution,
        key: &str,
    ) -> Result<Option<PreparedNativeTransitions>> {
        Self::prepare_completion(conn, completed, key, false).await
    }

    // Owning successor planners select this mode only after proving the actual
    // successor graph and before acquiring any outer target locks.
    pub(super) async fn prepare_cache_activation(
        conn: &mut PgConnection,
        completed: &Execution,
        key: &str,
    ) -> Result<Option<PreparedNativeTransitions>> {
        Self::prepare_completion(conn, completed, key, true).await
    }

    async fn prepare_completion(
        conn: &mut PgConnection,
        completed: &Execution,
        key: &str,
        successor_activation: bool,
    ) -> Result<Option<PreparedNativeTransitions>> {
        let Some(metadata) = &completed.workflow_task else {
            return Ok(None);
        };
        if metadata.task_index.is_some()
            || !(matches!(
                completed.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Timeout
            ) || successor_activation
                && matches!(
                    completed.status,
                    ExecutionStatus::Cancelled | ExecutionStatus::Abandoned
                ))
        {
            return Ok(None);
        }
        let actual_child = ExecutionRepository::find_by_id(&mut *conn, completed.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(&actual_child)? != serde_json::to_value(completed)? {
            return Err(changed());
        }
        let workflow =
            WorkflowExecutionRepository::find_by_id(&mut *conn, metadata.workflow_execution)
                .await?
                .ok_or_else(changed)?;
        if matches!(
            workflow.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Canceling
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Timeout
                | ExecutionStatus::Abandoned
        ) {
            return Ok(None);
        }
        let parent = ExecutionRepository::find_by_id(&mut *conn, workflow.execution)
            .await?
            .ok_or_else(changed)?;
        if matches!(
            parent.status,
            ExecutionStatus::Canceling | ExecutionStatus::Cancelled
        ) {
            return Ok(None);
        }
        if ExecutionAdmissionRepository::find(conn, RuntimeOwner::Execution(parent.id))
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let empty_completion = if !successor_activation && !plain_tasks(graph)? {
            let Some((empty, ordinary)) = super::native_failure_materialization::NativeFailureMaterializationRepository::empty_success_tasks(graph)? else { return Ok(None); };
            if metadata.task_name != ordinary
                || metadata.triggered_by.as_deref() != Some(empty.as_str())
                || completed.parent != Some(parent.id)
                || completed.status != ExecutionStatus::Completed
            {
                return Ok(None);
            }
            Some(super::native_input_snapshot::NativeInputSnapshotRepository::prepare_empty_success_witness(
                conn, &parent, &workflow, &empty, key).await?)
        } else {
            None
        };
        let bindings = DeclaredInputRepository::bindings(conn, parent.id).await?;
        let task_path = super::declared_input_materialization::DeclaredInputMaterializationRepository::workflow_task_site_path(
            conn, parent.id, &metadata.task_name, &completed.action_ref,
        ).await?;
        let task = graph
            .pointer(task_path.strip_prefix("/workflow").ok_or_else(changed)?)
            .ok_or_else(changed)?;
        if task
            .get("iterate_cache")
            .is_some_and(|value| !value.is_null())
            || task.get("with_items").is_some_and(|value| !value.is_null())
            || task.get("wait_for").is_some_and(|value| !value.is_null())
        {
            return Ok(None);
        }
        let Some(completion_task) = CanonicalCompletionTask::prepare(task)? else {
            return Ok(None);
        };
        let canonical_next = completion_task.next().to_vec();
        let next = canonical_next.as_slice();
        // This canonical parser-derived condition is backed by the actual
        // completed outcome, not an invented authored /next/0/when declaration.
        if completion_task.legacy_publish() && completed.status != ExecutionStatus::Completed {
            return Ok(None);
        }
        let staged = completion_uses_stages(graph, completed.status, successor_activation)?
            || empty_completion.is_some();
        let terminal_output_only = next.is_empty()
            && (completed.status == ExecutionStatus::Completed
                || graph
                    .get("output_map")
                    .and_then(Value::as_object)
                    .is_some_and(|fields| !fields.is_empty()));
        if !terminal_output_only && !eligible_for(next, staged)? {
            return Ok(None);
        }
        let bindings: BTreeMap<_, _> = bindings
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| changed())?;
        let original_secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            parent.id,
        )
        .await?;
        if original_secrets
            .iter()
            .any(|field| field.json_path.is_empty())
        {
            return Ok(None);
        }
        let Some(mut sources) = measure(
            Op::RootSources,
            prepare_sources(conn, &parent, Some(completed), &workflow, key),
        )
        .await
        .map_err(|error| source_phase_error(error, "prepare completion sources", &task_path))?
        else {
            return Ok(None);
        };
        if completion_task.legacy_publish()
            && !sources
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.value("succeeded") == Some(Value::Bool(true)))
        {
            return Err(changed());
        }
        if staged && (!sources.completed || !workflow.failed_tasks.is_empty()) {
            return Ok(None);
        }
        sources.ordinary_task_path = Some(task_path.clone());
        sources.completion_task = Some(completion_task);
        let transitions = Self::prepare_sequence(
            conn,
            parent.id,
            &metadata.task_name,
            &task_path,
            next,
            &workflow,
            &bindings,
            &sources,
            &revision,
            staged,
            key,
        )
        .await?;
        let output_controls = decision_controls(&transitions);
        let output = prepare_output(
            conn,
            parent.id,
            graph,
            next,
            &workflow,
            &transitions,
            &bindings,
            &sources,
            &output_controls,
            &revision,
            key,
        )
        .await?;
        drop(output_controls);
        let plan = PreparedNativeTransitions {
            empty_completion,
            workflow,
            task_name: Some(metadata.task_name.clone()),
            authored: next.to_vec(),
            transitions,
            original_secrets,
            recorded: std::sync::OnceLock::new(),
            output,
            output_receipts: std::sync::OnceLock::new(),
            inherited_controls: inherited_control_sources(&sources),
            cache_sources: std::sync::Arc::new(sources),
            cache_publication: std::sync::OnceLock::new(),
        };
        plan.ensure_fresh_on(conn).await?;
        Ok(Some(plan))
    }

    // One canonical ordered program for genuine child and genuine group sources.
    // The caller has authenticated the owning retained task and source kind.
    #[expect(
        clippy::too_many_arguments,
        reason = "The canonical ordered program binds task identity, declarations, source proofs, and staging mode explicitly"
    )]
    async fn prepare_sequence(
        conn: &mut PgConnection,
        execution: Id,
        task_name: &str,
        task_path: &str,
        next: &[Value],
        workflow: &WorkflowExecution,
        bindings: &BTreeMap<String, DeclaredInputBinding>,
        sources: &Sources,
        revision: &str,
        staged: bool,
        key: &str,
    ) -> Result<Vec<Transition>> {
        if sources.iteration.is_some()
            && (sources.native_result.is_some()
                || sources.completion_task.is_some()
                || sources.ordinary_task_path.is_some()
                || sources.outcome.is_some())
        {
            // A whole group is not a child/current-result or legacy-child gate.
            return Err(changed());
        }
        if sources
            .iteration
            .as_ref()
            .is_some_and(|group| group.task_path != task_path)
            || sources
                .ordinary_task_path
                .as_deref()
                .is_some_and(|path| path != task_path)
            || sources.completion_task.is_some() != sources.ordinary_task_path.is_some()
            || sources
                .completion_task
                .as_ref()
                .is_some_and(|task| task.next() != next)
        {
            return Err(changed());
        }
        let legacy = sources
            .completion_task
            .as_ref()
            .is_some_and(CanonicalCompletionTask::legacy_publish);
        if legacy
            && sources.outcome.as_ref().is_none_or(|outcome| {
                outcome.task_name() != task_name
                    || outcome.value("succeeded") != Some(Value::Bool(true))
            })
        {
            return Err(changed());
        }
        let mut transitions = Vec::new();
        for (index, transition) in next.iter().enumerate() {
            let mut controls = decision_controls(&transitions);
            let (fires, condition) = if legacy {
                (true, None)
            } else if transition.get("when").is_some_and(|value| !value.is_null()) {
                let path = format!("{task_path}/next/{index}/when");
                let binding = bindings.get(&path).ok_or_else(changed)?;
                let rendered = evaluate(
                    conn,
                    execution,
                    binding,
                    sources,
                    &controls,
                    &all_publications(&transitions, &[]),
                    InputSitePurpose::Condition,
                    key,
                )
                .await?;
                let fires = rendered.value().as_bool().ok_or_else(changed)?;
                let site = InputSite {
                    execution,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Condition,
                    path,
                };
                let condition = measure(Op::ConditionDecision, async {
                    if let Some(result) = &sources.native_result {
                        if result.depends_on_view(&rendered)? {
                            return Ok::<_, Error>(ConditionProof::Native(
                                result
                                    .prepare_condition(conn, &rendered, &site, key)
                                    .await?,
                            ));
                        }
                    }
                    Ok(ConditionProof::Ordinary(
                        Inputs::prepare_condition_decision(conn, &rendered, &site, key).await?,
                    ))
                })
                .await?;
                // False conditions also control later scheduling and output.
                (fires, Some(condition))
            } else {
                // Absence is actual immutable control structure, not an authored
                // "true" expression. Never create a nonexistent /when receipt.
                (true, None)
            };
            controls.extend(condition.iter().map(ConditionProof::source));
            let mut publications = Vec::new();
            if fires {
                let saved = workflow
                    .task_graph
                    .get("nodes")
                    .and_then(|nodes| nodes.get(task_name))
                    .and_then(|node| node.get("transitions"))
                    .and_then(|transitions| transitions.get(index));
                for (ordinal, name, authored) in publication_order(transition, saved)? {
                    if transition["publish"][ordinal].is_string() {
                        if sources.iteration.is_some() {
                            return Err(changed());
                        }
                        let result = sources.native_result.as_ref().ok_or_else(changed)?;
                        let directive = publication_source_path(sources, task_path, index, ordinal);
                        let shorthand = result
                            .prepare_shorthand(conn, &directive, &controls, key)
                            .await?;
                        let value = shorthand.value().clone();
                        publications.push(Publication {
                            name: name.to_owned(),
                            value,
                            leaves: vec![(String::new(), PublicationProof::WholeResult(shorthand))],
                        });
                        continue;
                    }
                    let prior = all_publications(&transitions, &publications);
                    let mut leaves = Vec::new();
                    collect_leaves(authored, String::new(), &mut leaves, 0)?;
                    let mut value = authored.clone();
                    let mut prepared = Vec::new();
                    for (suffix, leaf) in leaves {
                        let declaration = format!(
                            "{}/{}{suffix}",
                            publication_source_path(sources, task_path, index, ordinal),
                            escape(name)
                        );
                        let rendered = if let Some(binding) = bindings.get(&declaration) {
                            evaluate(
                                conn,
                                execution,
                                binding,
                                sources,
                                &controls,
                                &prior,
                                InputSitePurpose::Value,
                                key,
                            )
                            .await?
                        } else {
                            if contains_template(&leaf) {
                                return Err(changed());
                            }
                            render_literal(
                                conn,
                                execution,
                                revision,
                                &declaration,
                                sources,
                                &controls,
                            )
                            .await?
                        };
                        *value.pointer_mut(&suffix).ok_or_else(changed)? = rendered.value().clone();
                        let site = InputSite {
                            execution,
                            kind: InputEntityKind::WorkflowVariables,
                            purpose: InputSitePurpose::Value,
                            path: format!("/{}{suffix}", escape(name)),
                        };
                        let proof = if staged {
                            let value = measure(
                                Op::StagedValue,
                                Inputs::prepare_staged_value(conn, &rendered, &site, key),
                            )
                            .await?;
                            if let Some(result) = &sources.native_result {
                                result.retain_staged_lineage(&rendered, &value)?;
                            }
                            PublicationProof::Staged(value)
                        } else {
                            let destination =
                                Inputs::prepare_destination(conn, &rendered, &site, key).await?;
                            PublicationProof::Independent(rendered, destination)
                        };
                        prepared.push((suffix, proof));
                    }
                    check_bounds(&value)?;
                    drop(prior);
                    publications.push(Publication {
                        name: name.to_owned(),
                        value,
                        leaves: prepared,
                    });
                }
            }
            drop(controls);
            transitions.push(Transition {
                fires,
                condition,
                publications,
            });
        }
        Ok(transitions)
    }
}

// Keep denial semantics and expose only a fixed phase and authored/stored field
// site. Never include the selected value, expression text or decrypted input.
fn source_phase_error(error: Error, phase: &str, path: &str) -> Error {
    match error {
        Error::PermissionDenied(reason) => {
            Error::PermissionDenied(format!("Native transition {phase} at {path}: {reason}"))
        }
        error => error,
    }
}

fn changed() -> Error {
    Error::PermissionDenied("Native transition source changed or is unavailable".into())
}

// Only the ordinary owning factory sets this descriptor. Keep the original H
// mapping method; group sources retain their original explicit next sites.
fn publication_source_path(sources: &Sources, task: &str, index: usize, ordinal: usize) -> String {
    if let Some(completion) = &sources.completion_task {
        completion.publication_path(task, index, ordinal)
    } else {
        format!("{task}/next/{index}/publish/{ordinal}")
    }
}

fn empty_output_context(authored: &Value) -> Result<bool> {
    let mut leaves = Vec::new();
    collect_leaves(authored, String::new(), &mut leaves, 0)?;
    for (_, value) in leaves {
        if let Some(template) = value.as_str().filter(|value| value.contains("{{")) {
            if expression_names(template)?.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "result"
                        | "task"
                        | "tasks"
                        | "item"
                        | "index"
                        | "inquiry"
                        | "execution"
                        | "work_queue_item"
                )
            }) || expression_functions(template)?.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "result" | "succeeded" | "failed" | "timed_out" | "cancelled"
                )
            }) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn inherited_control_sources(sources: &Sources) -> Vec<SecretPathSource> {
    sources
        .fields
        .iter()
        .filter_map(|field| field.source.as_ref())
        .map(|source| SecretPathSource {
            path: String::new(),
            source: SecretSource::Bound(SecretProvenance {
                origins: vec![source.origin()],
                templates: vec![],
            }),
        })
        .collect()
}

async fn render_literal(
    conn: &mut PgConnection,
    execution: Id,
    revision: &str,
    path: &str,
    sources: &Sources,
    controls: &[&PreparedInputSource],
) -> Result<RenderedReceiptContext> {
    let mut rendered = measure(
        Op::Literal,
        Inputs::literal(conn, execution, revision, path),
    )
    .await?;
    attach_literal_controls(&mut rendered, sources, controls)?;
    Ok(rendered)
}

fn attach_literal_controls(
    rendered: &mut RenderedReceiptContext,
    sources: &Sources,
    controls: &[&PreparedInputSource],
) -> Result<()> {
    if let Some(empty) = &sources.empty_iteration {
        Inputs::attach_empty_iteration_control(
            rendered,
            empty.empty_outcome().ok_or_else(changed)?,
        )?;
    }
    if let Some(group) = &sources.iteration {
        Inputs::attach_native_iteration_control(rendered, &group.proof)?;
    }
    for source in sources
        .fields
        .iter()
        .filter_map(|field| field.source.as_ref())
        .chain(controls.iter().copied())
    {
        Inputs::attach_rendered_control(rendered, source)?;
    }
    Ok(())
}

fn all_publications<'a>(
    transitions: &'a [Transition],
    current: &'a [Publication],
) -> Vec<&'a Publication> {
    transitions
        .iter()
        .flat_map(|transition| transition.publications.iter())
        .chain(current)
        .collect()
}

fn decision_controls(transitions: &[Transition]) -> Vec<&PreparedInputSource> {
    transitions
        .iter()
        .filter_map(|transition| transition.condition.as_ref())
        .map(ConditionProof::source)
        .collect()
}

// The graph builder preserves directive order, but parses each directive's map
// into a HashMap. Its persisted PublishVar vector is the actual execution order
// within that map. Match every item back to its real immutable authored leaf.
fn publication_order<'a>(
    authored: &'a Value,
    saved: Option<&Value>,
) -> Result<Vec<(usize, &'a str, &'a Value)>> {
    let Some(directives) = authored.get("publish").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let saved = saved
        .map(|saved| {
            saved
                .get("publish")
                .and_then(Value::as_array)
                .ok_or_else(changed)
        })
        .transpose()?;
    let mut ordered = Vec::new();
    let mut offset = 0;
    for (ordinal, directive) in directives.iter().enumerate() {
        if let Some(name) = directive.as_str() {
            if let Some(saved) = saved {
                let item = saved.get(offset).ok_or_else(changed)?;
                if item.get("name").and_then(Value::as_str) != Some(name)
                    || item.get("value").and_then(Value::as_str) != Some("{{ result() }}")
                {
                    return Err(changed());
                }
            }
            ordered.push((ordinal, name, directive));
            offset += 1;
            continue;
        }
        let fields = directive.as_object().ok_or_else(changed)?;
        if let Some(saved) = saved {
            let mut seen = BTreeSet::new();
            for item in saved
                .get(offset..offset + fields.len())
                .ok_or_else(changed)?
            {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(changed)?;
                let (name, value) = fields.get_key_value(name).ok_or_else(changed)?;
                if !seen.insert(name) || item.get("value") != Some(value) {
                    return Err(changed());
                }
                ordered.push((ordinal, name.as_str(), value));
            }
        } else {
            // Repository fixtures may omit a derived graph. Singleton directives
            // have unambiguous order without it; multi-field maps do not.
            if fields.len() > 1 {
                return Err(changed());
            }
            ordered.extend(
                fields
                    .iter()
                    .map(|(name, value)| (ordinal, name.as_str(), value)),
            );
        }
        offset += fields.len();
    }
    if saved.is_some_and(|saved| saved.len() != offset) {
        return Err(changed());
    }
    Ok(ordered)
}

/// Syntax-only dependency detection. No template, condition or dynamic selector
/// is evaluated here. Inspect the entire plan before loading any selected Keys.
#[cfg(test)]
fn eligible(transitions: &[Value]) -> Result<bool> {
    eligible_for(transitions, false)
}

fn eligible_for(transitions: &[Value], staged: bool) -> Result<bool> {
    if transitions.is_empty() || transitions.len() > 128 {
        return Ok(false);
    }
    let mut written = BTreeSet::new();
    for transition in transitions {
        if let Some(condition) = transition.get("when").filter(|value| !value.is_null()) {
            let condition = condition.as_str().ok_or_else(changed)?;
            let names = expression_names(condition)?;
            if !(if staged {
                value_names_supported(&names)
            } else {
                independent(&names, &written)
            }) {
                return Ok(false);
            }
        }
        if let Some(directives) = transition.get("publish") {
            let Some(directives) = directives.as_array() else {
                return Ok(false);
            };
            for directive in directives {
                if let Some(name) = directive.as_str() {
                    if !written.insert(name.to_owned()) && !staged {
                        return Ok(false);
                    }
                    continue;
                }
                let Some(values) = directive.as_object() else {
                    return Ok(false);
                };
                for (name, authored) in values {
                    let mut leaves = Vec::new();
                    collect_leaves(authored, String::new(), &mut leaves, 0)?;
                    for (_, value) in leaves {
                        if let Some(template) = value.as_str().filter(|value| value.contains("{{"))
                        {
                            let names = expression_names(template)?;
                            if !(if staged {
                                value_names_supported(&names)
                            } else {
                                independent(&names, &written)
                            }) {
                                return Ok(false);
                            }
                        }
                    }
                    if !written.insert(name.clone()) && !staged {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}
fn completion_uses_stages(
    graph: &Value,
    status: ExecutionStatus,
    successor_activation: bool,
) -> Result<bool> {
    // Ordinary successors consume the real ordered publications after record(),
    // just like terminal output. A do edge must not force that program into the
    // independent-only profile and reject its genuine same-advance stages.
    Ok(status == ExecutionStatus::Completed && (successor_activation || plain_tasks(graph)?))
}

fn plain_tasks(graph: &Value) -> Result<bool> {
    let mut pending = vec![graph];
    let mut nodes = 0;
    while let Some(node) = pending.pop() {
        nodes += 1;
        if nodes > 16384 {
            return Err(changed());
        }
        if ["with_items", "iterate_cache", "wait_for"]
            .iter()
            .any(|name| node.get(*name).is_some_and(|value| !value.is_null()))
        {
            return Ok(false);
        }
        if let Some(tasks) = node.get("tasks").filter(|tasks| !tasks.is_null()) {
            match tasks {
                Value::Array(tasks) => pending.extend(tasks),
                Value::Object(tasks) => pending.extend(tasks.values()),
                _ => return Err(changed()),
            }
        }
    }
    Ok(true)
}

fn independent(names: &BTreeSet<String>, written: &BTreeSet<String>) -> bool {
    value_names_supported(names)
        && !names.iter().any(|name| {
            written.contains(name)
                || !written.is_empty() && matches!(name.as_str(), "workflow" | "vars" | "variables")
        })
}

fn value_names_supported(names: &BTreeSet<String>) -> bool {
    !names.iter().any(|name| {
        matches!(
            name.as_str(),
            "item" | "index" | "inquiry" | "execution" | "work_queue_item"
        )
    })
}

pub(super) async fn prepare_sources(
    conn: &mut PgConnection,
    parent: &Execution,
    completed: Option<&Execution>,
    workflow: &WorkflowExecution,
    key: &str,
) -> Result<Option<Sources>> {
    let system = Inputs::prepare_system(conn, parent.id).await?;
    let root = RuntimeAuthorityRepository::execution_admission(conn, parent.id)
        .await?
        .ok_or_else(changed)?;
    let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, root.id).await?;
    let outcome = if let Some(completed) = completed {
        let outcome = Inputs::prepare_outcome(conn, parent.id, completed.id).await?;
        let source = RuntimeAuthorityRepository::execution_admission(conn, completed.id)
            .await?
            .ok_or_else(changed)?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, source.id).await?,
        );
        Some(outcome)
    } else {
        None
    };
    let mut fields = Vec::new();
    if !append_fields(
        conn,
        parent.id,
        InputEntityKind::ExecutionConfig,
        parent.id,
        parent.config.as_ref().unwrap_or(&Value::Null),
        "",
        &["/parameters".into()],
        false,
        key,
        &mut fields,
    )
    .await?
    {
        return Ok(None);
    }
    if !append_fields(
        conn,
        parent.id,
        InputEntityKind::WorkflowVariables,
        parent.id,
        &workflow.variables,
        "",
        &["/workflow".into(), "/vars".into(), "/variables".into()],
        true,
        key,
        &mut fields,
    )
    .await?
    {
        return Ok(None);
    }
    let children = if completed.is_some() {
        ExecutionRepository::find_by_parent(&mut *conn, parent.id).await?
    } else {
        Vec::new()
    };
    let mut names = BTreeSet::new();
    let mut native_result = None;
    // Same task-name retries are reduced to the authoritative latest child.
    for child in children.iter().rev() {
        let Some(metadata) = child.workflow_task.as_ref() else {
            continue;
        };
        if metadata.workflow_execution != workflow.id
            || metadata.task_index.is_some()
            || !matches!(
                child.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Timeout
            )
            || !names.insert(metadata.task_name.clone())
        {
            continue;
        }
        if let Some(current) = completed.filter(|current| current.id == child.id) {
            if serde_json::to_value(current)? != serde_json::to_value(child)? {
                return Err(changed());
            }
        }
        let _held = Inputs::prepare_outcome(conn, parent.id, child.id).await?;
        // A terminal outcome does not imply that a result VALUE exists. Keep
        // the outcome proof above, but never manufacture a stored null field
        // for a child whose result column is absent.
        let Some(raw) = child.result.as_ref() else {
            continue;
        };
        let mut prefixes = vec![
            format!("/task/{}", escape(&metadata.task_name)),
            format!("/tasks/{}", escape(&metadata.task_name)),
        ];
        if completed.is_some_and(|completed| child.id == completed.id) {
            prefixes.push("/result".into());
        }
        let legacy = !crate::secret_values::is_redaction_marker(raw)
            && append_fields(
                conn,
                parent.id,
                InputEntityKind::ExecutionResult,
                child.id,
                raw,
                "",
                &prefixes,
                false,
                key,
                &mut fields,
            )
            .await?;
        if completed.is_some_and(|completed| child.id == completed.id)
            && (!legacy || PreparedNativeResult::required(parent, &metadata.task_name)?)
        {
            native_result =
                Some(PreparedNativeResult::prepare(conn, parent.id, child, !legacy, key).await?);
        } else if !legacy {
            return Ok(None);
        }
    }
    if fields.len() + usize::from(native_result.is_some()) > 256 {
        return Err(changed());
    }
    Ok(Some(Sources {
        empty_iteration: None,
        iteration: None,
        completion_task: None,
        ordinary_task_path: None,
        child_versions: child_versions(&children)?,
        transaction: sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?,
        parent: parent.clone(),
        encryption_key_hash: crate::crypto::hash_encryption_key(key),
        native_result,
        fence,
        fields,
        system,
        outcome,
        completed: children
            .iter()
            .all(|child| child.status == ExecutionStatus::Completed),
    }))
}

fn child_versions(children: &[Execution]) -> Result<Vec<(Id, String)>> {
    children
        .iter()
        .map(|child| {
            Ok((
                child.id,
                crate::value_schema::digest(&serde_json::to_value(child)?)
                    .map_err(|_| changed())?,
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn prepare_output(
    conn: &mut PgConnection,
    execution: Id,
    graph: &Value,
    task_next: &[Value],
    workflow: &WorkflowExecution,
    transitions: &[Transition],
    bindings: &BTreeMap<String, DeclaredInputBinding>,
    sources: &Sources,
    controls: &[&PreparedInputSource],
    revision: &str,
    key: &str,
) -> Result<Option<Output>> {
    // Do not speculatively read output Keys while successors remain. The owning
    // target phase still recomputes completion before recording these values.
    if !sources.completed || !workflow.failed_tasks.is_empty() {
        return Ok(None);
    }
    if transitions
        .iter()
        .zip(task_next)
        .any(|(transition, authored)| {
            transition.fires
                && authored
                    .get("do")
                    .and_then(Value::as_array)
                    .is_some_and(|tasks| !tasks.is_empty())
        })
    {
        return Ok(None);
    }
    let Some(authored) = graph
        .get("output_map")
        .and_then(Value::as_object)
        .filter(|fields| !fields.is_empty())
    else {
        return Ok(None);
    };
    if authored.contains_key("output_validation_warning") {
        return Ok(None);
    }
    let mut all_leaves = Vec::new();
    let prior = all_publications(transitions, &[]);
    let staged = prior.iter().any(|publication| {
        publication
            .leaves
            .iter()
            .any(|(_, proof)| matches!(proof, PublicationProof::Staged(_)))
    });
    let written = prior
        .iter()
        .map(|publication| publication.name.clone())
        .collect();
    for (name, value) in authored {
        collect_leaves(value, format!("/{}", escape(name)), &mut all_leaves, 0)?;
    }
    for (_, value) in &all_leaves {
        if let Some(template) = value.as_str().filter(|value| value.contains("{{")) {
            let names = expression_names(template)?;
            if !(if staged {
                value_names_supported(&names)
            } else {
                independent(&names, &written)
            }) {
                return Ok(None);
            }
        }
    }
    let binding = super::schema_contract::SchemaContractRepository::find(
        conn,
        super::schema_contract::BindingOwner::Execution,
        execution,
    )
    .await?
    .ok_or_else(changed)?;
    binding.validate()?;
    if let Some(contract) = &binding.output {
        let contract =
            crate::value_schema::CompiledContract::new(contract).map_err(|_| changed())?;
        let metadata = serde_json::json!({"succeeded":true,"output_validation_warning":{"code":"validation","path":"/field"}});
        if !contract
            .secret_paths(&metadata)
            .map_err(|_| changed())?
            .is_empty()
        {
            return Ok(None);
        }
    }
    let mut value = Value::Object(authored.clone());
    let mut leaves = Vec::new();
    for (path, leaf) in all_leaves {
        let declaration = format!("/workflow/output_map{path}");
        let rendered = if let Some(binding) = bindings.get(&declaration) {
            evaluate(
                conn,
                execution,
                binding,
                sources,
                controls,
                &prior,
                InputSitePurpose::Value,
                key,
            )
            .await?
        } else {
            if contains_template(&leaf) {
                return Err(changed());
            }
            render_literal(conn, execution, revision, &declaration, sources, controls).await?
        };
        *value.pointer_mut(&path).ok_or_else(changed)? = rendered.value().clone();
        let site = InputSite {
            execution,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: path.clone(),
        };
        let destination = if let Some(result) = &sources.native_result {
            result
                .prepare_output_destination(conn, &rendered, &site, key)
                .await?
        } else {
            ResultOutputDestination::Ordinary(
                Inputs::prepare_destination(conn, &rendered, &site, key).await?,
            )
        };
        leaves.push((path, rendered, destination));
    }
    check_bounds(&value)?;
    Ok(Some(Output {
        value,
        contract: binding.output,
        leaves,
    }))
}

// Canonical result() preserves the envelope and data, adding only noncolliding
// data aliases. Bind every value at its REAL source pointer. Split public object
// roots to avoid overlapping namespace bindings; never invent /result/key as a
// stored execution pointer for an alias that actually came from /data/key.
fn result_source_paths(raw: &Value, protected: &[String]) -> Result<Option<Vec<String>>> {
    check_bounds(raw)?;
    let Some(object) = raw.as_object().filter(|object| !object.is_empty()) else {
        let mut paths = public_leaves(raw, protected)?;
        paths.extend(protected.iter().cloned());
        return Ok(Some(paths));
    };
    // Parsed stdout aliases and root/whole-data secret projections need A's
    // opaque result-view constructor. Do not pretend parsed bytes are fields.
    if !object.contains_key("data") && object.contains_key("stdout")
        || object
            .get("data")
            .is_some_and(crate::secret_values::is_redaction_marker)
        || protected.iter().any(String::is_empty)
    {
        return Ok(None);
    }
    let mut roots = Vec::new();
    for (name, value) in object {
        let path = format!("/{}", escape(name));
        if name == "data" && value.as_object().is_some_and(|fields| !fields.is_empty()) {
            for (name, value) in value.as_object().ok_or_else(changed)? {
                roots.push((format!("{path}/{}", escape(name)), value));
            }
        } else {
            roots.push((path, value));
        }
    }
    let mut paths = Vec::new();
    for (root, value) in roots {
        if protected.iter().any(|path| {
            path != &root && crate::secret_values::pointer_suffix(&root, path).is_some()
        }) {
            return Ok(None);
        }
        let relative = protected
            .iter()
            .filter_map(|path| crate::secret_values::pointer_suffix(path, &root))
            .collect::<Vec<_>>();
        for path in public_leaves(value, &relative)? {
            paths.push(crate::secret_values::pointer_join(&root, &path));
        }
    }
    paths.extend(protected.iter().cloned());
    if paths.len() > 256 {
        return Err(changed());
    }
    Ok(Some(paths))
}

fn result_data_alias(raw: &Value, source: &str) -> Option<String> {
    raw.get("data")?.as_object()?;
    let alias = source.strip_prefix("/data/")?;
    let name = alias
        .split('/')
        .next()?
        .replace("~1", "/")
        .replace("~0", "~");
    if raw.as_object()?.contains_key(&name) {
        return None;
    }
    Some(format!("/{alias}"))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn append_fields(
    conn: &mut PgConnection,
    renderer: Id,
    kind: InputEntityKind,
    execution: Id,
    raw: &Value,
    root: &str,
    prefixes: &[String],
    aliases: bool,
    key: &str,
    fields: &mut Vec<Field>,
) -> Result<bool> {
    let entity = match kind {
        InputEntityKind::ExecutionConfig => crate::secret_values::ENTITY_EXECUTION_CONFIG,
        InputEntityKind::ExecutionResult => crate::secret_values::ENTITY_EXECUTION_RESULT,
        InputEntityKind::WorkflowVariables => crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
    };
    let stored =
        ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, entity, execution)
            .await?;
    // Representation-only scope check, before expression evaluation. A denied
    // typed source never falls back to ordinary origins or current human IAM.
    for field in &stored {
        if field.source_kind != "provenance" {
            return Ok(false);
        }
        let proof: SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().ok_or_else(changed)?)?;
        if !matches!(proof.origins.as_slice(), [crate::secret_provenance::SecretOrigin::Entity { entity_type, .. }] if entity_type == &format!("declared_input:{}:value", kind.as_str()))
        {
            return Ok(false);
        }
    }
    let protected: Vec<_> = stored
        .iter()
        .filter_map(|field| crate::secret_values::pointer_suffix(&field.json_path, root))
        .collect();
    if !root.is_empty()
        && stored
            .iter()
            .any(|field| crate::secret_values::pointer_suffix(root, &field.json_path).is_some())
    {
        return Ok(false);
    }
    let raw = raw.pointer(root).ok_or_else(changed)?;
    let mut paths = if kind == InputEntityKind::ExecutionResult {
        let Some(paths) = result_source_paths(raw, &protected)? else {
            return Ok(false);
        };
        paths
    } else {
        let mut paths = public_leaves(raw, &protected)?;
        paths.extend(protected.iter().cloned());
        paths
    };
    // Split a public root for direct variable aliases without overlapping a
    // namespace binding. Both paths still point at the actual stored field.
    if aliases
        && paths == [String::new()]
        && raw.as_object().is_some_and(|fields| !fields.is_empty())
    {
        paths = raw
            .as_object()
            .ok_or_else(changed)?
            .keys()
            .map(|name| format!("/{}", escape(name)))
            .collect();
    }
    for path in paths {
        let actual_path = crate::secret_values::pointer_join(root, &path);
        let site = InputSite {
            execution,
            kind,
            purpose: InputSitePurpose::Value,
            path: actual_path.clone(),
        };
        let mut context_paths: Vec<_> = prefixes
            .iter()
            .map(|prefix| format!("{prefix}{path}"))
            .collect();
        if kind == InputEntityKind::ExecutionResult {
            if let Some(alias) = result_data_alias(raw, &path) {
                context_paths.extend(prefixes.iter().map(|prefix| format!("{prefix}{alias}")));
            }
        }
        if aliases && !path.is_empty() {
            let name = path
                .split('/')
                .nth(1)
                .ok_or_else(changed)?
                .replace("~1", "/")
                .replace("~0", "~");
            if !reserved(&name) {
                context_paths.push(path.clone());
            }
        }
        let source = if let Some(field) = stored.iter().find(|field| field.json_path == actual_path)
        {
            if field.source_kind != "provenance" {
                return Err(changed());
            }
            let proof: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(changed)?)?;
            if proof.origins.len() != 1 {
                return Err(changed());
            }
            Some(
                Inputs::prepare_source(conn, renderer, &site, &proof.origins[0], key)
                    .await
                    .map_err(|error| {
                        source_phase_error(error, "prepare typed source", &site.path)
                    })?,
            )
        } else {
            None
        };
        fields.push(Field {
            site,
            paths: context_paths,
            source,
        });
    }
    Ok(true)
}

#[expect(
    clippy::too_many_arguments,
    reason = "One evaluation binds its exact declaration, sources, controls, prior publications, and purpose"
)]
async fn evaluate(
    conn: &mut PgConnection,
    execution: Id,
    binding: &DeclaredInputBinding,
    sources: &Sources,
    controls: &[&PreparedInputSource],
    prior: &[&Publication],
    purpose: InputSitePurpose,
    key: &str,
) -> Result<RenderedReceiptContext> {
    evaluate_with_cache(
        conn, execution, binding, sources, controls, prior, purpose, None, key,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_with_cache(
    conn: &mut PgConnection,
    execution: Id,
    binding: &DeclaredInputBinding,
    sources: &Sources,
    controls: &[&PreparedInputSource],
    prior: &[&Publication],
    purpose: InputSitePurpose,
    cache: Option<&super::native_input_snapshot::PreparedCacheInput>,
    key: &str,
) -> Result<RenderedReceiptContext> {
    let mut session: RenderSession = measure(
        Op::SessionBegin,
        Inputs::begin_render(
            conn,
            execution,
            &binding.declaration_revision,
            &binding.source_path,
            &binding.expression,
            purpose,
        ),
    )
    .await?;
    measure(
        Op::FrameBinding,
        bind_evaluation_sources(conn, &mut session, binding, sources, controls, prior, key),
    )
    .await?;
    if purpose == InputSitePurpose::Condition {
        if let Some(empty) = &sources.empty_iteration {
            Inputs::bind_empty_iteration_outcome(
                &mut session,
                empty.empty_outcome().ok_or_else(changed)?,
            )?;
        }
    }
    if let Some(cache) = cache {
        Inputs::bind_cache_input(&mut session, cache)?;
    }
    let mut rendered = if purpose == InputSitePurpose::Condition {
        measure(
            Op::Expression,
            Inputs::evaluate_condition_with_keys(conn, session, key),
        )
        .await
        .map_err(|error| source_phase_error(error, "evaluate condition", &binding.source_path))
    } else {
        measure(
            Op::Expression,
            Inputs::evaluate_with_keys(conn, session, key),
        )
        .await
        .map_err(|error| source_phase_error(error, "evaluate value", &binding.source_path))
    }?;
    if let Some(empty) = &sources.empty_iteration {
        Inputs::attach_empty_iteration_control(
            &mut rendered,
            empty.empty_outcome().ok_or_else(changed)?,
        )?;
    }
    if let Some(group) = &sources.iteration {
        Inputs::attach_native_iteration_control(&mut rendered, &group.proof)?;
    }
    Ok(rendered)
}

async fn bind_evaluation_sources(
    conn: &mut PgConnection,
    session: &mut RenderSession,
    binding: &DeclaredInputBinding,
    sources: &Sources,
    controls: &[&PreparedInputSource],
    prior: &[&Publication],
    key: &str,
) -> Result<()> {
    let names = expression_names(&binding.expression)?;
    let native_result = binding.source_path.starts_with("/workflow/output_map/")
        || names
            .iter()
            .any(|name| matches!(name.as_str(), "task" | "tasks" | "result"))
        || expression_functions(&binding.expression)?.contains("result");
    if let Some(result) = &sources.native_result {
        if native_result {
            result.bind(session)?;
        }
    }
    let mut latest = BTreeMap::new();
    for publication in prior {
        if publication.leaves.iter().any(|(_, proof)| {
            matches!(
                proof,
                PublicationProof::Staged(_) | PublicationProof::WholeResult(_)
            )
        }) {
            latest.insert(publication.name.as_str(), *publication);
        }
    }
    for field in &sources.fields {
        if native_result
            && field.site.kind == InputEntityKind::ExecutionResult
            && sources
                .native_result
                .as_ref()
                .is_some_and(|result| result.execution() == field.site.execution)
        {
            continue;
        }
        if field.site.kind == InputEntityKind::WorkflowVariables && !latest.is_empty() {
            let name = field
                .site
                .path
                .split('/')
                .nth(1)
                .unwrap_or_default()
                .replace("~1", "/")
                .replace("~0", "~");
            if field.site.path.is_empty() || latest.contains_key(name.as_str()) {
                continue;
            }
        }
        for path in &field.paths {
            let namespace = path
                .split('/')
                .nth(1)
                .unwrap_or_default()
                .replace("~1", "/")
                .replace("~0", "~");
            if !names.contains(&namespace) && namespace != "result" {
                continue;
            }
            if let Some(source) = &field.source {
                Inputs::bind_prepared(session, path, source, source.value())?;
            } else {
                Inputs::bind_public_field(conn, session, path, &field.site).await?;
            }
        }
    }
    for (name, publication) in latest {
        for (suffix, proof) in &publication.leaves {
            for namespace in ["workflow", "vars", "variables"] {
                if names.contains(namespace) {
                    match proof {
                        PublicationProof::Staged(staged) => Inputs::bind_staged_value(
                            session,
                            format!("/{namespace}/{}{suffix}", escape(name)),
                            staged,
                        )?,
                        PublicationProof::WholeResult(shorthand) if suffix.is_empty() => shorthand
                            .bind(
                                session,
                                match namespace {
                                    "workflow" => {
                                        super::inherited_input::NativeResultStageBinding::Workflow
                                    }
                                    "vars" => {
                                        super::inherited_input::NativeResultStageBinding::Vars
                                    }
                                    "variables" => {
                                        super::inherited_input::NativeResultStageBinding::Variables
                                    }
                                    _ => return Err(changed()),
                                },
                            )?,
                        _ => return Err(changed()),
                    }
                }
            }
            if names.contains(name) && !reserved(name) {
                match proof {
                    PublicationProof::Staged(staged) => Inputs::bind_staged_value(
                        session,
                        format!("/{}{suffix}", escape(name)),
                        staged,
                    )?,
                    PublicationProof::WholeResult(shorthand) if suffix.is_empty() => shorthand
                        .bind(
                            session,
                            super::inherited_input::NativeResultStageBinding::Direct,
                        )?,
                    _ => return Err(changed()),
                }
            }
        }
    }
    if let Some(outcome) = &sources.outcome {
        Inputs::bind_outcome(session, outcome)?;
    }
    if let Some(group) = &sources.iteration {
        if binding
            .source_path
            .strip_prefix(&group.task_path)
            .is_some_and(|suffix| suffix.starts_with("/next/") && suffix.ends_with("/when"))
        {
            Inputs::bind_native_iteration_outcome(session, &group.proof)?;
        }
    }
    if names.contains("system") {
        Inputs::bind_prepared_system(session, &sources.system)?;
    }
    if binding.uses_pack_configuration() {
        Inputs::bind_configuration(conn, session, key)
            .await
            .map_err(|error| {
                source_phase_error(error, "bind pack configuration", &binding.source_path)
            })?;
    }
    // Native effects retain the protected accepted/variable source closure even
    // when this particular value is literal or does not read an earlier publish.
    // These are CONTROL edges from actual prepared receipts, never claims that
    // their values equal the new condition/publication/output value.
    for source in sources
        .fields
        .iter()
        .filter_map(|field| field.source.as_ref())
    {
        Inputs::attach_prepared_control(session, source)?;
    }
    for source in controls {
        Inputs::attach_prepared_control(session, source)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_plain_successors_keep_the_same_ordered_stage_profile() {
        let next = json!([
            {"publish":["whole", {"copy":"{{ vars.whole }}"}]},
            {"when":"{{ workflow.copy.literal == vars.whole.literal }}","do":["consumer"]}
        ]);
        let graph = json!({"tasks":[{"name":"source","next":next}, {"name":"consumer"}]});
        assert!(completion_uses_stages(&graph, ExecutionStatus::Completed, false).unwrap());
        assert!(eligible_for(next.as_array().unwrap(), true).unwrap());
        assert!(!eligible_for(next.as_array().unwrap(), false).unwrap());
        for status in [ExecutionStatus::Failed, ExecutionStatus::Timeout] {
            assert!(!completion_uses_stages(&graph, status, false).unwrap());
        }
        for field in ["with_items", "iterate_cache", "wait_for"] {
            let mut unsupported = graph.clone();
            unsupported["tasks"][1][field] = json!({});
            assert!(
                !completion_uses_stages(&unsupported, ExecutionStatus::Completed, false).unwrap()
            );
        }
    }

    #[test]
    fn dependency_scan_preserves_dynamic_selectors_without_evaluating_them() {
        assert!(eligible(
            json!([
                {"when":"{{ succeeded() and parameters.ready }}", "publish":[
                    {"selected":"{{ keystore[config[parameters.selector]] }}"},
                    {"literal":"caller data stays data"}
                ]},
            {"when":"parameters.other == true", "publish":[{"another":"{{ task.first.value }}"}]}
            ])
            .as_array()
            .unwrap()
        )
        .unwrap());
    }

    #[test]
    fn unconditional_transitions_need_no_invented_when_declaration() {
        assert!(eligible(
            serde_json::json!([
                {"publish":[{"literal":{"accepted":true}}, {"supplied":"{{ parameters.value }}"}]},
                {"when":null, "do":["next"]},
                {"do":[]}
            ])
            .as_array()
            .unwrap()
        )
        .unwrap());
    }

    #[test]
    fn empty_outputs_use_real_parent_fields_not_fabricated_task_defaults() {
        assert!(empty_output_context(&serde_json::json!({
            "value":"{{ workflow.result }}", "key":"{{ keystore[parameters.selector] }}",
            "time":"{{ system.workflow_start }}", "literal":"result() is literal text"
        }))
        .unwrap());
        for template in [
            "{{ result() }}",
            "{{ succeeded() }}",
            "{{ length(result().items) }}",
            "{{ task.first.value }}",
        ] {
            assert!(!empty_output_context(&Value::String(template.into())).unwrap());
        }
    }

    #[test]
    fn result_bindings_preserve_envelope_data_and_exact_noncolliding_alias_sites() {
        let result = serde_json::json!({"data":{"message":"value","collision":"data loses"},
            "collision":"envelope wins","exit_code":0});
        let paths = result_source_paths(&result, &[]).unwrap().unwrap();
        for path in [
            "/data/message",
            "/data/collision",
            "/collision",
            "/exit_code",
        ] {
            assert!(paths.contains(&path.to_owned()));
        }
        assert_eq!(
            result_data_alias(&result, "/data/message"),
            Some("/message".into())
        );
        assert_eq!(result_data_alias(&result, "/data/collision"), None);
        assert_eq!(
            result_data_alias(&serde_json::json!({"data":[1]}), "/data/0"),
            None
        );
        assert!(
            result_source_paths(&serde_json::json!({"stdout":"{\"decoded\":1}"}), &[])
                .unwrap()
                .is_none()
        );
        assert!(result_source_paths(
            &serde_json::json!({"data":crate::secret_values::redaction_marker()}),
            &["/data".into()]
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn independent_mode_rejects_staged_dependencies_overwrites_and_iteration_names() {
        for next in [
            json!([{"when":"true", "publish":[{"value":"{{ config.first }}"}, {"copy":"{{ workflow.value }}"}]}]),
            json!([{"when":"true", "publish":[{"value":1}]}, {"when":"value == 1"}]),
            json!([{"when":"true", "publish":[{"value":1}, {"value":2}]}]),
            json!([{"when":"{{ item.enabled }}"}]),
        ] {
            assert!(!eligible(next.as_array().unwrap()).unwrap(), "{next}");
        }
    }

    #[test]
    fn staged_terminal_path_accepts_value_dependencies_overwrites_and_decisions() {
        let next = json!([
            {"publish":[{"a":"{{ parameters.value }}"},{"b":"{{ workflow.a }}"}]},
            {"when":"{{ vars.b == 'yes' }}","publish":[{"a":"{{ variables.b }}"}]},
            {"when":"{{ false and keystore['never-selected'] }}","publish":[{"skip":"{{ keystore['never-published'] }}"}]}
        ]);
        assert!(eligible_for(next.as_array().unwrap(), true).unwrap());
        assert!(!eligible(next.as_array().unwrap()).unwrap());
    }

    #[test]
    fn shorthand_order_matches_the_canonical_saved_whole_result_directive() {
        let authored = json!({"publish":["whole"]});
        assert!(eligible(std::slice::from_ref(&authored)).unwrap());
        let saved = json!({"publish":[{"name":"whole","value":"{{ result() }}"}]});
        let ordered = publication_order(&authored, Some(&saved)).unwrap();
        assert_eq!(ordered[0], (0, "whole", &json!("whole")));
        let wrong = json!({"publish":[{"name":"whole","value":"{{ result().whole }}"}]});
        assert!(publication_order(&authored, Some(&wrong)).is_err());
    }

    #[test]
    fn publication_order_preserves_directives_and_actual_map_order_without_collapsing_overwrites() {
        let authored = json!({"publish":[{"a":1,"b":2},{"a":3}]});
        let saved = json!({"publish":[{"name":"b","value":2},{"name":"a","value":1},{"name":"a","value":3}]});
        let order = publication_order(&authored, Some(&saved)).unwrap();
        assert_eq!(
            order
                .iter()
                .map(|(ordinal, name, _)| (*ordinal, *name))
                .collect::<Vec<_>>(),
            vec![(0, "b"), (0, "a"), (1, "a")]
        );
        assert!(publication_order(&authored, None).is_err());
        let mut changed = saved.clone();
        changed["publish"][2]["value"] = json!(4);
        assert!(publication_order(&authored, Some(&changed)).is_err());
        let mut changed = saved;
        changed["publish"].as_array_mut().unwrap().swap(0, 2);
        assert!(publication_order(&authored, Some(&changed)).is_err());
    }

    #[test]
    fn unknown_truthiness_never_causes_a_second_selector_evaluation() {
        for expression in [
            "parameters.flag",
            "{{ keystore[parameters.selector] }}",
            "1",
            "prefix {{ parameters.flag }}",
        ] {
            assert!(eligible(json!([{"when":expression}]).as_array().unwrap()).unwrap());
        }
        for expression in [
            "{{ succeeded() }}",
            "{{ not parameters.flag }}",
            "{{ keystore[parameters.selector] == 'yes' }}",
            "bool(parameters.flag)",
        ] {
            assert!(eligible(json!([{"when":expression}]).as_array().unwrap()).unwrap());
        }
    }
}
