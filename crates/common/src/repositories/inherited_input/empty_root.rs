//! A finite root recipe whose prerequisite is an empty CONTROL, not a child
//! result. Output leaves retain their ordinary declaration receipts.
use super::super::native_input_snapshot::NativeInputSnapshotRepository as Snapshots;
use super::super::FindById;
use super::*;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootField {
    path: String,
    encrypted: Value,
    key_hash: Option<String>,
    source_kind: String,
    source_ref: Option<String>,
}
async fn original_fields(
    conn: &mut PgConnection,
    kind: &str,
    execution: Id,
) -> Result<Vec<RootField>> {
    Ok(super::super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, kind, execution).await?
        .into_iter().map(|field| RootField { path: field.json_path, encrypted: field.encrypted_value, key_hash: field.encryption_key_hash,
            source_kind: field.source_kind, source_ref: field.source_ref }).collect())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRootRecipe {
    snapshot: Id,
    renderer: Id,
    execution: Id,
    workflow: Id,
    transaction: String,
    executable: crate::models::ExecutionExecutableSnapshot,
    contract: super::super::schema_contract::ActionSchemaBinding,
    original_parent: crate::models::Execution,
    original_workflow: crate::models::WorkflowExecution,
    config_fields: Vec<RootField>,
    result_fields: Vec<RootField>,
    variable_fields: Vec<RootField>,
    leaves: Vec<(String, Id, InputSite)>,
    value_digest: String,
}

pub(crate) struct PreparedEmptyRoot {
    recipe: EmptyRootRecipe,
    value: Value,
    leaves: Vec<(
        String,
        RenderedReceiptContext,
        PreparedReceiptDestination,
        PreparedReceiptArchive,
    )>,
    root_archive: PreparedReceiptArchive,
    archive_budget: PreparedArchiveGraphBudget,
    publication_bytes: usize,
    fence: ApprovalEffectFence,
}
impl PreparedEmptyRoot {
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
    pub(crate) fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.archive_budget
            .require_capacity(self.publication_bytes, self.leaves.len() + 1)?;
        for (_, rendered, destination, _) in &self.leaves {
            rendered.ensure_fresh()?;
            destination.fence.ensure_fresh()?;
        }
        Ok(())
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.recipe.transaction).await?;
        self.fence.ensure_fresh_on(conn).await?;
        self.ensure_fresh()?;
        for (_, rendered, destination, _) in &self.leaves {
            rendered.ensure_fresh_on(conn).await?;
            destination.fence.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}

impl InheritedInputRepository {
    pub(crate) async fn replay_empty_root(
        conn: &mut PgConnection,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
        key: &str,
    ) -> Result<Vec<ReceiptContext>> {
        let row: Option<(Id, Id, Value, String)> = sqlx::query_as("SELECT snapshot,renderer,recipe,recipe_digest FROM runtime_native_empty_root_release WHERE execution=$1 AND workflow=$2")
            .bind(parent.id).bind(workflow.id).fetch_optional(&mut *conn).await?;
        let (snapshot, renderer, body, digest) = row.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != digest {
            return Err(denied());
        }
        let recipe: EmptyRootRecipe = serde_json::from_value(body)?;
        if parent.status != crate::models::ExecutionStatus::Completed
            || workflow.status != crate::models::ExecutionStatus::Completed
            || !workflow.current_tasks.is_empty()
            || !workflow.failed_tasks.is_empty()
            || parent
                .executable_snapshot
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?
                != Some(serde_json::to_value(&recipe.executable)?)
        {
            return Err(denied());
        }
        let task = recipe
            .executable
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition
            .get("tasks")
            .ok_or_else(denied)?;
        let name = match task {
            Value::Array(tasks) if tasks.len() == 1 => tasks[0].get("name").and_then(Value::as_str),
            Value::Object(tasks) if tasks.len() == 1 => tasks.keys().next().map(String::as_str),
            _ => None,
        }
        .ok_or_else(denied)?;
        if workflow.completed_tasks != [name.to_owned()] {
            return Err(denied());
        }
        let fields = super::super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn, "execution_result", parent.id).await?;
        if fields.len() != 1
            || !fields[0].json_path.is_empty()
            || parent.result.as_ref() != Some(&crate::secret_values::redaction_marker())
        {
            return Err(denied());
        }
        let (ordinary, receipts) = super::super::immutable_input_origin::prepare_input_receipts(
            conn,
            "execution_result",
            parent.id,
            &fields,
            key,
        )
        .await?;
        if !ordinary.is_empty() || receipts.len() != 1 {
            return Err(denied());
        }
        let receipt = &receipts[0];
        let (proof, digest): (Value, String) =
            sqlx::query_as("SELECT proof,proof_digest FROM runtime_input_receipt WHERE id=$1")
                .bind(receipt.id)
                .fetch_one(&mut *conn)
                .await?;
        if hash(&proof)? != digest {
            return Err(denied());
        }
        let proof: RenderProof = serde_json::from_value(proof)?;
        if !matches!(proof.native.as_ref(), Some(NativeValueRecipe::EmptyRoot { snapshot: actual }) if *actual == snapshot)
        {
            return Err(denied());
        }
        recipe_parents(
            conn,
            renderer,
            snapshot,
            Some((receipt.accepted_value(), key)),
        )
        .await?;
        if fields[0].encryption_key_hash.as_deref()
            != Some(crate::crypto::hash_encryption_key(key).as_str())
            || crate::crypto::decrypt_json(&fields[0].encrypted_value, key)?
                != *receipt.accepted_value()
        {
            return Err(denied());
        }
        Ok(receipts)
    }

    pub(crate) async fn prepare_empty_root(
        conn: &mut PgConnection,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
        source: &super::super::native_input_snapshot::PreparedNativeEmptyIterationOutcome,
        leaves: Vec<(String, RenderedReceiptContext)>,
        key: &str,
    ) -> Result<PreparedEmptyRoot> {
        source.ensure_fresh_on(conn).await?;
        let executable = parent.executable_snapshot.clone().ok_or_else(denied)?;
        let contract = super::super::schema_contract::SchemaContractRepository::find(
            conn,
            super::super::schema_contract::BindingOwner::Execution,
            parent.id,
        )
        .await?
        .ok_or_else(denied)?;
        let graph = &executable
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let value = Snapshots::compose_success_output(
            graph,
            leaves
                .iter()
                .map(|(path, rendered)| (path.as_str(), rendered.value())),
            contract.output.as_ref(),
        )?;
        let renderer = DeclaredInputRepository::require_source(conn, parent.id).await?;
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, renderer.id)
            .await?
            .intersect(&ApprovalEffectFence::from_input_approval(
                source.held_deadline()?,
            )?);
        let mut prepared = Vec::new();
        let mut archive_budget = source.archive_budget().clone();
        let mut publication_bytes = 0usize;
        let config_fields = original_fields(conn, "execution_config", parent.id).await?;
        let result_fields = original_fields(conn, "execution_result", parent.id).await?;
        let variable_fields = original_fields(conn, "workflow_variables", parent.id).await?;
        // Match recipe_parents' independent archive inventory, including original
        // fields even when no output selects them. The selected graph budget is
        // a different limit and cannot stand in for this recipe-level predicate.
        let inventory = [
            leaves.len(),
            config_fields.len(),
            result_fields.len(),
            variable_fields.len(),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add);
        if inventory.is_none_or(|count| count > 256) {
            return Err(Error::PermissionDenied(
                "Empty root recipe inventory exceeds bound".into(),
            ));
        }
        for (path, rendered) in leaves {
            let destination = Self::prepare_archive_destination(
                conn,
                &rendered,
                &InputSite {
                    execution: parent.id,
                    kind: InputEntityKind::ExecutionResult,
                    purpose: InputSitePurpose::Value,
                    path: path.clone(),
                },
                &mut archive_budget,
                key,
            )
            .await?;
            fence = fence.intersect(&ApprovalEffectFence::from_input_approval(
                destination.held_deadline()?,
            )?);
            let archive = PreparedReceiptArchive::new(
                &RenderProof {
                    native: rendered.native.clone(),
                    site: destination.site.clone(),
                    renderer: rendered.renderer.id,
                    declaration: rendered.declaration,
                    literal: rendered.literal.clone(),
                    accepted: None,
                    selections: rendered.selections.clone(),
                },
                rendered.value(),
                key,
            )?;
            publication_bytes = publication_bytes
                .checked_add(archive.bytes()?)
                .ok_or_else(denied)?;
            archive_budget.require_capacity(publication_bytes, prepared.len() + 2)?;
            prepared.push((path, rendered, destination, archive));
        }
        let recipe = EmptyRootRecipe {
            snapshot: source.snapshot(),
            renderer: renderer.id,
            execution: parent.id,
            workflow: workflow.id,
            transaction: transaction(conn).await?,
            executable,
            contract,
            original_parent: parent.clone(),
            original_workflow: workflow.clone(),
            config_fields,
            result_fields,
            variable_fields,
            leaves: Vec::new(),
            value_digest: hash(&value)?,
        };
        // Receipt IDs are not allocated until publication. Maximum-width IDs
        // reserve the complete recipe without inventing any receipt or child.
        let mut reserved_recipe = recipe.clone();
        reserved_recipe.leaves = prepared
            .iter()
            .map(|(path, _, destination, _)| (path.clone(), Id::MAX, destination.site.clone()))
            .collect();
        let body = serde_json::to_value(&reserved_recipe)?;
        bounded(&body)?;
        let root_archive = PreparedReceiptArchive::new(
            &RenderProof {
                native: Some(NativeValueRecipe::EmptyRoot {
                    snapshot: source.snapshot(),
                }),
                site: InputSite {
                    execution: parent.id,
                    kind: InputEntityKind::ExecutionResult,
                    purpose: InputSitePurpose::Value,
                    path: String::new(),
                },
                renderer: renderer.id,
                declaration: None,
                literal: None,
                accepted: None,
                selections: vec![ReadSelection {
                    context_path: "$control".into(),
                    source: SelectionSource::NativeSnapshot {
                        snapshot: source.snapshot(),
                    },
                }],
            },
            &value,
            key,
        )?;
        let root_bytes = root_archive.bytes()?;
        publication_bytes = publication_bytes
            .checked_add(serde_json::to_vec(&body)?.len())
            .and_then(|bytes| bytes.checked_add(root_bytes))
            .ok_or_else(denied)?;
        archive_budget.require_capacity(publication_bytes, prepared.len() + 1)?;
        Ok(PreparedEmptyRoot {
            recipe,
            value,
            leaves: prepared,
            fence,
            root_archive,
            archive_budget,
            publication_bytes,
        })
    }

    pub(crate) async fn record_empty_root(
        conn: &mut PgConnection,
        prepared: &PreparedEmptyRoot,
        source: &super::super::native_input_snapshot::PreparedNativeEmptyIterationOutcome,
        key: &str,
    ) -> Result<ReceiptContext> {
        prepared.ensure_fresh_on(conn).await?;
        source.require_released(conn).await?;
        let parent = super::super::execution::ExecutionRepository::find_by_id(
            &mut *conn,
            prepared.recipe.execution,
        )
        .await?
        .ok_or_else(denied)?;
        let workflow = super::super::workflow::WorkflowExecutionRepository::find_by_id(
            &mut *conn,
            prepared.recipe.workflow,
        )
        .await?
        .ok_or_else(denied)?;
        if source.snapshot() != prepared.recipe.snapshot
            || serde_json::to_value(&parent)?
                != serde_json::to_value(&prepared.recipe.original_parent)?
            || serde_json::to_value(&parent.executable_snapshot)?
                != serde_json::to_value(Some(&prepared.recipe.executable))?
            || serde_json::to_value(workflow)?
                != serde_json::to_value(&prepared.recipe.original_workflow)?
        {
            return Err(denied());
        }
        for (kind, expected) in [
            ("execution_config", &prepared.recipe.config_fields),
            ("execution_result", &prepared.recipe.result_fields),
            ("workflow_variables", &prepared.recipe.variable_fields),
        ] {
            if original_fields(conn, kind, prepared.recipe.execution).await? != *expected {
                return Err(denied());
            }
        }
        let mut recipe = prepared.recipe.clone();
        let mut receipts = Vec::new();
        for (path, rendered, destination, archive) in &prepared.leaves {
            let receipt = Self::record_prepared_archive(
                conn,
                rendered,
                destination,
                rendered.value(),
                rendered.selections(),
                key,
                false,
                Some(archive),
            )
            .await?;
            recipe
                .leaves
                .push((path.clone(), receipt.id, receipt.site.clone()));
            receipts.push(receipt);
        }
        let body = serde_json::to_value(&recipe)?;
        bounded(&body)?;
        sqlx::query("INSERT INTO runtime_native_empty_root_release(snapshot,execution,workflow,renderer,release_transaction,recipe,recipe_digest) VALUES($1,$2,$3,$4,pg_current_xact_id()::TEXT,$5,$6)")
            .bind(recipe.snapshot).bind(recipe.execution).bind(recipe.workflow).bind(recipe.renderer).bind(&body).bind(hash(&body)?).execute(&mut *conn).await?;
        for (_, receipt, _) in &recipe.leaves {
            sqlx::query(
                "INSERT INTO runtime_native_empty_root_leaf(snapshot,receipt) VALUES($1,$2)",
            )
            .bind(recipe.snapshot)
            .bind(receipt)
            .execute(&mut *conn)
            .await?;
        }
        let renderer = DeclaredInputRepository::require_source(conn, recipe.execution).await?;
        let rendered = RenderedReceiptContext {
            transaction: recipe.transaction.clone(),
            renderer: renderer.clone(),
            declaration: None,
            literal: None,
            native: Some(NativeValueRecipe::EmptyRoot {
                snapshot: recipe.snapshot,
            }),
            value: prepared.value.clone(),
            selections: vec![ReadSelection {
                context_path: "$control".into(),
                source: SelectionSource::NativeSnapshot {
                    snapshot: recipe.snapshot,
                },
            }],
            fence: prepared.fence.clone(),
            purpose: InputSitePurpose::Value,
            wait_reads: vec![],
            result_reads: vec![],
            empty_reads: vec![],
        };
        let mut source_receipts = Vec::new();
        let mut origins = Vec::new();
        let mut public_sources = Vec::new();
        let mut outcomes = Vec::new();
        let mut native_sources = Vec::new();
        let mut parents = BTreeSet::new();
        // Retain the genuine empty source through its original CONTROL receipt.
        let empty = Self::empty_root_control(conn, source.snapshot(), renderer.id, key).await?;
        receipts.push(empty);
        for receipt in &receipts {
            collect_source_receipts(receipt, &mut source_receipts)?;
            parents.insert(receipt.id);
            for origin in &receipt.origins {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
            for site in &receipt.public_sources {
                if !public_sources.contains(site) {
                    public_sources.push(site.clone());
                }
            }
            for outcome in &receipt.outcomes {
                if !outcomes.contains(outcome) {
                    outcomes.push(outcome.clone());
                }
            }
            for native in &receipt.native_sources {
                if !native_sources.contains(native) {
                    native_sources.push(native.clone());
                }
            }
        }
        let evidence =
            Snapshots::empty_root_control_evidence(conn, source.snapshot(), renderer.id, key)
                .await?;
        if !native_sources.contains(&evidence.metadata) {
            native_sources.push(evidence.metadata);
        }
        for (id, _) in evidence.parents {
            parents.insert(id);
        }
        let destination = PreparedReceiptDestination {
            transaction: recipe.transaction,
            renderer: renderer.id,
            admission: renderer.id,
            render_digest: render_digest(&rendered)?,
            site: InputSite {
                execution: recipe.execution,
                kind: InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: String::new(),
            },
            source_receipts,
            snapshot_parents: parents.into_iter().collect(),
            native_sources,
            origins,
            public_sources,
            outcomes,
            fence: prepared.fence.clone(),
            wait_reads: vec![],
            result_reads: vec![],
            empty_reads: vec![],
        };
        Self::record_prepared_archive(
            conn,
            &rendered,
            &destination,
            &prepared.value,
            &rendered.selections,
            key,
            false,
            Some(&prepared.root_archive),
        )
        .await
    }
}

pub(super) async fn recipe_parents(
    conn: &mut PgConnection,
    renderer: Id,
    snapshot: Id,
    plaintext: Option<(&Value, &str)>,
) -> Result<(Vec<(Id, InputSite)>, usize)> {
    let row: Option<(Id, Id, Id, String, Value, String)> = sqlx::query_as("SELECT execution,workflow,renderer,release_transaction,recipe,recipe_digest FROM runtime_native_empty_root_release WHERE snapshot=$1")
        .bind(snapshot).fetch_optional(&mut *conn).await?;
    let (execution, workflow, issuer, transaction, body, digest) = row.ok_or_else(denied)?;
    bounded(&body)?;
    if hash(&body)? != digest {
        return Err(denied());
    }
    let recipe: EmptyRootRecipe = serde_json::from_value(body.clone())?;
    if recipe.snapshot != snapshot
        || recipe.renderer != renderer
        || issuer != renderer
        || recipe.execution != execution
        || recipe.workflow != workflow
        || recipe.transaction != transaction
        || recipe.leaves.len()
            + recipe.config_fields.len()
            + recipe.result_fields.len()
            + recipe.variable_fields.len()
            > 256
        || recipe.original_parent.id != execution
        || recipe.original_parent.status != crate::models::ExecutionStatus::Running
        || recipe.original_workflow.id != workflow
        || recipe.original_workflow.execution != execution
        || recipe.original_workflow.status != crate::models::ExecutionStatus::Running
    {
        return Err(denied());
    }
    Snapshots::require_empty_root_binding(
        conn,
        snapshot,
        renderer,
        execution,
        workflow,
        &recipe.executable,
        &recipe.contract,
    )
    .await?;
    let expected: BTreeSet<_> = recipe.leaves.iter().map(|(_, id, _)| *id).collect();
    let actual: Vec<Id> = sqlx::query_scalar(
        "SELECT receipt FROM runtime_native_empty_root_leaf WHERE snapshot=$1 ORDER BY receipt",
    )
    .bind(snapshot)
    .fetch_all(&mut *conn)
    .await?;
    if expected.len() != recipe.leaves.len() || expected != actual.into_iter().collect() {
        return Err(denied());
    }
    let mut values = Vec::new();
    let mut paths = BTreeSet::new();
    let renderer_record =
        RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
    let graph = &recipe
        .executable
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    for (path, id, site) in &recipe.leaves {
        if !paths.insert(path)
            || site.execution != execution
            || site.kind != InputEntityKind::ExecutionResult
            || site.purpose != InputSitePurpose::Value
            || site.path != *path
        {
            return Err(denied());
        }
        let row: Option<(Value,String,Value,String)> = sqlx::query_as("SELECT proof,proof_digest,encrypted_value,encryption_key_hash FROM runtime_input_receipt WHERE id=$1 AND renderer_admission=$2 AND execution=$3 AND entity_kind='execution_result' AND destination_purpose='value' AND destination_path=$4")
            .bind(id).bind(renderer).bind(execution).bind(path).fetch_optional(&mut *conn).await?;
        let (proof, digest, encrypted, key_hash) = row.ok_or_else(denied)?;
        bounded(&proof)?;
        bounded(&encrypted)?;
        if hash(&proof)? != digest {
            return Err(denied());
        }
        let proof: RenderProof = serde_json::from_value(proof)?;
        let source_path = format!("/workflow/output_map{path}");
        if proof.native.is_some()
            || proof.accepted.is_some()
            || proof.renderer != renderer
            || proof.site != *site
        {
            return Err(denied());
        }
        if let Some(declaration) = proof.declaration {
            if proof.literal.is_some() {
                return Err(denied());
            }
            let (body, expected): (Value, String) = sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2")
                .bind(declaration).bind(renderer_record.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
            if hash(&body)? != expected {
                return Err(denied());
            }
            let declaration: super::super::declared_input::DeclaredInputBinding =
                serde_json::from_value(body)?;
            if declaration.source_path != source_path
                || graph
                    .pointer(&format!("/output_map{path}"))
                    .and_then(Value::as_str)
                    != Some(declaration.expression.as_str())
            {
                return Err(denied());
            }
        } else if proof
            .literal
            .as_ref()
            .is_none_or(|leaf| leaf.path != source_path)
        {
            return Err(denied());
        }
        if let Some((_, key)) = plaintext {
            if key_hash != crate::crypto::hash_encryption_key(key) {
                return Err(denied());
            }
            values.push((path.as_str(), crate::crypto::decrypt_json(&encrypted, key)?));
        }
    }
    let mut expected = Vec::new();
    if let Some(output) = graph.get("output_map").filter(|value| !value.is_null()) {
        super::super::native_workflow_materialization::collect_leaves(
            output,
            String::new(),
            &mut expected,
            0,
        )?;
    }
    if paths
        .into_iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected
            .iter()
            .filter(|(path, _)| !path.is_empty())
            .map(|(path, _)| path.as_str())
            .collect()
    {
        return Err(denied());
    }
    if let Some((actual, _)) = plaintext {
        let value = Snapshots::compose_success_output(
            graph,
            values.iter().map(|(path, value)| (*path, value)),
            recipe.contract.output.as_ref(),
        )?;
        if &value != actual || hash(&value)? != recipe.value_digest {
            return Err(denied());
        }
    }
    Ok((
        recipe
            .leaves
            .into_iter()
            .map(|(_, id, site)| (id, site))
            .collect(),
        serde_json::to_vec(&body)?.len(),
    ))
}
