//! Server renderers consume exact authored sites, never evaluated caller strings.
use super::{
    component_configuration::ComponentConfigurationRepository,
    declared_input::{DeclaredInputPermit, DeclaredInputRepository},
    key::KeyRepository,
};
use crate::{
    key_access::ResolvedKey, models::Id, secret_provenance::SecretOrigin,
    secret_values::SecretPathSource, Error, Result,
};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::BTreeMap;

pub struct DeclaredInputMaterializationRepository;
impl DeclaredInputMaterializationRepository {
    /// Final creation boundary, after every input/schema/contributor write.
    /// Sealing rewrites actual field provenance to first typed deliveries while
    /// preserving ciphertext. Never write a pre-seal buffer after this returns.
    pub async fn seal_execution_inputs(
        conn: &mut PgConnection,
        execution: Id,
        encryption_key: &str,
    ) -> Result<Vec<super::inherited_input::ReceiptContext>> {
        super::accepted_input::AcceptedInputRepository::seal_execution(
            conn,
            execution,
            encryption_key,
        )
        .await?;
        Self::prepare_execution_input_receipts(conn, execution, encryption_key).await
    }

    /// Source-only revalidation for later publication/recovery. This never seals
    /// an old execution or replaces accepted inputs with current evidence.
    pub async fn prepare_execution_input_receipts(
        conn: &mut PgConnection,
        execution: Id,
        encryption_key: &str,
    ) -> Result<Vec<super::inherited_input::ReceiptContext>> {
        let actual =
            super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *conn,
                crate::secret_values::ENTITY_EXECUTION_CONFIG,
                execution,
            )
            .await?;
        let (ordinary, mut receipts) = super::immutable_input_origin::prepare_input_receipts(
            conn,
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            execution,
            &actual,
            encryption_key,
        )
        .await?;
        receipts.extend(
            super::accepted_input::AcceptedInputRepository::prepare_input_controls(
                conn,
                execution,
                encryption_key,
            )
            .await?,
        );
        crate::key_access::authorize_execution_parameter_key_origins_with_conn(
            conn, execution, None, &ordinary,
        )
        .await?;
        super::immutable_input_origin::bind_input_receipt_buffers(
            conn,
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            execution,
            &actual,
        )
        .await?;
        for receipt in &receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(receipts)
    }

    /// An authored workflow subtree, not a rendered/supplied value. This permits
    /// the renderer to evaluate each captured declaration leaf exactly once.
    pub async fn workflow_value_sites(
        conn: &mut PgConnection,
        execution: Id,
        source_path: &str,
        value: &Value,
    ) -> Result<Vec<(String, super::declared_input::DeclaredInputBinding)>> {
        use super::{execution::ExecutionRepository, FindById};
        let pointer = source_path
            .strip_prefix("/workflow/")
            .map(|suffix| format!("/{suffix}"))
            .ok_or_else(|| Error::validation("Expected an exact authored workflow path"))?;
        let row = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Workflow declaration source unavailable".into())
            })?;
        let graph = row
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .ok_or_else(|| {
                Error::PermissionDenied("Retained workflow declaration unavailable".into())
            })?;
        if graph.definition.pointer(&pointer) != Some(value) {
            return Err(Error::PermissionDenied(
                "Workflow value differs from its retained authored site".into(),
            ));
        }
        Ok(DeclaredInputRepository::bindings(conn, execution)
            .await?
            .into_iter()
            .filter_map(|site| {
                site.source_path
                    .strip_prefix(source_path)
                    .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                    .map(|suffix| (suffix.to_owned(), site.clone()))
            })
            .collect())
    }

    pub async fn workflow_task_site_path(
        conn: &mut PgConnection,
        parent: Id,
        task: &str,
        target: &str,
    ) -> Result<String> {
        use super::{
            execution::ExecutionRepository,
            execution_runtime_admission::ExecutionAdmissionRepository, FindById,
        };
        let (_, binding) =
            ExecutionAdmissionRepository::workflow_invocation(conn, parent, task, target).await?;
        let crate::runtime_authority::InvocationBinding::WorkflowTask { task_path, .. } = binding
        else {
            return Err(Error::validation("Expected retained task declaration"));
        };
        let row = ExecutionRepository::find_by_id(&mut *conn, parent)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Workflow source unavailable".into()))?;
        let graph = &row
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .ok_or_else(|| Error::PermissionDenied("Retained graph unavailable".into()))?
            .definition;
        let node = super::workflow_policy::task_at_authored_path(graph, &task_path)
            .ok_or_else(|| Error::PermissionDenied("Retained task site unavailable".into()))?;
        Ok(format!("/workflow{}", pointer_to_node(graph, node)?))
    }
    /// Finalize a newly admitted child, before publication, using its exact
    /// contract and pre-default supplied-field mask. Equal-looking supplied
    /// strings remain literal. Existing origin evidence is preserved verbatim.
    pub async fn workflow_child_defaults(
        conn: &mut PgConnection,
        child: Id,
        supplied: &Value,
        secrets: Vec<crate::secret_values::SecretValueInput>,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Vec<crate::secret_values::SecretValueInput>)> {
        use super::schema_contract::{BindingOwner, SchemaContractRepository};
        let binding = SchemaContractRepository::find(&mut *conn, BindingOwner::Execution, child)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Child input contract unavailable".into()))?;
        let contract = crate::value_schema::CompiledContract::new(&binding.input)
            .map_err(|_| Error::validation("Invalid admitted input contract"))?;
        let mut value = contract
            .apply_defaults(supplied)
            .map_err(|_| Error::validation("Invalid admitted input defaults"))?;
        let mut sources = Vec::new();
        for secret in secrets {
            if secret.source_kind != "provenance" {
                return Err(Error::validation("Child input provenance unavailable"));
            }
            let provenance: crate::secret_provenance::SecretProvenance = serde_json::from_str(
                secret
                    .source_ref
                    .as_deref()
                    .ok_or_else(|| Error::validation("Child input provenance missing"))?,
            )?;
            *value
                .pointer_mut(&secret.json_path)
                .ok_or_else(|| Error::validation("Child secret destination shape changed"))? =
                secret.value;
            sources.push(SecretPathSource {
                path: secret.json_path,
                source: crate::secret_values::SecretSource::Bound(provenance),
            });
        }
        let defaults = contract
            .apply_defaults(&value)
            .map_err(|_| Error::validation("Invalid admitted input defaults"))?;
        let (value, authored_sources) =
            Self::action_defaults(conn, child, supplied, defaults, encryption_key).await?;
        sources.extend(authored_sources);
        contract
            .validate(&value)
            .map_err(|_| Error::validation("Invalid admitted input value"))?;
        crate::secret_values::validate_contract_secret_destination_paths(
            &value,
            &contract,
            &sources
                .iter()
                .map(|source| source.path.clone())
                .collect::<Vec<_>>(),
        )?;
        let (value, secrets) =
            crate::secret_values::merge_contract_secret_redactions(value, &sources, &contract)?;
        Ok((value, secrets))
    }
    /// Return actual immutable task-input sites. Authored name selectors are
    /// resolved against the retained graph, then converted to its real pointer.
    pub async fn workflow_sites(
        conn: &mut PgConnection,
        parent: Id,
        task: &str,
        action_ref: &str,
        input: &Value,
    ) -> Result<Vec<(String, super::declared_input::DeclaredInputBinding)>> {
        use super::{
            execution::ExecutionRepository,
            execution_runtime_admission::ExecutionAdmissionRepository, FindById,
        };
        let (_, binding) =
            ExecutionAdmissionRepository::workflow_invocation(conn, parent, task, action_ref)
                .await?;
        let crate::runtime_authority::InvocationBinding::WorkflowTask { task_path, .. } = binding
        else {
            return Err(Error::validation("Expected a workflow task binding"));
        };
        let parent = ExecutionRepository::find_by_id(&mut *conn, parent)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Workflow input source unavailable".into()))?;
        let definition = &parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .ok_or_else(|| {
                Error::PermissionDenied("Workflow declaration graph unavailable".into())
            })?
            .definition;
        let node = super::workflow_policy::task_at_authored_path(definition, &task_path)
            .ok_or_else(|| Error::PermissionDenied("Workflow input site is ambiguous".into()))?;
        if node
            .get("input")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}))
            != *input
        {
            return Err(Error::PermissionDenied(
                "Rendered task input differs from its authored declaration".into(),
            ));
        }
        let canonical = pointer_to_node(definition, node)?;
        let prefix = format!("/workflow{canonical}/input");
        Ok(DeclaredInputRepository::bindings(conn, parent.id)
            .await?
            .into_iter()
            .filter_map(|site| {
                site.source_path
                    .strip_prefix(&prefix)
                    .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
                    .map(|suffix| (suffix.to_string(), site.clone()))
            })
            .collect())
    }

    /// Called only after the child's admission and parent dependency exist.
    /// Contributor-derived origins do not acquire authored delivery receipts.
    pub async fn workflow_deliveries(
        conn: &mut PgConnection,
        child: Id,
        secrets: &[crate::secret_values::SecretValueInput],
    ) -> Result<()> {
        use super::{
            execution::ExecutionRepository,
            execution_runtime_admission::ExecutionAdmissionRepository,
            runtime_authority::RuntimeOwner, FindById,
        };
        if ExecutionAdmissionRepository::find(conn, RuntimeOwner::Execution(child))
            .await?
            .is_none()
        {
            return Ok(());
        }
        let execution = ExecutionRepository::find_by_id(&mut *conn, child)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Workflow input destination unavailable".into())
            })?;
        let (Some(parent), Some(task)) = (execution.parent, execution.workflow_task.as_ref())
        else {
            return Ok(());
        };
        let (_, binding) = ExecutionAdmissionRepository::workflow_invocation(
            conn,
            parent,
            &task.task_name,
            &execution.action_ref,
        )
        .await?;
        let crate::runtime_authority::InvocationBinding::WorkflowTask { task_path, .. } = binding
        else {
            return Err(Error::validation("Expected task declaration"));
        };
        let source = ExecutionRepository::find_by_id(&mut *conn, parent)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Workflow input source unavailable".into()))?;
        let definition = &source
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .ok_or_else(|| Error::PermissionDenied("Workflow graph unavailable".into()))?
            .definition;
        let input = super::workflow_policy::task_at_authored_path(definition, &task_path)
            .and_then(|node| node.get("input"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let sites =
            Self::workflow_sites(conn, parent, &task.task_name, &execution.action_ref, &input)
                .await?;
        for secret in secrets {
            if secret.source_kind != "provenance" {
                continue;
            }
            let provenance: crate::secret_provenance::SecretProvenance = serde_json::from_str(
                secret
                    .source_ref
                    .as_deref()
                    .ok_or_else(|| Error::validation("Input provenance missing"))?,
            )?;
            let site = sites
                .iter()
                .filter(|(path, _)| {
                    secret.json_path == *path || secret.json_path.starts_with(&format!("{path}/"))
                })
                .max_by_key(|(path, _)| path.len());
            let Some((_, site)) = site else {
                continue;
            };
            // Only a single direct authored selector can mint a receipt here.
            // Mixed contributor expressions need render-time, per-expression
            // evidence; their combined provenance must not inherit this permit.
            let Some(selector) = site
                .expression
                .trim()
                .strip_prefix("{{")
                .and_then(|value| value.strip_suffix("}}"))
                .map(str::trim)
                .filter(|value| !value.contains("{{") && !value.contains("}}"))
            else {
                continue;
            };
            let references = crate::template_resolver::referenced_keystore_expressions(
                &Value::String(site.expression.clone()),
                &crate::template_resolver::TemplateContext::new(
                    Value::Null,
                    Value::Null,
                    Value::Null,
                ),
            )
            .unwrap_or_default();
            let key_reference = references.get(selector);
            let configuration = selector == "pack.config"
                || selector.starts_with("pack.config.")
                || selector == "config"
                || selector.starts_with("config.");
            let permit = DeclaredInputRepository::permit(
                conn,
                parent,
                &site.declaration_revision,
                &site.source_path,
                &site.expression,
            )
            .await?;
            for origin in provenance.origins {
                if matches!(&origin, SecretOrigin::Key(key) if key_reference == Some(&key.key_ref))
                    || configuration
                        && matches!(&origin, SecretOrigin::Entity { entity_type, .. } if entity_type == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION)
                {
                    DeclaredInputRepository::record_delivery(
                        conn,
                        &permit,
                        child,
                        &secret.json_path,
                        &origin,
                    )
                    .await?;
                }
            }
        }
        // Public configuration needs render-time revision evidence too. Do not
        // re-read current configuration here and manufacture a receipt for a
        // revision that may differ from the one actually rendered.
        Ok(())
    }
    pub async fn keys(
        conn: &mut PgConnection,
        permit: &DeclaredInputPermit,
        refs: &[String],
        encryption_key: Option<&str>,
    ) -> Result<BTreeMap<String, ResolvedKey>> {
        let mut keys = BTreeMap::new();
        for reference in refs.iter().collect::<std::collections::BTreeSet<_>>() {
            let key = KeyRepository::find_by_ref_for_share(conn, reference)
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied("Authored input Key is unavailable".into())
                })?;
            DeclaredInputRepository::authorize_key(conn, permit, key.id, key.encrypted).await?;
            let origin = crate::key_access::key_origin(&key);
            let value = if key.encrypted {
                crate::crypto::keystore::decrypt_json(
                    &key.value,
                    encryption_key.ok_or_else(|| {
                        Error::invalid_state("Authored Key decryption is unavailable")
                    })?,
                    key.id,
                )?
            } else {
                key.value
            };
            keys.insert(key.r#ref, ResolvedKey { origin, value });
        }
        Ok(keys)
    }

    pub async fn pack_values(
        conn: &mut PgConnection,
        permit: &DeclaredInputPermit,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Id, Vec<String>)> {
        let (value, revision) = DeclaredInputRepository::pack_configuration(
            conn,
            permit,
            permit.declaration().pack,
            encryption_key,
        )
        .await?;
        let row = ComponentConfigurationRepository::find(&mut *conn, revision)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Authored configuration revision disappeared".into())
            })?;
        let stored_contract: crate::value_schema::Contract =
            serde_json::from_value(row.contract)
                .map_err(|_| Error::validation("Invalid authored configuration contract"))?;
        let contract = crate::value_schema::CompiledContract::new(&stored_contract)
            .map_err(|_| Error::validation("Invalid authored configuration contract"))?;
        let paths = contract
            .secret_paths(&value)
            .map_err(|_| Error::validation("Invalid authored configuration value"))?;
        Ok((value, revision, paths))
    }

    /// The execution and its admission must already exist on this connection.
    /// Supplied top-level fields remain literal, including strings identical to
    /// an authored expression. Each default leaf is rendered exactly once.
    pub async fn action_defaults(
        conn: &mut PgConnection,
        execution: Id,
        supplied: &Value,
        mut defaults: Value,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Vec<SecretPathSource>)> {
        use super::inherited_input::{
            InheritedInputRepository as Inherit, InputEntityKind, InputSite, InputSitePurpose,
        };
        let mut sources = Vec::new();
        for site in DeclaredInputRepository::bindings(conn, execution).await? {
            let Some(path) = site.source_path.strip_prefix("/param_schema/") else {
                continue;
            };
            let Some((field, suffix)) = path.split_once("/default") else {
                continue;
            };
            if field.contains('/') {
                return Err(Error::validation("Invalid authored default field path"));
            }
            let field_name = field.replace("~1", "/").replace("~0", "~");
            if supplied
                .as_object()
                .is_some_and(|values| values.contains_key(&field_name))
            {
                continue;
            }
            let destination = format!("/{field}{suffix}");
            if defaults.pointer(&destination) != Some(&Value::String(site.expression.clone())) {
                continue;
            }
            let permit = DeclaredInputRepository::permit(
                conn,
                execution,
                &site.declaration_revision,
                &site.source_path,
                &site.expression,
            )
            .await?;
            let key = encryption_key.ok_or_else(|| {
                Error::invalid_state("Authored input receipt encryption is unavailable")
            })?;
            let mut session = Inherit::begin_render(
                conn,
                execution,
                &site.declaration_revision,
                &site.source_path,
                &site.expression,
                InputSitePurpose::Value,
            )
            .await?;
            if site.uses_pack_configuration() {
                Inherit::bind_configuration(conn, &mut session, key).await?;
            }
            // Key member loading happens at the actual selected AST member.
            // No selector prepass and no second expression/dependency evaluation.
            let rendered = Inherit::evaluate_with_keys(conn, session, key).await?;
            let receiving = InputSite {
                execution,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Value,
                path: destination.clone(),
            };
            let prepared = Inherit::prepare_destination(conn, &rendered, &receiving, key).await?;
            let receipt = Inherit::record_prepared(
                conn,
                &rendered,
                &prepared,
                rendered.value(),
                rendered.selections(),
                key,
            )
            .await?;
            // Retain only the existing immutable configuration proof. Key
            // evidence uses the typed receipt above, not the old authorize_key
            // path, which cannot represent declaration-scoped input approvals.
            for origin in receipt.source_origins() {
                if matches!(origin, SecretOrigin::Entity { entity_type, .. }
                    if entity_type == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION)
                {
                    DeclaredInputRepository::record_delivery(
                        conn,
                        &permit,
                        execution,
                        &destination,
                        origin,
                    )
                    .await?;
                }
            }
            if Self::origins_require_encryption(conn, receipt.source_origins()).await? {
                sources.push(SecretPathSource {
                    path: destination.clone(),
                    source: crate::secret_values::SecretSource::Bound(receipt.provenance()),
                });
            }
            *defaults.pointer_mut(&destination).ok_or_else(|| {
                Error::validation("Authored default destination is unavailable")
            })? = rendered.value().clone();
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok((defaults, sources))
    }

    /// Classify immutable read-time origin metadata, not current Key/config
    /// values. Public configuration keeps its receipt without forcing a secret
    /// annotation on a public default; every Key-derived value remains protected.
    pub async fn origins_require_encryption(
        conn: &mut PgConnection,
        origins: &[SecretOrigin],
    ) -> Result<bool> {
        for origin in origins {
            match origin {
                SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } if entity_type
                    == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION =>
                {
                    let revision = ComponentConfigurationRepository::find(&mut *conn, *entity_id)
                        .await?
                        .ok_or_else(|| {
                            Error::PermissionDenied(
                                "Selected configuration revision unavailable".into(),
                            )
                        })?;
                    if crate::secret_values::redacted_paths(&revision.config)
                        .iter()
                        .any(|secret| {
                            crate::secret_values::pointer_suffix(secret, path).is_some()
                                || crate::secret_values::pointer_suffix(path, secret).is_some()
                        })
                    {
                        return Ok(true);
                    }
                }
                _ => return Ok(true),
            }
        }
        Ok(false)
    }
}

pub(super) fn pointer_to_node(root: &Value, target: &Value) -> Result<String> {
    let mut pending = vec![(root, String::new(), 0usize)];
    let mut count = 0;
    while let Some((value, pointer, depth)) = pending.pop() {
        count += 1;
        if count > 16384 || depth > 32 {
            return Err(Error::validation(
                "Workflow declaration graph exceeds its bound",
            ));
        }
        if std::ptr::eq(value, target) {
            return Ok(pointer);
        }
        match value {
            Value::Object(fields) => {
                for (field, value) in fields {
                    pending.push((
                        value,
                        format!("{pointer}/{}", field.replace('~', "~0").replace('/', "~1")),
                        depth + 1,
                    ));
                }
            }
            Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    pending.push((value, format!("{pointer}/{index}"), depth + 1));
                }
            }
            _ => {}
        }
    }
    Err(Error::validation("Workflow declaration pointer missing"))
}
