//! Initial native variables are prepared from immutable declarations before the
//! workflow row exists. Values and selected evidence share A's single evaluator.
use super::{
    declared_input::DeclaredInputRepository,
    execution::ExecutionRepository,
    execution_secret_value::ExecutionSecretValueRepository,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        PreparedInputSource, ReceiptContext,
    },
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository},
    FindById,
};
use crate::{
    models::Id,
    secret_provenance::SecretProvenance,
    secret_values::{PreparedSecretValue, SecretPathSource, SecretSource},
    Error, Result,
};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};

pub struct PreparedWorkflowVariables {
    variables: Value,
    secrets: Vec<PreparedSecretValue>,
    receipts: Vec<ReceiptContext>,
    fence: ApprovalEffectFence,
}
impl PreparedWorkflowVariables {
    pub fn variables(&self) -> &Value {
        &self.variables
    }
    pub fn secrets(&self) -> &[PreparedSecretValue] {
        &self.secrets
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.fence.ensure_fresh_on(conn).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}

pub struct NativeWorkflowMaterializationRepository;
impl NativeWorkflowMaterializationRepository {
    /// Caller protects the actual parent and first confirms that no workflow
    /// state exists. Do not call on deduplicated startup or reuse these contexts
    /// in another transaction. The caller inserts these EXACT variables and
    /// ciphertext, or rolls this transaction back in full.
    pub async fn prepare_initial_variables(
        conn: &mut PgConnection,
        execution: Id,
        key: &str,
    ) -> Result<PreparedWorkflowVariables> {
        let admission = DeclaredInputRepository::require_source(conn, execution).await?;
        let fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?;
        let row = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(denied)?;
        let snapshot = row.executable_snapshot.as_ref().ok_or_else(denied)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let authored = graph
            .get("vars")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        check_bounds(&authored)?;
        let fields = authored.as_object().ok_or_else(denied)?;
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| denied())?;
        let declarations: BTreeMap<_, _> = DeclaredInputRepository::bindings(conn, execution)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let system = Inputs::prepare_system(conn, execution).await?;
        let config = row.config.unwrap_or_else(|| serde_json::json!({}));
        let stored = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            execution,
        )
        .await?;
        if stored.len() > 256 {
            return Err(denied());
        }
        let mut parameters = Vec::new();
        let mut protected = Vec::new();
        for field in &stored {
            let original: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
            let site = InputSite {
                execution,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Value,
                path: field.json_path.clone(),
            };
            if field.source_kind != "provenance" || original.origins.len() != 1 {
                return Err(denied());
            }
            let source =
                Inputs::prepare_source(conn, execution, &site, &original.origins[0], key).await?;
            protected.push(field.json_path.clone());
            parameters.push((field.json_path.clone(), source));
        }
        let public_parameters = public_leaves(&config, &protected)?;
        let mut leaves = Vec::new();
        for (name, value) in fields {
            collect_leaves(value, format!("/{}", escape(name)), &mut leaves, 0)?;
        }
        // Literal initial values are available to later authored expressions,
        // irrespective of map ordering. They still come from exact code sites.
        leaves.sort_by_key(|(_, value)| contains_template(value));
        let mut variables = authored.clone();
        let mut sources = Vec::new();
        let mut receipts = Vec::new();
        let mut published: Vec<(String, PreparedInputSource)> = Vec::new();
        for (path, value) in leaves {
            let source_path = format!("/workflow/vars{path}");
            let rendered = if let Some(binding) = declarations.get(&source_path) {
                let mut session = Inputs::begin_render(
                    conn,
                    execution,
                    &binding.declaration_revision,
                    &binding.source_path,
                    &binding.expression,
                    InputSitePurpose::Value,
                )
                .await?;
                let names = expression_names(&binding.expression)?;
                if names.contains("parameters") {
                    for (path, source) in &parameters {
                        Inputs::bind_prepared(
                            &mut session,
                            &format!("/parameters{path}"),
                            source,
                            source.value(),
                        )?;
                    }
                    for path in &public_parameters {
                        Inputs::bind_public_field_with_key(
                            conn,
                            &mut session,
                            &format!("/parameters{path}"),
                            &InputSite {
                                execution,
                                kind: InputEntityKind::ExecutionConfig,
                                purpose: InputSitePurpose::Value,
                                path: path.clone(),
                            },
                            key,
                        )
                        .await?;
                    }
                }
                if names.contains("system") {
                    Inputs::bind_prepared_system(&mut session, &system)?;
                }
                if binding.uses_pack_configuration() {
                    Inputs::bind_configuration(conn, &mut session, key).await?;
                }
                for (path, source) in &published {
                    for namespace in ["workflow", "vars", "variables"] {
                        if names.contains(namespace) {
                            Inputs::bind_prepared(
                                &mut session,
                                &format!("/{namespace}{path}"),
                                source,
                                source.value(),
                            )?;
                        }
                    }
                    let name = path
                        .split('/')
                        .nth(1)
                        .ok_or_else(denied)?
                        .replace("~1", "/")
                        .replace("~0", "~");
                    if names.contains(&name) && !reserved(&name) {
                        Inputs::bind_prepared(&mut session, path, source, source.value())?;
                    }
                }
                Inputs::evaluate_with_keys(conn, session, key).await?
            } else {
                if contains_template(&value) {
                    return Err(denied());
                }
                Inputs::literal(conn, execution, &revision, &source_path).await?
            };
            let site = InputSite {
                execution,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Value,
                path: path.clone(),
            };
            let destination = Inputs::prepare_destination(conn, &rendered, &site, key).await?;
            let receipt = Inputs::record_prepared(
                conn,
                &rendered,
                &destination,
                rendered.value(),
                rendered.selections(),
                key,
            )
            .await?;
            *variables.pointer_mut(&path).ok_or_else(denied)? = rendered.value().clone();
            check_bounds(&variables)?;
            // No workflow row exists yet. The same-transaction receipt is the
            // exact pending value; source and destination preparation stay ahead
            // of the INSERT. The caller must persist this exact complete object.
            let source =
                Inputs::prepare_source(conn, execution, &site, &receipt.origin(), key).await?;
            sources.push(SecretPathSource {
                path: path.clone(),
                source: SecretSource::Bound(receipt.provenance()),
            });
            published.push((path, source));
            receipts.push(receipt);
        }
        let (variables, secrets) =
            crate::secret_values::redact_secret_path_sources(variables, &sources);
        check_bounds(&variables)?;
        let secrets = crate::secret_values::prepare_secret_values(secrets, key)?;
        let mut stored_bytes = 0usize;
        for secret in &secrets {
            stored_bytes = stored_bytes
                .checked_add(serde_json::to_vec(secret)?.len())
                .ok_or_else(denied)?;
            if stored_bytes > 1024 * 1024 {
                return Err(denied());
            }
        }
        let prepared = PreparedWorkflowVariables {
            variables,
            secrets,
            receipts,
            fence,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }
}

fn denied() -> Error {
    Error::PermissionDenied("Initial workflow declaration/source is unavailable or changed".into())
}
pub(super) fn check_bounds(value: &Value) -> Result<()> {
    let mut pending = vec![(value, 0)];
    let mut nodes = 0usize;
    let mut bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 {
            return Err(denied());
        }
        bytes = bytes.checked_add(8).ok_or_else(denied)?;
        match value {
            Value::String(value) => bytes = bytes.checked_add(value.len()).ok_or_else(denied)?,
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                for (name, value) in values {
                    bytes = bytes.checked_add(name.len()).ok_or_else(denied)?;
                    pending.push((value, depth + 1));
                }
            }
            _ => {}
        }
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
    }
    // Account for JSON escaping as well as source UTF-8 bytes. The walk above
    // already bounds allocation/depth before this exact serialized-byte check.
    if serde_json::to_vec(value)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    Ok(())
}
pub(super) fn escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
pub(super) fn reserved(name: &str) -> bool {
    matches!(
        name,
        "parameters"
            | "workflow"
            | "vars"
            | "variables"
            | "task"
            | "tasks"
            | "result"
            | "config"
            | "pack"
            | "keystore"
            | "system"
            | "item"
            | "index"
            | "inquiry"
            | "execution"
            | "work_queue_item"
    )
}
pub(super) fn contains_template(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains("{{"),
        Value::Array(values) => values.iter().any(contains_template),
        Value::Object(values) => values.values().any(contains_template),
        _ => false,
    }
}
pub(super) fn collect_leaves(
    value: &Value,
    path: String,
    leaves: &mut Vec<(String, Value)>,
    depth: usize,
) -> Result<()> {
    if depth > 32 || leaves.len() >= 256 {
        return Err(denied());
    }
    if !contains_template(value) || value.is_string() {
        leaves.push((path, value.clone()));
        return Ok(());
    }
    match value {
        Value::Object(values) => {
            for (field, value) in values {
                collect_leaves(
                    value,
                    format!("{path}/{}", escape(field)),
                    leaves,
                    depth + 1,
                )?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_leaves(value, format!("{path}/{index}"), leaves, depth + 1)?;
            }
        }
        _ => return Err(denied()),
    }
    Ok(())
}
pub(super) fn public_leaves(value: &Value, protected: &[String]) -> Result<Vec<String>> {
    let mut pending = vec![(String::new(), value, 0)];
    let mut paths = Vec::new();
    let mut nodes = 0;
    while let Some((path, value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 16384 || depth > 32 || paths.len() > 256 {
            return Err(denied());
        }
        if protected
            .iter()
            .any(|secret| crate::secret_values::pointer_suffix(&path, secret).is_some())
        {
            continue;
        }
        if !protected
            .iter()
            .any(|secret| crate::secret_values::pointer_suffix(secret, &path).is_some())
        {
            paths.push(path);
            continue;
        }
        match value {
            Value::Object(values) => {
                for (field, value) in values {
                    pending.push((format!("{path}/{}", escape(field)), value, depth + 1));
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    pending.push((format!("{path}/{index}"), value, depth + 1));
                }
            }
            _ => return Err(denied()),
        }
    }
    Ok(paths)
}
pub(super) fn expression_names(template: &str) -> Result<BTreeSet<String>> {
    expression_symbols(template, false)
}
pub(super) fn expression_functions(template: &str) -> Result<BTreeSet<String>> {
    expression_symbols(template, true)
}
fn expression_symbols(template: &str, functions: bool) -> Result<BTreeSet<String>> {
    use crate::workflow::expression::{parse_expression, Expr};
    let mut names = BTreeSet::new();
    let wrapped;
    let mut cursor = if template.contains("{{") {
        template
    } else {
        wrapped = format!("{{{{ {template} }}}}");
        &wrapped
    };
    while let Some(open) = cursor.find("{{") {
        let rest = &cursor[open + 2..];
        let close = rest.find("}}").ok_or_else(denied)?;
        let ast = parse_expression(rest[..close].trim()).map_err(|_| denied())?;
        let mut pending = vec![(&ast, 0)];
        let mut nodes = 0;
        while let Some((node, depth)) = pending.pop() {
            nodes += 1;
            if nodes > 16384 || depth > 32 {
                return Err(denied());
            }
            match node {
                Expr::Ident(name) => {
                    if !functions {
                        names.insert(name.clone());
                    }
                }
                Expr::DotAccess { object, .. } => pending.push((object, depth + 1)),
                Expr::IndexAccess { object, index } => {
                    pending.push((object, depth + 1));
                    pending.push((index, depth + 1));
                }
                Expr::BinaryOp { left, right, .. } => {
                    pending.push((left, depth + 1));
                    pending.push((right, depth + 1));
                }
                Expr::UnaryOp { operand, .. } => pending.push((operand, depth + 1)),
                Expr::FunctionCall { name, args } => {
                    if functions {
                        names.insert(name.clone());
                    }
                    pending.extend(args.iter().map(|value| (value, depth + 1)))
                }
                Expr::Array(values) => {
                    pending.extend(values.iter().map(|value| (value, depth + 1)))
                }
                Expr::Literal(_) => {}
            }
        }
        cursor = &rest[close + 2..];
    }
    Ok(names)
}
