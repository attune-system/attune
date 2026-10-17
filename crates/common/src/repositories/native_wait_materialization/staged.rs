//! Ordered actual publications, including values captured before an overwrite.
//! All sources are issued by the canonical renderer and inherited-input APIs.
use super::super::inherited_input::{PreparedInputSource, PreparedStagedValue, RenderSession};
use super::*;
use crate::repositories::Update;

pub(super) struct Publication {
    name: String,
    value: Value,
    fields: Vec<(String, PreparedStagedValue)>,
}
pub(super) struct Program {
    controls: Vec<PreparedInputSource>,
    publications: Vec<Publication>,
    pub(super) successors: Vec<String>,
}

impl Program {
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        observation: &Observation,
        key: &str,
    ) -> Result<Self> {
        let next = context
            .task
            .get("next")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        let mut program = Self {
            controls: vec![],
            publications: vec![],
            successors: vec![],
        };
        for (index, transition) in next.iter().enumerate() {
            let relative = format!("/next/{index}/when");
            let rendered = program
                .render(
                    conn,
                    context,
                    &relative,
                    InputSitePurpose::Condition,
                    observation,
                    key,
                )
                .await?;
            let fires = rendered.value().as_bool().ok_or_else(denied)?;
            let control = Inputs::prepare_condition_decision(
                conn,
                &rendered,
                &InputSite {
                    execution: context.parent.id,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Condition,
                    path: format!("{}{relative}", context.path),
                },
                key,
            )
            .await?;
            program.controls.push(control);
            if !fires {
                continue;
            }
            for (entry, name, authored) in publications(transition)? {
                let mut value = authored.clone();
                let mut leaves = vec![];
                collect_leaves(authored, String::new(), &mut leaves, 0)?;
                let mut fields = vec![];
                for (suffix, _) in leaves {
                    let relative = format!("/next/{index}/publish{entry}/{}{suffix}", escape(name));
                    let rendered = program
                        .render(
                            conn,
                            context,
                            &relative,
                            InputSitePurpose::Value,
                            observation,
                            key,
                        )
                        .await?;
                    *value.pointer_mut(&suffix).ok_or_else(denied)? = rendered.value().clone();
                    let stage = Inputs::prepare_staged_value(
                        conn,
                        &rendered,
                        &InputSite {
                            execution: context.parent.id,
                            kind: InputEntityKind::WorkflowVariables,
                            purpose: InputSitePurpose::Value,
                            path: format!("/{}{suffix}", escape(name)),
                        },
                        key,
                    )
                    .await?;
                    fields.push((suffix, stage));
                }
                check_bounds(&value)?;
                program.publications.push(Publication {
                    name: name.into(),
                    value,
                    fields,
                });
            }
            for name in transition
                .get("do")
                .and_then(Value::as_array)
                .ok_or_else(denied)?
            {
                let name = name.as_str().ok_or_else(denied)?;
                if !program.successors.iter().any(|saved| saved == name) {
                    program.successors.push(name.into());
                }
            }
            let bytes = program
                .publications
                .iter()
                .try_fold(0usize, |bytes, publication| {
                    Ok::<_, Error>(
                        bytes
                            .checked_add(serde_json::to_vec(&publication.value)?.len())
                            .ok_or_else(denied)?,
                    )
                })?;
            if bytes > 1024 * 1024 {
                return Err(denied());
            }
        }
        program.ensure_fresh_on(conn).await?;
        Ok(program)
    }

    async fn render(
        &self,
        conn: &mut PgConnection,
        context: &Context,
        relative: &str,
        purpose: InputSitePurpose,
        observation: &Observation,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let path = format!("{}{relative}", context.path);
        let authored = context.task.pointer(relative).ok_or_else(denied)?;
        self.render_at(
            conn,
            context,
            &path,
            authored,
            purpose,
            Some(observation),
            key,
        )
        .await
    }

    pub(super) async fn render_at(
        &self,
        conn: &mut PgConnection,
        context: &Context,
        path: &str,
        authored: &Value,
        purpose: InputSitePurpose,
        observation: Option<&Observation>,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let Some(binding) = context.declarations.get(path) else {
            if purpose != InputSitePurpose::Value || contains_template(authored) {
                return Err(denied());
            }
            let mut rendered =
                Inputs::literal(conn, context.parent.id, &context.revision, path).await?;
            self.attach_literal(&context.sources, &mut rendered)?;
            return Ok(rendered);
        };
        let mut session = Inputs::begin_render(
            conn,
            context.parent.id,
            &binding.declaration_revision,
            &binding.source_path,
            &binding.expression,
            purpose,
        )
        .await?;
        self.bind(conn, &context.sources, &mut session, binding, key)
            .await?;
        if let Some(observation) = observation {
            observation.bind(&mut session)?;
            if purpose == InputSitePurpose::Condition {
                observation.bind_outcome(&mut session)?;
            }
        }
        if purpose == InputSitePurpose::Condition {
            Inputs::evaluate_condition_with_keys(conn, session, key).await
        } else {
            Inputs::evaluate_with_keys(conn, session, key).await
        }
    }

    pub(super) async fn bind(
        &self,
        conn: &mut PgConnection,
        sources: &Sources,
        session: &mut RenderSession,
        binding: &DeclaredInputBinding,
        key: &str,
    ) -> Result<()> {
        let names = expression_names(&binding.expression)?;
        let mut latest = BTreeMap::new();
        // Keep the complete history in publications. Only binding chooses the
        // latest complete value for a name; older captured stages stay intact.
        for publication in &self.publications {
            latest.insert(publication.name.as_str(), publication);
        }
        for field in &sources.fields {
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
            for (suffix, stage) in &publication.fields {
                for namespace in ["workflow", "vars", "variables"] {
                    if names.contains(namespace) {
                        Inputs::bind_staged_value(
                            session,
                            format!("/{namespace}/{}{suffix}", escape(name)),
                            stage,
                        )?;
                    }
                }
                if names.contains(name)
                    && !super::super::native_workflow_materialization::reserved(name)
                {
                    Inputs::bind_staged_value(
                        session,
                        format!("/{}{suffix}", escape(name)),
                        stage,
                    )?;
                }
            }
        }
        if names.contains("system") {
            Inputs::bind_prepared_system(session, &sources.system)?;
        }
        if binding.uses_pack_configuration() {
            Inputs::bind_configuration(conn, session, key).await?;
        }
        for source in sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
            .chain(self.controls.iter())
        {
            Inputs::attach_prepared_control(session, source)?;
        }
        Ok(())
    }

    pub(super) fn attach_literal(
        &self,
        sources: &Sources,
        rendered: &mut RenderedReceiptContext,
    ) -> Result<()> {
        for source in sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
            .chain(self.controls.iter())
        {
            Inputs::attach_rendered_control(rendered, source)?;
        }
        Ok(())
    }

    pub(super) fn controls(&self) -> &[PreparedInputSource] {
        &self.controls
    }
    pub(super) fn bytes(&self) -> Result<usize> {
        self.publications
            .iter()
            .try_fold(0usize, |bytes, publication| {
                Ok(bytes
                    .checked_add(serde_json::to_vec(&publication.value)?.len())
                    .ok_or_else(denied)?)
            })
    }

    pub(super) async fn publish(
        &self,
        conn: &mut PgConnection,
        workflow: Id,
        parent: Id,
        mut variables: Value,
        original: &[StoredSecretValue],
        key: &str,
    ) -> Result<(Value, Vec<crate::secret_values::PreparedSecretValue>)> {
        let mut secrets = original
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
        for publication in &self.publications {
            let sources = publication
                .fields
                .iter()
                .map(|(suffix, stage)| SecretPathSource {
                    path: suffix.clone(),
                    source: SecretSource::Bound(stage.provenance()),
                })
                .collect::<Vec<_>>();
            let path = format!("/{}", escape(&publication.name));
            secrets.retain(|field| {
                crate::secret_values::pointer_suffix(&field.json_path, &path).is_none()
            });
            let (value, fields) = crate::secret_values::redact_secret_path_sources(
                publication.value.clone(),
                &sources,
            );
            for mut field in crate::secret_values::prepare_secret_values(fields, key)? {
                field.json_path = crate::secret_values::pointer_join(&path, &field.json_path);
                secrets.push(field);
            }
            variables
                .as_object_mut()
                .ok_or_else(denied)?
                .insert(publication.name.clone(), value);
            check_bounds(&variables)?;
            if secrets.len() > 256 || serde_json::to_vec(&secrets)?.len() > 1024 * 1024 {
                return Err(denied());
            }
            Secrets::replace_entity_with_conn(conn, ENTITY_WORKFLOW_VARIABLES, parent, &secrets)
                .await?;
            Workflows::update(
                &mut *conn,
                workflow,
                super::super::workflow::UpdateWorkflowExecutionInput {
                    variables: Some(variables.clone()),
                    ..Default::default()
                },
            )
            .await?;
            // Finalize each actual buffer before publishing a later stage that
            // depends on it. Public recording still requires every native release.
            for (_, stage) in &publication.fields {
                Inputs::finalize_staged_value(conn, stage, key).await?;
            }
        }
        self.ensure_fresh_on(conn).await?;
        Ok((variables, secrets))
    }
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        for control in &self.controls {
            control.ensure_fresh_on(conn).await?;
        }
        for publication in &self.publications {
            for (_, stage) in &publication.fields {
                stage.ensure_fresh_on(conn).await?;
            }
        }
        Ok(())
    }
    pub(super) fn ensure_fresh(&self) -> Result<()> {
        for control in &self.controls {
            control.ensure_fresh()?;
        }
        for publication in &self.publications {
            for (_, stage) in &publication.fields {
                stage.ensure_fresh()?;
            }
        }
        Ok(())
    }
}

fn escape(name: &str) -> String {
    super::super::native_workflow_materialization::escape(name)
}

/// Retain the original definition's array positions for canonical declarations.
/// Only enumerate authored entries here; values are evaluated once by render_at.
pub(super) fn publications(transition: &Value) -> Result<Vec<(String, &str, &Value)>> {
    let Some(publish) = transition.get("publish").filter(|value| !value.is_null()) else {
        return Ok(vec![]);
    };
    let mut entries = vec![];
    match publish {
        Value::Object(values) => {
            for (name, value) in values {
                entries.push((String::new(), name.as_str(), value));
            }
        }
        Value::Array(values) => {
            for (index, entry) in values.iter().enumerate() {
                for (name, value) in entry.as_object().ok_or_else(denied)? {
                    entries.push((format!("/{index}"), name.as_str(), value));
                }
            }
        }
        _ => return Err(denied()),
    }
    if entries.len() > 256 {
        return Err(denied());
    }
    Ok(entries)
}
