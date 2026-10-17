//! Ordinary success is proved by H's complete inventory and B's actual leaf
//! renders. Snapshot issuance is deferred until canonical leaves are recorded,
//! so no pending-result exception or fabricated declaration is needed.
use super::super::{
    native_failure_materialization::PreparedNativeSuccess,
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    FindById,
};
use super::*;
use crate::models::{Execution, ExecutionStatus, WorkflowExecution};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSuccessOutputLeaf {
    path: String,
    receipt: Id,
    proof: Value,
    proof_digest: String,
    encrypted_value: Value,
    key_hash: String,
}
impl NativeSuccessOutputLeaf {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn receipt(&self) -> Id {
        self.receipt
    }
    pub fn original_proof(&self) -> &Value {
        &self.proof
    }
    pub fn proof_digest(&self) -> &str {
        &self.proof_digest
    }
    pub fn encrypted_value(&self) -> &Value {
        &self.encrypted_value
    }
    pub fn encryption_key_hash(&self) -> &str {
        &self.key_hash
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SuccessSource {
    renderer: Id,
    pub(super) execution: Id,
    pub(super) workflow: Id,
    pub(super) completed: Id,
    pub(super) transaction: String,
    // Execution's public serializer deliberately omits the executable pin.
    // Private archive validation needs the original graph after deserialization.
    #[serde(serialize_with = "serialize_archived_parent")]
    parent: Execution,
    workflow_original: WorkflowExecution,
    pub(super) contract: ActionSchemaBinding,
    contract_digest: String,
    pub(super) root_contributors: Vec<super::super::accepted_input::AcceptedContributorManifest>,
    config_fields: Vec<ArchivedField>,
    result_fields: Vec<ArchivedField>,
    variable_fields: Vec<ArchivedField>,
    pub(super) target: BoundRuntimeTarget,
    pub(super) read_owner: super::super::runtime_read::RuntimeReadOwner,
    children: Vec<ResultViewSource>,
    child_contracts: Vec<(Id, ActionSchemaBinding)>,
    projected_completed: Vec<String>,
    pub(super) parents: Vec<(Id, InputSite)>,
    controls: Vec<(Id, InputSite)>,
    leaves: Vec<NativeSuccessOutputLeaf>,
    value_digest: String,
}

fn serialize_archived_parent<S: serde::Serializer>(
    parent: &Execution,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut archived = serde_json::to_value(parent).map_err(serde::ser::Error::custom)?;
    archived["executable_snapshot"] =
        serde_json::to_value(&parent.executable_snapshot).map_err(serde::ser::Error::custom)?;
    archived.serialize(serializer)
}
pub struct NativeSuccessMetadataEvidence {
    snapshot: Id,
    source: SuccessSource,
}
impl NativeSuccessMetadataEvidence {
    pub(super) fn new(snapshot: Id, source: &SuccessSource) -> Self {
        Self {
            snapshot,
            source: source.clone(),
        }
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn original_renderer_admission(&self) -> Id {
        self.source.renderer
    }
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        &self.source.workflow_original
    }
    pub fn original_parent(&self) -> &Execution {
        &self.source.parent
    }
    pub fn completed_execution(&self) -> Id {
        self.source.completed
    }
    pub fn original_contract(&self) -> &ActionSchemaBinding {
        &self.source.contract
    }
    pub fn root_contributor_manifests(
        &self,
    ) -> &[super::super::accepted_input::AcceptedContributorManifest] {
        &self.source.root_contributors
    }
    pub fn children(&self) -> Vec<NativeCompletionSource> {
        self.source
            .children
            .iter()
            .cloned()
            .map(NativeCompletionSource::new)
            .collect()
    }
    pub fn original_child_contracts(&self) -> &[(Id, ActionSchemaBinding)] {
        &self.source.child_contracts
    }
    pub fn output_leaves(&self) -> &[NativeSuccessOutputLeaf] {
        &self.source.leaves
    }
    pub fn receipt_edges(&self) -> &[(Id, InputSite)] {
        &self.source.parents
    }
    pub fn controls(&self) -> &[(Id, InputSite)] {
        &self.source.controls
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
    pub fn projected_completed_tasks(&self) -> &[String] {
        &self.source.projected_completed
    }
    pub fn transaction(&self) -> &str {
        &self.source.transaction
    }
    pub fn read_owner(&self) -> &super::super::runtime_read::RuntimeReadOwner {
        &self.source.read_owner
    }
    pub fn target(&self) -> &BoundRuntimeTarget {
        &self.source.target
    }
}

pub struct PreparedNativeSuccessMetadata {
    source: SuccessSource,
    value: Value,
    renderer: super::super::runtime_authority::RuntimeAdmissionRecord,
    fence: ApprovalEffectFence,
    parents: Vec<ReceiptContext>,
    original_checked: std::sync::OnceLock<()>,
    recorded: std::sync::OnceLock<Id>,
}
impl PreparedNativeSuccessMetadata {
    pub fn execution(&self) -> Id {
        self.source.execution
    }
    pub fn require_original_rows(
        &self,
        parent: &Execution,
        workflow: &WorkflowExecution,
    ) -> Result<()> {
        if serde_json::to_value(parent)? != serde_json::to_value(&self.source.parent)?
            || serde_json::to_value(&parent.executable_snapshot)?
                != serde_json::to_value(&self.source.parent.executable_snapshot)?
            || serde_json::to_value(workflow)?
                != serde_json::to_value(&self.source.workflow_original)?
        {
            return Err(denied());
        }
        self.original_checked.set(()).map_err(|_| denied())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.source.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub(crate) fn renderer(&self) -> &super::super::runtime_authority::RuntimeAdmissionRecord {
        &self.renderer
    }
    pub(crate) fn fence(&self) -> &ApprovalEffectFence {
        &self.fence
    }
    pub(crate) fn parents(&self) -> &[ReceiptContext] {
        &self.parents
    }
    pub(crate) fn require_original_guard(&self) -> Result<()> {
        if self.original_checked.get().is_none() || self.recorded.get().is_some() {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) fn mark_recorded(&self, snapshot: Id) -> Result<()> {
        self.recorded.set(snapshot).map_err(|_| denied())
    }
    pub(crate) fn retain_destination(
        &mut self,
        destination: &super::super::inherited_input::PreparedReceiptDestination,
    ) -> Result<()> {
        self.fence = self
            .fence
            .intersect(&ApprovalEffectFence::from_input_approval(
                destination.held_deadline()?,
            )?);
        Ok(())
    }
    pub fn secret_path_sources(
        &self,
        receipt: &ReceiptContext,
    ) -> Result<Vec<crate::secret_values::SecretPathSource>> {
        let snapshot = self.recorded.get().ok_or_else(denied)?;
        if receipt.site().execution != self.execution()
            || receipt.site().kind
                != super::super::inherited_input::InputEntityKind::ExecutionResult
            || receipt.site().purpose != InputSitePurpose::Value
            || !receipt.site().path.is_empty()
            || receipt.accepted_value() != self.value()
            || !receipt.native_source_metadata().iter().any(|metadata| {
                metadata.snapshot() == *snapshot
                    && metadata.source_kind() == "ordinary_success_metadata"
            })
        {
            return Err(denied());
        }
        Ok(vec![crate::secret_values::SecretPathSource {
            path: String::new(),
            source: crate::secret_values::SecretSource::Bound(receipt.provenance()),
        }])
    }
}
impl NativeInputSnapshotRepository {
    pub(crate) async fn require_success_target(
        conn: &mut PgConnection,
        prepared: &PreparedNativeSuccessMetadata,
        state: &PreparedNativeSuccess<'_>,
        projected: &WorkflowExecution,
    ) -> Result<()> {
        prepared.require_original_guard()?;
        require_transaction(conn, &prepared.source.transaction).await?;
        state.require_original_sources_on(conn).await?;
        state.require_projected_workflow(projected)?;
        state.output_plan().require_frame_publication(conn).await?;
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
        if serde_json::to_value(&parent)? != serde_json::to_value(&prepared.source.parent)?
            || serde_json::to_value(&parent.executable_snapshot)?
                != serde_json::to_value(&prepared.source.parent.executable_snapshot)?
        {
            return Err(Error::PermissionDenied(
                "Ordinary success original parent changed".into(),
            ));
        }
        if projected.status != ExecutionStatus::Running
            || state.execution() != prepared.source.execution
            || state.completed_execution() != prepared.source.completed
            || serde_json::to_value(&workflow)? != serde_json::to_value(projected)?
        {
            return Err(denied());
        }
        prepared.ensure_fresh_on(conn).await
    }
    pub(crate) async fn prepare_success_observation(
        conn: &mut PgConnection,
        state: &PreparedNativeSuccess<'_>,
        key: &str,
    ) -> Result<PreparedNativeSuccessMetadata> {
        state.output_plan().require_source_phase()?;
        state.require_current_on(conn).await?;
        let renderer = DeclaredInputRepository::require_source(conn, state.execution()).await?;
        let mut fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, renderer.id).await?;
        for (_, rendered) in state.output_plan().prepared_output_leaves() {
            rendered.ensure_fresh_on(conn).await?;
            fence = fence.intersect(&ApprovalEffectFence::from_input_approval(
                rendered.approval_deadline(),
            )?);
        }
        for outcome in state.outcomes() {
            fence = fence.intersect(&ApprovalEffectFence::from_input_approval(
                outcome.approval_deadline(),
            )?);
        }
        let mut children = Vec::new();
        let mut child_contracts = Vec::new();
        let mut edges = Vec::new();
        let mut held = Vec::new();
        for actual in state.source_executions() {
            let (child, _, child_fence) =
                Self::prepare_result_source(conn, &renderer, actual.id, key).await?;
            let contract = Schemas::find(conn, BindingOwner::Execution, actual.id)
                .await?
                .ok_or_else(denied)?;
            state.require_original_child(
                actual,
                &contract,
                &child
                    .fields
                    .iter()
                    .map(ArchivedField::stored)
                    .collect::<Vec<_>>(),
            )?;
            if child.status != "completed"
                || child.timed_out
                || child.workflow != state.workflow().id
            {
                return Err(denied());
            }
            fence = fence.intersect(&child_fence);
            for edge in &child.parents {
                if !edges.contains(edge) {
                    let origin = SecretOrigin::Entity {
                        entity_type: edge.1.kind.origin_kind(edge.1.purpose),
                        entity_id: edge.0,
                        path: edge.1.path.clone(),
                    };
                    let context =
                        InheritedInputRepository::receipt_context(conn, &edge.1, &origin, key)
                            .await?;
                    fence = fence.intersect(context.accepted_fence());
                    held.push(context);
                    edges.push(edge.clone());
                }
            }
            children.push(child);
            child_contracts.push((actual.id, contract));
        }
        let mut controls = Vec::new();
        for control in state.decisions() {
            fence = fence.intersect(&ApprovalEffectFence::from_input_approval(
                control.approval_deadline(),
            )?);
            for context in
                InheritedInputRepository::native_control_contexts(conn, renderer.id, control)
                    .await?
            {
                fence = fence.intersect(context.accepted_fence());
                let SecretOrigin::Entity { entity_id: id, .. } = context.origin() else {
                    return Err(denied());
                };
                let edge = (id, context.site().clone());
                if !held.iter().any(|receipt|receipt.site()==&edge.1 && matches!(receipt.origin(),SecretOrigin::Entity {entity_id,..} if entity_id==edge.0)) {held.push(context);}
                if !edges.contains(&edge) {
                    edges.push(edge.clone());
                }
                if !controls.contains(&edge) {
                    controls.push(edge);
                }
            }
        }
        let (_, root_contributors) =
            super::super::accepted_input::AcceptedInputRepository::source_contributors(
                conn,
                state.execution(),
            )
            .await?;
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(state.execution()),
            renderer.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let graph = &state
            .parent()
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let value = compose(
            graph,
            state
                .output_plan()
                .prepared_output_leaves()
                .map(|(path, rendered)| (path, rendered.value())),
            state.output_contract().output.as_ref(),
        )?;
        let source = SuccessSource {
            renderer: renderer.id,
            execution: state.execution(),
            workflow: state.workflow().id,
            completed: state.completed_execution(),
            transaction: transaction(conn).await?,
            parent: state.parent().clone(),
            workflow_original: state.workflow().clone(),
            contract: state.output_contract().clone(),
            contract_digest: hash(&serde_json::to_value(state.output_contract())?)?,
            root_contributors,
            config_fields: state
                .root_config_fields()
                .iter()
                .map(ArchivedField::from)
                .collect(),
            result_fields: state
                .root_result_fields()
                .iter()
                .map(ArchivedField::from)
                .collect(),
            variable_fields: state
                .variable_fields()
                .iter()
                .map(ArchivedField::from)
                .collect(),
            target: bound_target(state.execution(), &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            children,
            child_contracts,
            projected_completed: state.projected_completed_tasks().to_vec(),
            parents: edges,
            controls,
            leaves: Vec::new(),
            value_digest: hash(&value)?,
        };
        bounded(&serde_json::to_value(&source)?)?;
        state.ensure_fresh_on(conn).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeSuccessMetadata {
            source,
            value,
            renderer,
            fence,
            parents: held,
            original_checked: std::sync::OnceLock::new(),
            recorded: std::sync::OnceLock::new(),
        })
    }

    /// Writes only already-captured facts. No target-phase source authority or
    /// fresh declaration/schema lookup. Leaf receipts came from the one evaluator.
    pub(crate) async fn publish_success_observation(
        conn: &mut PgConnection,
        prepared: &PreparedNativeSuccessMetadata,
        state: &PreparedNativeSuccess<'_>,
        projected: &WorkflowExecution,
        leaves: &[(String, ReceiptContext)],
        actual: &Value,
        key: &str,
    ) -> Result<(Id, NativeSnapshotMetadata)> {
        publication_step(prepared.require_original_guard(), "original-guard")?;
        publication_step(
            require_transaction(conn, &prepared.source.transaction).await,
            "transaction",
        )?;
        publication_step(
            state.require_original_sources_on(conn).await,
            "original-sources",
        )?;
        publication_step(
            state.require_projected_workflow(projected),
            "projected-workflow",
        )?;
        publication_step(
            state.output_plan().require_frame_publication(conn).await,
            "frame-publication",
        )?;
        if projected.status != ExecutionStatus::Running {
            return Err(denied());
        }
        if actual != &prepared.value
            || state.execution() != prepared.source.execution
            || state.completed_execution() != prepared.source.completed
        {
            return Err(denied());
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
        if serde_json::to_value(&parent)? != serde_json::to_value(&prepared.source.parent)?
            || serde_json::to_value(&parent.executable_snapshot)?
                != serde_json::to_value(&prepared.source.parent.executable_snapshot)?
            || serde_json::to_value(&workflow)? != serde_json::to_value(projected)?
        {
            return Err(denied());
        }
        for (kind, fields) in [
            ("execution_config", &prepared.source.config_fields),
            ("execution_result", &prepared.source.result_fields),
        ] {
            if load_fields(conn, kind, parent.id).await? != *fields {
                return Err(Error::PermissionDenied(
                    "Ordinary success original parent fields changed".into(),
                ));
            }
        }
        for child in &prepared.source.children {
            publication_step(
                result_view::compare_current(conn, child).await,
                "child-version",
            )?;
        }
        let expected = state
            .output_plan()
            .prepared_output_leaves()
            .collect::<Vec<_>>();
        if expected.len() != leaves.len() {
            return Err(denied());
        }
        let mut source = prepared.source.clone();
        for ((expected_path, rendered), (path, receipt)) in expected.into_iter().zip(leaves) {
            if path != expected_path
                || receipt.site().execution != source.execution
                || receipt.site().kind
                    != super::super::inherited_input::InputEntityKind::ExecutionResult
                || receipt.site().purpose != InputSitePurpose::Value
                || receipt.site().path != *path
                || receipt.accepted_value() != rendered.value()
            {
                return Err(denied());
            }
            receipt.ensure_fresh_on(conn).await?;
            let SecretOrigin::Entity { entity_id: id, .. } = receipt.origin() else {
                return Err(denied());
            };
            let (proof,digest,encrypted,key_hash):(Value,String,Value,String)=sqlx::query_as("SELECT proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1 AND execution=$2 AND renderer_admission=$3 AND entity_kind='execution_result' AND destination_purpose='value' AND destination_path=$4").bind(id).bind(source.execution).bind(prepared.renderer.id).bind(path).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
            if hash(&proof)? != digest
                || key_hash != crate::crypto::hash_encryption_key(key)
                || crate::crypto::decrypt_json(&encrypted, key)? != *rendered.value()
            {
                return Err(denied());
            }
            let edge = (id, receipt.site().clone());
            if !source.parents.contains(&edge) {
                source.parents.push(edge);
            }
            source.leaves.push(NativeSuccessOutputLeaf {
                path: path.clone(),
                receipt: id,
                proof,
                proof_digest: digest,
                encrypted_value: encrypted,
                key_hash,
            });
        }
        validate_storage(&source).map_err(|error| {
            if matches!(error, Error::PermissionDenied(_)) {
                Error::PermissionDenied("Ordinary success storage shape denied".into())
            } else {
                error
            }
        })?;
        publication_step(
            InheritedInputRepository::require_cache_selector_publications(conn, &source.parents)
                .await,
            "parent-publications",
        )?;
        let body = Body {
            renderer: prepared.renderer.id,
            execution: source.execution,
            task_path: String::new(),
            authority: prepared.renderer.authority.0.clone(),
            approval_deadline: prepared.fence.approval_deadline(),
            source: Source::SuccessMetadata(source.clone()),
        };
        let snapshot = publication_step(save(conn, &body, actual, key).await, "snapshot-save")?;
        sqlx::query("INSERT INTO runtime_native_success_metadata_observation(snapshot,observation) VALUES($1,$2)").bind(snapshot).bind(serde_json::to_value(&source)?).execute(&mut *conn).await?;
        for (receipt, _) in &source.parents {
            sqlx::query(
                "INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2)",
            )
            .bind(snapshot)
            .bind(receipt)
            .execute(&mut *conn)
            .await?;
        }
        for admission in admissions(&source, prepared.renderer.id) {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)").bind(snapshot).bind(admission).execute(&mut *conn).await?;
        }
        sqlx::query("INSERT INTO runtime_native_success_metadata_release(snapshot,execution,workflow,completed_execution,release_transaction) VALUES($1,$2,$3,$4,pg_current_xact_id()::TEXT)").bind(snapshot).bind(source.execution).bind(source.workflow).bind(source.completed).execute(&mut *conn).await?;
        prepared.fence.ensure_fresh_on(conn).await?;
        state.ensure_fresh_on(conn).await?;
        Ok((snapshot, metadata(snapshot, &body)))
    }
    pub(crate) async fn validate_native_success_recipe(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        plaintext: Option<(&Value, &str)>,
    ) -> Result<()> {
        let body = if let Some((actual, key)) = plaintext {
            let (body, value, _, _) = load(conn, snapshot, key).await?;
            if &value != actual {
                return Err(denied());
            }
            let Source::SuccessMetadata(source) = &body.source else {
                return Err(denied());
            };
            validate_value(conn, snapshot, source, &value, key).await?;
            body
        } else {
            let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
            body
        };
        let Source::SuccessMetadata(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer {
            return Err(denied());
        }
        require_release(conn, snapshot, source).await
    }
}

fn publication_step<T>(result: Result<T>, stage: &'static str) -> Result<T> {
    result.map_err(|error| match error {
        Error::PermissionDenied(message) => Error::PermissionDenied(format!(
            "Ordinary success snapshot [{stage}] denied: {message}"
        )),
        other => other,
    })
}

fn compose<'a>(
    graph: &Value,
    leaves: impl Iterator<Item = (&'a str, &'a Value)>,
    contract: Option<&crate::value_schema::Contract>,
) -> Result<Value> {
    let mut candidate = graph
        .get("output_map")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if candidate
        .as_object()
        .is_none_or(|object| object.contains_key("output_validation_warning"))
    {
        return Err(denied());
    }
    let mut seen = BTreeSet::new();
    for (path, value) in leaves {
        if !path.starts_with('/') || !seen.insert(path) {
            return Err(denied());
        }
        *candidate.pointer_mut(path).ok_or_else(denied)? = value.clone();
    }
    candidate
        .as_object_mut()
        .ok_or_else(denied)?
        .entry("succeeded".to_owned())
        .or_insert(Value::Bool(true));
    bounded(&candidate)?;
    if let Some(contract) = contract {
        let compiled =
            crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
        if let Err(error) = compiled.validate(&candidate) {
            candidate["output_validation_warning"] = json!({"code":error.code,"path":error.path});
        }
    }
    bounded(&candidate)?;
    Ok(candidate)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_status_needs_no_authored_leaf() {
        assert_eq!(
            compose(&json!({}), std::iter::empty(), None).unwrap(),
            json!({"succeeded":true})
        );
    }
    #[test]
    fn canonical_original_contract_derives_warning_from_actual_leaf() {
        let contract = crate::value_schema::compile_inline(
            "success.test",
            crate::value_schema::SchemaContext::FlatDocument,
            &json!({"value":{"type":"integer"},"succeeded":{"type":"boolean"}}),
        )
        .unwrap();
        let graph = json!({"output_map":{"value":"{{ task().result }}"}});
        let actual = json!("literal output");
        let candidate = json!({"value":"literal output","succeeded":true});
        let compiled = crate::value_schema::CompiledContract::new(&contract).unwrap();
        let error = compiled.validate(&candidate).unwrap_err();
        let result = compose(
            &graph,
            std::iter::once(("/value", &actual)),
            Some(&contract),
        )
        .unwrap();
        assert_eq!(
            result["output_validation_warning"],
            json!({"code":error.code,"path":error.path})
        );
        assert_eq!(result["value"], actual);
        assert_eq!(result["succeeded"], true);
    }
    #[test]
    fn authored_status_is_preserved_and_diagnostics_cannot_be_supplied() {
        assert_eq!(
            compose(
                &json!({"output_map":{"succeeded":false}}),
                std::iter::empty(),
                None
            )
            .unwrap(),
            json!({"succeeded":false})
        );
        assert!(compose(
            &json!({"output_map":{"output_validation_warning":null}}),
            std::iter::empty(),
            None
        )
        .is_err());
        assert!(compose(&json!({"output_map":[]}), std::iter::empty(), None).is_err());
        assert!(compose(
            &json!({"output_map":{}}),
            std::iter::once(("/missing", &Value::Null)),
            None
        )
        .is_err());
    }
}
fn graph(source: &SuccessSource) -> Result<&Value> {
    Ok(&source
        .parent
        .executable_snapshot
        .as_ref()
        .ok_or_else(denied)?
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition)
}
pub(super) fn admissions(source: &SuccessSource, renderer: Id) -> Vec<Id> {
    std::iter::once(renderer)
        .chain(source.children.iter().map(|child| child.admission))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub(super) fn origins(snapshot: Id) -> Vec<SecretOrigin> {
    vec![SecretOrigin::Entity {
        entity_type: "ordinary_success_metadata".into(),
        entity_id: snapshot,
        path: String::new(),
    }]
}
pub(super) fn read_version(source: &SuccessSource) -> Value {
    json!({"completed":source.completed,"value_digest":source.value_digest,"contract_digest":source.contract_digest,"children":source.children.iter().map(|child|json!([child.execution,child.binding_digest,&child.version])).collect::<Vec<_>>(),"leaves":source.leaves.iter().map(|leaf|json!([leaf.path,leaf.receipt,leaf.proof_digest])).collect::<Vec<_>>()})
}
pub(super) fn associations(source: &SuccessSource, renderer: Id) -> Value {
    json!({"budget":renderer,"admissions":admissions(source,renderer),"release":null,"success_metadata_observation":[Id::MAX,source],"success_metadata_release":[Id::MAX,source.execution,source.workflow,source.completed,source.transaction]})
}
pub(super) fn nodes(source: &SuccessSource) -> usize {
    source.leaves.len()
        + source.children.len()
        + source.config_fields.len()
        + source.result_fields.len()
        + source.variable_fields.len()
        + source.root_contributors.len()
        + source
            .children
            .iter()
            .map(|child| child.fields.len() + child.contributors.len() + child.parents.len() + 1)
            .sum::<usize>()
}
pub(super) fn physical_parents(source: &SuccessSource) -> Result<Vec<NativeStorageParent>> {
    let mut parents = vec![NativeStorageParent::WorkflowExecution {
        workflow: source.workflow,
    }];
    for child in &source.children {
        parents.extend([
            NativeStorageParent::Execution {
                execution: child.execution,
            },
            NativeStorageParent::Admission {
                admission: child.admission,
            },
        ]);
        parents.extend(storage_owners(&storage_owner(&child.read_owner)?)?);
        if let Some(pack) = child.target.pack_id {
            parents.push(NativeStorageParent::Pack { pack });
        }
    }
    Ok(parents)
}
pub(super) fn validate_storage(source: &SuccessSource) -> Result<()> {
    if source.renderer <= 0
        || source.execution <= 0
        || source.workflow <= 0
        || source.completed <= 0
        || source.parent.id != source.execution
        || source.workflow_original.id != source.workflow
        || source.workflow_original.execution != source.execution
        || source.parent.status != ExecutionStatus::Running
        || source.workflow_original.status != ExecutionStatus::Running
        || source.transaction.is_empty()
        || source.children.is_empty()
        || source.children.len() > 255
        || source.parents.len() > 256
        || nodes(source) > 256
    {
        return Err(denied());
    }
    source.contract.validate()?;
    if hash(&serde_json::to_value(&source.contract)?)? != source.contract_digest {
        return Err(denied());
    }
    for field in source
        .config_fields
        .iter()
        .chain(&source.result_fields)
        .chain(&source.variable_fields)
    {
        field.provenance()?;
    }
    for contributor in &source.root_contributors {
        contributor.validate()?;
    }
    let tasks = graph(source)?
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(denied)?;
    if tasks.len() != source.children.len()
        || tasks.iter().any(|task| {
            ["with_items", "iterate_cache", "wait_for"]
                .iter()
                .any(|name| task.get(*name).is_some_and(|value| !value.is_null()))
                || task
                    .get("next")
                    .and_then(Value::as_array)
                    .is_some_and(|edges| {
                        edges.iter().any(|edge| {
                            edge.get("do").is_some_and(|value| {
                                !value.is_null()
                                    && value.as_array().is_none_or(|targets| !targets.is_empty())
                            })
                        })
                    })
        })
    {
        return Err(denied());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for child in &source.children {
        result_view::validate_storage(child)?;
        if !ids.insert(child.execution)
            || !names.insert(child.task_name.clone())
            || child.workflow != source.workflow
            || child.transaction != source.transaction
            || child.status != "completed"
            || child.timed_out
        {
            return Err(denied());
        }
        let (_, contract) = source
            .child_contracts
            .iter()
            .find(|(id, _)| *id == child.execution)
            .ok_or_else(denied)?;
        contract.validate()?;
        if hash(&serde_json::to_value(contract)?)? != child.binding_digest {
            return Err(denied());
        }
        for edge in &child.parents {
            if !source.parents.contains(edge) {
                return Err(denied());
            }
        }
    }
    if !ids.contains(&source.completed)
        || source.child_contracts.len() != ids.len()
        || tasks.iter().any(|task| {
            task.get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| !names.contains(name))
        })
        || source
            .projected_completed
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != names
        || source.projected_completed.len() != names.len()
    {
        return Err(denied());
    }
    let mut paths = BTreeSet::new();
    for leaf in &source.leaves {
        if leaf.receipt <= 0
            || !paths.insert(&leaf.path)
            || hash(&leaf.proof)? != leaf.proof_digest
            || leaf.proof.get("renderer").and_then(Value::as_i64) != Some(source.renderer)
            || !source.parents.iter().any(|(id, site)| {
                *id == leaf.receipt
                    && site.execution == source.execution
                    && site.kind == super::super::inherited_input::InputEntityKind::ExecutionResult
                    && site.purpose == InputSitePurpose::Value
                    && site.path == leaf.path
            })
        {
            return Err(denied());
        }
    }
    for edge in &source.controls {
        if !source.parents.contains(edge) {
            return Err(denied());
        }
    }
    Ok(())
}
pub(super) async fn validate_normalized(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &SuccessSource,
) -> Result<()> {
    validate_storage(source)?;
    let original: Value = sqlx::query_scalar(
        "SELECT observation FROM runtime_native_success_metadata_observation WHERE snapshot=$1",
    )
    .bind(snapshot)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(denied)?;
    if original != serde_json::to_value(source)?
        || source.leaves.iter().any(|leaf| leaf.receipt <= 0)
    {
        return Err(denied());
    }
    for leaf in &source.leaves {
        let row:Option<(Value,String,Value,String)>=sqlx::query_as("SELECT proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1 AND execution=$2 AND entity_kind='execution_result' AND destination_purpose='value' AND destination_path=$3 AND renderer_admission=$4").bind(leaf.receipt).bind(source.execution).bind(&leaf.path).bind(source.renderer).fetch_optional(&mut *conn).await?;
        if row
            != Some((
                leaf.proof.clone(),
                leaf.proof_digest.clone(),
                leaf.encrypted_value.clone(),
                leaf.key_hash.clone(),
            ))
        {
            return Err(denied());
        }
    }
    Ok(())
}
pub(super) async fn require_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &SuccessSource,
) -> Result<()> {
    let present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_success_metadata_release WHERE snapshot=$1 AND execution=$2 AND workflow=$3 AND completed_execution=$4 AND release_transaction=$5)").bind(snapshot).bind(source.execution).bind(source.workflow).bind(source.completed).bind(&source.transaction).fetch_one(conn).await?;
    if !present {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn validate_value(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &SuccessSource,
    actual: &Value,
    key: &str,
) -> Result<()> {
    validate_normalized(conn, snapshot, source).await?;
    let mut values = Vec::new();
    for leaf in &source.leaves {
        if leaf.key_hash != crate::crypto::hash_encryption_key(key) {
            return Err(denied());
        }
        values.push((
            leaf.path.as_str(),
            crate::crypto::decrypt_json(&leaf.encrypted_value, key)?,
        ));
    }
    let candidate = compose(
        graph(source)?,
        values.iter().map(|(path, value)| (*path, value)),
        source.contract.output.as_ref(),
    )?;
    if candidate != *actual || hash(&candidate)? != source.value_digest {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn validate_body(body: &Body, source: &SuccessSource) -> Result<()> {
    validate_storage(source)?;
    if body.execution != source.execution
        || body.renderer != source.renderer
        || !body.task_path.is_empty()
        || body.renderer <= 0
        || body.authority.owner.sponsor_identity_id <= 0
    {
        return Err(denied());
    }
    Ok(())
}
