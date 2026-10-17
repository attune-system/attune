//! Genuine persisted-child result observations. No caller data, aliases or
//! pointer assertions can construct this finite native projection.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResultContractKind {
    Workflow,
    Action { format: crate::models::OutputFormat },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeResultPointerMapping {
    view: String,
    source: String,
    decoded: Option<String>,
}
impl NativeResultPointerMapping {
    pub fn view_path(&self) -> &str {
        &self.view
    }
    pub fn source_path(&self) -> &str {
        &self.source
    }
    pub fn decoded_path(&self) -> Option<&str> {
        self.decoded.as_deref()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResultViewSource {
    pub(super) execution: Id,
    pub(super) admission: Id,
    pub(super) workflow: Id,
    pub(super) task_name: String,
    pub(super) status: String,
    pub(super) timed_out: bool,
    pub(super) target: BoundRuntimeTarget,
    pub(super) read_owner: super::super::runtime_read::RuntimeReadOwner,
    pub(super) transaction: String,
    pub(super) raw: Value,
    pub(super) version: Value,
    pub(super) contract: Option<Value>,
    pub(super) contract_kind: NativeResultContractKind,
    pub(super) binding_digest: String,
    pub(super) fields: Vec<ArchivedField>,
    pub(super) contributors: Vec<super::super::accepted_input::AcceptedContributorManifest>,
    pub(super) parents: Vec<(Id, InputSite)>,
    pub(super) origins: Vec<SecretOrigin>,
    pub(super) mappings: Vec<NativeResultPointerMapping>,
}

pub struct PreparedNativeResultView {
    pub(crate) transaction: String,
    pub(crate) renderer: Id,
    pub(crate) snapshot: Id,
    pub(crate) value: Value,
    pub(crate) task_name: String,
    pub(crate) fence: ApprovalEffectFence,
    pub(super) source: ResultViewSource,
    pub(super) archived: bool,
}
impl PreparedNativeResultView {
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn source_execution(&self) -> Id {
        self.source.execution
    }
    pub fn workflow(&self) -> Id {
        self.source.workflow
    }
    pub fn mappings(&self) -> &[NativeResultPointerMapping] {
        &self.source.mappings
    }
    pub fn contract_kind(&self) -> NativeResultContractKind {
        self.source.contract_kind
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    /// Target phase only. Compare held source buffers/contracts, then publish
    /// this actual result observation. No source IAM lookup or new authority.
    pub async fn finalize(&self, conn: &mut PgConnection) -> Result<()> {
        self.require_current(conn).await?;
        if self.archived {
            return Err(denied());
        }
        let changed=sqlx::query("INSERT INTO runtime_native_result_view_release(snapshot,source_admission,source_execution,release_transaction) VALUES($1,$2,$3,pg_current_xact_id()::TEXT) ON CONFLICT DO NOTHING")
            .bind(self.snapshot).bind(self.source.admission).bind(self.source.execution).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }
    pub async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if self.archived {
            require_release(conn, self.snapshot, &self.source).await?;
        } else {
            compare_current(conn, &self.source).await?;
        }
        self.ensure_fresh_on(conn).await
    }
}

impl NativeInputSnapshotRepository {
    pub(crate) async fn native_shorthand_source(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        path: &str,
        key: Option<&str>,
        pending: bool,
    ) -> Result<(InputSite, Option<String>, Option<Value>)> {
        let (body, value) = if let Some(key) = key {
            let (body, value, _, _) = load(conn, snapshot, key).await?;
            (body, Some(value))
        } else {
            let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
            (body, None)
        };
        let Source::ResultView(source) = &body.source else {
            return Err(denied());
        };
        let record = RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
        if body.renderer != renderer
            || body.execution != record.owner_id
            || body.authority != record.authority.0
        {
            return Err(denied());
        }
        if !pending || source.transaction != transaction(conn).await? {
            require_release(conn, snapshot, source).await?;
        }
        if let (Some(value), Some(key)) = (&value, key) {
            validate_value(conn, snapshot, source, value, key).await?;
        }
        let child =
            RuntimeAuthorityRepository::require_retained_admission(conn, source.admission).await?;
        let InvocationBinding::WorkflowTask {
            workflow_ref,
            task_path,
            ..
        } = &child.authority.binding
        else {
            return Err(denied());
        };
        if child.owner_id != source.execution
            || workflow_ref != record.authority.binding.action_ref()
        {
            return Err(denied());
        }
        let graph = workflow_definition(conn, &record).await?;
        let task = canonical_task_path(&graph, task_path).ok_or_else(denied)?;
        let prefix = format!("/workflow{task}");
        let suffix = path.strip_prefix(&prefix).ok_or_else(denied)?;
        let tokens = suffix
            .strip_prefix('/')
            .ok_or_else(denied)?
            .split('/')
            .collect::<Vec<_>>();
        let when = match tokens.as_slice() {
            ["publish", ordinal] if ordinal.parse::<usize>().is_ok() => {
                let raw_task = graph.pointer(&task).ok_or_else(denied)?;
                require_legacy_publish_success(raw_task, &source.version)?;
                None
            }
            ["next", index, "publish", ordinal]
                if index.parse::<usize>().is_ok() && ordinal.parse::<usize>().is_ok() =>
            {
                let when = format!("{prefix}/next/{index}/when");
                graph
                    .pointer(when.strip_prefix("/workflow").ok_or_else(denied)?)
                    .filter(|value| !value.is_null())
                    .map(|_| when)
            }
            _ => return Err(denied()),
        };
        let variable = graph
            .pointer(path.strip_prefix("/workflow").ok_or_else(denied)?)
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && !name.contains("{{"))
            .ok_or_else(denied)?;
        Ok((
            InputSite {
                execution: record.owner_id,
                kind: super::super::inherited_input::InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Value,
                path: format!("/{}", variable.replace('~', "~0").replace('/', "~1")),
            },
            when,
            value,
        ))
    }
    pub(super) async fn prepare_result_source(
        conn: &mut PgConnection,
        record: &super::super::runtime_authority::RuntimeAdmissionRecord,
        child: Id,
        key: &str,
    ) -> Result<(ResultViewSource, Value, ApprovalEffectFence)> {
        use super::super::{
            schema_contract::{BindingOwner, SchemaContractRepository},
            FindById,
        };
        let renderer = record.owner_id;
        let outcome = InheritedInputRepository::prepare_outcome(conn, renderer, child).await?;
        sqlx::query("SELECT id FROM execution WHERE id=$1 FOR SHARE")
            .bind(child)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(denied)?;
        let row = super::super::execution::ExecutionRepository::find_by_id(&mut *conn, child)
            .await?
            .ok_or_else(denied)?;
        let metadata = row.workflow_task.as_ref().ok_or_else(denied)?;
        if row.parent != Some(renderer)
            || outcome.execution() != child
            || outcome.task_name() != metadata.task_name
        {
            return Err(denied());
        }
        let admission = RuntimeAuthorityRepository::execution_admission(conn, child)
            .await?
            .ok_or_else(denied)?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?,
            );
        let binding = SchemaContractRepository::find(conn, BindingOwner::Execution, child)
            .await?
            .ok_or_else(denied)?;
        binding.validate()?;
        let executable = row.executable_snapshot.as_ref().ok_or_else(denied)?;
        if row.action != Some(executable.executable.action.id)
            || row.action_ref != executable.executable.action.r#ref
            || row.pack_release != Some(executable.release.id)
            || row.pack_release_digest.as_deref() != Some(executable.release.digest.as_str())
            || binding.input.source
                != (crate::value_schema::SourceIdentity::Released {
                    component_ref: executable.executable.action.r#ref.clone(),
                    release_id: executable.release.id,
                    release_digest: executable.release.digest.clone(),
                })
        {
            return Err(denied());
        }
        let contract = binding
            .output
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let raw = row.result.clone().unwrap_or(Value::Null);
        let contract_kind = if executable.executable.workflow_definition.is_some() {
            NativeResultContractKind::Workflow
        } else {
            NativeResultContractKind::Action {
                format: executable.executable.action.output_format,
            }
        };
        sqlx::query("SELECT id FROM execution_secret_value WHERE entity_type='execution_result' AND entity_id=$1 ORDER BY json_path FOR SHARE").bind(child).fetch_all(&mut *conn).await?;
        let fields = load_fields(conn, "execution_result", child).await?;
        validate_field_paths(&raw, &fields)?;
        let restored = crate::secret_values::restore_secret_values(
            raw.clone(),
            &fields.iter().map(ArchivedField::stored).collect::<Vec<_>>(),
            key,
        )?;
        if !crate::secret_values::redacted_paths(&restored).is_empty() {
            return Err(denied());
        }
        let mut parents = Vec::new();
        let mut origins = vec![SecretOrigin::Entity {
            entity_type: "execution_result".into(),
            entity_id: child,
            path: String::new(),
        }];
        let mut ordinary = Vec::new();
        let schema_sources = schema_sources(&restored, &contract, contract_kind, child)?;
        if schema_sources.iter().any(|(path, _)| {
            !fields.iter().any(|field| {
                field.json_path == *path
                    || field.json_path.is_empty()
                    || path
                        .strip_prefix(&field.json_path)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        }) {
            return Err(denied());
        }
        for field in &fields {
            let mut value_source = false;
            let provenance = field.provenance()?;
            if provenance.origins.is_empty() {
                return Err(denied());
            }
            for origin in provenance.origins {
                if super::super::immutable_input_origin::is_input_receipt(&origin) {
                    let parent =
                        InheritedInputRepository::receipt_source_site(conn, &origin).await?;
                    let context =
                        InheritedInputRepository::receipt_context(conn, &parent.1, &origin, key)
                            .await?;
                    let original_path = if parent.1.execution == child
                        && parent.1.kind
                            == super::super::inherited_input::InputEntityKind::ExecutionResult
                        && covers(&field.json_path, &parent.1.path)
                    {
                        &parent.1.path
                    } else {
                        &field.json_path
                    };
                    if parent.1.purpose == InputSitePurpose::Value
                        && Some(context.accepted_value()) != restored.pointer(original_path)
                    {
                        return Err(denied());
                    }
                    value_source |= parent.1.purpose == InputSitePurpose::Value;
                    fence = fence.intersect(context.accepted_fence());
                    if !parents.contains(&parent) {
                        parents.push(parent);
                    }
                } else if schema_sources.iter().any(|(path, origins)| {
                    covers(&field.json_path, path) && origins.contains(&origin)
                }) {
                    // Canonical schema redaction, including the producer's
                    // protected whole-result fallback for an invalid output.
                    value_source = true;
                } else {
                    if matches!(origin, SecretOrigin::Local { .. }) {
                        return Err(denied());
                    }
                    ordinary.push(origin.clone());
                    value_source = true;
                }
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
            if !value_source {
                return Err(denied());
            }
        }
        let contributors = if ordinary.is_empty() {
            vec![]
        } else {
            let (authorities, manifests) =
                super::super::accepted_input::AcceptedInputRepository::source_contributors(
                    conn, child,
                )
                .await?;
            super::super::immutable_input_origin::ImmutableInputOriginRepository::require(
                conn,
                &authorities,
                &ordinary,
            )
            .await?;
            manifests
        };
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Execution(child),
            record.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let (value, mappings) = project(&restored)?;
        let mut version:Value=sqlx::query_scalar("SELECT jsonb_build_object('updated',updated,'status',status,'workflow_task',workflow_task) FROM execution WHERE id=$1").bind(child).fetch_one(&mut *conn).await?;
        version["projection_digest"] = json!(hash(&raw)?);
        version["ciphertext_digest"] = json!(hash(&serde_json::to_value(&fields)?)?);
        version["contract_digest"] = json!(contract.as_ref().map(hash).transpose()?);
        version["contract_kind"] = serde_json::to_value(contract_kind)?;
        let source = ResultViewSource {
            execution: child,
            admission: admission.id,
            workflow: metadata.workflow_execution,
            task_name: metadata.task_name.clone(),
            status: serde_json::to_value(row.status)?
                .as_str()
                .ok_or_else(denied)?
                .into(),
            timed_out: metadata.timed_out,
            target: bound_target(child, &resolved)?,
            read_owner: super::super::runtime_read::RuntimeReadOwner::of(&resolved),
            transaction: transaction(conn).await?,
            raw,
            version,
            contract,
            contract_kind,
            binding_digest: hash(&serde_json::to_value(&binding)?)?,
            fields,
            contributors,
            parents,
            origins,
            mappings,
        };
        validate_storage(&source)?;
        fence.ensure_fresh_on(conn).await?;
        Ok((source, value, fence))
    }

    pub async fn prepare_native_result_view(
        conn: &mut PgConnection,
        renderer: Id,
        child: Id,
        key: &str,
    ) -> Result<PreparedNativeResultView> {
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let (source, value, fence) = Self::prepare_result_source(conn, &record, child, key).await?;
        let body = Body {
            renderer: record.id,
            execution: renderer,
            task_path: String::new(),
            authority: record.authority.0.clone(),
            approval_deadline: fence.approval_deadline(),
            source: Source::ResultView(source.clone()),
        };
        let snapshot = save(conn, &body, &value, key).await?;
        for (receipt, _) in &source.parents {
            sqlx::query("INSERT INTO runtime_native_input_snapshot_parent(snapshot,receipt) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(snapshot).bind(receipt).execute(&mut *conn).await?;
        }
        sqlx::query(
            "INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)",
        )
        .bind(snapshot)
        .bind(source.admission)
        .execute(&mut *conn)
        .await?;
        archive_wait(conn, snapshot, &source.fields).await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeResultView {
            transaction: source.transaction.clone(),
            renderer: record.id,
            snapshot,
            value,
            task_name: source.task_name.clone(),
            fence,
            source,
            archived: false,
        })
    }

    pub async fn replay_native_result_view(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        key: &str,
    ) -> Result<PreparedNativeResultView> {
        let (body, value, _, _) = load(conn, snapshot, key).await?;
        let Source::ResultView(source) = body.source else {
            return Err(denied());
        };
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        if body.renderer != record.id
            || body.execution != renderer
            || body.authority != record.authority.0
        {
            return Err(denied());
        }
        require_release(conn, snapshot, &source).await?;
        validate_value(conn, snapshot, &source, &value, key).await?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(
                &RuntimeAuthorityRepository::retained_admission_fence(conn, source.admission)
                    .await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?);
        for (parent, site) in &source.parents {
            let origin = SecretOrigin::Entity {
                entity_type: site.kind.origin_kind(site.purpose),
                entity_id: *parent,
                path: site.path.clone(),
            };
            fence = fence.intersect(
                InheritedInputRepository::receipt_context(conn, site, &origin, key)
                    .await?
                    .accepted_fence(),
            );
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeResultView {
            transaction: transaction(conn).await?,
            renderer: record.id,
            snapshot,
            value,
            task_name: source.task_name.clone(),
            fence,
            source,
            archived: true,
        })
    }
}

/// The canonical parser gives a publish-only legacy task an implicit succeeded
/// transition. Authenticate that finite gate from the actual archived child
/// version, not a fabricated /when declaration or a supplied outcome Boolean.
fn require_legacy_publish_success(task: &Value, version: &Value) -> Result<()> {
    if version.get("status").and_then(Value::as_str) != Some("completed")
        || task
            .get("publish")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(denied());
    }
    // Task::normalize_transitions gives populated explicit next precedence.
    // Empty next still normalizes the finite legacy publish-only form.
    if task
        .get("next")
        .is_some_and(|next| next.as_array().is_none_or(|next| !next.is_empty()))
        || ["on_success", "on_failure", "on_complete", "on_timeout"]
            .iter()
            .any(|name| task.get(*name).is_some_and(|value| !value.is_null()))
        || task.get("decision").is_some_and(|decisions| {
            decisions
                .as_array()
                .is_none_or(|decisions| !decisions.is_empty())
        })
    {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn validate_storage(source: &ResultViewSource) -> Result<()> {
    validate_field_paths(&source.raw, &source.fields)?;
    if source.version.get("projection_digest") != Some(&json!(hash(&source.raw)?))
        || source.version.get("ciphertext_digest")
            != Some(&json!(hash(&serde_json::to_value(&source.fields)?)?))
        || source.version.get("contract_digest")
            != Some(&json!(source.contract.as_ref().map(hash).transpose()?))
        || source.version.get("contract_kind") != Some(&serde_json::to_value(source.contract_kind)?)
    {
        return Err(denied());
    }
    if source.execution <= 0
        || source.admission <= 0
        || source.workflow <= 0
        || source.target.id != source.execution
        || source.target.kind != "execution"
        || source.task_name.is_empty()
        || source.transaction.is_empty()
        || source.fields.len()
            + source.parents.len()
            + source.origins.len()
            + source.contributors.len()
            + source.mappings.len()
            > 256
    {
        return Err(denied());
    }
    if !matches!(
        source.status.as_str(),
        "completed" | "failed" | "timeout" | "cancelled" | "abandoned"
    ) {
        return Err(denied());
    }
    if source.binding_digest.is_empty()
        || source.version.get("status").and_then(Value::as_str) != Some(source.status.as_str())
        || source
            .version
            .pointer("/workflow_task/task_name")
            .and_then(Value::as_str)
            != Some(source.task_name.as_str())
        || source
            .version
            .pointer("/workflow_task/workflow_execution")
            .and_then(Value::as_i64)
            != Some(source.workflow)
        || source
            .version
            .pointer("/workflow_task/timed_out")
            .and_then(Value::as_bool)
            != Some(source.timed_out)
        || !source.origins.contains(&SecretOrigin::Entity {
            entity_type: "execution_result".into(),
            entity_id: source.execution,
            path: String::new(),
        })
    {
        return Err(denied());
    }
    for manifest in &source.contributors {
        manifest.validate()?;
    }
    Ok(())
}
fn validate_field_paths(raw: &Value, fields: &[ArchivedField]) -> Result<()> {
    use std::collections::BTreeSet;
    let actual: BTreeSet<_> = crate::secret_values::redacted_paths(raw)
        .into_iter()
        .collect();
    let expected: BTreeSet<_> = fields.iter().map(|field| field.json_path.clone()).collect();
    if fields.len() != expected.len() || actual != expected {
        return Err(denied());
    }
    Ok(())
}
fn covers(parent: &str, child: &str) -> bool {
    parent.is_empty()
        || parent == child
        || child
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn schema_sources(
    restored: &Value,
    contract: &Option<Value>,
    kind: NativeResultContractKind,
    execution: Id,
) -> Result<Vec<(String, Vec<SecretOrigin>)>> {
    use crate::secret_values::{SecretSource, SecretValueInput};
    let contract = contract
        .as_ref()
        .map(|value| serde_json::from_value::<crate::value_schema::Contract>(value.clone()))
        .transpose()?;
    let fields = match kind {
        NativeResultContractKind::Action { format } => {
            crate::secret_values::redact_bound_action_result(
                restored.clone(),
                contract.as_ref(),
                format,
                execution,
                &[],
                &[],
            )?
            .1
        }
        NativeResultContractKind::Workflow => {
            if let Some(contract) = &contract {
                let compiled =
                    crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
                match compiled.secret_paths(restored) {
                    Ok(_) => {
                        crate::secret_values::merge_contract_secret_redactions(
                            restored.clone(),
                            &[],
                            &compiled,
                        )?
                        .1
                    }
                    Err(_) if compiled.declares_secrets() => vec![SecretValueInput {
                        json_path: String::new(),
                        value: restored.clone(),
                        source_kind: "parameter_schema".into(),
                        source_ref: Some(String::new()),
                    }],
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
    };
    fields
        .into_iter()
        .map(|field| {
            let provenance = match field.source_kind.as_str() {
                "parameter_schema" => SecretSource::ParameterSchema {
                    path: field.source_ref.ok_or_else(denied)?,
                }
                .provenance(),
                "provenance" => serde_json::from_str::<crate::secret_provenance::SecretProvenance>(
                    field.source_ref.as_deref().ok_or_else(denied)?,
                )?,
                _ => return Err(denied()),
            };
            Ok((field.json_path, provenance.origins))
        })
        .collect()
}
pub(super) fn context_path(source: &ResultViewSource, path: &str) -> bool {
    let name = source.task_name.replace('~', "~0").replace('/', "~1");
    [
        "/result".to_owned(),
        format!("/task/{name}"),
        format!("/tasks/{name}"),
    ]
    .iter()
    .any(|prefix| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}
pub(super) async fn validate_value(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &ResultViewSource,
    value: &Value,
    key: &str,
) -> Result<()> {
    validate_storage(source)?;
    if load_fields(conn, "native_input_snapshot", snapshot).await? != source.fields {
        return Err(denied());
    }
    if original_value(source, key)? != *value {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn original_value(source: &ResultViewSource, key: &str) -> Result<Value> {
    validate_storage(source)?;
    let raw = crate::secret_values::restore_secret_values(
        source.raw.clone(),
        &source
            .fields
            .iter()
            .map(ArchivedField::stored)
            .collect::<Vec<_>>(),
        key,
    )?;
    if !crate::secret_values::redacted_paths(&raw).is_empty() {
        return Err(denied());
    }
    let schema = schema_sources(
        &raw,
        &source.contract,
        source.contract_kind,
        source.execution,
    )?;
    if schema.iter().any(|(path, _)| {
        !source
            .fields
            .iter()
            .any(|field| covers(&field.json_path, path))
    }) {
        return Err(denied());
    }
    for field in &source.fields {
        for origin in field.provenance()?.origins {
            if matches!(origin, SecretOrigin::Local { .. })
                && !schema.iter().any(|(path, origins)| {
                    covers(&field.json_path, path) && origins.contains(&origin)
                })
            {
                return Err(denied());
            }
        }
    }
    let (expected, mappings) = project(&raw)?;
    if mappings != source.mappings {
        return Err(denied());
    }
    if let Some(contract) = &source.contract {
        let contract: crate::value_schema::Contract = serde_json::from_value(contract.clone())?;
        crate::value_schema::CompiledContract::new(&contract).map_err(|_| denied())?;
    }
    Ok(expected)
}
pub(super) async fn require_release(
    conn: &mut PgConnection,
    snapshot: Id,
    source: &ResultViewSource,
) -> Result<()> {
    let matched:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_result_view_release WHERE snapshot=$1 AND source_admission=$2 AND source_execution=$3 AND release_transaction=$4)").bind(snapshot).bind(source.admission).bind(source.execution).bind(&source.transaction).fetch_one(conn).await?;
    if !matched {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn compare_current(
    conn: &mut PgConnection,
    source: &ResultViewSource,
) -> Result<()> {
    use super::super::FindById;
    let original =
        super::super::execution::ExecutionRepository::find_by_id(&mut *conn, source.execution)
            .await?
            .ok_or_else(denied)?;
    let snapshot = original.executable_snapshot.as_ref().ok_or_else(denied)?;
    let kind = if snapshot.executable.workflow_definition.is_some() {
        NativeResultContractKind::Workflow
    } else {
        NativeResultContractKind::Action {
            format: snapshot.executable.action.output_format,
        }
    };
    let row:Option<(Option<Value>,Value)>=sqlx::query_as("SELECT result,jsonb_build_object('updated',updated,'status',status,'workflow_task',workflow_task) FROM execution WHERE id=$1").bind(source.execution).fetch_optional(&mut *conn).await?;
    let (raw, mut version) = row.ok_or_else(denied)?;
    let fields = load_fields(conn, "execution_result", source.execution).await?;
    version["projection_digest"] = json!(hash(&raw.clone().unwrap_or(Value::Null))?);
    version["ciphertext_digest"] = json!(hash(&serde_json::to_value(&fields)?)?);
    version["contract_digest"] = source.version["contract_digest"].clone();
    version["contract_kind"] = serde_json::to_value(kind)?;
    if raw.unwrap_or(Value::Null) != source.raw
        || version != source.version
        || fields != source.fields
    {
        return Err(denied());
    }
    let binding = super::super::schema_contract::SchemaContractRepository::find(
        conn,
        super::super::schema_contract::BindingOwner::Execution,
        source.execution,
    )
    .await?
    .ok_or_else(denied)?;
    if binding
        .output
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        != source.contract
        || hash(&serde_json::to_value(binding)?)? != source.binding_digest
    {
        return Err(denied());
    }
    Ok(())
}

fn project(raw: &Value) -> Result<(Value, Vec<NativeResultPointerMapping>)> {
    bounded(raw)?;
    let mut mappings = vec![NativeResultPointerMapping {
        view: String::new(),
        source: String::new(),
        decoded: None,
    }];
    let Some(object) = raw.as_object() else {
        return Ok((raw.clone(), mappings));
    };
    let mut merged = object.clone();
    for name in object.keys() {
        let path = format!("/{}", name.replace('~', "~0").replace('/', "~1"));
        mappings.push(NativeResultPointerMapping {
            view: path.clone(),
            source: path,
            decoded: None,
        });
    }
    if let Some(Value::Object(data)) = object.get("data") {
        for (name, value) in data {
            if !merged.contains_key(name) {
                let path = format!("/{}", name.replace('~', "~0").replace('/', "~1"));
                merged.insert(name.clone(), value.clone());
                mappings.push(NativeResultPointerMapping {
                    view: path.clone(),
                    source: format!("/data{path}"),
                    decoded: None,
                });
            }
        }
    } else if !object.contains_key("data") {
        if let Some(stdout) = object.get("stdout").and_then(Value::as_str) {
            if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(stdout.trim()) {
                for (name, value) in parsed {
                    if !merged.contains_key(&name) {
                        let path = format!("/{}", name.replace('~', "~0").replace('/', "~1"));
                        merged.insert(name, value);
                        mappings.push(NativeResultPointerMapping {
                            view: path.clone(),
                            source: "/stdout".into(),
                            decoded: Some(path),
                        });
                    }
                }
            }
        }
    }
    let value = Value::Object(merged);
    bounded(&value)?;
    if mappings.len() > 256 {
        return Err(denied());
    }
    Ok((value, mappings))
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_publish_gate_matches_the_actual_parser_success_normalization() {
        let raw = serde_json::json!({"name":"source","action":"fixture.leaf","publish":["whole"],"next":[]});
        super::require_legacy_publish_success(&raw, &serde_json::json!({"status":"completed"}))
            .unwrap();
        let mut task: crate::workflow::Task = serde_json::from_value(raw).unwrap();
        task.normalize_transitions();
        assert_eq!(task.next.len(), 1);
        assert_eq!(task.next[0].when.as_deref(), Some("{{ succeeded() }}"));
        assert_eq!(task.next[0].publish.len(), 1);
    }

    #[test]
    fn legacy_publish_denies_noncompleted_or_missing_original_child_versions() {
        let task = serde_json::json!({"publish":["whole"]});
        for status in ["failed", "timeout", "cancelled", "abandoned", "running"] {
            assert!(super::require_legacy_publish_success(
                &task,
                &serde_json::json!({"status":status})
            )
            .is_err());
        }
        assert!(super::require_legacy_publish_success(&task, &serde_json::json!({})).is_err());
    }

    #[test]
    fn explicit_next_cannot_authorize_an_ignored_legacy_publish_directive() {
        let raw = serde_json::json!({"name":"source","action":"fixture.leaf","publish":["ignored"],"next":[{"publish":["explicit"]}]});
        assert!(super::require_legacy_publish_success(
            &raw,
            &serde_json::json!({"status":"completed"})
        )
        .is_err());
        let mut task: crate::workflow::Task = serde_json::from_value(raw).unwrap();
        task.normalize_transitions();
        assert!(task.publish.is_empty());
        assert_eq!(task.next[0].when, None);
        assert_eq!(task.next[0].publish.len(), 1);
    }

    #[test]
    fn other_legacy_successor_and_decision_forms_do_not_gain_an_implicit_gate() {
        for task in [
            serde_json::json!({}),
            serde_json::json!({"publish":{"0":"whole"}}),
            serde_json::json!({"publish":[]}),
        ] {
            assert!(super::require_legacy_publish_success(
                &task,
                &serde_json::json!({"status":"completed"})
            )
            .is_err());
        }
        for field in ["on_success", "on_failure", "on_complete", "on_timeout"] {
            let mut task = serde_json::json!({"publish":["whole"]});
            task[field] = serde_json::json!("other");
            assert!(super::require_legacy_publish_success(
                &task,
                &serde_json::json!({"status":"completed"})
            )
            .is_err());
        }
        assert!(super::require_legacy_publish_success(&serde_json::json!({"publish":["whole"],"decision":[{"when":"{{ true }}","next":"other"}]}),&serde_json::json!({"status":"completed"})).is_err());
    }
    use super::*;
    #[test]
    fn schema_sources_use_the_actual_worker_data_envelope_and_workflow_whole_result() {
        use crate::value_schema::{compile_inline, SchemaContext};
        let contract = Some(
            serde_json::to_value(
                compile_inline(
                    "fixture.result",
                    SchemaContext::FlatDocument,
                    &json!({"token":{"type":"string","secret":true,"required":true}}),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let action =
            json!({"data":{"token":"{{ opaque data }}"},"stdout":"diagnostic","succeeded":true});
        let expected = schema_sources(
            &action,
            &contract,
            NativeResultContractKind::Action {
                format: crate::models::OutputFormat::Json,
            },
            1,
        )
        .unwrap();
        let (_, origins) = expected
            .iter()
            .find(|(path, _)| path == "/data/token")
            .unwrap();
        assert!(origins.contains(&SecretOrigin::Local {
            source_kind: "parameter_schema".into(),
            source_ref: Some("/token".into())
        }));
        assert!(expected.iter().any(|(path, _)| path == "/stdout"));
        assert!(!expected.iter().any(|(path, _)| path == "/token"));
        let mut invalid = action.clone();
        invalid["data"]["token"] = json!(42);
        assert!(schema_sources(
            &invalid,
            &contract,
            NativeResultContractKind::Action {
                format: crate::models::OutputFormat::Json
            },
            1
        )
        .unwrap()
        .iter()
        .any(|(path, _)| path == "/data"));
        let workflow = json!({"token":"{{ opaque workflow }}"});
        assert!(
            schema_sources(&workflow, &contract, NativeResultContractKind::Workflow, 1)
                .unwrap()
                .iter()
                .any(|(path, _)| path == "/token")
        );
        let invalid = json!({"token":42});
        assert!(
            schema_sources(&invalid, &contract, NativeResultContractKind::Workflow, 1)
                .unwrap()
                .iter()
                .any(|(path, _)| path.is_empty())
        );
    }
    #[test]
    fn stdout_is_decoded_once_as_data_and_aliases_keep_real_source_pointers() {
        let (view, mapping) = project(
            &json!({"stdout":"{\"token\":\"{{ keystore['never'] }}\",\"same\":2}","same":1}),
        )
        .unwrap();
        assert_eq!(view["token"], json!("{{ keystore['never'] }}"));
        assert_eq!(view["same"], json!(1));
        let token = mapping
            .iter()
            .find(|mapping| mapping.view == "/token")
            .unwrap();
        assert_eq!(token.source_path(), "/stdout");
        assert_eq!(token.decoded_path(), Some("/token"));
        let (view, mapping) =
            project(&json!({"data":{"a/b":3},"stdout":"{\"ignored\":true}"})).unwrap();
        assert_eq!(view["a/b"], json!(3));
        assert!(view.get("ignored").is_none());
        assert_eq!(
            mapping
                .iter()
                .find(|mapping| mapping.view == "/a~1b")
                .unwrap()
                .source_path(),
            "/data/a~1b"
        );
    }

    #[test]
    fn ciphertext_must_replace_the_actual_original_marker_not_an_alias_pointer() {
        let raw = json!({"data":crate::secret_values::redaction_marker()});
        let mut field = ArchivedField {
            json_path: "/data".into(),
            encrypted_value: Value::Null,
            encryption_key_hash: None,
            source_kind: "parameter_schema".into(),
            source_ref: Some("/data".into()),
        };
        validate_field_paths(&raw, &[field.clone()]).unwrap();
        field.json_path = "/token".into();
        assert!(validate_field_paths(&raw, &[field]).is_err());
        assert!(validate_field_paths(&raw, &[]).is_err());
    }

    #[test]
    fn whole_result_and_data_aliases_preserve_envelope_and_literal_values() {
        let raw = json!({"token":"envelope","data":{"token":"hidden collision","nested":{"literal":"{{ result().never }}"}}});
        let (view, mappings) = project(&raw).unwrap();
        assert_eq!(view["token"], raw["token"]);
        assert_eq!(view["data"], raw["data"]);
        assert_eq!(view["nested"]["literal"], raw["data"]["nested"]["literal"]);
        assert_eq!(
            mappings
                .iter()
                .find(|mapping| mapping.view == "/nested")
                .unwrap()
                .source,
            "/data/nested"
        );
        assert!(!mappings
            .iter()
            .any(|mapping| mapping.source == "/data/token"));
        assert_eq!(project(&json!(42)).unwrap().0, json!(42));
    }
}
