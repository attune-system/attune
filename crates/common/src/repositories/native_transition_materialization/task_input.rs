//! Canonical ordinary successor inputs from an already recorded transition frame.
//! No caller-provided provenance is promoted to a whole destination VALUE.
use super::*;

pub struct PreparedNativeTaskInput {
    parent: Id,
    workflow: Id,
    task: String,
    action_ref: String,
    key_hash: String,
    supplied: Value,
    fields: Vec<(String, RenderedReceiptContext)>,
}

// This representation profile is chosen BEFORE evaluating anything. Other
// profiles keep their existing owner; an error after selection never falls back.
fn root_input_profile(input: &Value, secret_paths: &[String]) -> bool {
    let Some(fields) = input.as_object().filter(|fields| !fields.is_empty()) else {
        return false;
    };
    fields.len() <= 256
        && secret_paths.len() == fields.len()
        && fields.iter().all(|(name, value)| {
            secret_paths.contains(&format!("/{}", escape(name)))
                && (value.is_string() || !contains_template(value))
        })
}

impl PreparedNativeTransitions {
    fn task_input_mode_supported(&self) -> bool {
        self.transitions
            .iter()
            .flat_map(|transition| &transition.publications)
            .flat_map(|publication| &publication.leaves)
            .all(|(_, proof)| match proof {
                PublicationProof::Staged(_) | PublicationProof::WholeResult(_) => true,
                PublicationProof::Independent(_, _) => false,
            })
    }

    pub fn supports_task_input(
        &self,
        input: &Value,
        contract: &crate::value_schema::CompiledContract<'_>,
    ) -> Result<bool> {
        // Inspect the opaque frame representation before any input evaluation.
        // The latest-value binder owns Staged/WholeResult, not Independent.
        if !self.task_input_mode_supported() {
            return Ok(false);
        }
        Ok(root_input_profile(
            input,
            &contract.secret_paths(input).map_err(|_| changed())?,
        ))
    }

    pub async fn prepare_task_input(
        &self,
        conn: &mut PgConnection,
        task: &str,
        action_ref: &str,
        authored: &Value,
        contract: &crate::value_schema::CompiledContract<'_>,
        key: &str,
    ) -> Result<PreparedNativeTaskInput> {
        // A direct caller cannot bypass structural nonselection in the owner.
        if !self.task_input_mode_supported() {
            return Err(changed());
        }
        if !self.supports_task_input(authored, contract)? || self.recorded.get().is_none() {
            return Err(changed());
        }
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        let mut selected = false;
        for (index, transition) in self.authored.iter().enumerate() {
            if self.fires(index)?
                && transition
                    .get("do")
                    .and_then(Value::as_array)
                    .is_some_and(|targets| {
                        targets.iter().any(|target| target.as_str() == Some(task))
                    })
            {
                selected = true;
            }
        }
        if transaction != self.cache_sources.transaction
            || crate::crypto::hash_encryption_key(key) != self.cache_sources.encryption_key_hash
            || !selected
        {
            return Err(changed());
        }
        self.require_frame_publication(conn).await?;
        let sites: BTreeMap<_, _> = super::super::declared_input_materialization::DeclaredInputMaterializationRepository::workflow_sites(
            conn, self.execution(), task, action_ref, authored,
        ).await?.into_iter().collect();
        let path = super::super::declared_input_materialization::DeclaredInputMaterializationRepository::workflow_task_site_path(
            conn, self.execution(), task, action_ref,
        ).await?;
        let revision = crate::value_schema::digest(&serde_json::to_value(
            &self
                .cache_sources
                .parent
                .executable_snapshot
                .as_ref()
                .ok_or_else(changed)?
                .executable,
        )?)
        .map_err(|_| changed())?;
        let controls = self.task_input_controls()?;
        let mut supplied = serde_json::json!({});
        let mut fields = Vec::new();
        for (name, value) in authored.as_object().ok_or_else(changed)? {
            let destination = format!("/{}", escape(name));
            let rendered = if let Some(binding) = sites.get(&destination) {
                let mut session = Inputs::begin_render(
                    conn,
                    self.execution(),
                    &binding.declaration_revision,
                    &binding.source_path,
                    &binding.expression,
                    InputSitePurpose::Value,
                )
                .await?;
                self.bind_prepared_frame(conn, &mut session, binding, key)
                    .await?;
                for (site, origin) in &controls {
                    Inputs::attach_control(conn, &mut session, site, origin, key).await?;
                }
                Inputs::evaluate_with_keys(conn, session, key).await?
            } else {
                if contains_template(value) {
                    return Err(changed());
                }
                let mut rendered = Inputs::controlled_literal(
                    conn,
                    self.execution(),
                    &revision,
                    &format!("{path}/input{destination}"),
                    &controls,
                    key,
                )
                .await?;
                self.attach_frame_literal(&mut rendered)?;
                rendered
            };
            supplied
                .as_object_mut()
                .ok_or_else(changed)?
                .insert(name.clone(), rendered.value().clone());
            fields.push((destination, rendered));
        }
        check_bounds(&supplied)?;
        // Check the actual evaluated shape too. A type-changing selector may not
        // silently switch this prepared root profile to a nested projection.
        if !root_input_profile(
            authored,
            &contract.secret_paths(&supplied).map_err(|_| changed())?,
        ) {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeTaskInput {
            parent: self.execution(),
            workflow: self.workflow.id,
            task: task.into(),
            action_ref: action_ref.into(),
            key_hash: self.cache_sources.encryption_key_hash.clone(),
            supplied,
            fields,
        })
    }

    fn task_input_controls(
        &self,
    ) -> Result<Vec<(InputSite, crate::secret_provenance::SecretOrigin)>> {
        let mut controls = Vec::new();
        let mut retain =
            |site: &InputSite, origin: crate::secret_provenance::SecretOrigin| -> Result<()> {
                let identity = (site.clone(), origin);
                if !controls.contains(&identity) {
                    controls.push(identity);
                }
                if controls.len() > 256 {
                    return Err(changed());
                }
                Ok(())
            };
        // Immutable earlier versions remain CONTROL even when a later version
        // has the same path. Deduplicate complete identity, never just the path.
        for transition in &self.transitions {
            for publication in &transition.publications {
                for (_, proof) in &publication.leaves {
                    match proof {
                        PublicationProof::Staged(stage) => {
                            let provenance = stage.provenance();
                            let [origin] = provenance.origins.as_slice() else {
                                return Err(changed());
                            };
                            retain(stage.site(), origin.clone())?;
                        }
                        PublicationProof::WholeResult(stage) => {
                            retain(stage.receipt().site(), stage.receipt().origin())?;
                        }
                        PublicationProof::Independent(_, _) => {}
                    }
                }
            }
        }
        for receipt in self.recorded.get().ok_or_else(changed)? {
            retain(receipt.site(), receipt.origin())?;
        }
        Ok(controls)
    }
}

impl PreparedNativeTaskInput {
    pub fn supplied(&self) -> &Value {
        &self.supplied
    }

    /// Record genuine whole-field VALUE receipts before one-shot initialization.
    /// Caller data can select neither parents nor a different rendered value.
    pub async fn record_child(
        self,
        conn: &mut PgConnection,
        child: Id,
        actual: &Value,
        key: &str,
    ) -> Result<Vec<crate::secret_values::SecretValueInput>> {
        if crate::crypto::hash_encryption_key(key) != self.key_hash {
            return Err(changed());
        }
        if actual != &self.supplied {
            return Err(Error::PermissionDenied(
                "Prepared workflow child input value differs from its original render".into(),
            ));
        }
        let row = ExecutionRepository::find_by_id(&mut *conn, child)
            .await?
            .ok_or_else(changed)?;
        if row.parent != Some(self.parent)
            || row.action_ref != self.action_ref
            || row.config.is_some()
            || row.status != ExecutionStatus::Requested
            || !row.workflow_task.as_ref().is_some_and(|metadata| {
                metadata.workflow_execution == self.workflow
                    && metadata.task_name == self.task
                    && metadata.task_index.is_none()
                    && metadata.task_batch.is_none()
            })
        {
            return Err(changed());
        }
        let original: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM execution WHERE id=$1 AND initial_input_transaction=pg_current_xact_id() AND config IS NULL AND status='requested' AND result IS NULL AND worker IS NULL AND original_execution IS NULL AND started_at IS NULL)",
        ).bind(child).fetch_one(&mut *conn).await?;
        if !original {
            return Err(changed());
        }
        let mut secrets = Vec::new();
        for (path, rendered) in self.fields {
            let site = InputSite {
                execution: child,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Value,
                path: path.clone(),
            };
            let receipt = Inputs::record_render(
                conn,
                &rendered,
                &site,
                actual.pointer(&path).ok_or_else(changed)?,
                rendered.selections(),
                key,
            )
            .await?;
            receipt.ensure_fresh_on(conn).await?;
            secrets.push(crate::secret_values::SecretValueInput {
                json_path: path,
                value: rendered.value().clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&receipt.provenance())?),
            });
        }
        Ok(secrets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn root_profile_keeps_direct_object_selection_and_literal_data_distinct() {
        assert!(root_input_profile(
            &json!({"whole":"{{ variables.whole }}"}),
            &["/whole".into()]
        ));
        assert!(root_input_profile(
            &json!({"whole":{"literal":"replacement"}}),
            &["/whole".into()]
        ));
        assert!(!root_input_profile(
            &json!({"whole":{"literal":"{{ variables.part }}"}}),
            &["/whole".into()]
        ));
        assert!(!root_input_profile(
            &json!({"whole":"{{ variables.whole }}"}),
            &["/whole/literal".into()]
        ));
        assert!(!root_input_profile(&json!({}), &[]));
    }

    #[test]
    fn root_profile_uses_the_bound_compiled_contract_without_validating_template_text_as_an_object()
    {
        use crate::value_schema::{compile_inline, CompiledContract, SchemaContext};
        let contract = compile_inline("fixture.native_task_input", SchemaContext::FlatDocument,
            &json!({"value":{"type":"string","secret":true},"whole":{"type":"object","secret":true}})).unwrap();
        let compiled = CompiledContract::new(&contract).unwrap();
        let authored = json!({"value":"{{ vars.copied }}", "whole":"{{ variables.whole }}"});
        assert!(root_input_profile(
            &authored,
            &compiled.secret_paths(&authored).unwrap()
        ));
    }
}
