// Included only in scheduler::tests. This does not sign or enable credentials.
const INQUIRY_SCHEDULER_INPUT_KEY: &str = "inquiry-scheduler-fixture-only-input-encryption";
const INQUIRY_WAIT_TARGET_PERMISSION: &str = "scheduler_inquiry_it.wait_target_execute";

enum InquiryFixtureSource {
    Human(i64),
    Execution(i64),
}

async fn create_admitted_inquiry_fixture_executions(
    pool: &PgPool,
    pack: &attune_common::models::Pack,
    actions: &[Action],
    definition: &attune_common::models::workflow::WorkflowDefinition,
    graph: &TaskGraph,
    queue_id: i64,
) -> Result<(Execution, i64, Execution)> {
    use attune_common::{
        repositories::{
            component_lifecycle::PackProjectionIds,
            execution_secret_value::ExecutionSecretValueRepository,
            managed_executable::ManagedExecutableRepository,
            native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
            runtime_authority::RuntimeAuthorityRepository, schema_type::SchemaTypeRepository,
        },
        runtime_authority::{
            InvocationBinding, InvocationPolicy, PermissionSetDefinition, RuntimePolicy,
        },
        secret_values::ENTITY_WORKFLOW_VARIABLES,
    };
    let workflow_action = actions
        .first()
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its workflow action"))?;
    let request_action = actions
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its request action"))?;
    ActionRepository::link_workflow_def(pool, workflow_action.id, definition.id).await?;
    let requester = IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: "scheduler-inquiry-requester".into(),
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
            digest: "a".repeat(64),
            object_key: "scheduler-inquiry-integration".into(),
            provider_version: "test".into(),
            content_path: "/tmp/opencode/scheduler-inquiry-integration".into(),
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
            work_queues: vec![queue_id],
            ..Default::default()
        },
    )
    .await?;
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut deployment, pack.id).await?;
    ManagedExecutableRepository::capture_deployment_actions(&mut deployment, release.id).await?;
    // Only the workflow root can invoke this one local action as nested work.
    // Its workflow tasks do not inherit this runtime API selection.
    let target_definition: PermissionSetDefinition = serde_json::from_value(serde_json::json!({
        "ref": INQUIRY_WAIT_TARGET_PERMISSION,
        "grants": [{
            "resource": "actions",
            "actions": ["execute"],
            "constraints": {"refs": [request_action.r#ref.clone()]}
        }]
    }))?;
    let target_set = target_definition.compile(&owner, &[])?;
    let invocation = |binding| InvocationPolicy {
        metadata_revision: None,
        source_id: None,
        dispatch_template_digest: None,
        foreign_workflow_policy: None,
        binding,
        target_release_id: release.id,
        target_release_digest: release.digest.clone(),
        permission_sets: BTreeMap::new(),
        standard_access: false,
    };
    let mut invocations = Vec::new();
    for action in actions {
        let mut selected = invocation(InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        });
        if action.id == workflow_action.id {
            selected
                .permission_sets
                .insert(target_set.reference.clone(), target_set.clone());
        }
        invocations.push(selected);
    }
    let tasks = definition
        .definition
        .get("tasks")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires an authored task array"))?;
    for (index, task) in tasks.iter().enumerate() {
        let action_ref = task
            .get("action")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires static action targets"))?;
        invocations.push(invocation(InvocationBinding::WorkflowTask {
            workflow_ref: workflow_action.r#ref.clone(),
            task_path: format!("/tasks/{index}"),
            action_ref: action_ref.into(),
        }));
    }
    // No queue invocation is fabricated. This fixture observes persisted queue
    // item lifecycle metadata, never admits or dispatches the queue's workload.
    let policy = RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations,
    };
    let revision = RuntimeAuthorityRepository::capture_policy(&mut deployment, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut deployment, revision.id).await?;
    deployment.commit().await?;

    let provisional_parent = accept_inquiry_fixture_action(
        pool,
        pack.id,
        workflow_action.id,
        &workflow_action.r#ref,
        InquiryFixtureSource::Human(requester.id),
        vec![INQUIRY_WAIT_TARGET_PERMISSION.into()],
        serde_json::json!({}),
    )
    .await?;
    let mut startup = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut startup, CacheTransactionMode::PinMutation)
        .await?;
    SchemaTypeRepository::lock_selection(&mut startup).await?;
    let parent = ExecutionRepository::update(
        &mut *startup,
        provisional_parent.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await?;
    let variables =
        Variables::prepare_initial_variables(&mut startup, parent.id, INQUIRY_SCHEDULER_INPUT_KEY)
            .await?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *startup,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: serde_json::to_value(graph)?,
            variables: variables.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut startup,
        ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        variables.secrets(),
    )
    .await?;
    let request_task = graph
        .get_task("request")
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its authored request task"))?;
    let context = WorkflowContext::new(serde_json::json!({}), HashMap::new());
    let mut messages = WorkflowPublications::default();
    let mut completions = Vec::new();
    ExecutionScheduler::dispatch_workflow_task_with_conn(
        &mut startup,
        &AtomicUsize::new(0),
        &parent,
        &workflow.id,
        request_task,
        &context,
        Some(INQUIRY_SCHEDULER_INPUT_KEY),
        None,
        &mut messages,
        &mut completions,
    )
    .await?;
    anyhow::ensure!(
        messages.len() == 1,
        "request task must create exactly one child"
    );
    anyhow::ensure!(
        completions.is_empty(),
        "request task must not be a terminal sentinel"
    );
    let created = ExecutionRepository::find_by_id(&mut *startup, messages[0].execution_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("persisted request child is missing"))?;
    anyhow::ensure!(
        created.parent == Some(parent.id)
            && created.action == Some(request_action.id)
            && created.workflow_task.as_ref().is_some_and(|task| {
                task.workflow_execution == workflow.id && task.task_name == "request"
            }),
        "request child must retain its actual authored workflow lineage"
    );
    let request = ExecutionRepository::update(
        &mut *startup,
        created.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await?;
    // Do not seal the scheduler-created request here. Production owns its
    // acceptance boundary. The regression below proves any missing seal.
    variables.ensure_fresh_on(&mut startup).await?;
    startup.commit().await?;
    variables.ensure_fresh()?;
    Ok((parent, workflow.id, request))
}

async fn accept_inquiry_fixture_action(
    pool: &PgPool,
    pack: i64,
    action: i64,
    action_ref: &str,
    source: InquiryFixtureSource,
    permission_set_refs: Vec<String>,
    supplied: JsonValue,
) -> Result<Execution> {
    use attune_common::{
        repositories::{
            accepted_input::AcceptedInputRepository,
            declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
            execution_admission::ExecutionAdmissionRepository as Admission,
            recorded_input_authority::RecordedInputAuthorityRepository,
            rule_authority::RuleAuthorityRepository,
            runtime_authority::{
                RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner,
            },
            runtime_read::RuntimeReadRepository,
            runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
            schema_contract::{BindingOwner, SchemaContractRepository},
            schema_type::SchemaTypeRepository,
        },
        runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
        secret_values::{
            merge_contract_secret_redactions, prepare_secret_values,
            validate_contract_secret_destination_paths,
        },
        value_schema::CompiledContract,
    };
    let mut acceptance = pool.begin().await?;
    SchemaTypeRepository::lock_selection(&mut acceptance).await?;
    let mut source_authority = None;
    let mut source_input_fence = None;
    let (executor, parent, attribution) = match source {
        InquiryFixtureSource::Human(identity) => {
            let human = IdentityRepository::find_by_id(&mut *acceptance, identity)
                .await?
                .filter(|human| !human.frozen)
                .ok_or_else(|| anyhow::anyhow!("inquiry fixture requester is unavailable"))?;
            (
                Some(human.id),
                None,
                RequestAttribution::Human {
                    actor: HumanAttribution {
                        identity_id: human.id,
                        login: human.login,
                    },
                },
            )
        }
        InquiryFixtureSource::Execution(parent_id) => {
            let record =
                Admission::require(&mut acceptance, RuntimeOwner::Execution(parent_id)).await?;
            // Bind validation to the real recorded attempt, without minting a
            // JWT or creating a credential from live human grants.
            let credential = RuntimeCredentialBinding {
                execution_id: parent_id,
                admission_id: record.id,
                sponsor_identity_id: record.authority.owner.sponsor_identity_id,
                action_ref: record.authority.binding.action_ref().into(),
                attempt: record.authority.attempt,
            };
            let authority = RuntimeAuthorityRepository::validate_execution_credential(
                &mut acceptance,
                &credential,
            )
            .await?;
            RuntimeReadRepository::require_untainted(&mut acceptance, record.id).await?;
            source_input_fence = Some(
                AcceptedInputRepository::revalidate_execution(
                    &mut acceptance,
                    parent_id,
                    INQUIRY_SCHEDULER_INPUT_KEY,
                )
                .await?,
            );
            let target = RuntimeTargetRepository::resolve(
                &mut acceptance,
                RuntimeTarget::Component(ComponentTarget::Action, action),
                record.authority.owner.sponsor_identity_id,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("nested wait target action is unavailable"))?;
            anyhow::ensure!(
                authority.allows_new_effect(&target, attune_common::rbac::Action::Execute),
                "the admitted parent cannot invoke this nested wait target action"
            );
            let parent = ExecutionRepository::find_by_id(&mut *acceptance, parent_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("nested wait target parent is unavailable"))?;
            source_authority = Some(authority);
            (
                parent.executor,
                Some(parent_id),
                RequestAttribution::ParentExecution {
                    execution_id: parent_id,
                },
            )
        }
    };
    let invocation = InvocationBinding::Action {
        action_ref: action_ref.into(),
    };
    let snapshot = Admission::resolve_current_snapshot(
        &mut acceptance,
        pack,
        RuntimeTarget::Component(ComponentTarget::Action, action),
        &invocation,
    )
    .await?;
    if let Some(parent_id) = parent {
        let parent = ExecutionRepository::find_by_id(&mut *acceptance, parent_id)
            .await?
            .and_then(|parent| parent.executable_snapshot)
            .ok_or_else(|| anyhow::anyhow!("nested parent executable is unavailable"))?;
        let authority = source_authority
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("nested parent authority is unavailable"))?;
        attune_common::action_visibility::ensure_action_reference_allowed(
            &snapshot.executable.action,
            Some(&parent.executable.action.pack_ref),
            "execution",
            authority.authority().binding.action_ref(),
        )?;
    }
    let binding =
        SchemaContractRepository::materialize_action_pair(&mut acceptance, &snapshot).await?;
    let contract = CompiledContract::new(&binding.input)?;
    let selected = Admission::select(
        &mut acceptance,
        pack,
        invocation,
        &snapshot,
        &permission_set_refs,
        attribution,
        RuntimeTarget::Component(ComponentTarget::Action, action),
    )
    .await?;
    let provisional = ExecutionRepository::create_pinned(
        &mut *acceptance,
        CreateExecutionInput {
            action: Some(action),
            action_ref: action_ref.into(),
            executor,
            parent,
            permission_set_refs,
            config: None,
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let record = selected
        .attach(&mut acceptance, RuntimeOwner::Execution(provisional.id))
        .await?;
    if let Some(parent) = parent {
        RecordedInputAuthorityRepository::copy(
            &mut acceptance,
            "execution",
            parent,
            "execution",
            provisional.id,
        )
        .await?;
        RuleAuthorityRepository::copy_execution(&mut acceptance, parent, provisional.id).await?;
    } else {
        RecordedInputAuthorityRepository::capture_human(
            &mut acceptance,
            "execution",
            provisional.id,
            executor
                .ok_or_else(|| anyhow::anyhow!("human requester attribution is unavailable"))?,
        )
        .await?;
    }
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
        Some(INQUIRY_SCHEDULER_INPUT_KEY),
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
    let prepared = prepare_secret_values(secret_inputs, INQUIRY_SCHEDULER_INPUT_KEY)?;
    ExecutionRepository::initialize_config_with_conn(
        &mut acceptance,
        provisional.id,
        Some(record.id),
        &snapshot,
        Some(redacted),
        &prepared,
    )
    .await?;
    let receipts = Materialize::seal_execution_inputs(
        &mut acceptance,
        provisional.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await?;
    selected.ensure_fresh_on(&mut acceptance).await?;
    if let Some(authority) = &source_authority {
        authority.ensure_fresh_on(&mut acceptance).await?;
    }
    if let Some(fence) = &source_input_fence {
        fence.ensure_fresh_on(&mut acceptance).await?;
    }
    for receipt in &receipts {
        receipt.ensure_fresh_on(&mut acceptance).await?;
    }
    let execution = ExecutionRepository::find_by_id(&mut *acceptance, provisional.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("accepted inquiry fixture execution is missing"))?;
    acceptance.commit().await?;
    selected.effect_fence().ensure_fresh()?;
    if let Some(authority) = &source_authority {
        authority.ensure_fresh()?;
    }
    if let Some(fence) = &source_input_fence {
        fence.ensure_fresh()?;
    }
    for receipt in &receipts {
        receipt.ensure_fresh()?;
    }
    Ok(execution)
}

#[tokio::test]
async fn inquiry_scheduler_authored_request_keeps_original_input_acceptance() {
    use attune_common::repositories::accepted_input::AcceptedInputRepository;
    let fixture = InquirySchedulerFixture::create().await;
    let mut validation = fixture.database.pool().begin().await.unwrap();
    let root = AcceptedInputRepository::revalidate_execution(
        &mut validation,
        fixture.parent.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await
    .expect("fixture root has a genuine original acceptance seal");
    let request = AcceptedInputRepository::revalidate_execution(
        &mut validation,
        fixture.request.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await;
    root.ensure_fresh_on(&mut validation).await.unwrap();
    validation.rollback().await.unwrap();
    fixture.database.cleanup().await.unwrap();
    request
        .expect("scheduler-created empty request input must retain its original acceptance seal");
}

#[tokio::test]
async fn inquiry_scheduler_wait_targets_keep_action_admission_and_parent_ownership() {
    use attune_common::{
        repositories::{
            accepted_input::AcceptedInputRepository,
            execution_admission::ExecutionAdmissionRepository as Admission,
            identity::UpdateIdentityInput, runtime_authority::RuntimeOwner,
        },
        runtime_authority::{InvocationBinding, RequestAttribution},
    };
    let fixture = InquirySchedulerFixture::create().await;
    let unrelated = fixture
        .create_execution(ExecutionStatus::Completed, false)
        .await;
    IdentityRepository::update(
        fixture.database.pool(),
        fixture.parent.executor.unwrap(),
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let owned = fixture
        .create_execution(ExecutionStatus::Completed, true)
        .await;
    assert_eq!(owned.parent, Some(fixture.parent.id));
    assert_eq!(unrelated.parent, None);
    assert!(owned.workflow_task.is_none());
    assert!(unrelated.workflow_task.is_none());
    let mut validation = fixture.database.pool().begin().await.unwrap();
    for target in [&owned, &unrelated] {
        let admission = Admission::require(&mut validation, RuntimeOwner::Execution(target.id))
            .await
            .unwrap();
        assert_eq!(
            admission.authority.binding,
            InvocationBinding::Action {
                action_ref: fixture.request.action_ref.clone(),
            }
        );
        if target.id == owned.id {
            assert_eq!(
                admission.authority.attribution,
                RequestAttribution::ParentExecution {
                    execution_id: fixture.parent.id,
                }
            );
        } else {
            assert!(matches!(
                &admission.authority.attribution,
                RequestAttribution::Human { .. }
            ));
        }
        AcceptedInputRepository::revalidate_execution(
            &mut validation,
            target.id,
            INQUIRY_SCHEDULER_INPUT_KEY,
        )
        .await
        .unwrap()
        .ensure_fresh_on(&mut validation)
        .await
        .unwrap();
    }
    validation.rollback().await.unwrap();
    let before = ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
        .await
        .unwrap()
        .len();
    let guarded = &fixture
        .parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .pack_executables["scheduler_inquiry_it.guarded"]
        .executable
        .action;
    let denied = accept_inquiry_fixture_action(
        fixture.database.pool(),
        guarded.pack,
        guarded.id,
        &guarded.r#ref,
        InquiryFixtureSource::Execution(fixture.parent.id),
        Vec::new(),
        serde_json::json!({}),
    )
    .await
    .expect_err("nested authority is limited to the exact request action");
    assert!(denied.to_string().contains("cannot invoke"), "{denied}");
    assert_eq!(
        ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .len(),
        before,
        "a rejected nested target must not insert a child"
    );
    fixture.database.cleanup().await.unwrap();
}
