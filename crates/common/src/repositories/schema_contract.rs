//! Immutable contract selection for record creation. Callers supply schema bodies
//! from their already-selected executable/component snapshot, never request data.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgConnection, Postgres};

use super::{pack_release::PackReleaseRepository, schema_type::SchemaTypeRepository};
use crate::models::PackRelease;
use crate::value_schema::{
    compile, inspect_definition, Contract, Limits, RegistryEntry, RegistryView, SchemaContext,
    SourceIdentity,
};
use crate::{Error, Result};

pub struct SchemaContractRepository;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSchemaBinding {
    pub input: Contract,
    pub output: Option<Contract>,
    #[serde(default)]
    pub static_actions: BTreeMap<String, StaticActionSchemaBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticActionSchemaBinding {
    pub input: Contract,
    pub output: Option<Contract>,
    pub executable_snapshot: Value,
}

impl ActionSchemaBinding {
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.input.source, SourceIdentity::Released { .. })
            || self.input.context != SchemaContext::FlatDocument
        {
            return Err(Error::validation(
                "Action schema binding requires one released input/output source",
            ));
        }
        crate::value_schema::CompiledContract::new(&self.input).map_err(|error| {
            Error::validation(format!("Invalid action input contract: {error}"))
        })?;
        if let Some(output) = &self.output {
            if output.source != self.input.source || output.context != SchemaContext::FlatDocument {
                return Err(Error::validation(
                    "Action output contract does not match its input source",
                ));
            }
            crate::value_schema::CompiledContract::new(output).map_err(|error| {
                Error::validation(format!("Invalid action output contract: {error}"))
            })?;
        }
        for (action_ref, binding) in &self.static_actions {
            let snapshot: crate::models::ExecutionExecutableSnapshot =
                serde_json::from_value(binding.executable_snapshot.clone()).map_err(|_| {
                    Error::validation("Invalid static workflow executable snapshot")
                })?;
            let expected = SourceIdentity::Released {
                component_ref: action_ref.clone(),
                release_id: snapshot.release.id,
                release_digest: snapshot.release.digest,
            };
            if snapshot.executable.action.r#ref != *action_ref || binding.input.source != expected {
                return Err(Error::validation(
                    "Static workflow contract does not match its executable",
                ));
            }
            ActionSchemaBinding {
                input: binding.input.clone(),
                output: binding.output.clone(),
                static_actions: BTreeMap::new(),
            }
            .validate()?;
        }
        if serde_json::to_vec(self)
            .map_err(|_| Error::validation("Invalid action schema binding"))?
            .len()
            > Limits::default().max_compiled_bytes.saturating_mul(2)
        {
            return Err(Error::validation(
                "Action schema binding exceeds its bundle budget",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BindingOwner {
    Enforcement,
    Execution,
}

impl BindingOwner {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enforcement => "enforcement",
            Self::Execution => "execution",
        }
    }
}

impl SchemaContractRepository {
    pub async fn materialize_action(
        transaction: &mut PgConnection,
        snapshot: &crate::models::ExecutionExecutableSnapshot,
    ) -> Result<ActionSchemaBinding> {
        let mut binding = Self::materialize_action_pair(transaction, snapshot).await?;
        Self::capture_static_actions(transaction, snapshot, &mut binding).await?;
        binding.validate()?;
        Ok(binding)
    }

    async fn capture_static_actions(
        transaction: &mut PgConnection,
        snapshot: &crate::models::ExecutionExecutableSnapshot,
        binding: &mut ActionSchemaBinding,
    ) -> Result<()> {
        if let Some(workflow) = &snapshot.executable.workflow_definition {
            let definition: crate::workflow::parser::WorkflowDefinition =
                serde_json::from_value(workflow.definition.clone())
                    .map_err(|_| Error::validation("Invalid immutable workflow definition"))?;
            let mut pending = definition.tasks.iter().collect::<Vec<_>>();
            let mut actions = BTreeSet::new();
            let mut nodes = 0usize;
            while let Some(task) = pending.pop() {
                nodes += 1;
                if nodes > Limits::default().max_schema_nodes {
                    return Err(Error::validation(
                        "Workflow contract task graph exceeds its node budget",
                    ));
                }
                if let Some(children) = &task.tasks {
                    pending.extend(children);
                }
                if let Some(action_ref) = &task.action {
                    if action_ref.contains("{{") {
                        continue;
                    }
                    crate::schema::RefValidator::validate_component_ref(action_ref)?;
                    actions.insert(action_ref.clone());
                }
            }
            for action_ref in actions {
                let mut task_snapshot = if let Some(selected) =
                    snapshot.pack_executables.get(&action_ref)
                {
                    crate::models::ExecutionExecutableSnapshot {
                        release: selected.release.clone(),
                        executable: selected.executable.clone(),
                        pack_executables: snapshot.pack_executables.clone(),
                    }
                } else {
                    super::executable_snapshot::ExecutableSnapshotRepository::resolve_for_action_ref(&mut *transaction, &action_ref).await?
                };
                if task_snapshot.executable.workflow_definition.is_none() {
                    task_snapshot.pack_executables.clear();
                }
                let task_binding =
                    Self::materialize_action_pair(transaction, &task_snapshot).await?;
                binding.static_actions.insert(
                    action_ref,
                    StaticActionSchemaBinding {
                        input: task_binding.input,
                        output: task_binding.output,
                        executable_snapshot: serde_json::to_value(task_snapshot)
                            .map_err(|_| Error::validation("Invalid static workflow snapshot"))?,
                    },
                );
                binding.validate()?;
            }
        }
        Ok(())
    }

    /// Compile only the exact pinned executable's input/output declarations.
    /// Metadata readmission must not discover task targets through current refs.
    pub async fn materialize_action_pair(
        transaction: &mut PgConnection,
        snapshot: &crate::models::ExecutionExecutableSnapshot,
    ) -> Result<ActionSchemaBinding> {
        let action = &snapshot.executable.action;
        let source = SourceIdentity::Released {
            component_ref: action.r#ref.clone(),
            release_id: snapshot.release.id,
            release_digest: snapshot.release.digest.clone(),
        };
        let empty = serde_json::json!({});
        let workflow = snapshot.executable.workflow_definition.as_ref();
        let graph = workflow.map(|definition| &definition.definition);
        let workflow_input = graph
            .and_then(|graph| graph.get("parameters"))
            .filter(|schema| !schema.is_null());
        let workflow_output = graph
            .and_then(|graph| graph.get("output"))
            .filter(|schema| !schema.is_null());
        let inputs = [
            action.param_schema.as_ref(),
            workflow.and_then(|workflow| workflow.param_schema.as_ref()),
            workflow_input,
        ];
        let outputs = [
            action.out_schema.as_ref(),
            workflow.and_then(|workflow| workflow.out_schema.as_ref()),
            workflow_output,
        ];
        for declarations in [&inputs, &outputs] {
            let mut declarations = declarations.iter().flatten();
            if let Some(first) = declarations.next() {
                if declarations.any(|schema| schema != first) {
                    return Err(Error::validation(
                        "Workflow action metadata and graph declare different schemas",
                    ));
                }
            }
        }
        let input = Self::materialize(
            transaction,
            source.clone(),
            SchemaContext::FlatDocument,
            inputs.into_iter().flatten().next().unwrap_or(&empty),
        )
        .await?;
        let output = if let Some(schema) = outputs.into_iter().flatten().next() {
            Some(Self::materialize(transaction, source, SchemaContext::FlatDocument, schema).await?)
        } else {
            None
        };
        let binding = ActionSchemaBinding {
            input,
            output,
            static_actions: BTreeMap::new(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub async fn workflow_child_binding(
        transaction: &mut PgConnection,
        parent: i64,
        snapshot: &crate::models::ExecutionExecutableSnapshot,
    ) -> Result<ActionSchemaBinding> {
        SchemaTypeRepository::lock_selection(&mut *transaction).await?;
        let root = Self::find(transaction, BindingOwner::Execution, parent)
            .await?
            .ok_or_else(|| Error::validation("Workflow parent is missing its schema binding"))?;
        if let Some(selected) = root.static_actions.get(&snapshot.executable.action.r#ref) {
            let expected = SourceIdentity::Released {
                component_ref: snapshot.executable.action.r#ref.clone(),
                release_id: snapshot.release.id,
                release_digest: snapshot.release.digest.clone(),
            };
            if selected.input.source != expected {
                return Err(Error::validation(
                    "Workflow child executable differs from its admitted contract",
                ));
            }
            // Nested workflow task sources are captured at that workflow's own
            // creation; this child's input/output pair remains inherited.
            let mut binding = ActionSchemaBinding {
                input: selected.input.clone(),
                output: selected.output.clone(),
                static_actions: BTreeMap::new(),
            };
            Self::capture_static_actions(transaction, snapshot, &mut binding).await?;
            binding.validate()?;
            Ok(binding)
        } else {
            Self::materialize_action(transaction, snapshot).await
        }
    }

    pub async fn attach(
        transaction: &mut PgConnection,
        owner: BindingOwner,
        id: i64,
        binding: &ActionSchemaBinding,
    ) -> Result<()> {
        binding.validate()?;
        let body = serde_json::to_value(binding)
            .map_err(|_| Error::validation("Invalid action schema binding"))?;
        let digest = crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid action binding digest"))?;
        let table = match owner {
            BindingOwner::Enforcement => "enforcement",
            BindingOwner::Execution => "execution",
        };
        let owner_snapshot: Option<Option<Value>> = sqlx::query_scalar(&format!(
            "SELECT executable_snapshot FROM {table} WHERE id = $1 FOR KEY SHARE"
        ))
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        let owner_snapshot = owner_snapshot.flatten().ok_or_else(|| {
            Error::validation("Runtime schema binding owner requires an executable snapshot")
        })?;
        let owner_snapshot: crate::models::ExecutionExecutableSnapshot =
            serde_json::from_value(owner_snapshot)
                .map_err(|_| Error::validation("Invalid runtime binding owner snapshot"))?;
        let expected = SourceIdentity::Released {
            component_ref: owner_snapshot.executable.action.r#ref,
            release_id: owner_snapshot.release.id,
            release_digest: owner_snapshot.release.digest,
        };
        if binding.input.source != expected {
            return Err(Error::validation(
                "Runtime schema binding does not match its owner's executable",
            ));
        }
        sqlx::query("INSERT INTO runtime_schema_binding (entity_kind, entity_id, contract, binding_digest) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(owner.as_str()).bind(id).bind(body).bind(&digest).execute(&mut *transaction).await?;
        let stored: String = sqlx::query_scalar("SELECT binding_digest FROM runtime_schema_binding WHERE entity_kind = $1 AND entity_id = $2")
            .bind(owner.as_str()).bind(id).fetch_one(&mut *transaction).await?;
        if stored != digest {
            return Err(Error::validation(
                "Runtime record already has a different immutable schema binding",
            ));
        }
        Ok(())
    }

    pub async fn find(
        transaction: &mut PgConnection,
        owner: BindingOwner,
        id: i64,
    ) -> Result<Option<ActionSchemaBinding>> {
        Self::find_with_executor(&mut *transaction, owner, id).await
    }

    pub async fn find_with_executor<'e, E>(
        executor: E,
        owner: BindingOwner,
        id: i64,
    ) -> Result<Option<ActionSchemaBinding>>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let row: Option<(Value, String)> = sqlx::query_as("SELECT contract, binding_digest FROM runtime_schema_binding WHERE entity_kind = $1 AND entity_id = $2")
            .bind(owner.as_str()).bind(id).fetch_optional(executor).await?;
        let Some((body, digest)) = row else {
            return Ok(None);
        };
        Self::decode_binding(body, &digest).map(Some)
    }

    pub async fn find_many<'e, E>(
        executor: E,
        owner: BindingOwner,
        ids: &[i64],
    ) -> Result<Vec<(i64, ActionSchemaBinding)>>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(i64, Value, String)> = sqlx::query_as(
            "SELECT entity_id, contract, binding_digest FROM runtime_schema_binding \
             WHERE entity_kind = $1 AND entity_id = ANY($2)",
        )
        .bind(owner.as_str())
        .bind(ids)
        .fetch_all(executor)
        .await?;
        rows.into_iter()
            .map(|(id, body, digest)| {
                Self::decode_binding(body, &digest).map(|binding| (id, binding))
            })
            .collect()
    }

    fn decode_binding(body: Value, digest: &str) -> Result<ActionSchemaBinding> {
        if crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid stored action binding digest"))?
            != digest
        {
            return Err(Error::validation(
                "Stored action schema binding digest mismatch",
            ));
        }
        let binding: ActionSchemaBinding = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid stored action schema binding"))?;
        binding.validate()?;
        Ok(binding)
    }

    pub async fn inherit_enforcement(
        transaction: &mut PgConnection,
        enforcement: i64,
        execution: i64,
    ) -> Result<bool> {
        let Some(binding) = Self::find(transaction, BindingOwner::Enforcement, enforcement).await?
        else {
            return Ok(false);
        };
        Self::attach(transaction, BindingOwner::Execution, execution, &binding).await?;
        Ok(true)
    }
    /// Resolve within the caller's creation transaction. This does not persist
    /// the contract or authorize execution, Key access, or historical disclosure.
    pub async fn materialize(
        transaction: &mut PgConnection,
        source: SourceIdentity,
        context: SchemaContext,
        schema: &Value,
    ) -> Result<Contract> {
        SchemaTypeRepository::lock_selection(&mut *transaction).await?;
        let SourceIdentity::Released {
            component_ref,
            release_id,
            release_digest,
        } = &source
        else {
            return Err(Error::validation(
                "Runtime schema source must be an immutable release",
            ));
        };
        let root = PackReleaseRepository::find_by_id(&mut *transaction, *release_id)
            .await?
            .ok_or_else(|| Error::validation("Missing runtime schema source release"))?;
        if root.digest != *release_digest
            || component_ref
                .split_once('.')
                .map(|(pack, _)| pack)
                .unwrap_or(component_ref)
                != root.pack_ref
        {
            return Err(Error::validation(
                "Runtime schema source does not match its release",
            ));
        }
        Self::compile_selected(transaction, source, context, schema, Some(root)).await
    }

    /// Key assignments and generic previews have no declaring pack. Their root references
    /// may select any installed type; referenced type definitions retain their
    /// own immutable dependency/version contracts. Caller authorization concerns
    /// the owning value, not schema metadata.
    pub async fn materialize_assignment(
        transaction: &mut PgConnection,
        source: SourceIdentity,
        context: SchemaContext,
        schema: &Value,
    ) -> Result<Contract> {
        if !matches!(
            source,
            SourceIdentity::Assigned { .. } | SourceIdentity::Authoring { .. }
        ) {
            return Err(Error::validation(
                "Schema assignment requires an assigned or preview source",
            ));
        }
        SchemaTypeRepository::lock_selection(&mut *transaction).await?;
        Self::compile_selected(transaction, source, context, schema, None).await
    }

    /// Components without a declaring pack can consume public installed types.
    /// Only Keys and generic rendering previews bypass root reference policy.
    pub async fn materialize_unowned(
        connection: &mut PgConnection,
        source: SourceIdentity,
        context: SchemaContext,
        schema: &Value,
    ) -> Result<Contract> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let references =
            inspect_definition(context, schema, Limits::default()).map_err(|error| {
                Error::validation(format!("Invalid unowned consumer schema: {error}"))
            })?;
        let references = references
            .into_iter()
            .map(|reference| reference.type_ref)
            .collect::<BTreeSet<_>>();
        for reference in references {
            let current = SchemaTypeRepository::find_by_ref(&mut *connection, &reference)
                .await?
                .filter(|definition| definition.retired_at.is_none())
                .ok_or_else(|| Error::validation("Referenced schema type is unavailable"))?;
            let revision = SchemaTypeRepository::active_revision(&mut *connection, &reference)
                .await?
                .ok_or_else(|| Error::validation("Referenced schema type has no active release"))?;
            let released: crate::schema_type::SchemaTypeDefinition =
                serde_json::from_value(revision.definition)?;
            if current.reference_visibility != crate::models::ActionReferenceVisibility::Public
                || released.reference_visibility != crate::models::ActionReferenceVisibility::Public
            {
                return Err(Error::validation(
                    "An unowned schema consumer requires public root types",
                ));
            }
        }
        Self::materialize_assignment(connection, source, context, schema).await
    }

    /// Mutable component/configuration assignments retain pack dependency and
    /// reference policy even before that pack has an executable release.
    pub async fn compile_owned_assignment(
        connection: &mut PgConnection,
        source: SourceIdentity,
        context: SchemaContext,
        schema: &Value,
        pack: &crate::models::Pack,
    ) -> Result<Contract> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        if let Some(release) = pack.active_release {
            let root = PackReleaseRepository::find_by_id(&mut *connection, release)
                .await?
                .filter(|release| release.pack == pack.id && release.pack_ref == pack.r#ref)
                .ok_or_else(|| Error::validation("Schema declaring release is unavailable"))?;
            return Self::compile_selected(connection, source, context, schema, Some(root)).await;
        }
        let dependencies =
            crate::pack_registry::schema_types::parse_dependencies(&pack.dependencies)?;
        let references =
            inspect_definition(context, schema, Limits::default()).map_err(|error| {
                Error::validation(format!("Invalid owned schema assignment: {error}"))
            })?;
        let references = references
            .into_iter()
            .map(|reference| reference.type_ref)
            .collect::<BTreeSet<_>>();
        for reference in references {
            let revision = SchemaTypeRepository::active_revision(&mut *connection, &reference)
                .await?
                .ok_or_else(|| Error::validation("Referenced schema type is unavailable"))?;
            let definition: crate::schema_type::SchemaTypeDefinition =
                serde_json::from_value(revision.definition)?;
            let current = SchemaTypeRepository::find_by_ref(&mut *connection, &reference)
                .await?
                .filter(|definition| definition.retired_at.is_none())
                .ok_or_else(|| Error::validation("Referenced schema type is retired"))?;
            if !definition.reference_allowed(&pack.r#ref)
                || !current.definition().reference_allowed(&pack.r#ref)
            {
                return Err(Error::validation(
                    "Schema reference policy denies this assignment",
                ));
            }
            if revision.pack_ref != pack.r#ref {
                let requirement = dependencies.get(&revision.pack_ref).ok_or_else(|| {
                    Error::validation("Cross-pack schemas require a declared dependency")
                })?;
                let release =
                    PackReleaseRepository::find_by_id(&mut *connection, revision.release_id)
                        .await?
                        .ok_or_else(|| {
                            Error::validation("Schema dependency release is unavailable")
                        })?;
                if !semver::VersionReq::parse(requirement)
                    .map_err(|_| Error::validation("Invalid schema dependency constraint"))?
                    .matches(&semver::Version::parse(&release.version).map_err(|_| {
                        Error::validation("Invalid schema dependency release version")
                    })?)
                {
                    return Err(Error::validation(
                        "Latest schema dependency release does not satisfy its constraint",
                    ));
                }
            }
        }
        Self::compile_selected(connection, source, context, schema, None).await
    }

    /// A runtime-created schema belongs to its creator's immutable executable
    /// release. Unlike a Key assignment, its named edges retain that pack's
    /// dependency and reference policy even though the root body is dynamic.
    pub async fn materialize_dynamic(
        connection: &mut PgConnection,
        creator: &crate::models::ExecutionExecutableSnapshot,
        context: SchemaContext,
        schema: &Value,
    ) -> Result<Contract> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let release = PackReleaseRepository::find_by_id(&mut *connection, creator.release.id)
            .await?
            .ok_or_else(|| Error::validation("Dynamic schema creator release is unavailable"))?;
        if release.digest != creator.release.digest
            || release.pack_ref != creator.executable.action.pack_ref
        {
            return Err(Error::validation(
                "Dynamic schema creator does not match its immutable source",
            ));
        }
        let source = SourceIdentity::Assigned {
            component_ref: creator.executable.action.r#ref.clone(),
            assignment_id: uuid::Uuid::new_v4(),
        };
        Self::compile_selected(connection, source, context, schema, Some(release)).await
    }

    /// Caller-selected mutable component assignments retain their declaring
    /// release's type view and dependency policy without claiming their authored
    /// body is an immutable executable snapshot.
    pub(crate) async fn compile_selected(
        transaction: &mut PgConnection,
        source: SourceIdentity,
        context: SchemaContext,
        schema: &Value,
        root: Option<PackRelease>,
    ) -> Result<Contract> {
        let limits = Limits::default();
        let root_release = root.as_ref().map(|release| release.id);
        let mut sources = root
            .into_iter()
            .map(|release| (release.pack_ref.clone(), release))
            .collect::<BTreeMap<_, _>>();
        let mut views = BTreeMap::<i64, RegistryView>::new();
        let mut registry = RegistryView::new();
        let mut checked = BTreeSet::new();
        let mut pending = VecDeque::from([(
            source.component_ref().to_owned(),
            root_release,
            context,
            schema.clone(),
        )]);
        let mut bytes = 0usize;
        while let Some((declaring_ref, declaring_release, context, body)) = pending.pop_front() {
            if !checked.insert((
                declaring_ref.clone(),
                declaring_release,
                context == SchemaContext::SingleValue,
            )) {
                continue;
            }
            bytes = bytes.saturating_add(
                serde_json::to_vec(&body)
                    .map_err(|_| Error::validation("Invalid runtime schema source"))?
                    .len(),
            );
            if bytes > limits.max_source_bytes || checked.len() > limits.max_schema_nodes {
                return Err(Error::validation(
                    "Runtime schema graph exceeds its source budget",
                ));
            }
            let declaring = if let Some(release) = declaring_release {
                let pack = declaring_ref
                    .split_once('.')
                    .map(|(pack, _)| pack)
                    .unwrap_or(&declaring_ref);
                let declaring = sources
                    .get(pack)
                    .ok_or_else(|| Error::validation("Missing runtime schema declaring release"))?
                    .clone();
                if declaring.id != release {
                    return Err(Error::validation(
                        "Conflicting runtime schema release selections",
                    ));
                }
                Some(declaring)
            } else {
                None
            };
            let mut targets = BTreeSet::new();
            for reference in inspect_definition(context, &body, limits)
                .map_err(|error| {
                    Error::validation(format!("Invalid runtime schema source: {error}"))
                })?
                .into_iter()
                .filter(|reference| targets.insert(reference.type_ref.clone()))
            {
                let target_pack = reference
                    .type_ref
                    .split_once('.')
                    .map(|(pack, _)| pack)
                    .ok_or_else(|| Error::validation("Invalid runtime named type ref"))?;
                let selected = if let Some(declaring) = declaring
                    .as_ref()
                    .filter(|declaring| declaring.pack_ref == target_pack)
                {
                    declaring.clone()
                } else {
                    let selected = PackReleaseRepository::find_active_by_pack_ref(
                        &mut *transaction,
                        target_pack,
                    )
                    .await?
                    .ok_or_else(|| Error::validation("Missing active schema dependency release"))?;
                    if let Some(declaring) = &declaring {
                        check_dependency(declaring, &selected)?;
                    }
                    selected
                };
                if let Some(previous) = sources.get(target_pack) {
                    if previous.id != selected.id {
                        return Err(Error::validation(
                            "Conflicting runtime schema release selections",
                        ));
                    }
                } else {
                    sources.insert(target_pack.into(), selected.clone());
                }
                if !views.contains_key(&selected.id) {
                    views.insert(
                        selected.id,
                        SchemaTypeRepository::release_view(
                            &mut *transaction,
                            selected.id,
                            target_pack,
                        )
                        .await?,
                    );
                }
                let entry = views[&selected.id]
                    .get(&reference.type_ref)
                    .ok_or_else(|| {
                        Error::validation("Named type is absent from selected immutable release")
                    })?;
                let RegistryEntry::Available(revision) = entry else {
                    return Err(Error::validation("Named type is retired or inaccessible"));
                };
                // Current ownership/policy still governs new references. Freezing
                // a body never grants authority to a retired or private component.
                let current =
                    SchemaTypeRepository::find_by_ref(&mut *transaction, &reference.type_ref)
                        .await?
                        .ok_or_else(|| Error::validation("Missing named type owner"))?;
                if current.retired_at.is_some()
                    || current.pack != selected.pack
                    || declaring.as_ref().is_some_and(|declaring| {
                        !current.definition().reference_allowed(&declaring.pack_ref)
                    })
                {
                    return Err(Error::validation(
                        "Named type reference policy denies this dependency",
                    ));
                }
                if let Some(RegistryEntry::Available(previous)) = registry.get(&reference.type_ref)
                {
                    if previous.revision_id != revision.revision_id {
                        return Err(Error::validation(
                            "Conflicting runtime named type revisions",
                        ));
                    }
                }
                if !registry.contains_key(&reference.type_ref) {
                    registry.insert(reference.type_ref.clone(), entry.clone());
                    pending.push_back((
                        reference.type_ref,
                        Some(selected.id),
                        SchemaContext::SingleValue,
                        revision.schema.clone(),
                    ));
                }
            }
        }
        compile(source, context, schema, &registry, limits)
            .map_err(|error| Error::validation(format!("Invalid runtime schema contract: {error}")))
    }
}

pub(crate) fn check_dependency(declaring: &PackRelease, selected: &PackRelease) -> Result<()> {
    let dependencies: BTreeMap<String, String> = serde_json::from_value(
        declaring
            .manifest
            .get("dependencies")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|_| Error::validation("Invalid immutable schema dependency contract"))?;
    let constraint = dependencies.get(&selected.pack_ref).ok_or_else(|| {
        Error::validation("Cross-pack named types require a declared pack dependency")
    })?;
    let requirement = semver::VersionReq::parse(constraint)
        .map_err(|_| Error::validation("Invalid schema dependency version constraint"))?;
    let version = semver::Version::parse(&selected.version)
        .map_err(|_| Error::validation("Invalid schema dependency release version"))?;
    if !requirement.matches(&version) {
        return Err(Error::validation(
            "Latest schema dependency release does not satisfy the declared constraint",
        ));
    }
    Ok(())
}
