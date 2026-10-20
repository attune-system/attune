// Included only in scheduler::tests. No process-global config or credentials.
const CACHE_PARTITION_INPUT_KEY: &str = "cache-partition-fixture-only-input-encryption";

async fn create_admitted_cache_partition_parent(
    pool: &PgPool,
    pack: &attune_common::models::Pack,
    actions: &[Action],
    graph: &TaskGraph,
    supplied: JsonValue,
) -> Result<(Execution, i64)> {
    use attune_common::{
        repositories::{
            component_lifecycle::PackProjectionIds,
            managed_executable::ManagedExecutableRepository,
            runtime_authority::RuntimeAuthorityRepository, schema_type::SchemaTypeRepository,
        },
        runtime_authority::{InvocationBinding, InvocationPolicy, RuntimePolicy},
    };

    let workflow_action = actions
        .first()
        .ok_or_else(|| anyhow::anyhow!("cache fixture requires its workflow action"))?;
    let consume_action = actions
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("cache fixture requires its consume action"))?;
    let requester = IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: "cache-partition-requester".into(),
            display_name: None,
            password_hash: None,
            attributes: serde_json::json!({}),
        },
    )
    .await?;

    let mut deployment = pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut deployment).await?;
    let release = PackReleaseRepository::create_or_get(
        &mut deployment,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "b".repeat(64),
            object_key: "cache-partition-integration".into(),
            provider_version: "test".into(),
            content_path: "/tmp/opencode/cache-partition-integration".into(),
            archive_size: 1,
            manifest: serde_json::json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut deployment,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: actions.iter().map(|action| action.id).collect(),
            ..Default::default()
        },
    )
    .await?;
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut deployment, pack.id).await?;
    ManagedExecutableRepository::capture_deployment_actions(&mut deployment, release.id).await?;
    let invocation = |binding| InvocationPolicy {
        metadata_revision: None,
        source_id: None,
        dispatch_template_digest: None,
        foreign_workflow_policy: None,
        binding,
        target_release_id: release.id,
        target_release_digest: release.digest.clone(),
        permission_sets: BTreeMap::new(),
        standard_access: true,
    };
    let root_binding = InvocationBinding::Action {
        action_ref: workflow_action.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![
            invocation(root_binding.clone()),
            invocation(InvocationBinding::Action {
                action_ref: consume_action.r#ref.clone(),
            }),
            invocation(InvocationBinding::WorkflowTask {
                workflow_ref: workflow_action.r#ref.clone(),
                task_path: "/tasks/0".into(),
                action_ref: consume_action.r#ref.clone(),
            }),
        ],
    };
    // This repository fixture has no named permission grants to delegate.
    // Policy capture is not a replacement for production registrar authorization.
    let revision = RuntimeAuthorityRepository::capture_policy(&mut deployment, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut deployment, revision.id).await?;
    let snapshot = ManagedExecutableRepository::resolve_invocation(
        &mut deployment,
        &revision.policy,
        &root_binding,
    )
    .await?;
    deployment.commit().await?;

    accept_cache_partition_parent(pool, &snapshot, &requester, graph, supplied).await
}

async fn accept_cache_partition_parent(
    pool: &PgPool,
    snapshot: &ExecutionExecutableSnapshot,
    requester: &attune_common::models::Identity,
    graph: &TaskGraph,
    supplied: JsonValue,
) -> Result<(Execution, i64)> {
    use attune_common::{
        repositories::{
            declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
            execution_admission::ExecutionAdmissionRepository as Admission,
            execution_secret_value::ExecutionSecretValueRepository,
            native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
            recorded_input_authority::RecordedInputAuthorityRepository,
            runtime_authority::RuntimeOwner,
            runtime_target::{ComponentTarget, RuntimeTarget},
            schema_contract::{BindingOwner, SchemaContractRepository},
            schema_type::SchemaTypeRepository,
        },
        runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
        secret_values::{
            merge_contract_secret_redactions, prepare_secret_values,
            validate_contract_secret_destination_paths, ENTITY_WORKFLOW_VARIABLES,
        },
        value_schema::CompiledContract,
    };
    let workflow_action = &snapshot.executable.action;
    let root_binding = InvocationBinding::Action {
        action_ref: workflow_action.r#ref.clone(),
    };
    let mut acceptance = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut acceptance, CacheTransactionMode::PinMutation)
        .await?;
    SchemaTypeRepository::lock_selection(&mut acceptance).await?;
    let binding =
        SchemaContractRepository::materialize_action_pair(&mut acceptance, snapshot).await?;
    let contract = CompiledContract::new(&binding.input)?;
    let selected = Admission::select(
        &mut acceptance,
        workflow_action.pack,
        root_binding,
        snapshot,
        &["standard".into()],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: requester.id,
                login: requester.login.clone(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, workflow_action.id),
    )
    .await?;
    let provisional = ExecutionRepository::create_pinned(
        &mut *acceptance,
        CreateExecutionInput {
            action: Some(workflow_action.id),
            action_ref: workflow_action.r#ref.clone(),
            executor: Some(requester.id),
            permission_set_refs: vec!["standard".into()],
            config: None,
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        snapshot,
    )
    .await?;
    let record = selected
        .attach(&mut acceptance, RuntimeOwner::Execution(provisional.id))
        .await?;
    RecordedInputAuthorityRepository::capture_human(
        &mut acceptance,
        "execution",
        provisional.id,
        requester.id,
    )
    .await?;
    SchemaContractRepository::attach(
        &mut acceptance,
        BindingOwner::Execution,
        provisional.id,
        &binding,
    )
    .await?;
    let defaults = contract.apply_defaults(&supplied)?;
    let (values, declared) = Materialize::action_defaults(
        &mut acceptance,
        provisional.id,
        &supplied,
        defaults,
        Some(CACHE_PARTITION_INPUT_KEY),
    )
    .await?;
    contract.validate(&values)?;
    validate_contract_secret_destination_paths(
        &values,
        &contract,
        &declared
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>(),
    )?;
    let (redacted, secret_inputs) = merge_contract_secret_redactions(values, &declared, &contract)?;
    let prepared = prepare_secret_values(secret_inputs, CACHE_PARTITION_INPUT_KEY)?;
    ExecutionRepository::initialize_config_with_conn(
        &mut acceptance,
        provisional.id,
        Some(record.id),
        snapshot,
        Some(redacted),
        &prepared,
    )
    .await?;
    // Seal even an empty input object. Do not synthesize an accepted-input row.
    let receipts = Materialize::seal_execution_inputs(
        &mut acceptance,
        provisional.id,
        CACHE_PARTITION_INPUT_KEY,
    )
    .await?;
    let parent = ExecutionRepository::update(
        &mut *acceptance,
        provisional.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await?;
    // The new row is locked by this transaction and has no previous state.
    let variables =
        Variables::prepare_initial_variables(&mut acceptance, parent.id, CACHE_PARTITION_INPUT_KEY)
            .await?;
    let workflow_definition = snapshot
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("captured cache fixture workflow is missing"))?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *acceptance,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: workflow_definition.id,
            task_graph: serde_json::to_value(graph)?,
            variables: variables.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut acceptance,
        ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        variables.secrets(),
    )
    .await?;
    selected.ensure_fresh_on(&mut acceptance).await?;
    variables.ensure_fresh_on(&mut acceptance).await?;
    for receipt in &receipts {
        receipt.ensure_fresh_on(&mut acceptance).await?;
    }
    acceptance.commit().await?;
    selected.effect_fence().ensure_fresh()?;
    variables.ensure_fresh()?;
    for receipt in &receipts {
        receipt.ensure_fresh()?;
    }
    Ok((parent, workflow.id))
}
